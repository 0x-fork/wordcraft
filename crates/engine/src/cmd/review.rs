//! Review tab: comments, track changes, word count, spelling (via the proofing hook).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use wordcraft_doc::para::InlineObject;
use wordcraft_doc::{Comment, Paragraph, PartKind, Pos, StoryRef, para_block};

use super::{fmt_revisions, now_iso, pos_json, sel_result};
use crate::{CmdError, CmdResult, CommandSpec, Selection, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("review.newComment", "New Comment", "Review › Comments", new_comment).key(super::mac_or("Mod+Alt+A / Mod+Alt+M", "Mod+Alt+M")).params(r#"{"text": string}"#),
        CommandSpec::new("review.reply", "Reply", "Review › Comments", reply)
            .params(r#"{"id": non-negative JSON integer, "text": string}"#)
            .when(comments_change),
        CommandSpec::new("review.editComment", "Edit Comment", "Review › Comments", edit_comment)
            .params(r#"{"id": non-negative JSON integer, "text": string}"#)
            .when(comments_change),
        CommandSpec::new("review.deleteComment", "Delete", "Review › Comments", delete_comment).params(r#"{"id"?: non-negative JSON integer (omit to use the caret), "all"?: bool}"#),
        CommandSpec::new("review.resolveComment", "Resolve", "Review › Comments", |s, v| {
            validate_comment_id(v)?;
            let id = p::u64(v, "id").map(|x| x as u32).or_else(|| comment_at_caret(s)).ok_or_else(|| CmdError::Params("`id` required".into()))?;
            let c = s.doc.comments.get_mut(&id).ok_or_else(|| CmdError::Params("no such comment".into()))?;
            c.resolved = p::bool(v, "value").unwrap_or(!c.resolved);
            sel_result(s)
        })
        .params(r#"{"id"?: non-negative JSON integer (omit to use the caret), "value"?: bool (omit to toggle)}"#),
        CommandSpec::new("review.nextComment", "Next", "Review › Comments", |s, _| nav_comment(s, 1)).pure(),
        CommandSpec::new("review.previousComment", "Previous", "Review › Comments", |s, _| nav_comment(s, -1)).pure(),
        CommandSpec::new("review.comments", "Show Comments", "Review › Comments", list_comments).pure(),
        CommandSpec::new("review.trackChanges", "Track Changes", "Review › Tracking", |s, v| {
            s.doc.settings.track_changes = p::bool(v, "value").unwrap_or(!s.doc.settings.track_changes);
            Ok(json!({"value": s.doc.settings.track_changes}))
        })
        .params(r#"{"value"?: bool (omit to toggle)}"#)
        .key("Mod+Shift+E"),
        CommandSpec::new("review.acceptAll", "Accept All Changes", "Review › Changes", |s, v| resolve_all_then(s, v, true))
            .params(r#"{"stopTracking"?: bool (also turn Track Changes off)}"#),
        CommandSpec::new("review.rejectAll", "Reject All Changes", "Review › Changes", |s, v| resolve_all_then(s, v, false))
            .params(r#"{"stopTracking"?: bool (also turn Track Changes off)}"#),
        CommandSpec::new("review.accept", "Accept", "Review › Changes", |s, v| resolve_sel_then(s, v, true))
            .params(r#"{"next"?: bool (then select the next change)}"#),
        CommandSpec::new("review.reject", "Reject", "Review › Changes", |s, v| resolve_sel_then(s, v, false))
            .params(r#"{"next"?: bool (then select the next change)}"#),
        CommandSpec::new("review.nextChange", "Next Change", "Review › Changes", |s, _| nav_change(s, 1)).pure(),
        CommandSpec::new("review.previousChange", "Previous Change", "Review › Changes", |s, _| nav_change(s, -1)).pure(),
        CommandSpec::new("review.markup", "Display for Review", "Review › Tracking", |s, v| {
            s.view.show_markup = p::str(v, "value").map(|m| m != "noMarkup" && m != "original").unwrap_or(!s.view.show_markup);
            // Without markup, deletions leave the layout.
            s.relayout();
            Ok(json!({"showMarkup": s.view.show_markup}))
        })
        .pure(),
        CommandSpec::new("review.trackingOptions", "Track Changes Options", "Review › Tracking", tracking_options)
            .params(
                r#"{"comments"?: bool, "ink"?: bool, "insertionsDeletions"?: bool, "formatting"?: bool, "balloons"?: "revisions|inline|commentsAndFormatting", "commentDisplay"?: "margin|selected", "insertMark"?: "underline|doubleUnderline|bold|italic|strikethrough|colorOnly|none", "insertColor"?: "byAuthor" | "RRGGBB", "deleteMark"?: "strikethrough|doubleStrikethrough|hidden|caret|hash|underline|doubleUnderline|bold|italic|colorOnly|none", "deleteColor"?: "byAuthor" | "RRGGBB", "changedLines"?: "outside|left|right|none", "changedLinesColor"?: "auto" | "RRGGBB", "trackFormatting"?: bool, "moveFromMark"?: (as deleteMark; none = plain text, no colour), "moveFromColor"?: "byAuthor" | "RRGGBB", "moveToMark"?: (as insertMark; none = plain text, no colour), "moveToColor"?: "byAuthor" | "RRGGBB", "trackMoves"?: bool (the document's setting; off = a cut and paste is a deletion and an insertion, not a move)} → the options (omit all to read them)"#,
            )
            .pure(),
        CommandSpec::new("review.wordCount", "Word Count", "Review › Proofing", word_count)
            .params(r#"{"includeTextBoxes"?: bool}"#)
            // ⌘⇧G is Find Previous on macOS.
            .key(super::non_mac("Mod+Shift+G"))
            .pure(),
        CommandSpec::new("review.changes", "Reviewing Pane", "Review › Tracking", list_changes).pure(),
        CommandSpec::new("review.spelling", "Spelling & Grammar", "Review › Proofing", next_issue)
            .key(super::mac_or("F7 / Alt+F7 / Mod+Alt+L", "F7 / Alt+F7"))
            .pure(),
        CommandSpec::new("review.issues", "Proofing Issues", "Review › Proofing", all_issues).pure(),
        CommandSpec::new("review.suggestions", "Spelling Suggestions", "Review › Proofing", suggestions).params(r#"{"pos"?: Pos}"#).pure(),
        CommandSpec::new("review.addToDictionary", "Add to Dictionary", "Review › Proofing", |s, v| {
            let w = match p::str(v, "word") {
                Some(w) => w.to_string(),
                None => issue_at(s, &s.sel.focus.clone())
                    .map(|(a, b, _)| s.doc.para_at(&a).and_then(|p| p.text.get(a.off..b.off).map(str::to_string)).unwrap_or_default())
                    .unwrap_or_default(),
            };
            if w.is_empty() {
                return Err(CmdError::Params("no word".into()));
            }
            // The same rule as loading the dictionary file, so a word added is never dropped later.
            if wordcraft_proof::custom_word(&w).is_none() {
                let max = wordcraft_proof::MAX_CUSTOM_WORD_CHARS;
                return Err(CmdError::Params(format!("{w:?} isn't a single word of at most {max} characters")));
            }
            wordcraft_proof::add_word(w.trim());
            s.persist_user_dictionary(&[]);
            s.relayout();
            Ok(json!({"added": w}))
        })
        .params(r#"{"word"?: string}"#)
        .pure(),
        CommandSpec::new("review.ignoreAll", "Ignore All", "Review › Proofing", |s, v| {
            let w = match p::str(v, "word") {
                Some(w) => w.to_string(),
                None => issue_at(s, &s.sel.focus.clone())
                    .map(|(a, b, _)| s.doc.para_at(&a).and_then(|p| p.text.get(a.off..b.off).map(str::to_string)).unwrap_or_default())
                    .unwrap_or_default(),
            };
            // For this session only, as in Word: not added to (or saved with) the dictionary.
            if !w.trim().is_empty() {
                wordcraft_proof::ignore_word(w.trim());
            }
            s.relayout();
            Ok(json!({"ignored": w}))
        })
        .pure(),
        CommandSpec::new("review.applySuggestion", "Change", "Review › Proofing", |s, v| {
            let text = p::req_str(v, "text")?.to_string();
            let at = s.sel.focus.clone();
            if let Some((a, b, _)) = issue_at(s, &at) {
                s.sel = Selection { anchor: a, focus: b };
            }
            super::type_text(s, &text)?;
            sel_result(s)
        })
        .params(r#"{"text": string}"#),
        CommandSpec::new("review.proofing", "Check Spelling as You Type", "File › Options › Proofing", |s, v| {
            s.view.proofing = p::bool(v, "value").unwrap_or(!s.view.proofing);
            s.prefs.check_spelling = s.view.proofing;
            Ok(json!({"value": s.view.proofing}))
        })
        .pure(),
        CommandSpec::new("review.thesaurus", "Thesaurus", "Review › Proofing", thesaurus).key("Shift+F7").pure(),
    ]
}

fn new_comment(s: &mut Session, v: &Value) -> CmdResult {
    let text = p::str(v, "text").unwrap_or("").to_string();
    let (a, b) = s.sel.ordered();
    let (a, b) = if a == b {
        // Comment on the word at the caret.
        match s.doc.para_at(&a) {
            Some(para) if !para.is_empty() => {
                let (x, y) = para.word_at(a.off);
                let y = para.text.get(x..y).map(|w| x + w.trim_end().len()).unwrap_or(y);
                (Pos { off: x, ..a.clone() }, Pos { off: y, ..a })
            }
            _ => (a.clone(), a),
        }
    } else {
        (a, b)
    };
    let part = s.doc.try_add_part(PartKind::Comment, vec![para_block(Paragraph::with_text(&text, Default::default()))])?;
    let id = comment_id(s)?;
    let initials = s.user_initials();
    let now = now_iso();
    s.doc.comments.insert(
        id,
        Comment {
            author: s.author.clone(),
            initials,
            date: now.clone(),
            date_utc: Some(now),
            parent: None,
            resolved: false,
            part,
            ..Default::default()
        },
    );
    let props = Default::default();
    s.doc.insert_object(&b, InlineObject::CommentEnd { id }, &props)?;
    s.doc.insert_object(&a, InlineObject::CommentStart { id }, &props)?;
    s.view.comments_pane = true;
    Ok(json!({"id": id, "story": part}))
}

/// An omitted ID permits caret targeting; an explicit malformed value must not.
/// Unlike the general numeric parameter helper, comment IDs never coerce floats.
fn validate_comment_id(v: &Value) -> Result<(), CmdError> {
    if v.get("id").is_some_and(|id| id.as_u64().is_none_or(|n| u32::try_from(n).is_err())) {
        return Err(CmdError::Params("`id` must be a non-negative 32-bit JSON integer".into()));
    }
    Ok(())
}

fn reply(s: &mut Session, v: &Value) -> CmdResult {
    validate_comment_id(v)?;
    let parent = p::u64(v, "id").map(|x| x as u32).ok_or_else(|| CmdError::Params("`id` required".into()))?;
    if !s.doc.comments.contains_key(&parent) {
        return Err(CmdError::Params("no such comment".into()));
    }
    let text = p::req_str(v, "text")?;
    let part = s.doc.try_add_part(PartKind::Comment, vec![para_block(Paragraph::with_text(text, Default::default()))])?;
    let id = comment_id(s)?;
    let initials = s.user_initials();
    let now = now_iso();
    s.doc.comments.insert(
        id,
        Comment {
            author: s.author.clone(),
            initials,
            date: now.clone(),
            date_utc: Some(now),
            parent: Some(parent),
            resolved: false,
            part,
            ..Default::default()
        },
    );
    Ok(json!({"id": id}))
}

/// An id no comment has: one past the largest, or the lowest free one once a file used `u32::MAX`.
fn comment_id(s: &Session) -> Result<u32, CmdError> {
    let ids = &s.doc.comments;
    wordcraft_doc::free_id(ids.keys().next_back().copied(), ids.keys().copied(), 0).ok_or_else(|| CmdError::Failed("no free comment id".into()))
}

/// Comments can't be written or edited in a document protected as read only or for filling in
/// forms (the comment fields are read-only there, #1348).
fn comments_change(s: &Session) -> Option<&'static str> {
    match s.doc.settings.protection.as_deref() {
        Some("readOnly" | "forms") => Some("the document is protected"),
        _ => None,
    }
}

/// Replace a comment's (or reply's) text; each line becomes a paragraph.
fn edit_comment(s: &mut Session, v: &Value) -> CmdResult {
    validate_comment_id(v)?;
    let id = p::u64(v, "id").and_then(|x| u32::try_from(x).ok()).ok_or_else(|| CmdError::Params("`id` required".into()))?;
    let text = p::req_str(v, "text")?;
    let part = s.doc.comments.get(&id).map(|c| c.part).ok_or_else(|| CmdError::Params("no such comment".into()))?;
    let blocks = text.split('\n').take(10_000).map(|l| para_block(Paragraph::with_text(l.trim_end_matches('\r'), Default::default()))).collect();
    s.doc.set_story(StoryRef::Part(part), blocks)?;
    Ok(json!({"id": id}))
}

/// The innermost comment whose range holds the caret.
pub fn comment_range_at_caret(s: &Session) -> Option<u32> {
    let f = &s.sel.focus;
    let p = s.doc.para_at(f)?;
    let mut open: Vec<u32> = Vec::new();
    for off in p.object_offsets() {
        if off > f.off {
            break;
        }
        match p.object_at(off) {
            Some(InlineObject::CommentStart { id }) => open.push(*id),
            Some(InlineObject::CommentEnd { id }) => open.retain(|x| x != id),
            _ => {}
        }
    }
    open.last().copied()
}

/// Comment anchored around the caret, else the first one in its paragraph.
fn comment_at_caret(s: &Session) -> Option<u32> {
    let f = &s.sel.focus;
    let p = s.doc.para_at(f)?;
    comment_range_at_caret(s).or_else(|| {
        p.object_offsets().into_iter().find_map(|o| match p.object_at(o) {
            Some(InlineObject::CommentStart { id }) | Some(InlineObject::CommentEnd { id }) => Some(*id),
            _ => None,
        })
    })
}

/// Removes the comments' anchors from every story: the body, headers, footers, notes and text
/// boxes (#1345).
fn remove_anchors(s: &mut Session, ids: &BTreeSet<u32>) -> Result<(), CmdError> {
    let stories: Vec<StoryRef> = std::iter::once(StoryRef::Body).chain(s.doc.parts.keys().map(|k| StoryRef::Part(*k))).collect();
    for story in stories {
        for path in s.doc.para_paths(story) {
            let offs: Vec<usize> = s
                .doc
                .para(story, &path)
                .map(|p| {
                    p.object_offsets()
                        .into_iter()
                        .filter(|o| matches!(p.object_at(*o), Some(InlineObject::CommentStart { id }) | Some(InlineObject::CommentEnd { id }) if ids.contains(id)))
                        .collect()
                })
                .unwrap_or_default();
            if offs.is_empty() {
                continue;
            }
            let para = s.doc.para_mut(story, &path)?;
            for o in offs.into_iter().rev() {
                para.delete(o, o + wordcraft_doc::para::OBJ.len_utf8())?;
            }
        }
    }
    Ok(())
}

/// The comment and every reply below it, replies to replies included (#1345). A parent cycle
/// from a hostile file ends the walk instead of looping.
fn thread_of(s: &Session, id: u32) -> Vec<u32> {
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (k, c) in &s.doc.comments {
        if let Some(parent) = c.parent {
            children.entry(parent).or_default().push(*k);
        }
    }
    let mut seen = BTreeSet::from([id]);
    let mut ids = vec![id];
    let mut i = 0;
    while let Some(&at) = ids.get(i) {
        for k in children.get(&at).into_iter().flatten() {
            if seen.insert(*k) {
                ids.push(*k);
            }
        }
        i += 1;
    }
    ids
}

fn delete_comment(s: &mut Session, v: &Value) -> CmdResult {
    validate_comment_id(v)?;
    let ids: Vec<u32> = if p::bool(v, "all").unwrap_or(false) {
        s.doc.comments.keys().copied().collect()
    } else {
        let id = p::u64(v, "id").map(|x| x as u32).or_else(|| comment_at_caret(s)).ok_or_else(|| CmdError::Params("no comment here".into()))?;
        thread_of(s, id)
    };
    remove_anchors(s, &ids.iter().copied().collect())?;
    for id in &ids {
        if let Some(c) = s.doc.comments.remove(id) {
            s.doc.parts.remove(&c.part);
        }
    }
    s.clamp_selection();
    Ok(json!({"deleted": ids}))
}

/// Comments in reading order with their anchors: every story the reader sees, in
/// `Document::shown_stories` order (#2289). A comment anchored only in a story nothing shows
/// (a removed header) has no anchor.
pub fn comment_list(s: &Session) -> Vec<(u32, Option<Pos>)> {
    let mut found: Vec<(u32, Option<Pos>)> = Vec::new();
    // The balloons ask every frame; most documents have no comments.
    if s.doc.comments.is_empty() {
        return found;
    }
    for story in s.doc.shown_stories() {
        for path in s.doc.para_paths(story) {
            let Some(p) = s.doc.para(story, &path) else { continue };
            for o in p.object_offsets() {
                if let Some(InlineObject::CommentStart { id }) = p.object_at(o) {
                    found.push((*id, Some(Pos { story, path: path.clone(), off: o })));
                }
            }
        }
    }
    for id in s.doc.comments.keys() {
        if !found.iter().any(|(x, _)| x == id) {
            found.push((*id, None));
        }
    }
    found
}

fn list_comments(s: &mut Session, _: &Value) -> CmdResult {
    let list = comment_list(s);
    Ok(Value::Array(
        list.iter()
            .filter_map(|(id, pos)| {
                let c = s.doc.comments.get(id)?;
                Some(json!({
                    "id": id, "author": c.author, "date": c.date, "resolved": c.resolved, "parent": c.parent,
                    "text": s.doc.plain_text(StoryRef::Part(c.part)),
                    "anchor": pos.as_ref().map(pos_json),
                }))
            })
            .collect(),
    ))
}

fn nav_comment(s: &mut Session, dir: i32) -> CmdResult {
    let list: Vec<Pos> = comment_list(s).into_iter().filter_map(|(_, p)| p).collect();
    // Reading order: the story's place in `shown_stories`, then the place in the story. Part ids
    // follow creation, not reading, so `Pos` order won't do across stories.
    let rank: BTreeMap<StoryRef, usize> = s.doc.shown_stories().into_iter().enumerate().map(|(i, st)| (st, i)).collect();
    let key = |p: &Pos| rank.get(&p.story).map(|i| (*i, p.path.clone(), p.off));
    let caret = key(&s.sel.focus);
    let t = if dir > 0 {
        list.iter().find(|p| caret.is_some() && key(p) > caret).or(list.first())
    } else {
        list.iter().rev().find(|p| caret.is_some() && key(p) < caret).or(list.last())
    };
    if let Some(p) = t {
        s.sel = Selection::caret(p.clone());
    }
    sel_result(s)
}

/// Accept or reject the tracked formatting and text of `from..to` in one paragraph (not its mark).
fn resolve_runs(para: &mut wordcraft_doc::Paragraph, from: usize, to: usize, props: bool, accept: bool) -> Result<(), CmdError> {
    if props && (para.props.fmt_change.is_some() || para.props.num_change.is_some()) {
        fmt_revisions::resolve_para(&mut para.props, accept);
        para.touch();
    }
    let ranges: Vec<(usize, usize, bool, bool, bool)> = para
        .run_ranges()
        .filter(|(r, _)| r.end > from && r.start < to)
        .map(|(r, c)| (r.start.max(from), r.end.min(to), c.ins.is_some(), c.del.is_some(), c.fmt_change.is_some()))
        .collect();
    for (a, b, ins, del, fmt) in ranges.into_iter().rev() {
        if (del && accept) || (ins && !accept) {
            para.delete(a, b)?;
        } else if ins || del || fmt {
            para.format(a, b, &|c| {
                c.ins = None;
                c.del = None;
                fmt_revisions::resolve_char(c, accept);
            })?;
        }
    }
    Ok(())
}

/// Accept or reject a revision range in one paragraph; `mark` includes its paragraph mark (and
/// the section break on it), `props` the paragraph's own formatting change.
#[allow(clippy::too_many_arguments)]
fn resolve_para(
    s: &mut Session,
    story: StoryRef,
    path: &wordcraft_doc::Path,
    from: usize,
    to: usize,
    mark: bool,
    props: bool,
    accept: bool,
) -> Result<(), CmdError> {
    let para = s.doc.para_mut(story, path)?;
    resolve_runs(para, from, to, props, accept)?;
    if !mark {
        return Ok(());
    }
    if para.mark.fmt_change.is_some() || para.section.as_ref().is_some_and(|x| x.fmt_change.is_some()) {
        fmt_revisions::resolve_char(&mut para.mark, accept);
        if let Some(sec) = para.section.as_deref_mut() {
            fmt_revisions::resolve_section(sec, accept);
        }
        para.touch();
    }
    let (ins, del) = (para.mark.ins, para.mark.del);
    if ins.is_none() && del.is_none() {
        return Ok(());
    }
    // An inserted paragraph mark that is rejected, or a deleted one that is accepted, goes away:
    // the next paragraph joins this one (the split is undone). A paragraph left empty goes away
    // whole, so the next one keeps its own style and numbering, as in Word. The mark at the end
    // of a story or cell can't go, so it just stops being a revision.
    if ((del.is_some() && accept) || (ins.is_some() && !accept))
        && (super::remove_empty_para(s, story, path)? || super::join_next_para(s, story, path)?)
    {
        return Ok(());
    }
    let para = s.doc.para_mut(story, path)?;
    para.mark.ins = None;
    para.mark.del = None;
    para.touch();
    Ok(())
}

/// Accept or reject the changes of the table cell `path` is in: the table's, its row's and the
/// cell's own. Returns whether there were any.
fn resolve_cell(s: &mut Session, story: StoryRef, path: &wordcraft_doc::Path, accept: bool) -> Result<bool, CmdError> {
    let Some((tp, r, c)) = path.cell() else { return Ok(false) };
    let has = s.doc.table(story, &tp).is_some_and(|t| {
        let row = t.rows.get(r);
        t.props.fmt_change.is_some()
            || row.is_some_and(|x| x.props.fmt_change.is_some())
            || row.and_then(|x| x.cells.get(c)).is_some_and(|x| x.props.fmt_change.is_some())
    });
    if !has {
        return Ok(false);
    }
    let t = s.doc.table_mut(story, &tp)?;
    fmt_revisions::resolve_table(t, accept);
    if let Some(row) = t.rows.get_mut(r) {
        fmt_revisions::resolve_row(row, accept);
        if let Some(cell) = row.cells.get_mut(c) {
            fmt_revisions::resolve_cell(cell, accept);
        }
    }
    Ok(true)
}

/// A tracked change, for navigation and the Reviewing Pane: its range, kind (`insert`,
/// `delete`, `format`), revision, and for a formatting change the properties it set.
struct Change {
    a: Pos,
    b: Pos,
    kind: &'static str,
    rev: Option<u32>,
    props: Vec<Value>,
}

impl Change {
    fn format(a: Pos, b: Pos, rev: u32, props: Vec<Value>) -> Change {
        Change { a, b, kind: "format", rev: Some(rev), props }
    }
}

/// The tracked paragraph mark at the end of the paragraph at `path`, as a change range: from
/// the paragraph's end to the start of the next paragraph (or the end itself when none follows).
fn mark_change(s: &Session, story: StoryRef, path: &wordcraft_doc::Path) -> Option<Change> {
    let p = s.doc.para(story, path)?;
    let kind = run_kind(&s.doc, p.mark.ins, p.mark.del);
    if kind.is_empty() {
        return None;
    }
    let a = Pos { story, path: path.clone(), off: p.len() };
    let b = if super::has_next_para(s, story, path) { Pos { story, path: path.with_last(path.last().saturating_add(1)), off: 0 } } else { a.clone() };
    Some(Change { a, b, kind, rev: p.mark.ins.or(p.mark.del), props: Vec::new() })
}

/// The kind of an insertion / deletion change: `insert`, `delete`, or for one end of a move
/// `moveTo` / `moveFrom`; empty when neither.
fn run_kind(doc: &wordcraft_doc::Document, ins: Option<u32>, del: Option<u32>) -> &'static str {
    match (ins, del) {
        (Some(_), _) if doc.move_name(ins).is_some() => "moveTo",
        (Some(_), _) => "insert",
        (None, Some(_)) if doc.move_name(del).is_some() => "moveFrom",
        (None, Some(_)) => "delete",
        _ => "",
    }
}

/// The moves in the body, for balloons: where each end starts, its author, and whether it is
/// the source (`true`) or the destination.
pub fn move_changes(s: &Session) -> Vec<(Pos, String, bool)> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for c in changes(s) {
        let from = match c.kind {
            "moveFrom" => true,
            "moveTo" => false,
            _ => continue,
        };
        let rev = c.rev.and_then(|r| s.doc.revisions.get(r as usize));
        let name = rev.and_then(|r| r.move_name.clone()).unwrap_or_default();
        // One balloon per end (an end runs on across paragraphs).
        if seen.insert((name, from)) {
            out.push((c.a, rev.map(|r| r.author.clone()).unwrap_or_default(), from));
        }
    }
    out
}

/// Accept or reject the tracked text and paragraph marks of the top-level paragraphs in a block
/// list that is not a story (a note separator).
fn resolve_loose_blocks(blocks: &mut wordcraft_doc::Blocks, accept: bool) -> Result<(), CmdError> {
    for b in blocks.iter_mut() {
        let wordcraft_doc::Block::Para(p) = std::sync::Arc::make_mut(b) else { continue };
        let len = p.len();
        resolve_runs(p, 0, len, true, accept)?;
        if p.mark.fmt_change.is_some() || p.section.as_ref().is_some_and(|x| x.fmt_change.is_some()) {
            fmt_revisions::resolve_char(&mut p.mark, accept);
            if let Some(sec) = p.section.as_deref_mut() {
                fmt_revisions::resolve_section(sec, accept);
            }
        }
        // A mark can't be removed here (no neighbour to join), so it just stops being a revision.
        p.mark.ins = None;
        p.mark.del = None;
        p.touch();
    }
    fmt_revisions::resolve_containers(blocks, accept, 0);
    Ok(())
}

fn resolve_all(s: &mut Session, accept: bool) -> CmdResult {
    let stories: Vec<StoryRef> = std::iter::once(StoryRef::Body).chain(s.doc.parts.keys().map(|k| StoryRef::Part(*k))).collect();
    for st in stories {
        for path in s.doc.para_paths(st).into_iter().rev() {
            let len = s.doc.para(st, &path).map(|p| p.len()).unwrap_or(0);
            resolve_para(s, st, &path, 0, len, true, true, accept)?;
        }
    }
    // Tables, rows, cells and sections.
    fmt_revisions::resolve_containers(&mut s.doc.body, accept, 0);
    for part in s.doc.parts.values_mut() {
        fmt_revisions::resolve_containers(&mut part.blocks, accept, 0);
    }
    fmt_revisions::resolve_section(&mut s.doc.last_section, accept);
    // The note separators are block lists of their own, not stories; they must not keep ids of
    // revisions that are about to be dropped.
    for sep in [&mut s.doc.footnote_separators, &mut s.doc.endnote_separators] {
        for blocks in [&mut sep.separator, &mut sep.continuation_separator, &mut sep.continuation_notice].into_iter().flatten() {
            resolve_loose_blocks(blocks, accept)?;
        }
    }
    s.doc.revisions.clear();
    s.clamp_selection();
    sel_result(s)
}

/// Accept or Reject All; with `stopTracking`, Track Changes is turned off as well.
fn resolve_all_then(s: &mut Session, v: &Value, accept: bool) -> CmdResult {
    let out = resolve_all(s, accept)?;
    if p::bool(v, "stopTracking") == Some(true) {
        s.doc.settings.track_changes = false;
    }
    Ok(out)
}

/// Accept or Reject at the selection; with `next`, the next change is selected afterwards.
fn resolve_sel_then(s: &mut Session, v: &Value, accept: bool) -> CmdResult {
    resolve_sel(s, accept, p::bool(v, "next") == Some(true))
}

/// The span of the tracked change around `off` in `p`: its run, joined across the comment anchors
/// that split one revision's text into several runs.
fn revision_span(p: &Paragraph, off: usize) -> Option<std::ops::Range<usize>> {
    let runs: Vec<_> = p.run_ranges().collect();
    let i = runs.iter().position(|(r, c)| r.start <= off && off <= r.end && (c.ins.is_some() || c.del.is_some() || c.fmt_change.is_some()))?;
    let (mut start, mut end) = (runs[i].0.start, runs[i].0.end);
    let key = (runs[i].1.ins, runs[i].1.del);
    if key == (None, None) {
        return Some(start..end);
    }
    // Comment anchors split a revision's text into several runs; the revision goes on past them.
    let is_anchor = |k: usize| matches!(p.object_at(runs[k].0.start), Some(InlineObject::CommentStart { .. } | InlineObject::CommentEnd { .. }));
    let same = |k: usize| (runs[k].1.ins, runs[k].1.del) == key;
    let mut k = i;
    while k > 0 {
        if same(k - 1) {
            k -= 1;
        } else if is_anchor(k - 1) && k > 1 && same(k - 2) {
            k -= 2;
        } else {
            break;
        }
        start = runs[k].0.start;
    }
    let mut k = i;
    while k + 1 < runs.len() {
        if same(k + 1) {
            k += 1;
        } else if is_anchor(k + 1) && k + 2 < runs.len() && same(k + 2) {
            k += 2;
        } else {
            break;
        }
        end = runs[k].0.end;
    }
    Some(start..end)
}

fn resolve_sel(s: &mut Session, accept: bool, next: bool) -> CmdResult {
    let (a, b) = s.sel.ordered();
    // A caret right before a tracked paragraph mark (and no tracked text around it): that mark.
    let mut caret_mark = None;
    // A caret in a paragraph whose own formatting changed (and no tracked text around it).
    let mut caret_props = None;
    let ranged = a != b;
    let (a, b) = if a == b {
        // The change at the caret: the run around it, else the paragraph mark right after it,
        // else the paragraph's formatting, else the table cell's.
        let found = s.doc.para_at(&a).and_then(|p| revision_span(p, a.off));
        let para_fmt = s.doc.para_at(&a).is_some_and(|p| p.props.fmt_change.is_some() || p.props.num_change.is_some());
        let mark_fmt = s.doc.para_at(&a).is_some_and(|p| a.off == p.len() && p.mark.fmt_change.is_some());
        match found {
            Some(r) => (Pos { off: r.start, ..a.clone() }, Pos { off: r.end, ..a }),
            None if mark_fmt || mark_change(s, a.story, &a.path).is_some_and(|m| m.a == a) => {
                caret_mark = Some(a.path.clone());
                (a.clone(), a)
            }
            None if para_fmt => {
                caret_props = Some(a.path.clone());
                (a.clone(), a)
            }
            None => {
                if resolve_cell(s, a.story, &a.path, accept)? {
                    return if next { select_next_unresolved(s) } else { sel_result(s) };
                }
                return nav_change(s, 1);
            }
        }
    } else {
        (a, b)
    };
    // Moves with an end in the range: their other ends go with them.
    let moves = super::moves::names_in(s, &a, &b, caret_mark.as_ref());
    for path in s.doc.paths_between(&a, &b).into_iter().rev() {
        let len = s.doc.para(a.story, &path).map(|p| p.len()).unwrap_or(0);
        let from = if path == a.path { a.off } else { 0 };
        let to = if path == b.path { b.off } else { len };
        // A paragraph's mark is in the selection when the selection goes on past it.
        let mark = path != b.path || caret_mark.as_ref() == Some(&path);
        let props = (ranged && (path != b.path || to > from)) || caret_props.as_ref() == Some(&path);
        resolve_para(s, a.story, &path, from, to, mark, props, accept)?;
        if ranged {
            resolve_cell(s, a.story, &path, accept)?;
        }
    }
    super::moves::resolve(s, &moves, accept)?;
    s.sel = Selection::caret(a);
    s.clamp_selection();
    if next { select_next_unresolved(s) } else { sel_result(s) }
}

fn changes(s: &Session) -> Vec<Change> {
    let mut out: Vec<Change> = Vec::new();
    let mk = |path: &wordcraft_doc::Path, off| Pos { story: StoryRef::Body, path: path.clone(), off };
    for path in s.doc.para_paths(StoryRef::Body) {
        let Some(p) = s.doc.para(StoryRef::Body, &path) else { continue };
        // A table's, row's and cell's changes sit at the start of the cell's first paragraph.
        if path.last() == 0
            && let Some((tp, r, c)) = path.cell()
            && let Some(t) = s.doc.table(StoryRef::Body, &tp)
        {
            let row = t.rows.get(r);
            let cell = row.and_then(|x| x.cells.get(c));
            let found = [
                (r == 0 && c == 0).then(|| t.props.fmt_change.as_ref().map(|ch| (ch.rev, fmt_revisions::table_diff(&t.props)))).flatten(),
                (c == 0).then(|| row.and_then(|x| x.props.fmt_change.as_ref().map(|ch| (ch.rev, fmt_revisions::row_diff(&x.props))))).flatten(),
                cell.and_then(|x| x.props.fmt_change.as_ref().map(|ch| (ch.rev, fmt_revisions::cell_diff(&x.props)))),
            ];
            for (rev, props) in found.into_iter().flatten() {
                out.push(Change::format(mk(&path, 0), mk(&path, 0), rev, props));
            }
        }
        if let Some(ch) = &p.props.fmt_change {
            out.push(Change::format(mk(&path, 0), mk(&path, p.len()), ch.rev, fmt_revisions::para_diff(&p.props)));
        }
        for (r, c) in p.run_ranges() {
            let kind = run_kind(&s.doc, c.ins, c.del);
            if !kind.is_empty() {
                out.push(Change { a: mk(&path, r.start), b: mk(&path, r.end), kind, rev: c.ins.or(c.del), props: Vec::new() });
            }
            let Some(ch) = &c.fmt_change else { continue };
            let props = fmt_revisions::char_diff(c);
            // Neighbouring runs changed the same way by the same revision are one change.
            if let Some(last) =
                out.last_mut().filter(|l| l.kind == "format" && l.rev == Some(ch.rev) && l.b == mk(&path, r.start) && l.props == props)
            {
                last.b = mk(&path, r.end);
            } else {
                out.push(Change::format(mk(&path, r.start), mk(&path, r.end), ch.rev, props));
            }
        }
        if let Some(ch) = &p.mark.fmt_change {
            let end = mk(&path, p.len());
            out.push(Change::format(end.clone(), end, ch.rev, fmt_revisions::char_diff(&p.mark)));
        }
        out.extend(mark_change(s, StoryRef::Body, &path));
        if let Some(ch) = p.section.as_ref().and_then(|x| x.fmt_change.as_ref().map(|ch| (ch.rev, x))) {
            let end = mk(&path, p.len());
            out.push(Change::format(end.clone(), end, ch.0, fmt_revisions::section_diff(ch.1)));
        }
    }
    if let Some(ch) = &s.doc.last_section.fmt_change {
        let end = s.doc.end_of(StoryRef::Body);
        out.push(Change::format(end.clone(), end, ch.rev, fmt_revisions::section_diff(&s.doc.last_section)));
    }
    out
}

/// The formatting changes in the body, for balloons: (position, author, properties set).
pub fn format_changes(s: &Session) -> Vec<(Pos, String, Vec<Value>)> {
    changes(s)
        .into_iter()
        .filter(|c| c.kind == "format")
        .map(|c| {
            let author = c.rev.and_then(|r| s.doc.revisions.get(r as usize)).map(|r| r.author.clone()).unwrap_or_default();
            (c.a, author, c.props)
        })
        .collect()
}

/// Whether the document has any tracked formatting change (cheap: stops at the first).
pub fn has_format_changes(doc: &wordcraft_doc::Document) -> bool {
    if doc.last_section.fmt_change.is_some() {
        return true;
    }
    doc.para_paths(StoryRef::Body).iter().any(|path| {
        doc.para(StoryRef::Body, path).is_some_and(|p| {
            p.props.fmt_change.is_some()
                || p.mark.fmt_change.is_some()
                || p.runs.iter().any(|r| r.props.fmt_change.is_some())
                || p.section.as_ref().is_some_and(|x| x.fmt_change.is_some())
                || path.cell().and_then(|(tp, r, c)| doc.table(StoryRef::Body, &tp).map(|t| (t, r, c))).is_some_and(|(t, r, c)| {
                    t.props.fmt_change.is_some()
                        || t.rows.get(r).is_some_and(|x| x.props.fmt_change.is_some() || x.cells.get(c).is_some_and(|x| x.props.fmt_change.is_some()))
                })
        })
    })
}

/// A tracked deletion in the body shown in a balloon (Show revisions in balloons).
#[derive(Clone, Debug, PartialEq)]
pub struct DeletionBalloon {
    /// Where the deleted text was: the balloon's anchor.
    pub at: Pos,
    pub author: String,
    /// The deleted text, a deleted paragraph mark as ¶, shortened to [`DELETION_TEXT_CHARS`].
    pub text: String,
}

/// At most this many characters of deleted text show in a balloon (a deleted page shows its start).
pub const DELETION_TEXT_CHARS: usize = 200;
/// At most this many deletion balloons (a document of many small deletions can't flood the area).
const MAX_DELETION_BALLOONS: usize = 500;

/// The tracked deletions in the body, for balloons, in document order. Deleted text running on
/// across runs, revisions and paragraph marks by the same author is one balloon. The source of a
/// move is left out: it has its "Moved from" balloon ([`move_changes`]).
pub fn deletion_balloons(s: &Session) -> Vec<DeletionBalloon> {
    let doc = &s.doc;
    let author = |rev: u32| doc.revisions.get(rev as usize).map(|r| r.author.clone()).unwrap_or_default();
    let mk = |path: &wordcraft_doc::Path, off| Pos { story: StoryRef::Body, path: path.clone(), off };
    let mut out = Vec::new();
    for path in doc.para_paths(StoryRef::Body) {
        let Some(p) = doc.para(StoryRef::Body, &path) else { continue };
        for (r, c) in p.run_ranges() {
            // Inserted and then deleted text is a deletion (it's drawn and left out as one).
            let Some(rev) = plain_deletion(doc, c.del) else { continue };
            push_deletion(&mut out, mk(&path, r.start), mk(&path, r.end), author(rev), p.text.get(r).unwrap_or(""));
        }
        if let Some(rev) = plain_deletion(doc, p.mark.del) {
            let at = mk(&path, p.len());
            let next = path.with_last(path.last().saturating_add(1));
            let end = if super::has_next_para(s, StoryRef::Body, &path) { mk(&next, 0) } else { at.clone() };
            push_deletion(&mut out, at, end, author(rev), "¶");
        }
    }
    out.into_iter().map(|(d, _, _)| d).collect()
}

/// Add `author`'s deleted `text` (from `at` to `end`) to `out`: to the last balloon when that
/// deletion ends at `at` and is theirs, else as a new one. Each balloon carries where its deletion
/// ends and how many characters its text has.
fn push_deletion(out: &mut Vec<(DeletionBalloon, Pos, usize)>, at: Pos, end: Pos, author: String, text: &str) {
    let joins = out.last().is_some_and(|(d, e, _)| *e == at && d.author == author);
    if !joins {
        if out.len() >= MAX_DELETION_BALLOONS {
            return;
        }
        out.push((DeletionBalloon { at, author, text: String::new() }, end.clone(), 0));
    }
    let Some((d, e, chars)) = out.last_mut() else { return };
    *e = end;
    for c in text.chars() {
        if *chars > DELETION_TEXT_CHARS {
            break;
        }
        let c = match c {
            // Pictures, fields and other objects show no text of their own.
            wordcraft_doc::para::OBJ => continue,
            '\t' | wordcraft_doc::para::LINE_BREAK | wordcraft_doc::para::PAGE_BREAK | wordcraft_doc::para::COLUMN_BREAK => ' ',
            c if c.is_control() => continue,
            c => c,
        };
        *chars += 1;
        d.text.push(if *chars > DELETION_TEXT_CHARS { '…' } else { c });
    }
}

/// A deletion revision that is not the source of a move (`w:moveFrom` is a deletion revision
/// with a move name).
fn plain_deletion(doc: &wordcraft_doc::Document, del: Option<u32>) -> Option<u32> {
    del.filter(|_| doc.move_name(del).is_none())
}

/// Whether the body has any tracked deletion other than a move's source (cheap: stops at the
/// first).
pub fn has_deletions(doc: &wordcraft_doc::Document) -> bool {
    doc.para_paths(StoryRef::Body).iter().any(|path| {
        doc.para(StoryRef::Body, path)
            .is_some_and(|p| plain_deletion(doc, p.mark.del).is_some() || p.runs.iter().any(|r| plain_deletion(doc, r.props.del).is_some()))
    })
}

fn nav_change(s: &mut Session, dir: i32) -> CmdResult {
    let list = changes(s);
    let caret = s.sel.ordered();
    let t = if dir > 0 {
        list.iter().find(|c| c.a >= caret.1 && (c.a > caret.0 || c.b > caret.1)).or(list.first())
    } else {
        list.iter().rev().find(|c| c.b <= caret.0 && (c.b < caret.1 || c.a < caret.0)).or(list.last())
    };
    if let Some(c) = t {
        s.sel = Selection { anchor: c.a.clone(), focus: c.b.clone() };
    }
    sel_result(s)
}

/// After a change at the caret is resolved: select the first change from the caret on, including
/// one that starts and ends there (an empty paragraph's, a paragraph mark's or a table's
/// formatting), else the first change in the document.
fn select_next_unresolved(s: &mut Session) -> CmdResult {
    let list = changes(s);
    let caret = s.sel.ordered().0;
    if let Some(c) = list.iter().find(|c| c.a >= caret).or(list.first()) {
        s.sel = Selection { anchor: c.a.clone(), focus: c.b.clone() };
    }
    sel_result(s)
}

fn list_changes(s: &mut Session, _: &Value) -> CmdResult {
    let list = changes(s);
    Ok(Value::Array(
        list.iter()
            .map(|c| {
                let rev = c.rev.and_then(|r| s.doc.revisions.get(r as usize));
                let mut v = json!({
                    "kind": c.kind, "start": pos_json(&c.a), "end": pos_json(&c.b),
                    "text": s.doc.copy_range(&c.a, &c.b).plain_text(),
                    "author": rev.map(|r| r.author.clone()), "date": rev.map(|r| r.date.clone()),
                });
                if c.kind == "format" {
                    v["description"] = json!(fmt_revisions::describe(&c.props));
                    v["props"] = json!(c.props);
                }
                // One end of a move: its name links it with the other end.
                if let Some(name) = rev.and_then(|r| r.move_name.as_deref()).filter(|_| c.kind.starts_with("move")) {
                    v["move"] = json!(name);
                    v["description"] = json!(if c.kind == "moveFrom" { "Moved from" } else { "Moved to" });
                }
                v
            })
            .collect(),
    ))
}

fn word_count(s: &mut Session, v: &Value) -> CmdResult {
    if let Some(b) = p::bool(v, "includeTextBoxes") {
        s.prefs.count_notes = b;
    }
    let stories = if s.prefs.count_notes { s.doc.counted_stories() } else { vec![StoryRef::Body] };
    // A selected picture, shape or text box isn't a text selection: count the document.
    let text = if s.sel.is_collapsed() || super::objects::object_selection(s).is_some() {
        stories.iter().map(|st| s.doc.plain_text(*st)).collect::<Vec<_>>().join("\n")
    } else {
        s.selected_text()
    };
    let words = if s.sel.is_collapsed() || super::objects::object_selection(s).is_some() {
        stories.iter().map(|st| s.doc.story_word_count(*st)).sum()
    } else {
        wordcraft_doc::count_words(&text)
    };
    let chars = text.chars().filter(|c| *c != '\n' && *c != '\r').count();
    let chars_no_spaces = text.chars().filter(|c| !c.is_whitespace()).count();
    let paragraphs = text.split('\n').filter(|l| !l.trim().is_empty()).count();
    let lines: usize = {
        let l = s.layout();
        l.pages
            .iter()
            .flat_map(|p| p.items.iter())
            .map(|it| {
                if let wordcraft_layout::Placed::Lines { l0, l1, story, .. } = it
                    && stories.contains(story)
                {
                    l1 - l0
                } else {
                    0
                }
            })
            .sum()
    };
    let pages = s.layout().pages.len();
    Ok(json!({
        "pages": pages, "words": words, "characters": chars_no_spaces, "charactersWithSpaces": chars, "paragraphs": paragraphs, "lines": lines,
        "includeTextBoxes": s.prefs.count_notes,
    }))
}

/// Issues (spelling + grammar) in one paragraph as positions, each in its run's language and
/// skipping text left unchecked — the same issues the layout underlines.
fn para_issues(s: &Session, story: StoryRef, path: &wordcraft_doc::Path) -> Vec<(Pos, Pos, wordcraft_proof::Issue)> {
    let Some(p) = s.doc.para(story, path) else { return Vec::new() };
    let runs = wordcraft_layout::para::proof_runs(&s.doc, p, None);
    wordcraft_layout::para::paragraph_issues(p, &runs, &s.prefs.spelling(), true, wordcraft_layout::para::is_list_item(&s.doc, p))
        .into_iter()
        .map(|i| (Pos { story, path: path.clone(), off: i.start }, Pos { story, path: path.clone(), off: i.end }, i))
        .collect()
}

/// The issue under a position.
fn issue_at(s: &Session, at: &Pos) -> Option<(Pos, Pos, wordcraft_proof::Issue)> {
    para_issues(s, at.story, &at.path).into_iter().find(|(a, b, _)| a.off <= at.off && at.off <= b.off)
}

fn issue_json(s: &Session, a: &Pos, b: &Pos, i: &wordcraft_proof::Issue) -> Value {
    let word = s.doc.para_at(a).and_then(|p| p.text.get(a.off..b.off)).unwrap_or("").to_string();
    let sugg = if i.kind == wordcraft_proof::IssueKind::Spelling { wordcraft_proof::suggest_in(&word, 6, i.lang) } else { i.suggestions.clone() };
    json!({
        "start": pos_json(a), "end": pos_json(b), "text": word,
        "kind": if i.kind == wordcraft_proof::IssueKind::Spelling { "spelling" } else { "grammar" },
        "message": i.message, "suggestions": sugg, "lang": i.lang.code(),
    })
}

/// F7: select the next issue after the caret and return it with suggestions.
fn next_issue(s: &mut Session, _: &Value) -> CmdResult {
    let caret = s.sel.ordered().1;
    let story = caret.story;
    let paths = s.doc.para_paths(story);
    let start = paths.iter().position(|p| *p == caret.path).unwrap_or(0);
    for k in 0..paths.len() {
        let Some(path) = paths.get((start + k) % paths.len().max(1)) else { break };
        for (a, b, i) in para_issues(s, story, path) {
            if k == 0 && a.off < caret.off && start + k < paths.len() && paths.len() > 1 {
                continue;
            }
            let r = issue_json(s, &a, &b, &i);
            s.sel = Selection { anchor: a, focus: b };
            return Ok(r);
        }
    }
    s.status = "The spelling and grammar check is complete.".into();
    Ok(json!({"done": true}))
}

fn all_issues(s: &mut Session, _: &Value) -> CmdResult {
    let mut out = Vec::new();
    for path in s.doc.para_paths(StoryRef::Body) {
        for (a, b, i) in para_issues(s, StoryRef::Body, &path) {
            out.push(issue_json(s, &a, &b, &i));
            if out.len() >= 500 {
                return Ok(Value::Array(out));
            }
        }
    }
    Ok(Value::Array(out))
}

fn suggestions(s: &mut Session, v: &Value) -> CmdResult {
    let at = v.get("pos").and_then(super::parse_pos).unwrap_or_else(|| s.sel.focus.clone());
    match issue_at(s, &at) {
        Some((a, b, i)) => Ok(issue_json(s, &a, &b, &i)),
        None => Ok(Value::Null),
    }
}

/// A tiny built-in thesaurus of common words (our own list).
fn thesaurus(s: &mut Session, v: &Value) -> CmdResult {
    const T: &[(&str, &[&str])] = &[
        ("good", &["fine", "excellent", "decent", "solid", "worthy"]),
        ("bad", &["poor", "inferior", "faulty", "unpleasant"]),
        ("big", &["large", "huge", "sizable", "vast", "major"]),
        ("small", &["little", "tiny", "compact", "minor", "modest"]),
        ("fast", &["quick", "rapid", "swift", "speedy"]),
        ("slow", &["unhurried", "gradual", "leisurely", "sluggish"]),
        ("happy", &["glad", "cheerful", "content", "joyful", "pleased"]),
        ("sad", &["unhappy", "sorrowful", "downcast", "gloomy"]),
        ("important", &["significant", "key", "vital", "essential", "major"]),
        ("show", &["display", "reveal", "present", "demonstrate"]),
        ("make", &["create", "build", "produce", "craft", "form"]),
        ("use", &["employ", "apply", "utilize", "operate"]),
        ("help", &["assist", "aid", "support", "serve"]),
        ("start", &["begin", "launch", "open", "initiate"]),
        ("end", &["finish", "close", "conclude", "stop"]),
        ("say", &["state", "tell", "remark", "mention"]),
        ("think", &["believe", "consider", "reckon", "suppose"]),
        ("new", &["fresh", "novel", "recent", "modern"]),
        ("old", &["aged", "former", "ancient", "vintage"]),
        ("beautiful", &["lovely", "attractive", "gorgeous", "stunning"]),
        ("easy", &["simple", "effortless", "straightforward"]),
        ("hard", &["difficult", "tough", "demanding", "firm"]),
        ("work", &["labor", "effort", "job", "task", "operate"]),
        ("idea", &["concept", "notion", "thought", "plan"]),
        ("change", &["alter", "modify", "adjust", "shift"]),
        ("great", &["excellent", "superb", "terrific", "considerable"]),
        ("very", &["extremely", "highly", "truly", "really"]),
        ("shared", &["common", "joint", "communal", "mutual"]),
        ("community", &["group", "society", "collective", "circle"]),
    ];
    let word = match p::str(v, "word") {
        Some(w) => w.to_lowercase(),
        None => {
            let f = s.sel.focus.clone();
            let p = s.doc.para_at(&f).ok_or_else(|| CmdError::Failed("no word".into()))?;
            let (a, b) = p.word_at(f.off);
            p.text.get(a..b).unwrap_or("").trim().to_lowercase()
        }
    };
    let syn = T.iter().find(|(w, _)| *w == word).map(|(_, l)| l.to_vec()).unwrap_or_default();
    Ok(json!({"word": word, "synonyms": syn}))
}

/// Track Changes Options: change the fields given, return them all. A per-user preference.
fn tracking_options(s: &mut Session, v: &Value) -> CmdResult {
    use wordcraft_doc::Rgb;
    use wordcraft_layout::display::{BalloonMode, ChangeBar, CommentDisplay, DeleteMark, InsertMark};
    let mut m = s.prefs.markup.clone();
    let color = |k: &str, auto: &str, cur: Option<Rgb>| -> Result<Option<Rgb>, CmdError> {
        match p::str(v, k) {
            None => Ok(cur),
            Some(c) if c == auto => Ok(None),
            Some(c) => Rgb::parse(c).map(Some).ok_or_else(|| CmdError::Params(format!("`{k}`: \"{auto}\" or RRGGBB"))),
        }
    };
    let pick = |k: &str| p::str(v, k);
    for (k, f) in [
        ("comments", &mut m.comments),
        ("ink", &mut m.ink),
        ("insertionsDeletions", &mut m.insertions_deletions),
        ("formatting", &mut m.formatting),
        ("trackFormatting", &mut m.track_formatting),
    ] {
        if let Some(b) = p::bool(v, k) {
            *f = b;
        }
    }
    if let Some(x) = pick("balloons") {
        m.balloons = BalloonMode::parse(x).ok_or_else(|| CmdError::Params("`balloons`: revisions|inline|commentsAndFormatting".into()))?;
    }
    if let Some(x) = pick("commentDisplay") {
        m.comment_display = CommentDisplay::parse(x).ok_or_else(|| CmdError::Params("`commentDisplay`: margin|selected".into()))?;
    }
    if let Some(x) = pick("insertMark") {
        m.insert_mark = InsertMark::parse(x).ok_or_else(|| CmdError::Params("unknown `insertMark`".into()))?;
    }
    if let Some(x) = pick("deleteMark") {
        m.delete_mark = DeleteMark::parse(x).ok_or_else(|| CmdError::Params("unknown `deleteMark`".into()))?;
    }
    if let Some(x) = pick("moveFromMark") {
        m.move_from_mark = DeleteMark::parse(x).ok_or_else(|| CmdError::Params("unknown `moveFromMark`".into()))?;
    }
    if let Some(x) = pick("moveToMark") {
        m.move_to_mark = InsertMark::parse(x).ok_or_else(|| CmdError::Params("unknown `moveToMark`".into()))?;
    }
    if let Some(x) = pick("changedLines") {
        m.changed_lines = ChangeBar::parse(x).ok_or_else(|| CmdError::Params("unknown `changedLines`".into()))?;
    }
    m.insert_color = color("insertColor", "byAuthor", m.insert_color)?;
    m.delete_color = color("deleteColor", "byAuthor", m.delete_color)?;
    m.changed_lines_color = color("changedLinesColor", "auto", m.changed_lines_color)?;
    m.move_from_color = color("moveFromColor", "byAuthor", m.move_from_color)?;
    m.move_to_color = color("moveToColor", "byAuthor", m.move_to_color)?;
    // Track moves is the document's (`w:doNotTrackMoves`), so changing it is an undo step; the
    // command is pure, so it records the step itself, once every parameter has been accepted.
    // The other options are the user's own, so the command stays available under Restrict
    // Editing; this one edits the document, so it passes the same check as an edit and isn't
    // limited to an exception.
    if let Some(b) = p::bool(v, "trackMoves")
        && b != s.doc.settings.track_moves
    {
        if super::protect::check_before(s, "review.trackingOptions", &json!({}))? {
            return Err(CmdError::Disabled(
                "review.trackingOptions: the document is protected; stop protection to change whether moves are tracked".into(),
            ));
        }
        s.checkpoint("Track Changes Options");
        s.doc.settings.track_moves = b;
        s.touch();
    }
    if m != s.prefs.markup {
        s.prefs.markup = m;
        // Hidden deletions leave the layout.
        s.relayout();
    }
    let mut out = tracking_json(&s.prefs.markup);
    out["trackMoves"] = json!(s.doc.settings.track_moves);
    Ok(out)
}

/// The Track Changes Options as `review.trackingOptions` takes them.
pub fn tracking_json(m: &wordcraft_layout::display::MarkupOptions) -> Value {
    let hex = |c: Option<wordcraft_doc::Rgb>, auto: &str| c.map(|c| c.hex()).unwrap_or_else(|| auto.to_string());
    json!({
        "comments": m.comments,
        "ink": m.ink,
        "insertionsDeletions": m.insertions_deletions,
        "formatting": m.formatting,
        "balloons": m.balloons.name(),
        "commentDisplay": m.comment_display.name(),
        "insertMark": m.insert_mark.name(),
        "insertColor": hex(m.insert_color, "byAuthor"),
        "deleteMark": m.delete_mark.name(),
        "deleteColor": hex(m.delete_color, "byAuthor"),
        "changedLines": m.changed_lines.name(),
        "changedLinesColor": hex(m.changed_lines_color, "auto"),
        "trackFormatting": m.track_formatting,
        "moveFromMark": m.move_from_mark.name(),
        "moveFromColor": hex(m.move_from_color, "byAuthor"),
        "moveToMark": m.move_to_mark.name(),
        "moveToColor": hex(m.move_to_color, "byAuthor"),
    })
}

#[cfg(test)]
mod tests;
