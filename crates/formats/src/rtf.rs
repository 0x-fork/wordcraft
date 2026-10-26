//! Rich Text Format.
//!
//! Export: font and colour tables, a small stylesheet (headings with outline levels, Title,
//! Quote, Code), list tables (`\listtable` / `\listoverridetable` with `\listtext` labels for
//! older readers), document info, page setup, paragraph formatting (`\pard`, alignment, page
//! break before), character formatting (`\b \i \ul \strike \super \sub \f \fs \cf \chcbpat`),
//! hyperlinks as `HYPERLINK` fields, bookmarks, pictures (`\pngblip` / `\jpegblip`) and tables
//! (`\trowd … \cellx … \cell … \row`, header rows, vertical merges, shading).
//!
//! Import: a tolerant reader for groups and destinations (unknown `\*` destinations, headers,
//! footers, notes and objects are skipped), `\par \line \tab \page`, `\uN` with `\ucN`
//! fallback skipping, `\'hh` and 8-bit text in the document's code page (`\ansicpg`, `\mac`, `\pc`, `\pca`) or
//! the current font's (`\fcharset`, `\cpg`: Cyrillic, Central European, Greek, Mac OS Roman, the
//! East Asian multibyte pages 932/936/949/950…; Windows-1252 when none is named). A multibyte
//! code page's bytes stream through one decoder, so a lead byte and its trail byte decode
//! together and a run that looks like a UTF-8 BOM is not mistaken for one; the formatting words
//! above, stylesheet-based
//! headings and `\outlinelevel`, lists through `\ls`/`\ilvl`, fields, bookmarks, pictures and
//! tables. Group nesting and picture sizes are capped.

use std::collections::HashMap;
use std::sync::Arc;

use wordcraft_doc::props::VAlign;
use wordcraft_doc::{Align, Document, Rgb};

use crate::model::{self, Cell, FBlock, FTable, Flow, Fmt, Inline, Kind, ListInfo, Meta, Para, RevMark, make_img};

/// Deepest group nesting we track; deeper groups share the innermost state.
const MAX_GROUPS: usize = 512;
/// Largest picture we decode (bytes).
const MAX_PICT: usize = 64 << 20;
/// Most sections we read (`\sect`); later breaks end paragraphs only.
const MAX_SECTIONS: usize = 10_000;
/// The encoding of an East Asian multibyte code page (932 Shift-JIS, 936 GBK, 949 UHC, 950 Big5),
/// which [`Reader::finish_multibyte`] decodes a run with; `None` for the single-byte code pages.
fn multibyte_encoding(code_page: u16) -> Option<&'static encoding_rs::Encoding> {
    let label = wordcraft_doc::encoding::multibyte_encoding_name(code_page)?;
    encoding_rs::Encoding::for_label(label.as_bytes())
}

// ---------------------------------------------------------------------------------------------
// Export

/// RTF-escape text (non-ASCII as `\uN?`).
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '{' => o.push_str("\\{"),
            '}' => o.push_str("\\}"),
            '\t' => o.push_str("\\tab "),
            '\n' => o.push_str("\\line "),
            '\u{000C}' => o.push_str("\\page "),
            '\u{000E}' => o.push_str("\\column "),
            '\u{00A0}' => o.push_str("\\~"),
            '\u{00AD}' => o.push_str("\\-"),
            '\u{2011}' => o.push_str("\\_"),
            c if c.is_ascii_control() => {}
            c if c.is_ascii() => o.push(c),
            c => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    o.push_str(&format!("\\u{}?", *u as i16));
                }
            }
        }
    }
    o
}

struct Tables {
    fonts: Vec<String>,
    colors: Vec<Rgb>,
}

impl Tables {
    fn font(&mut self, name: &str) -> usize {
        match self.fonts.iter().position(|f| f.eq_ignore_ascii_case(name)) {
            Some(i) => i,
            None => {
                self.fonts.push(name.to_string());
                self.fonts.len() - 1
            }
        }
    }
    /// Colour table index (1-based; 0 is "auto").
    fn color(&mut self, c: Rgb) -> usize {
        match self.colors.iter().position(|x| *x == c) {
            Some(i) => i + 1,
            None => {
                self.colors.push(c);
                self.colors.len()
            }
        }
    }
}

struct Writer {
    t: Tables,
    /// List definitions: per list, the ordered flag per level.
    lists: Vec<[bool; 9]>,
    /// Authors of tracked changes (`\revtbl` entries after "Unknown"), when the text has any.
    authors: Option<Vec<String>>,
    /// The character formatting the current paragraph's style gives its runs (heading bold and
    /// size, code font…): what a run's formatting without a direct value resolves to.
    base: Fmt,
    /// The body's sections in order (the last is the document's); empty when it has just one.
    sections: Vec<wordcraft_doc::SectionProps>,
    /// The section being written.
    sect_i: usize,
}

/// `\sectd` and a section's formatting, in the words the reader reads back.
fn sect_words(s: &wordcraft_doc::SectionProps) -> String {
    use wordcraft_doc::section::{NumFormat, SectionStart};
    let tw = |v: f32| if v.is_finite() { ((v * 20.0).round() as i64).max(0) } else { 0 };
    let mut o = String::from("\\sectd");
    o.push_str(match s.start {
        SectionStart::Continuous => "\\sbknone",
        SectionStart::NextColumn => "\\sbkcol",
        SectionStart::NextPage => "\\sbkpage",
        SectionStart::EvenPage => "\\sbkeven",
        SectionStart::OddPage => "\\sbkodd",
    });
    o.push_str(&format!("\\pgwsxn{}\\pghsxn{}", tw(s.page_w), tw(s.page_h)));
    if s.landscape {
        o.push_str("\\lndscpsxn");
    }
    o.push_str(&format!(
        "\\marglsxn{}\\margrsxn{}\\margtsxn{}\\margbsxn{}\\headery{}\\footery{}",
        tw(s.margin_left),
        tw(s.margin_right),
        tw(s.margin_top),
        tw(s.margin_bottom),
        tw(s.header),
        tw(s.footer)
    ));
    if s.columns.count > 1 {
        o.push_str(&format!("\\cols{}\\colsx{}", s.columns.count, tw(s.columns.space)));
        if s.columns.separator {
            o.push_str("\\linebetcol");
        }
    }
    if let Some(n) = s.page_num_start {
        o.push_str(&format!("\\pgnrestart\\pgnstarts{n}"));
    }
    match s.page_num_format {
        NumFormat::UpperRoman => o.push_str("\\pgnucrm"),
        NumFormat::LowerRoman => o.push_str("\\pgnlcrm"),
        NumFormat::UpperLetter => o.push_str("\\pgnucltr"),
        NumFormat::LowerLetter => o.push_str("\\pgnlcltr"),
        _ => {}
    }
    if s.title_page {
        o.push_str("\\titlepg");
    }
    o.push('\n');
    o
}

const HEAD_SIZES: [f32; 6] = [20.0, 16.0, 14.0, 12.0, 12.0, 11.0];

fn style_num(k: Kind) -> Option<u32> {
    match k {
        Kind::Heading(n) => Some(u32::from(n.clamp(1, 6))),
        Kind::Title => Some(7),
        Kind::Quote => Some(8),
        Kind::Code => Some(9),
        _ => None,
    }
}

fn rtf_border_style(s: wordcraft_doc::props::BorderStyle) -> &'static str {
    use wordcraft_doc::props::BorderStyle;
    match s {
        BorderStyle::None => "\\brdrnone",
        BorderStyle::Single => "\\brdrs",
        BorderStyle::Double => "\\brdrdb",
        BorderStyle::Dotted => "\\brdrdot",
        BorderStyle::Dashed => "\\brdrdash",
        BorderStyle::Thick => "\\brdrth",
        BorderStyle::Triple => "\\brdrtriple",
        BorderStyle::DotDash => "\\brdrdashd",
        BorderStyle::Wave => "\\brdrwavy",
    }
}

impl Writer {
    fn write_border(&mut self, prefix: &str, side: char, b: wordcraft_doc::props::Border, out: &mut String) {
        out.push_str(&format!("\\{prefix}{side}"));
        out.push_str(rtf_border_style(b.style));
        let w = ((b.width * 20.0).round() as i64).clamp(1, 150);
        out.push_str(&format!("\\brdrw{w}"));
        if prefix == "brdr" {
            let sp = (b.space * 20.0).round() as i64;
            if sp > 0 {
                out.push_str(&format!("\\brsp{sp}"));
            }
        }
        if let Some(c) = b.color {
            let i = self.t.color(c);
            out.push_str(&format!("\\brdrcf{i}"));
        }
    }

    fn cell_borders(&mut self, borders: Option<&wordcraft_doc::props::Borders>, borderless: bool, out: &mut String) {
        if let Some(b) = borders {
            for (side, opt) in [('t', &b.top), ('l', &b.left), ('b', &b.bottom), ('r', &b.right)] {
                if let Some(border) = opt.filter(|x| x.is_visible()) {
                    self.write_border("clbrdr", side, border, out);
                }
            }
        } else if !borderless {
            out.push_str("\\clbrdrt\\brdrs\\brdrw10\\clbrdrl\\brdrs\\brdrw10\\clbrdrb\\brdrs\\brdrw10\\clbrdrr\\brdrs\\brdrw10");
        }
    }

    /// `\revtbl` index of a tracked change's author (0 = "Unknown").
    fn author(&mut self, a: &str) -> usize {
        let authors = self.authors.get_or_insert_with(Vec::new);
        let a = a.trim();
        if a.is_empty() {
            return 0;
        }
        match authors.iter().position(|x| x == a) {
            Some(i) => i + 1,
            None => {
                authors.push(a.to_string());
                authors.len()
            }
        }
    }

    /// Control words of a tracked insertion and deletion (`\revised`, `\deleted`).
    fn rev_ctl(&mut self, ins: Option<&RevMark>, del: Option<&RevMark>) -> String {
        let mut ctl = String::new();
        if let Some(m) = ins {
            ctl.push_str(&self.rev_words("\\revised\\revauth", "\\revdttm", m));
        }
        if let Some(m) = del {
            ctl.push_str(&self.rev_words("\\deleted\\revauthdel", "\\revdttmdel", m));
        }
        ctl
    }

    /// `auth` with the `\revtbl` index of `m`'s author, then `date` with its DTTM when it has one.
    fn rev_words(&mut self, auth: &str, date: &str, m: &RevMark) -> String {
        let mut s = format!("{auth}{}", self.author(&m.author));
        if let Some(d) = iso_to_dttm(&m.date) {
            s.push_str(&format!("{date}{}", d as i32));
        }
        s
    }

    fn run(&mut self, t: &str, f: &Fmt, out: &mut String) {
        let mut ctl = String::new();
        if let Some(c) = &f.fmt_change {
            // RTF 1.9.1, as Word writes it: the properties the change altered, as they were,
            // then who changed it and when, all in the group. The group goes first so a control
            // word, not the group, ends the run's properties.
            let old = self.char_diff(&c.old, f);
            let words = self.rev_words("\\crauth", "\\crdate", &c.mark);
            ctl.push_str(&format!("{{\\*\\oldcprops {old}{words}}}"));
        }
        ctl.push_str(&self.rev_ctl(f.ins.as_ref(), f.del.as_ref()));
        ctl.push_str(&self.char_ctl(f));
        let body = if ctl.is_empty() { esc(t) } else { format!("{{{ctl} {}}}", esc(t)) };
        // A script, `file:`, UNC or other non-web target is written as plain text, as the HTML and
        // Markdown exporters do: Word follows a HYPERLINK field's target on click, and a UNC one
        // leaks the reader's NTLM credentials.
        match f.link.as_deref().filter(|l| wordcraft_doc::link::is_safe_href(l)) {
            Some(l) => {
                out.push_str(&format!("{{\\field{{\\*\\fldinst{{HYPERLINK \"{}\"}}}}{{\\fldrslt{{{}}}}}}}", esc(&l.replace('"', "%22")), body));
            }
            None => out.push_str(&body),
        }
    }

    /// The character formatting control words that turn `cur` back into `old`: only the
    /// properties that differ, with explicit "off" values (`\b0`, `\ulnone`, `\cf0`…).
    fn char_diff(&mut self, old: &Fmt, cur: &Fmt) -> String {
        // A link's underline and colour are the link's, before the change and after; what a run
        // doesn't set directly is its paragraph style's, as Word writes resolved values.
        let old = &styled(&self.base, &Fmt { link: cur.link.clone(), ..old.clone() });
        let cur = &styled(&self.base, cur);
        let mut ctl = String::new();
        let underline = |f: &Fmt| f.underline || f.link.is_some();
        let flags = [
            (old.bold, cur.bold, "\\b", "\\b0"),
            (old.italic, cur.italic, "\\i", "\\i0"),
            (underline(old), underline(cur), "\\ul", "\\ulnone"),
            (old.strike, cur.strike, "\\strike", "\\strike0"),
        ];
        for (o, c, on, off) in flags {
            if o != c {
                ctl.push_str(if o { on } else { off });
            }
        }
        if (old.sup, old.sub) != (cur.sup, cur.sub) {
            ctl.push_str(if old.sup {
                "\\super"
            } else if old.sub {
                "\\sub"
            } else {
                "\\nosupersub"
            });
        }
        fn font(f: &Fmt) -> Option<&str> {
            if f.code { Some(model::MONO_FONT) } else { f.font.as_deref() }
        }
        let old_font = font(old);
        if old_font != font(cur) {
            // No font is the document's default font, font 0 (Normal's).
            let i = old_font.map_or(0, |n| self.t.font(n));
            ctl.push_str(&format!("\\f{i}"));
        }
        let half_points = |s: Option<f32>| s.map(|s| (s * 2.0).round().clamp(2.0, 3276.0) as i32);
        let old_size = half_points(old.size);
        if old_size != half_points(cur.size) {
            // No size is Normal's, RTF's default 12 pt.
            ctl.push_str(&format!("\\fs{}", old_size.unwrap_or(24)));
        }
        let color = |f: &Fmt| f.color.or(if f.link.is_some() { Some(Rgb(0x05, 0x63, 0xC1)) } else { None });
        let old_color = color(old);
        if old_color != color(cur) {
            let i = old_color.map_or(0, |c| self.t.color(c));
            ctl.push_str(&format!("\\cf{i}"));
        }
        if old.background != cur.background {
            let i = old.background.map_or(0, |c| self.t.color(c));
            ctl.push_str(&format!("\\chcbpat{i}"));
        }
        ctl
    }

    /// Character formatting control words of `f` (no revision marks).
    fn char_ctl(&mut self, f: &Fmt) -> String {
        let mut ctl = String::new();
        if f.bold {
            ctl.push_str("\\b");
        }
        if f.italic {
            ctl.push_str("\\i");
        }
        if f.underline || f.link.is_some() {
            ctl.push_str("\\ul");
        }
        if f.strike {
            ctl.push_str("\\strike");
        }
        if f.sup {
            ctl.push_str("\\super");
        } else if f.sub {
            ctl.push_str("\\sub");
        }
        if f.code {
            let i = self.t.font(model::MONO_FONT);
            ctl.push_str(&format!("\\f{i}"));
        } else if let Some(fam) = &f.font {
            let i = self.t.font(fam);
            ctl.push_str(&format!("\\f{i}"));
        }
        if let Some(s) = f.size {
            ctl.push_str(&format!("\\fs{}", (s * 2.0).round().clamp(2.0, 3276.0) as i32));
        }
        let color = f.color.or(if f.link.is_some() { Some(Rgb(0x05, 0x63, 0xC1)) } else { None });
        if let Some(c) = color {
            let i = self.t.color(c);
            ctl.push_str(&format!("\\cf{i}"));
        }
        if let Some(c) = f.background {
            let i = self.t.color(c);
            ctl.push_str(&format!("\\chcbpat{i}"));
        }
        ctl
    }

    fn picture(&mut self, img: &model::Img, out: &mut String) {
        let (data, blip) = match img.ext.as_str() {
            "png" => (img.data.to_vec(), "\\pngblip"),
            "jpeg" | "jpg" => (img.data.to_vec(), "\\jpegblip"),
            _ => {
                // Re-encode other formats as PNG, bounding the decode so a tiny file declaring
                // huge dimensions can't exhaust memory on export.
                let Some(dec) = model::decode_bounded(&img.data) else { return };
                let mut buf = Vec::new();
                if dec.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).is_err() {
                    return;
                }
                (buf, "\\pngblip")
            }
        };
        let (pw, ph) = model::image_px(&data).unwrap_or((1, 1));
        out.push_str(&format!(
            "{{\\pict{blip}\\picw{pw}\\pich{ph}\\picwgoal{}\\pichgoal{}\n",
            (img.w * 20.0).round() as i64,
            (img.h * 20.0).round() as i64
        ));
        for chunk in data.chunks(64) {
            for b in chunk {
                out.push_str(&format!("{b:02x}"));
            }
            out.push('\n');
        }
        out.push('}');
    }

    fn inlines(&mut self, inl: &[Inline], out: &mut String) {
        let inl = model::equations_as_text(inl);
        for i in inl.iter() {
            match i {
                Inline::Text(t, f) => self.run(t, f, out),
                Inline::Image(img) => self.picture(img, out),
                Inline::Anchor(a) => {
                    let a = esc(a);
                    out.push_str(&format!("{{\\*\\bkmkstart {a}}}{{\\*\\bkmkend {a}}}"));
                }
                // RTF keeps no ruby yet: its base text, in the run's formatting.
                Inline::Ruby { base, fmt, .. } => self.run(base, fmt, out),
                Inline::Figure(_) | Inline::Equation { .. } => {}
            }
        }
    }

    fn para(&mut self, p: &Para, intbl: bool, ls: Option<(usize, String)>, end: &str, out: &mut String) {
        let ctl = self.para_ctl(p, intbl);
        out.push_str(&ctl);
        if let Some(c) = &p.fmt_change {
            // RTF 1.9.1: the paragraph formatting before the change, then who changed it and when.
            let old = self.para_ctl(&c.old, intbl);
            // The old list is the same list instance (only its level may differ).
            let list = match (c.old.list, &ls) {
                (Some(li), Some((n, _))) => format!("\\ls{n}\\ilvl{}", li.level.min(8)),
                _ => String::new(),
            };
            out.push_str(&format!("{{\\*\\oldpprops {old}{list}}}"));
            let words = self.rev_words("\\prauth", "\\prdate", &c.mark);
            out.push_str(&words);
        }
        if p.kind != Kind::Rule
            && let Some(b) = &p.borders
        {
            for (side, opt) in [('t', &b.top), ('l', &b.left), ('b', &b.bottom), ('r', &b.right)] {
                if let Some(border) = opt.filter(|x| x.is_visible()) {
                    self.write_border("brdr", side, border, out);
                }
            }
        }
        if let (Some(li), Some((n, label))) = (p.list, &ls) {
            let lv = li.level.min(8);
            out.push_str(&format!("\\ls{n}\\ilvl{lv}\\fi-360\\li{}", 720 * (u32::from(lv) + 1)));
            // The closing brace ends the label; a space after it would be text.
            out.push_str(&format!(" {{\\listtext\\pard\\plain {}\\tab}}", esc(label)));
        } else {
            out.push(' ');
        }
        self.base = style_fmt(p.kind);
        self.inlines(&p.inlines, out);
        self.base = Fmt::default();
        // A tracked paragraph mark is the `\par` inside the revision's group.
        let marks = self.rev_ctl(p.mark_ins.as_deref(), p.mark_del.as_deref());
        if end == "\\par" && !marks.is_empty() {
            out.push_str(&format!("{{{marks} {end}}}"));
        } else {
            out.push_str(end);
        }
        out.push('\n');
    }

    /// The paragraph formatting control words of `p`, from `\pard\plain` (its list left out).
    fn para_ctl(&mut self, p: &Para, intbl: bool) -> String {
        let mut out = String::from("\\pard\\plain");
        if intbl {
            out.push_str("\\intbl");
        }
        if let Some(s) = style_num(p.kind) {
            out.push_str(&format!("\\s{s}"));
        }
        if let Kind::Heading(n) = p.kind {
            out.push_str(&format!("\\outlinelevel{}", n.clamp(1, 6) - 1));
        }
        match p.align {
            Some(Align::Center) => out.push_str("\\qc"),
            Some(Align::Right) => out.push_str("\\qr"),
            Some(Align::Justify) => out.push_str("\\qj"),
            Some(Align::Distribute) => out.push_str("\\qd"),
            _ => out.push_str("\\ql"),
        }
        if p.page_break {
            out.push_str("\\pagebb");
        }
        match p.kind {
            Kind::Quote => out.push_str("\\li720\\ri720\\i"),
            Kind::Code => {
                let f = self.t.font(model::MONO_FONT);
                out.push_str(&format!("\\sa0\\f{f}\\fs20"));
            }
            Kind::Rule => out.push_str("\\brdrb\\brdrs\\brdrw15\\brsp20"),
            Kind::Heading(n) => {
                let sz = HEAD_SIZES.get(usize::from(n.clamp(1, 6)) - 1).copied().unwrap_or(12.0);
                out.push_str(&format!("\\sb240\\sa80\\keepn\\b\\fs{}", (sz * 2.0) as i32));
            }
            Kind::Title => out.push_str("\\sa80\\fs56"),
            Kind::Normal => {}
        }
        out
    }

    fn blocks(&mut self, blocks: &[FBlock], intbl: bool, last_end: &str, out: &mut String, depth: usize) {
        let mut counter = model::ListCounter::default();
        let mut cur_list: Option<usize> = None;
        let mut top_ordered = false;
        let n = blocks.len();
        for (i, b) in blocks.iter().enumerate() {
            let mut end = if i + 1 == n { last_end.to_string() } else { "\\par".to_string() };
            match b {
                FBlock::Para(p) => {
                    // A paragraph that ends a section ends it with `\sect`; the next one begins.
                    if p.section.is_some() && !intbl && depth == 0 && self.sect_i + 1 < self.sections.len() {
                        self.sect_i += 1;
                        end = format!("\\sect\n{}", self.sections.get(self.sect_i).map(sect_words).unwrap_or_default());
                        // A body that ends on the break keeps its last section: an empty paragraph holds it.
                        if i + 1 == n {
                            end.push_str("\\pard\\plain \\par");
                        }
                    }
                    let ls = match p.list {
                        Some(li) => {
                            // A top-level item of the other kind starts a new list.
                            if li.level == 0 && cur_list.is_some() && top_ordered != li.ordered {
                                cur_list = None;
                            }
                            if li.level == 0 {
                                top_ordered = li.ordered;
                            }
                            let id = match cur_list {
                                Some(id) => id,
                                None => {
                                    // Level kinds from the whole run of list paragraphs.
                                    let mut kinds = [li.ordered; 9];
                                    let mut seen = [false; 9];
                                    for q in blocks.iter().skip(i) {
                                        let FBlock::Para(q) = q else { break };
                                        let Some(ql) = q.list else { break };
                                        let l = ql.level.min(8) as usize;
                                        if l == 0 && seen[0] && kinds[0] != ql.ordered {
                                            break;
                                        }
                                        if let (Some(s), Some(k)) = (seen.get_mut(l), kinds.get_mut(l))
                                            && !*s
                                        {
                                            *s = true;
                                            *k = ql.ordered;
                                        }
                                    }
                                    self.lists.push(kinds);
                                    counter.reset();
                                    self.lists.len()
                                }
                            };
                            cur_list = Some(id);
                            Some((id, counter.label(li)))
                        }
                        None => {
                            cur_list = None;
                            None
                        }
                    };
                    self.para(p, intbl, ls, &end, out);
                }
                FBlock::Table(t) => {
                    cur_list = None;
                    if intbl || depth > 0 {
                        // Nested table: its cells' paragraphs flow into this cell.
                        let inner: Vec<FBlock> = t.rows.iter().flatten().flat_map(|c| c.blocks.clone()).collect();
                        if depth < model::MAX_DEPTH {
                            self.blocks(&inner, intbl, &end, out, depth + 1);
                        }
                    } else {
                        self.table(t, out, depth);
                    }
                }
            }
        }
        if blocks.is_empty() && !last_end.is_empty() {
            out.push_str(&format!("\\pard\\plain{} {last_end}\n", if intbl { "\\intbl" } else { "" }));
        }
    }

    fn table(&mut self, t: &FTable, out: &mut String, depth: usize) {
        let cols = t.cols().max(1);
        let total = 9360.0f32; // 6.5" in twips
        let widths: Vec<f32> = if t.widths.len() == cols && t.widths.iter().all(|w| w.is_finite() && *w > 0.0) {
            t.widths.iter().map(|w| w * 20.0).collect()
        } else {
            vec![total / cols as f32; cols]
        };
        for row in &t.rows {
            out.push_str("\\trowd\\trgaph108\\trleft0");
            if !row.is_empty() && row.iter().all(|c| c.header) {
                out.push_str("\\trhdr");
            }
            let mut g = 0usize;
            let mut x = 0.0f32;
            for c in row {
                let span = c.colspan.clamp(1, 63) as usize;
                for k in g..(g + span).min(widths.len()) {
                    x += widths.get(k).copied().unwrap_or(0.0);
                }
                g += span;
                if c.covered {
                    out.push_str("\\clvmrg");
                } else if c.rowspan > 1 {
                    out.push_str("\\clvmgf");
                }
                match c.valign {
                    Some(VAlign::Top) => out.push_str("\\clvertalt"),
                    Some(VAlign::Center) => out.push_str("\\clvertalc"),
                    Some(VAlign::Bottom) => out.push_str("\\clvertalb"),
                    None => {}
                }
                self.cell_borders(c.borders.as_ref(), t.borderless, out);
                if let Some(s) = c.shading {
                    let i = self.t.color(s);
                    out.push_str(&format!("\\clcbpat{i}"));
                }
                out.push_str(&format!("\\cellx{}", x.round() as i64));
            }
            out.push('\n');
            for c in row {
                if c.covered {
                    out.push_str("\\pard\\plain\\intbl \\cell\n");
                } else {
                    let blocks: Vec<FBlock> = c
                        .blocks
                        .iter()
                        .cloned()
                        .map(|b| match b {
                            FBlock::Para(mut p) if c.header => {
                                for i in &mut p.inlines {
                                    if let Inline::Text(_, f) = i {
                                        f.bold = true;
                                    }
                                }
                                FBlock::Para(p)
                            }
                            other => other,
                        })
                        .collect();
                    self.blocks(&blocks, true, "\\cell", out, depth);
                }
            }
            out.push_str("\\row\n");
        }
    }
}

fn level_def(ordered: bool, l: usize) -> String {
    let indent = 720 * (l + 1);
    if ordered {
        let nfc = [0, 4, 2].get(l % 3).copied().unwrap_or(0);
        format!(
            "{{\\listlevel\\levelnfc{nfc}\\levelnfcn{nfc}\\leveljc0\\leveljcn0\\levelfollow0\\levelstartat1\\levelspace0\\levelindent0{{\\leveltext\\'02\\'{l:02x}.;}}{{\\levelnumbers\\'01;}}\\fi-360\\li{indent}\\lin{indent}}}"
        )
    } else {
        let ch = ['\u{2022}', '\u{25E6}', '\u{25AA}'].get(l % 3).copied().unwrap_or('\u{2022}');
        format!(
            "{{\\listlevel\\levelnfc23\\levelnfcn23\\leveljc0\\leveljcn0\\levelfollow0\\levelstartat1\\levelspace0\\levelindent0{{\\leveltext\\'01\\u{}?;}}{{\\levelnumbers;}}\\fi-360\\li{indent}\\lin{indent}}}",
            ch as u32 as i16
        )
    }
}

fn rtf_date(s: &str) -> Option<String> {
    let num = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<u32>().ok());
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi) = (num(11, 13).unwrap_or(0), num(14, 16).unwrap_or(0));
    Some(format!("\\yr{y}\\mo{mo}\\dy{d}\\hr{h}\\min{mi}"))
}

fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Day of the week, 0 = Sunday (Sakamoto's method).
fn weekday(y: u32, m: u32, d: u32) -> u32 {
    const T: [u32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y.saturating_sub(1) } else { y };
    (y + y / 4 - y / 100 + y / 400 + T.get(m.saturating_sub(1) as usize).copied().unwrap_or(0) + d) % 7
}

/// An RTF revision date (DTTM, RTF 1.9.1 "Revision Marks": minute in bits 0–5, hour 6–10, day
/// 11–15, month 16–19, year − 1900 20–28, weekday 29–31) as ISO 8601 (`2024-03-15T10:30:00Z`).
/// Zero or an impossible date is no date. DTTM has no time zone or seconds: times are read as
/// UTC (`Z`).
pub fn dttm_to_iso(v: u32) -> Option<String> {
    let (mi, h, d, mo, y) = (v & 0x3F, (v >> 6) & 0x1F, (v >> 11) & 0x1F, (v >> 16) & 0xF, 1900 + ((v >> 20) & 0x1FF));
    if v == 0 || mi > 59 || h > 23 || d == 0 || d > days_in_month(y, mo) {
        return None;
    }
    Some(format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:00Z"))
}

/// An ISO 8601 date (`YYYY-MM-DD`, optionally `THH:MM…`; seconds and zone ignored) as an RTF
/// DTTM; `None` when it isn't one or falls outside DTTM's years (1900–2411). DTTM has no time
/// zone or seconds: the clock time is written as given (WordCraft keeps UTC, `Z`), and seconds
/// and any offset are dropped.
pub fn iso_to_dttm(s: &str) -> Option<u32> {
    let num = |a: usize, b: usize| s.get(a..b).filter(|t| t.bytes().all(|c| c.is_ascii_digit())).and_then(|t| t.parse::<u32>().ok());
    let sep = |i: usize, c: &[&str]| s.get(i..i + 1).is_some_and(|t| c.contains(&t));
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    if !sep(4, &["-"]) || !sep(7, &["-"]) {
        return None;
    }
    let (h, mi) = if s.len() > 10 {
        if !sep(10, &["T", " "]) || !sep(13, &[":"]) {
            return None;
        }
        (num(11, 13)?, num(14, 16)?)
    } else {
        (0, 0)
    };
    if !(1900..=2411).contains(&y) || mi > 59 || h > 23 || d == 0 || d > days_in_month(y, mo) {
        return None;
    }
    Some(mi | (h << 6) | (d << 11) | (mo << 16) | ((y - 1900) << 20) | (weekday(y, mo, d) << 29))
}

/// RTF text of a document.
pub fn export(doc: &Document) -> String {
    let flow = model::from_doc_tracked(doc);
    let base_font = doc.styles.default_chr.font.clone().unwrap_or_else(|| wordcraft_doc::styles::BODY_FONT.to_string());
    let mut w = Writer {
        t: Tables { fonts: vec![base_font], colors: Vec::new() },
        lists: Vec::new(),
        authors: None,
        base: Fmt::default(),
        sections: Vec::new(),
        sect_i: 0,
    };
    let mut body = String::new();
    // Every section starts with its own `\sectd`; `\sect` breaks only where there are several.
    let secs: Vec<wordcraft_doc::SectionProps> = doc.sections().into_iter().map(|(_, s)| s.clone()).collect();
    if let Some(first) = secs.first() {
        body.push_str(&sect_words(first));
    }
    if secs.len() > 1 {
        w.sections = secs;
    }
    w.blocks(&flow.blocks, false, "\\par", &mut body, 0);
    export_with(&flow.meta, &w, &body, &doc.last_section)
}

fn export_with(m: &Meta, w: &Writer, body: &str, sec: &wordcraft_doc::SectionProps) -> String {
    let mut out = String::from("{\\rtf1\\ansi\\ansicpg1252\\deff0\\deflang1033\\uc1\n{\\fonttbl");
    for (i, f) in w.t.fonts.iter().enumerate() {
        let fam = if model::is_mono(f) { "\\fmodern" } else { "\\fswiss" };
        out.push_str(&format!("{{\\f{i}{fam}\\fcharset0 {};}}", esc(f)));
    }
    out.push_str("}\n{\\colortbl;");
    for c in &w.t.colors {
        out.push_str(&format!("\\red{}\\green{}\\blue{};", c.0, c.1, c.2));
    }
    out.push_str("}\n{\\*\\generator WordCraft;}\n{\\stylesheet{\\s0\\snext0 Normal;}");
    for n in 1..=6u32 {
        let sz = HEAD_SIZES.get(n as usize - 1).copied().unwrap_or(12.0);
        out.push_str(&format!("{{\\s{n}\\sbasedon0\\snext0\\outlinelevel{}\\keepn\\b\\fs{} heading {n};}}", n - 1, (sz * 2.0) as i32));
    }
    out.push_str(
        "{\\s7\\sbasedon0\\snext0\\fs56 Title;}{\\s8\\sbasedon0\\snext0\\li720\\ri720\\i Quote;}{\\s9\\sbasedon0\\snext9\\sa0\\fs20 Code;}}\n",
    );
    if !w.lists.is_empty() {
        out.push_str("{\\*\\listtable");
        for (i, kinds) in w.lists.iter().enumerate() {
            let id = i + 1;
            out.push_str(&format!("\n{{\\list\\listtemplateid{id}\\listhybrid"));
            for (l, k) in kinds.iter().enumerate() {
                out.push_str(&level_def(*k, l));
            }
            out.push_str(&format!("{{\\listname ;}}\\listid{id}}}"));
        }
        out.push_str("}\n{\\*\\listoverridetable");
        for i in 1..=w.lists.len() {
            out.push_str(&format!("{{\\listoverride\\listid{i}\\listoverridecount0\\ls{i}}}"));
        }
        out.push_str("}\n");
    }
    if let Some(authors) = &w.authors {
        out.push_str("{\\*\\revtbl {Unknown;}");
        for a in authors {
            out.push_str(&format!("{{{};}}", esc(&a.replace(';', ","))));
        }
        out.push_str("}\n");
    }
    out.push_str("{\\info");
    for (k, v) in [("title", &m.title), ("author", &m.author), ("subject", &m.subject), ("keywords", &m.keywords), ("doccomm", &m.description)] {
        if !v.is_empty() {
            out.push_str(&format!("{{\\{k} {}}}", esc(v)));
        }
    }
    if let Some(d) = rtf_date(&m.created) {
        out.push_str(&format!("{{\\creatim{d}}}"));
    }
    out.push_str("}\n");
    let tw = |v: f32| (v * 20.0).round() as i64;
    let landscape = if sec.landscape { "\\landscape" } else { "" };
    out.push_str(&format!(
        "\\paperw{}\\paperh{}\\margl{}\\margr{}\\margt{}\\margb{}{landscape}\\viewkind4\n",
        tw(sec.page_w),
        tw(sec.page_h),
        tw(sec.margin_left),
        tw(sec.margin_right),
        tw(sec.margin_top),
        tw(sec.margin_bottom)
    ));
    out.push_str(body);
    out.push_str("}\n");
    out
}

// ---------------------------------------------------------------------------------------------
// Import

#[derive(Clone, Copy, Debug, PartialEq)]
enum Dest {
    Body,
    Skip,
    FontTbl,
    ColorTbl,
    StyleSheet,
    Info(u8),
    ListTable,
    ListOverride,
    FldInst,
    Pict,
    Bookmark,
    /// The author table of tracked changes (`\revtbl`).
    RevTbl,
    /// Formatting before a tracked change (`\oldpprops` when true, else `\oldcprops`): read
    /// into the group's own state, which the group's end keeps as the change's old formatting.
    OldProps(bool),
}

/// A tracked formatting change being read (`\crauth`/`\crdate` with `\oldcprops`,
/// `\prauth`/`\prdate` with `\oldpprops`): author (`\revtbl` index), date (DTTM) and the old
/// formatting, once its group has been read.
#[derive(Clone, Debug, Default)]
struct FmtRevSt<T> {
    auth: i32,
    date: u32,
    old: Option<Box<T>>,
}

/// Character formatting words of an `\oldcprops` group, with their parameters.
type OldWords = Vec<(String, Option<i32>)>;

/// Most words kept from one `\oldcprops` group (hostile input).
const MAX_OLD_WORDS: usize = 64;

/// Most authors read from a `\revtbl` (hostile input).
const MAX_REV_AUTHORS: usize = 10_000;

/// Tracked-change character properties: `\revised` / `\deleted`, their authors (`\revtbl`
/// indices) and dates (DTTM).
#[derive(Clone, Copy, Debug, Default)]
struct RevSt {
    ins: bool,
    del: bool,
    auth: i32,
    authdel: i32,
    dttm: u32,
    dttmdel: u32,
}

/// Which header or footer a `\header…`/`\footer…` group is: footer or header, and the pages
/// (`\headerf` first, `\headerl` left = even, `\header`/`\headerr` the others).
#[derive(Clone, Copy, Debug, PartialEq)]
struct HeaderFooter {
    footer: bool,
    pages: HfPages,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum HfPages {
    Default,
    First,
    Even,
}

/// The reader's output so far, set aside while a header or footer is read.
#[derive(Default)]
struct Output {
    body: Vec<FBlock>,
    para: Para,
    para_has: bool,
    table: Vec<Vec<Cell>>,
    row: Vec<Cell>,
    cell: Vec<FBlock>,
    defs: Vec<CellDef>,
    def_cur: CellDef,
    row_header: bool,
    rule_pending: bool,
    table_rights: Vec<Vec<i32>>,
}

#[derive(Clone, Debug)]
struct St {
    dest: Dest,
    fmt: Fmt,
    rev: RevSt,
    /// Tracked change of the character formatting (reset by `\plain`). Its old formatting is
    /// the `\oldcprops` words, applied on top of the run's formatting when text is read: Word
    /// writes only the properties that changed there.
    chg_c: FmtRevSt<OldWords>,
    /// While an `\oldcprops` group is read: its character formatting words, and the author and
    /// date of the change (`\crauth`/`\crdate`, which Word writes inside the group).
    old_words: OldWords,
    old_rev: (Option<i32>, Option<u32>),
    /// Tracked change of the paragraph formatting (reset by `\pard`).
    chg_p: FmtRevSt<Para>,
    uc: usize,
    hidden: bool,
    // Paragraph properties.
    align: Option<Align>,
    style: i32,
    intbl: bool,
    ls: Option<i32>,
    ilvl: u8,
    outline: Option<u8>,
    pagebb: bool,
    field: Option<usize>,
    /// The code page of the current font's charset (`\fcharset`), when it names one.
    cp: Option<u16>,
    /// Space before/after (`\sb`, `\sa`), line spacing (`\sl`, `\slmult`) and indents
    /// (`\li`, `\ri`, `\fi`), twips.
    /// The paragraph's borders (`\brdrt`, `\brdrb`, `\brdrl`, `\brdrr`) and the side being
    /// described.
    borders: wordcraft_doc::props::Borders,
    border_side: Option<char>,
    sb: Option<i32>,
    sa: Option<i32>,
    sl: i32,
    slmult: bool,
    li: Option<i32>,
    ri: Option<i32>,
    fi: Option<i32>,
}

impl Default for St {
    fn default() -> Self {
        St {
            dest: Dest::Body,
            fmt: Fmt::default(),
            rev: RevSt::default(),
            chg_c: FmtRevSt::default(),
            chg_p: FmtRevSt::default(),
            old_words: Vec::new(),
            old_rev: (None, None),
            uc: 1,
            hidden: false,
            align: None,
            style: 0,
            intbl: false,
            ls: None,
            ilvl: 0,
            outline: None,
            pagebb: false,
            field: None,
            cp: None,
            borders: Default::default(),
            border_side: None,
            sb: None,
            sa: None,
            sl: 0,
            slmult: false,
            li: None,
            ri: None,
            fi: None,
        }
    }
}

#[derive(Default, Clone)]
struct CellDef {
    vmerge: u8,
    hmerge: u8,
    shading: Option<Rgb>,
    right: i32,
    /// The borders the cell names (`\clbrdrt` … `\clbrdrr`, each with its line style, `\brdrw`
    /// width and `\brdrcf` colour).
    borders: wordcraft_doc::props::Borders,
    valign: Option<VAlign>,
}

/// Page size and margins in twips (`\paperw`, `\margl`… and their `…sxn` section forms).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PageGeom {
    w: Option<i32>,
    h: Option<i32>,
    left: Option<i32>,
    right: Option<i32>,
    top: Option<i32>,
    bottom: Option<i32>,
    header: Option<i32>,
    footer: Option<i32>,
    landscape: bool,
}

impl PageGeom {
    /// The section properties these give, Word's defaults where the file names none.
    fn section(&self) -> wordcraft_doc::SectionProps {
        let mut s = wordcraft_doc::SectionProps::default();
        // Twips to points, ignoring nonsense (zero, negative, or beyond Word's 22").
        let pt = |v: Option<i32>, lo: f32, hi: f32| v.map(|t| t as f32 / 20.0).filter(|v| v.is_finite() && (lo..=hi).contains(v));
        if let Some(w) = pt(self.w, 72.0, 1584.0) {
            s.page_w = w;
        }
        if let Some(h) = pt(self.h, 72.0, 1584.0) {
            s.page_h = h;
        }
        if self.landscape && s.page_w < s.page_h {
            std::mem::swap(&mut s.page_w, &mut s.page_h);
        }
        s.landscape = s.page_w > s.page_h;
        for (v, f) in [
            (self.left, &mut s.margin_left),
            (self.right, &mut s.margin_right),
            (self.top, &mut s.margin_top),
            (self.bottom, &mut s.margin_bottom),
            (self.header, &mut s.header),
            (self.footer, &mut s.footer),
        ] {
            if let Some(v) = pt(v, 0.0, 720.0) {
                *f = v;
            }
        }
        // Margins that leave no room for text fall back to Word's.
        let d = wordcraft_doc::SectionProps::default();
        if s.page_w - s.margin_left - s.margin_right < 72.0 {
            (s.margin_left, s.margin_right) = (d.margin_left, d.margin_right);
        }
        if s.page_h - s.margin_top - s.margin_bottom < 72.0 {
            (s.margin_top, s.margin_bottom) = (d.margin_top, d.margin_bottom);
        }
        s
    }
}

/// Section formatting (`\sectd` resets it): the section's page words (`\pgwsxn`…, over the
/// document's `\paperw`…), its break, columns, page numbering and title page.
#[derive(Clone, Debug, Default)]
struct SecFmt {
    page: PageGeom,
    start: wordcraft_doc::section::SectionStart,
    cols: Option<i32>,
    colsx: Option<i32>,
    linebetcol: bool,
    pgn_restart: bool,
    pgn_start: Option<i32>,
    pgn_fmt: wordcraft_doc::section::NumFormat,
    title_page: bool,
}

impl SecFmt {
    /// The section's properties over the document's page (`doc`); headers and footers unset.
    fn props(&self, doc: &PageGeom) -> wordcraft_doc::SectionProps {
        let p = &self.page;
        let page = PageGeom {
            w: p.w.or(doc.w),
            h: p.h.or(doc.h),
            left: p.left.or(doc.left),
            right: p.right.or(doc.right),
            top: p.top.or(doc.top),
            bottom: p.bottom.or(doc.bottom),
            header: p.header.or(doc.header),
            footer: p.footer.or(doc.footer),
            // A section that gives its own page size or orientation has its own orientation;
            // the document's `\landscape` only applies to sections that give neither.
            landscape: if p.w.is_some() || p.h.is_some() || p.landscape { p.landscape } else { doc.landscape },
        };
        let mut s = page.section();
        s.start = self.start;
        if let Some(n) = self.cols {
            s.columns.count = n.clamp(1, 45) as u32;
        }
        if let Some(x) = self.colsx {
            s.columns.space = (x as f32 / 20.0).clamp(0.0, 1584.0);
        }
        s.columns.separator = self.linebetcol;
        if self.pgn_restart {
            s.page_num_start = Some(self.pgn_start.unwrap_or(1).clamp(0, 32_767) as u32);
        }
        s.page_num_format = self.pgn_fmt;
        s.title_page = self.title_page;
        s
    }
}

#[derive(Default)]
struct Reader {
    st: St,
    stack: Vec<St>,
    overflow: usize,
    fonts: HashMap<i32, Arc<str>>,
    font_cur: i32,
    font_name: String,
    /// Code pages of the fonts whose charset names one (`\fcharset204` → 1251).
    font_cps: HashMap<i32, u16>,
    font_cp_cur: Option<u16>,
    /// The document's code page (`\ansicpg`); 0 until it names one (Windows-1252 then).
    ansicpg: u16,
    /// The default font (`\deff`), whose charset applies before any `\f`.
    deff: i32,
    /// The document's page (`\paperw`, `\margl`…) and the current section's formatting.
    page: PageGeom,
    sec: SecFmt,
    /// The headers and footers of each section ended so far (`\sect`); `hf_parts` holds the
    /// current section's.
    section_parts: Vec<Vec<(HeaderFooter, Vec<FBlock>)>>,
    /// The header or footer being read: the group depth it started at, which one, and the
    /// body's output, put back when the group closes.
    hf: Option<(usize, HeaderFooter, Box<Output>)>,
    /// The current section's headers and footers, in order.
    hf_parts: Vec<(HeaderFooter, Vec<FBlock>)>,
    /// The cell border side being described (`\clbrdrt`, `l`, `b`, `r`): its line style, width
    /// and colour follow.
    cell_side: Option<char>,
    /// Each table row's cell right edges (`\cellx`, twips), for the table's column grid.
    table_rights: Vec<Vec<i32>>,
    colors: Vec<Option<Rgb>>,
    color_cur: (u8, u8, u8, bool),
    styles: HashMap<i32, String>,
    /// Each style's own character formatting, as its `\stylesheet` entry writes it.
    style_defs: HashMap<i32, StyleDef>,
    style_cur: i32,
    style_name: String,
    style_def_cur: StyleDef,
    meta: Meta,
    info_text: String,
    // Lists: list id → ordered per level; override ls → list id.
    lists: HashMap<i32, Vec<bool>>,
    list_cur: Vec<bool>,
    list_cur_id: i32,
    overrides: HashMap<i32, i32>,
    ov_list: i32,
    ov_ls: i32,
    fields: Vec<String>,
    // Picture being read.
    pict_depth: Option<usize>,
    pict_hex: Vec<u8>,
    pict_half: Option<u8>,
    pict_kind: u8,
    pict_w: Option<f32>,
    pict_h: Option<f32>,
    pict_sx: f32,
    pict_sy: f32,
    bkmk: Option<(usize, String)>,
    /// Authors of tracked changes (`\revtbl`), and the entry being read.
    rev_authors: Vec<String>,
    rev_name: String,
    skip_chars: usize,
    pending_high: Option<u16>,
    /// A streaming decoder for the current multibyte code page, and the code page it was made for.
    /// A lead byte and its trail byte decode together even when a text word, a group boundary or the
    /// end of a chunk falls between them, so a run of any length never splits a character.
    mb_decoder: Option<encoding_rs::Decoder>,
    mb_cp: u16,
    // Output.
    body: Vec<FBlock>,
    para: Para,
    para_has: bool,
    table: Vec<Vec<Cell>>,
    row: Vec<Cell>,
    cell: Vec<FBlock>,
    defs: Vec<CellDef>,
    def_cur: CellDef,
    row_header: bool,
    created: Option<(u32, u32, u32, u32, u32)>,
    in_creatim: bool,
    rule_pending: bool,
    info_depth: Option<usize>,
}

impl Reader {
    fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Set the output aside (a header or footer starts) or put it back (it ended), returning the
    /// other one.
    fn swap_output(&mut self, mut o: Output) -> Output {
        std::mem::swap(&mut o.body, &mut self.body);
        std::mem::swap(&mut o.para, &mut self.para);
        std::mem::swap(&mut o.para_has, &mut self.para_has);
        std::mem::swap(&mut o.table, &mut self.table);
        std::mem::swap(&mut o.row, &mut self.row);
        std::mem::swap(&mut o.cell, &mut self.cell);
        std::mem::swap(&mut o.defs, &mut self.defs);
        std::mem::swap(&mut o.def_cur, &mut self.def_cur);
        std::mem::swap(&mut o.row_header, &mut self.row_header);
        std::mem::swap(&mut o.rule_pending, &mut self.rule_pending);
        std::mem::swap(&mut o.table_rights, &mut self.table_rights);
        o
    }

    /// The border of the cell side being described, else of the paragraph side.
    fn cell_border_slot(&mut self) -> Option<&mut Option<wordcraft_doc::props::Border>> {
        let (b, side) = match self.cell_side {
            Some(c) => (&mut self.def_cur.borders, c),
            None => (&mut self.st.borders, self.st.border_side?),
        };
        match side {
            't' => Some(&mut b.top),
            'l' => Some(&mut b.left),
            'b' => Some(&mut b.bottom),
            'r' => Some(&mut b.right),
            _ => None,
        }
    }

    /// A header or footer group starts: read it into an output of its own.
    fn start_header_footer(&mut self, which: HeaderFooter) {
        let body = self.swap_output(Output::default());
        self.hf = Some((self.depth(), which, Box::new(body)));
        self.st.intbl = false;
    }

    /// The header or footer group closed: keep what it held and go back to the body.
    fn end_header_footer(&mut self) {
        let Some((_, which, body)) = self.hf.take() else { return };
        if self.para_has || !self.para.inlines.is_empty() {
            self.end_para(false);
        }
        self.close_table();
        let part = self.swap_output(*body);
        // A section names each one once; a repeat replaces it.
        self.hf_parts.retain(|(w, _)| *w != which);
        if self.hf_parts.len() < 16 {
            self.hf_parts.push((which, part.body));
        }
    }

    /// `\sect`: end the paragraph, and with it the section, which takes its formatting and the
    /// headers and footers read since the last break. Inside a header, footer or table, and past
    /// [`MAX_SECTIONS`], it only ends the paragraph.
    fn end_section(&mut self) {
        self.end_para_mark();
        if self.hf.is_some() || self.st.intbl || self.section_parts.len() + 1 >= MAX_SECTIONS {
            return;
        }
        let props = self.sec.props(&self.page);
        if let Some(FBlock::Para(p)) = self.body.last_mut() {
            p.section = Some(Box::new(props));
            self.section_parts.push(std::mem::take(&mut self.hf_parts));
        }
    }

    /// A section formatting word: applied to the current section, unless it's inside a header
    /// or footer (whose `\sectd` isn't the body's). False when `w` isn't one.
    fn section_word(&mut self, w: &str, p: Option<i32>) -> bool {
        use wordcraft_doc::section::{NumFormat, SectionStart};
        let on = p != Some(0);
        let pv = p.unwrap_or(0);
        // Inside a header or footer the word is read into a scratch copy and dropped.
        let mut scratch;
        let sec = if self.hf.is_some() {
            scratch = self.sec.clone();
            &mut scratch
        } else {
            &mut self.sec
        };
        match w {
            "sectd" => *sec = SecFmt::default(),
            "titlepg" => sec.title_page = on,
            "pgwsxn" => sec.page.w = Some(pv),
            "pghsxn" => sec.page.h = Some(pv),
            "marglsxn" => sec.page.left = Some(pv),
            "margrsxn" => sec.page.right = Some(pv),
            "margtsxn" => sec.page.top = Some(pv),
            "margbsxn" => sec.page.bottom = Some(pv),
            "headery" => sec.page.header = Some(pv),
            "footery" => sec.page.footer = Some(pv),
            "lndscpsxn" => sec.page.landscape = on,
            "sbknone" => sec.start = SectionStart::Continuous,
            "sbkcol" => sec.start = SectionStart::NextColumn,
            "sbkpage" => sec.start = SectionStart::NextPage,
            "sbkeven" => sec.start = SectionStart::EvenPage,
            "sbkodd" => sec.start = SectionStart::OddPage,
            "cols" => sec.cols = Some(pv),
            "colsx" => sec.colsx = Some(pv),
            "linebetcol" => sec.linebetcol = on,
            "pgnrestart" => sec.pgn_restart = true,
            "pgncont" => sec.pgn_restart = false,
            "pgnstarts" => sec.pgn_start = Some(pv),
            "pgndec" => sec.pgn_fmt = NumFormat::Decimal,
            "pgnucrm" => sec.pgn_fmt = NumFormat::UpperRoman,
            "pgnlcrm" => sec.pgn_fmt = NumFormat::LowerRoman,
            "pgnucltr" => sec.pgn_fmt = NumFormat::UpperLetter,
            "pgnlcltr" => sec.pgn_fmt = NumFormat::LowerLetter,
            _ => return false,
        }
        true
    }

    fn open(&mut self) {
        self.skip_chars = 0;
        if self.stack.len() >= MAX_GROUPS {
            self.overflow += 1;
            return;
        }
        self.stack.push(self.st.clone());
    }

    fn close(&mut self) {
        // A group end decodes the multibyte bytes read in it, before the state is restored.
        self.finish_multibyte();
        self.skip_chars = 0;
        if self.overflow > 0 {
            self.overflow -= 1;
            return;
        }
        let closing = self.st.dest;
        // Destination-level commits.
        match closing {
            Dest::FontTbl if !self.font_name.is_empty() => self.commit_font(),
            Dest::StyleSheet if !self.style_name.is_empty() => self.commit_style(),
            Dest::RevTbl if !self.rev_name.trim().is_empty() => self.commit_author(),
            Dest::Info(k) => {
                let t = std::mem::take(&mut self.info_text).trim().to_string();
                match k {
                    1 => self.meta.title = t,
                    2 => self.meta.author = t,
                    3 => self.meta.subject = t,
                    4 => self.meta.keywords = t,
                    5 => self.meta.description = t,
                    _ => {}
                }
                self.in_creatim = false;
            }
            _ => {}
        }
        let Some(mut prev) = self.stack.pop() else { return };
        // The end of old formatting read in the body: the formatting it holds is the change's
        // old formatting (nested old-formatting groups only change their parent's copy).
        if let Dest::OldProps(para) = closing
            && prev.dest == Dest::Body
        {
            if para {
                prev.chg_p.old = Some(Box::new(self.para_props(&self.st)));
            } else {
                prev.chg_c.old = Some(Box::new(std::mem::take(&mut self.st.old_words)));
                if let Some(a) = self.st.old_rev.0 {
                    prev.chg_c.auth = a;
                }
                if let Some(d) = self.st.old_rev.1 {
                    prev.chg_c.date = d;
                }
            }
        }
        if self.pict_depth.is_some_and(|d| self.depth() < d) {
            self.pict_depth = None;
            self.finish_pict();
        }
        if let Some((d, _)) = &self.bkmk
            && self.depth() < *d
            && let Some((_, name)) = self.bkmk.take()
        {
            let name = name.trim().to_string();
            if !name.is_empty() {
                self.para.inlines.push(Inline::Anchor(name));
                self.para_has = true;
            }
        }
        self.st = prev;
        if self.hf.as_ref().is_some_and(|(d, _, _)| self.depth() < *d) {
            self.end_header_footer();
        }
    }

    fn commit_author(&mut self) {
        let name = std::mem::take(&mut self.rev_name).trim().to_string();
        if self.rev_authors.len() < MAX_REV_AUTHORS {
            self.rev_authors.push(name);
        }
    }

    /// The tracked-change mark for `\revtbl` author index `i` and DTTM `dttm`; an index outside
    /// the table is "Unknown".
    fn rev_mark(&self, i: i32, dttm: u32) -> model::RevMark {
        let author = usize::try_from(i).ok().and_then(|i| self.rev_authors.get(i)).cloned().unwrap_or_else(|| "Unknown".to_string());
        model::RevMark { author, date: dttm_to_iso(dttm).unwrap_or_default(), id: None }
    }

    fn commit_font(&mut self) {
        let name = std::mem::take(&mut self.font_name).trim().trim_end_matches(';').trim().to_string();
        if let Some(name) = model::kept_font(&name).filter(|n| !n.is_empty())
            && self.fonts.len() < 4096
        {
            self.fonts.insert(self.font_cur, name);
            if let Some(cp) = self.font_cp_cur.take() {
                self.font_cps.insert(self.font_cur, cp);
            }
        }
    }

    /// The code page 8-bit text is read in: the current font's, else the default font's, else the
    /// document's (Windows-1252 when none is named).
    fn code_page(&self) -> u16 {
        self.st.cp.or_else(|| self.font_cps.get(&self.deff).copied()).unwrap_or(if self.ansicpg == 0 { 1252 } else { self.ansicpg })
    }

    /// A byte of 8-bit text (`\'hh` or raw): a single-byte code page decodes it at once; a
    /// multibyte one is fed to a streaming decoder ([`Reader::finish_multibyte`]), so a lead byte decodes
    /// together with its trail byte.
    fn byte(&mut self, b: u8) {
        if self.skip_chars > 0 {
            self.skip_chars -= 1;
            return;
        }
        let cp = self.code_page();
        let Some(encoding) = multibyte_encoding(cp) else {
            self.finish_multibyte();
            let c = wordcraft_doc::encoding::code_page_byte(cp, b);
            self.ch(c);
            return;
        };
        // A code page switch ends the previous run (a lead byte left then becomes U+FFFD, as Word
        // writes it) before the new decoder starts.
        if self.mb_cp != cp || self.mb_decoder.is_none() {
            self.finish_multibyte();
            self.mb_cp = cp;
            self.mb_decoder = Some(encoding.new_decoder_without_bom_handling());
        }
        self.decode_run(&[b]);
    }

    /// Feed 8-bit text bytes to the open multibyte decoder and append what it emits. A single
    /// `\'hh` byte decodes into a stack buffer (no heap allocation per byte of escaped text); a
    /// raw run decodes in one call, so a long ASCII run in a Japanese, Chinese or Korean document
    /// costs one append instead of one per byte. The destination's spare capacity is all
    /// `decode_to_*` writes into, so a full buffer is drained and decoding continues.
    fn decode_run(&mut self, bytes: &[u8]) {
        // The decoder is taken out so the text it produced can be appended (which borrows the
        // reader) before it goes back.
        let Some(mut decoder) = self.mb_decoder.take() else { return };
        let mut read = 0;
        if bytes.len() <= 16 {
            // A character is at most 4 UTF-8 bytes, and a byte may also replace a pending lead
            // byte (3 more), so one escaped byte always fits the stack buffer.
            let mut buf = [0u8; 16];
            let (result, got, written, _) = decoder.decode_to_utf8(bytes, &mut buf, false);
            read = got;
            let full = matches!(result, encoding_rs::CoderResult::OutputFull);
            if written > 0
                && let Ok(s) = std::str::from_utf8(&buf[..written])
            {
                self.text(s);
            }
            if !full {
                self.mb_decoder = Some(decoder);
                return;
            }
        }
        let mut out = String::with_capacity(bytes.len().saturating_mul(2));
        let mut rest = bytes.get(read..).unwrap_or_default();
        loop {
            let (result, got, _) = decoder.decode_to_string(rest, &mut out, false);
            rest = rest.get(got..).unwrap_or_default();
            if !matches!(result, encoding_rs::CoderResult::OutputFull) {
                break;
            }
            out.reserve(16.max(rest.len().saturating_mul(2)));
        }
        self.mb_decoder = Some(decoder);
        if !out.is_empty() {
            self.text(&out);
        }
    }

    /// A run of raw 8-bit text from the file (not `\'hh` escapes): a single-byte code page maps
    /// each byte to a char; a multibyte one decodes the whole run in one pass, so a long ASCII run
    /// in a Japanese, Chinese or Korean document costs one decode and one append instead of one
    /// per byte.
    ///
    /// The decoder is created without BOM handling: the code page comes from `\ansicpg`, `\cpg` or
    /// `\fcharset` and must not change. With BOM sniffing, a run that starts `EF BB BF` in GBK,
    /// Big5 or UHC (a valid character pair followed by a lead byte) would be dropped as a UTF-8
    /// BOM and the rest decoded as UTF-8.
    fn run(&mut self, chunk: &[u8]) {
        if self.skip_chars > 0 {
            for b in chunk {
                self.byte(*b);
            }
            return;
        }
        let cp = self.code_page();
        let Some(encoding) = multibyte_encoding(cp) else {
            self.finish_multibyte();
            let mut out = String::with_capacity(chunk.len());
            for b in chunk {
                out.push(wordcraft_doc::encoding::code_page_byte(cp, *b));
            }
            if !out.is_empty() {
                self.text(&out);
            }
            return;
        };
        if self.mb_cp != cp || self.mb_decoder.is_none() {
            self.finish_multibyte();
            self.mb_cp = cp;
            self.mb_decoder = Some(encoding.new_decoder_without_bom_handling());
        }
        self.decode_run(chunk);
    }

    /// Ends the current multibyte run: the streaming decoder emits whatever it still holds (a lone
    /// lead byte becomes U+FFFD, as Word writes it) and is dropped. A code page switch, a single-byte
    /// run, a group end and the end of the document finish the run this way.
    fn finish_multibyte(&mut self) {
        let Some(mut decoder) = self.mb_decoder.take() else { return };
        let mut out = String::with_capacity(16);
        let _ = decoder.decode_to_string(&[], &mut out, true);
        if !out.is_empty() {
            self.text(&out);
        }
    }

    fn commit_style(&mut self) {
        let name = std::mem::take(&mut self.style_name).trim().trim_end_matches(';').trim().to_string();
        let def = std::mem::take(&mut self.style_def_cur);
        if self.styles.len() < 4096 {
            self.styles.insert(self.style_cur, name);
            self.style_defs.insert(self.style_cur, def);
        }
        // The next entry is style 0 (Normal) unless it names a number.
        self.style_cur = 0;
    }

    /// The character formatting style `id` gives its runs in this file: its stylesheet entry's,
    /// then its `\sbasedon` ancestors' (a bounded walk: loops and missing ids end it).
    fn file_style(&self, id: i32) -> StyleDef {
        let mut out = StyleDef::default();
        let mut seen: Vec<i32> = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            if seen.len() >= 16 || seen.contains(&i) {
                break;
            }
            seen.push(i);
            let Some(d) = self.style_defs.get(&i) else { break };
            out.bold = out.bold.or(d.bold);
            out.italic = out.italic.or(d.italic);
            out.size = out.size.or(d.size);
            out.font = out.font.or(d.font);
            cur = d.based_on;
        }
        out
    }

    /// The formatting a paragraph of kind `kind` in style `id` gives its runs. A style whose
    /// entry and `\sbasedon` chain set no character formatting (a bare `{\s2 heading 2;}`) has
    /// WordCraft's own (`style_fmt`); otherwise what the chain doesn't set is RTF's default (not
    /// bold, not italic, 12 pt). The font alone falls back per property: WordCraft writes the code
    /// font on each code paragraph, not in its Code entry.
    fn style_base(&self, kind: Kind, id: i32) -> Fmt {
        let mut f = style_fmt(kind);
        let d = self.file_style(id);
        if d.bold.is_some() || d.italic.is_some() || d.size.is_some() || d.font.is_some() {
            f.bold = d.bold.unwrap_or(false);
            f.italic = d.italic.unwrap_or(false);
            f.size = Some(d.size.unwrap_or(12.0));
        }
        // A font is matched the way `\f` sets a run's: a monospaced one is the code font.
        if let Some(name) = d.font.and_then(|i| self.fonts.get(&i)) {
            f.code = model::is_mono(name);
            f.font = (!f.code).then(|| name.clone());
        }
        f
    }

    fn finish_pict(&mut self) {
        if self.pict_half.take().is_some() {
            // Odd hex digit count: drop the dangling nibble.
        }
        let data = std::mem::take(&mut self.pict_hex);
        if self.pict_kind == 0 || data.is_empty() {
            return;
        }
        let sx = if self.pict_sx > 0.0 { self.pict_sx / 100.0 } else { 1.0 };
        let sy = if self.pict_sy > 0.0 { self.pict_sy / 100.0 } else { 1.0 };
        let w = self.pict_w.map(|v| v * sx);
        let h = self.pict_h.map(|v| v * sy);
        if let Some(img) = make_img(data, w, h, "") {
            self.para.inlines.push(Inline::Image(img));
            self.para_has = true;
        }
    }

    fn text(&mut self, s: &str) {
        match self.st.dest {
            Dest::Body => {
                if self.st.hidden {
                    return;
                }
                let mut f = self.st.fmt.clone();
                let r = self.st.rev;
                if r.ins {
                    f.ins = Some(self.rev_mark(r.auth, r.dttm));
                }
                if r.del {
                    f.del = Some(self.rev_mark(r.authdel, r.dttmdel));
                }
                let c = &self.st.chg_c;
                if let Some(words) = c.old.clone() {
                    let mark = self.rev_mark(c.auth, c.date);
                    let old = self.old_fmt(&words);
                    f.fmt_change = Some(Box::new(model::FmtChange { mark, old }));
                }
                self.para.push_text(s, &f);
                self.para_has = true;
            }
            Dest::FontTbl => {
                for c in s.chars() {
                    if c == ';' {
                        self.commit_font();
                    } else if self.font_name.len() <= model::MAX_FONT_NAME {
                        // One past the limit is enough for `commit_font` to drop the name.
                        self.font_name.push(c);
                    }
                }
            }
            Dest::ColorTbl => {
                for c in s.chars() {
                    if c == ';' {
                        let (r, g, b, set) = self.color_cur;
                        if self.colors.len() < 4096 {
                            self.colors.push(if set { Some(Rgb(r, g, b)) } else { None });
                        }
                        self.color_cur = (0, 0, 0, false);
                    }
                }
            }
            Dest::StyleSheet => {
                for c in s.chars() {
                    if c == ';' {
                        self.commit_style();
                    } else if self.style_name.len() < model::MAX_FONT_NAME {
                        self.style_name.push(c);
                    } else {
                        model::dropped(model::Dropped::StyleName);
                    }
                }
            }
            Dest::Info(_) => self.info_text.push_str(s),
            Dest::FldInst => {
                if let Some(f) = self.st.field.and_then(|i| self.fields.get_mut(i))
                    && f.len() < 4096
                {
                    f.push_str(s);
                }
            }
            Dest::Pict => {
                for b in s.bytes() {
                    let v = match b {
                        b'0'..=b'9' => b - b'0',
                        b'a'..=b'f' => b - b'a' + 10,
                        b'A'..=b'F' => b - b'A' + 10,
                        _ => continue,
                    };
                    match self.pict_half.take() {
                        Some(h) => {
                            if self.pict_hex.len() < MAX_PICT {
                                self.pict_hex.push((h << 4) | v);
                            }
                        }
                        None => self.pict_half = Some(v),
                    }
                }
            }
            Dest::Bookmark => {
                if let Some((_, n)) = &mut self.bkmk
                    && n.len() < 256
                {
                    n.push_str(s);
                }
            }
            Dest::RevTbl => {
                for c in s.chars() {
                    if c == ';' {
                        self.commit_author();
                    } else if self.rev_name.len() < 256 {
                        self.rev_name.push(c);
                    }
                }
            }
            Dest::Skip | Dest::ListTable | Dest::ListOverride | Dest::OldProps(_) => {}
        }
    }

    /// The formatting before a tracked character change: the current run's, with the
    /// `\oldcprops` words applied (a `\plain` among them starts from none).
    fn old_fmt(&mut self, words: &[(String, Option<i32>)]) -> Fmt {
        let fmt = Fmt { link: None, ins: None, del: None, fmt_change: None, ..self.st.fmt.clone() };
        let saved = std::mem::replace(&mut self.st, St { dest: Dest::OldProps(false), fmt, ..Default::default() });
        for (w, p) in words {
            self.word(w, *p, false, false);
        }
        let old = std::mem::replace(&mut self.st, saved).fmt;
        Fmt { link: None, ..old }
    }

    fn ch(&mut self, c: char) {
        if self.skip_chars > 0 {
            self.skip_chars -= 1;
            return;
        }
        self.finish_multibyte();
        let mut buf = [0u8; 4];
        self.text(c.encode_utf8(&mut buf));
    }

    /// The paragraph kind `st`'s style names, if it names one.
    fn style_kind(&self, st: &St) -> Option<Kind> {
        let name = self.styles.get(&st.style).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        if let Some(n) = name.strip_prefix("heading").and_then(|r| r.trim().parse::<u8>().ok()) {
            return Some(Kind::Heading(n.clamp(1, 6)));
        }
        if st.style != 0 {
            match name.as_str() {
                "title" => return Some(Kind::Title),
                "quote" | "intense quote" => return Some(Kind::Quote),
                "code" | "html preformatted" | "plain text" => return Some(Kind::Code),
                _ => {}
            }
        }
        None
    }

    fn para_kind(&self, st: &St) -> Kind {
        if let Some(k) = self.style_kind(st) {
            return k;
        }
        if let Some(o) = st.outline.filter(|o| *o < 9) {
            return Kind::Heading((o + 1).min(6));
        }
        Kind::Normal
    }

    /// The paragraph formatting of `st` (kind, alignment, page break, spacing, borders, list).
    fn para_props(&self, st: &St) -> Para {
        let mut p = Para::new(self.para_kind(st));
        p.align = st.align;
        p.page_break = st.pagebb;
        p.spacing = spacing(st);
        p.borders = st.borders.any_visible().then_some(st.borders);
        if let Some(ls) = st.ls {
            let list = self.overrides.get(&ls).and_then(|id| self.lists.get(id));
            let lv = st.ilvl.min(8);
            let ordered = list.and_then(|l| l.get(lv as usize).copied()).unwrap_or(false);
            p.list = Some(ListInfo { ordered, level: lv, start: 1 });
        }
        p
    }

    /// `\par` (or `\sect`): end the paragraph, whose mark is tracked as inserted or deleted when the text
    /// at it is.
    fn end_para_mark(&mut self) {
        let r = self.st.rev;
        self.para.mark_ins = r.ins.then(|| Box::new(self.rev_mark(r.auth, r.dttm)));
        self.para.mark_del = r.del.then(|| Box::new(self.rev_mark(r.authdel, r.dttmdel)));
        self.end_para(false);
    }

    /// End the current paragraph (`\par`, `\cell`).
    fn end_para(&mut self, in_cell: bool) {
        let read = std::mem::take(&mut self.para);
        let props = self.para_props(&self.st);
        let mut p = Para { inlines: read.inlines, mark_ins: read.mark_ins, mark_del: read.mark_del, ..props };
        if p.kind == Kind::Normal && p.inlines.is_empty() && self.rule_pending {
            p.kind = Kind::Rule;
        }
        self.rule_pending = false;
        let c = &self.st.chg_p;
        if let Some(old) = &c.old {
            p.fmt_change = Some(Box::new(model::FmtChange { mark: self.rev_mark(c.auth, c.date), old: (**old).clone() }));
        }
        // Headings, code, titles and quotes keep their style formatting implicit, in the text's
        // formatting and in the formatting before a tracked change alike. The style is the
        // file's when its name gave the kind (not an outline level on another style).
        let base = if self.style_kind(&self.st) == Some(p.kind) { self.style_base(p.kind, self.st.style) } else { style_fmt(p.kind) };
        for i in &mut p.inlines {
            if let Inline::Text(_, f) = i {
                drop_style_formatting(p.kind, &base, f);
                if let Some(c) = f.fmt_change.as_deref_mut() {
                    drop_style_formatting(p.kind, &base, &mut c.old);
                }
            }
        }
        self.para_has = false;
        if self.st.intbl || in_cell {
            self.cell.push(FBlock::Para(p));
        } else {
            self.close_table();
            self.body.push(FBlock::Para(p));
        }
    }

    fn end_cell(&mut self) {
        self.end_para(true);
        let blocks = std::mem::take(&mut self.cell);
        if self.row.len() < 63 {
            self.row.push(Cell { blocks, ..Default::default() });
        }
    }

    fn end_row(&mut self) {
        if self.para_has {
            self.end_cell();
        }
        let mut cells = std::mem::take(&mut self.row);
        let defs = self.defs.clone();
        let mut out: Vec<Cell> = Vec::new();
        let mut rights: Vec<i32> = Vec::new();
        for (i, mut c) in cells.drain(..).enumerate() {
            let d = defs.get(i).cloned().unwrap_or_default();
            // A merged-in cell widens the cell it joins.
            if d.hmerge == 2
                && let Some(last) = rights.last_mut()
            {
                *last = d.right;
            } else {
                rights.push(d.right);
            }
            c.shading = d.shading;
            c.borders = d.borders.any_visible().then_some(d.borders);
            c.header = self.row_header;
            c.valign = d.valign;
            match d.vmerge {
                1 => c.rowspan = 2,
                2 => c.covered = true,
                _ => {}
            }
            if d.hmerge == 2
                && let Some(prev) = out.last_mut()
            {
                prev.colspan = (prev.colspan + 1).min(63);
                continue;
            }
            out.push(c);
        }
        if !out.is_empty() && self.table.len() < wordcraft_doc::table::MAX_ROWS {
            rights.truncate(out.len());
            self.table.push(out);
            self.table_rights.push(rights);
        }
    }

    fn close_table(&mut self) {
        if !self.row.is_empty() || !self.cell.is_empty() {
            if !self.cell.is_empty() {
                let blocks = std::mem::take(&mut self.cell);
                self.row.push(Cell { blocks, ..Default::default() });
            }
            self.end_row();
        }
        if !self.table.is_empty() {
            let mut rows = std::mem::take(&mut self.table);
            let rights = std::mem::take(&mut self.table_rights);
            let widths = column_grid(&mut rows, &rights);
            // Word draws only the borders the cells name: no grid style, each cell its own.
            self.body.push(FBlock::Table(FTable { rows, widths, borderless: true }));
        }
    }

    fn word(&mut self, w: &str, p: Option<i32>, starred: bool, first_in_group: bool) {
        // Every control word ends a multibyte run: the code page it is read in may change here
        // (`\f`, `\fcharset`, `\cpg`, `\ansicpg`, `\pard`), and `\par` must see the text before it.
        self.finish_multibyte();
        let on = p != Some(0);
        let pv = p.unwrap_or(0);
        if self.st.dest == Dest::Skip && !(self.in_info() && matches!(w, "title" | "author" | "subject" | "keywords" | "doccomm" | "creatim")) {
            if self.in_creatim {
                let v = pv.clamp(0, 9999) as u32;
                let c = self.created.get_or_insert((0, 1, 1, 0, 0));
                match w {
                    "yr" => c.0 = v,
                    "mo" => c.1 = v,
                    "dy" => c.2 = v,
                    "hr" => c.3 = v,
                    "min" => c.4 = v,
                    _ => {}
                }
            }
            return;
        }
        // Destinations.
        let dest = match w {
            "fonttbl" => Some(Dest::FontTbl),
            "colortbl" => Some(Dest::ColorTbl),
            "stylesheet" => Some(Dest::StyleSheet),
            "info" => Some(Dest::Skip),
            "title" if self.st.dest == Dest::Skip && self.in_info() => Some(Dest::Info(1)),
            "author" if self.in_info() => Some(Dest::Info(2)),
            "subject" if self.in_info() => Some(Dest::Info(3)),
            "keywords" if self.in_info() => Some(Dest::Info(4)),
            "doccomm" if self.in_info() => Some(Dest::Info(5)),
            "listtable" => Some(Dest::ListTable),
            "revtbl" => Some(Dest::RevTbl),
            "oldcprops" => Some(Dest::OldProps(false)),
            "oldpprops" => Some(Dest::OldProps(true)),
            "listoverridetable" => Some(Dest::ListOverride),
            "fldinst" => Some(Dest::FldInst),
            // Transparent wrapper: read the nested pict without changing the destination.
            "shppict" => return,
            "pict" => Some(Dest::Pict),
            "bkmkstart" => Some(Dest::Bookmark),
            "header" | "footer" | "headerl" | "headerr" | "headerf" | "footerl" | "footerr" | "footerf"
                if self.hf.is_none() && self.st.dest == Dest::Body && !self.st.intbl =>
            {
                let footer = w.starts_with("footer");
                let pages = match w.strip_prefix(if footer { "footer" } else { "header" }) {
                    Some("f") => HfPages::First,
                    Some("l") => HfPages::Even,
                    _ => HfPages::Default,
                };
                self.start_header_footer(HeaderFooter { footer, pages });
                return;
            }
            "header" | "footer" | "headerl" | "headerr" | "headerf" | "footerl" | "footerr" | "footerf" | "footnote" | "annotation" | "pntext"
            | "listtext" | "object" | "nonshppict" | "xe" | "tc" | "txe" | "private" | "bkmkend" | "atnid" | "atnauthor" | "comment" | "shpinst"
            | "do" | "operator" | "company" | "manager" | "category" | "hlinkbase" | "revtime" | "printim" | "buptim" | "version" | "nofpages"
            | "nofwords" | "nofchars" | "edmins" | "vern" | "themedata" | "colorschememapping" | "latentstyles" | "datastore" | "xmlnstbl"
            | "rsidtbl" | "generator" | "mmathPr" | "pgdsctbl" | "filetbl" | "listpicture" | "userprops" | "docvar" | "objdata" | "fontemb"
            | "fontfile" | "panose" | "falt" | "mhtmltag" | "htmltag" | "pnseclvl" | "background" | "sp" | "pn" | "pgptbl" => Some(Dest::Skip),
            "creatim" if self.in_info() => {
                self.in_creatim = true;
                Some(Dest::Skip)
            }
            _ => None,
        };
        if let Some(d) = dest {
            match d {
                Dest::Bookmark => {
                    self.bkmk = Some((self.depth(), String::new()));
                }
                Dest::Pict => {
                    self.pict_depth = Some(self.depth());
                    self.pict_hex.clear();
                    self.pict_half = None;
                    self.pict_kind = 0;
                    self.pict_w = None;
                    self.pict_h = None;
                    self.pict_sx = 0.0;
                    self.pict_sy = 0.0;
                }
                Dest::FldInst if self.st.field.is_none() => self.start_field(),
                // Old character formatting holds the properties that changed (Word) or, after a
                // `\plain`, all of them; old paragraph formatting is complete from `\pard`.
                Dest::OldProps(false) => {
                    self.st.old_words.clear();
                    self.st.old_rev = (None, None);
                }
                Dest::OldProps(true) => self.word("pard", None, false, false),
                _ => {}
            }
            self.st.dest = d;
            return;
        }
        if starred && first_in_group {
            // Unknown ignorable destination.
            if !matches!(w, "fldinst" | "bkmkstart") {
                self.st.dest = Dest::Skip;
                return;
            }
        }
        match self.st.dest {
            Dest::Skip => {
                if self.in_creatim {
                    let v = pv.clamp(0, 9999) as u32;
                    let c = self.created.get_or_insert((0, 1, 1, 0, 0));
                    match w {
                        "yr" => c.0 = v,
                        "mo" => c.1 = v,
                        "dy" => c.2 = v,
                        "hr" => c.3 = v,
                        "min" => c.4 = v,
                        _ => {}
                    }
                }
                return;
            }
            // Font names take Unicode escapes (with fallback skipping) like the body.
            Dest::FontTbl if !matches!(w, "u" | "uc") => {
                match w {
                    "f" => {
                        self.font_cur = pv;
                        self.font_name.clear();
                        self.font_cp_cur = None;
                    }
                    "fcharset" => self.font_cp_cur = wordcraft_doc::encoding::charset_code_page(pv),
                    // A font's explicit code page (`\cpg1251`, `\cpg932`) wins over its charset.
                    "cpg" => {
                        if let Some(cp) = u16::try_from(pv).ok().filter(|cp| wordcraft_doc::encoding::is_text_code_page(*cp)) {
                            self.font_cp_cur = Some(cp);
                        }
                    }
                    _ => {}
                }
                return;
            }
            Dest::FontTbl => {}
            Dest::ColorTbl => {
                let v = pv.clamp(0, 255) as u8;
                match w {
                    "red" => {
                        self.color_cur.0 = v;
                        self.color_cur.3 = true;
                    }
                    "green" => {
                        self.color_cur.1 = v;
                        self.color_cur.3 = true;
                    }
                    "blue" => {
                        self.color_cur.2 = v;
                        self.color_cur.3 = true;
                    }
                    _ => {}
                }
                return;
            }
            Dest::StyleSheet => {
                let d = &mut self.style_def_cur;
                match w {
                    "s" | "cs" | "ds" | "ts" => {
                        self.style_cur = if w == "s" { pv } else { -1 - pv.saturating_abs() };
                        self.style_name.clear();
                        *d = StyleDef::default();
                    }
                    "sbasedon" => d.based_on = Some(pv),
                    "b" => d.bold = Some(on),
                    "i" => d.italic = Some(on),
                    "fs" => d.size = (pv > 0).then(|| (pv as f32 / 2.0).clamp(1.0, 1638.0)),
                    "f" => d.font = Some(pv),
                    _ => {}
                }
                return;
            }
            Dest::ListTable => {
                match w {
                    "list" => {
                        self.list_cur.clear();
                        self.list_cur_id = 0;
                    }
                    "listlevel" if self.list_cur.len() < 9 => {
                        self.list_cur.push(true);
                    }
                    "levelnfc" | "levelnfcn" => {
                        if let Some(l) = self.list_cur.last_mut() {
                            *l = !matches!(pv, 23 | 255);
                        }
                    }
                    "listid" => {
                        self.list_cur_id = pv;
                        if self.lists.len() < 4096 {
                            self.lists.insert(pv, self.list_cur.clone());
                        }
                    }
                    _ => {}
                }
                return;
            }
            Dest::ListOverride => {
                match w {
                    "listid" => self.ov_list = pv,
                    "ls" => {
                        self.ov_ls = pv;
                        if self.overrides.len() < 4096 {
                            self.overrides.insert(pv, self.ov_list);
                        }
                    }
                    _ => {}
                }
                return;
            }
            // Author names take Unicode escapes like the other text destinations.
            Dest::RevTbl if !matches!(w, "u" | "uc") => return,
            Dest::RevTbl => {}
            Dest::Pict => {
                match w {
                    "pngblip" => self.pict_kind = 1,
                    "jpegblip" => self.pict_kind = 2,
                    "picwgoal" => self.pict_w = Some(pv.clamp(1, 31_680) as f32 / 20.0),
                    "pichgoal" => self.pict_h = Some(pv.clamp(1, 31_680) as f32 / 20.0),
                    "picscalex" => self.pict_sx = pv.clamp(1, 1000) as f32,
                    "picscaley" => self.pict_sy = pv.clamp(1, 1000) as f32,
                    _ => {}
                }
                return;
            }
            // Text destinations take Unicode escapes (with fallback skipping) like the body.
            Dest::Info(_) | Dest::FldInst | Dest::Bookmark if !matches!(w, "u" | "uc") => return,
            Dest::Info(_) | Dest::FldInst | Dest::Bookmark => {}
            // Who changed the formatting and when, inside the old formatting (as Word writes it).
            Dest::OldProps(false) if matches!(w, "crauth" | "crdate") => {
                if w == "crauth" {
                    self.st.old_rev.0 = Some(pv);
                } else {
                    self.st.old_rev.1 = Some(pv as u32);
                }
                return;
            }
            // Old formatting takes formatting words only: no text, structure or document state,
            // and no paragraph reset among the character formatting.
            Dest::OldProps(_) if !is_format_word(w) => return,
            Dest::OldProps(false) if w == "pard" => return,
            // Border words there would edit the cell definition being read, not the group's copy.
            Dest::OldProps(_) if self.cell_side.is_some() && w.starts_with("brdr") => return,
            Dest::OldProps(false) => {
                if is_char_format_word(w) && self.st.old_words.len() < MAX_OLD_WORDS {
                    self.st.old_words.push((w.to_string(), p));
                }
            }
            Dest::OldProps(true) | Dest::Body => {}
        }
        match w {
            "par" => self.end_para_mark(),
            "sect" => self.end_section(),
            "cell" => self.end_cell(),
            "nestcell" => self.end_para(true),
            "row" => self.end_row(),
            "line" | "softline" => self.ch('\n'),
            "tab" => self.ch('\t'),
            "page" | "softpage" => self.ch('\u{000C}'),
            "column" => self.ch('\u{000E}'),
            "emdash" => self.ch('—'),
            "endash" => self.ch('–'),
            "bullet" => self.ch('•'),
            "lquote" => self.ch('‘'),
            "rquote" => self.ch('’'),
            "ldblquote" => self.ch('“'),
            "rdblquote" => self.ch('”'),
            "emspace" => self.ch('\u{2003}'),
            "enspace" => self.ch('\u{2002}'),
            "qmspace" => self.ch('\u{2005}'),
            "zwj" => self.ch('\u{200D}'),
            "zwnj" => self.ch('\u{200C}'),
            "zwbo" => self.ch('\u{200B}'),
            "zwnbo" => self.ch('\u{2060}'),
            "u" => {
                let u = (pv as i64).rem_euclid(65_536) as u16;
                if (0xD800..0xDC00).contains(&u) {
                    self.pending_high = Some(u);
                } else {
                    let c = match self.pending_high.take() {
                        Some(h) if (0xDC00..0xE000).contains(&u) => char::decode_utf16([h, u]).next().and_then(Result::ok).unwrap_or('\u{FFFD}'),
                        _ => char::from_u32(u32::from(u)).unwrap_or('\u{FFFD}'),
                    };
                    self.ch(c);
                }
                self.skip_chars = self.st.uc;
            }
            "uc" => self.st.uc = pv.clamp(0, 8) as usize,
            "plain" => {
                self.st.fmt = Fmt { link: self.st.fmt.link.clone(), ..Default::default() };
                self.st.rev = RevSt::default();
                self.st.chg_c = FmtRevSt::default();
                self.st.hidden = false;
            }
            "pard" => {
                let field = self.st.field;
                // Revision marks are character formatting, which `\pard` leaves alone.
                let (dest, fmt, rev, uc, hidden) = (self.st.dest, self.st.fmt.clone(), self.st.rev, self.st.uc, self.st.hidden);
                let chg_c = std::mem::take(&mut self.st.chg_c);
                self.st = St { dest, fmt, rev, chg_c, uc, hidden, field, ..Default::default() };
            }
            "revised" => self.st.rev.ins = on,
            "deleted" => self.st.rev.del = on,
            "revauth" => self.st.rev.auth = pv,
            "revauthdel" => self.st.rev.authdel = pv,
            // DTTMs are 32-bit patterns, written signed or unsigned.
            "revdttm" => self.st.rev.dttm = pv as u32,
            "revdttmdel" => self.st.rev.dttmdel = pv as u32,
            "crauth" => self.st.chg_c.auth = pv,
            "crdate" => self.st.chg_c.date = pv as u32,
            "prauth" => self.st.chg_p.auth = pv,
            "prdate" => self.st.chg_p.date = pv as u32,
            "b" => self.st.fmt.bold = on,
            "i" => self.st.fmt.italic = on,
            "ul" | "uld" | "uldash" | "uldashd" | "uldashdd" | "uldb" | "ulth" | "ulw" | "ulwave" | "ulhwave" | "ulldash" | "ulthd" => {
                self.st.fmt.underline = on
            }
            "ulnone" => self.st.fmt.underline = false,
            "strike" | "striked" => self.st.fmt.strike = on,
            "super" | "up" => {
                self.st.fmt.sup = on;
                self.st.fmt.sub = false;
            }
            "sub" | "dn" => {
                self.st.fmt.sub = on;
                self.st.fmt.sup = false;
            }
            "nosupersub" => {
                self.st.fmt.sup = false;
                self.st.fmt.sub = false;
            }
            "fs" => self.st.fmt.size = if pv > 0 { Some((pv as f32 / 2.0).clamp(1.0, 1638.0)) } else { None },
            "ansicpg" => {
                if let Some(cp) = u16::try_from(pv).ok().filter(|cp| wordcraft_doc::encoding::is_text_code_page(*cp)) {
                    self.ansicpg = cp;
                }
            }
            // The Macintosh and IBM PC character sets: 8-bit text is Mac OS Roman, or DOS code
            // page 437 (`\pc`) or 850 (`\pca`) (#1653).
            "mac" if self.ansicpg == 0 => self.ansicpg = wordcraft_doc::encoding::MAC_ROMAN_CODE_PAGE,
            "pc" if self.ansicpg == 0 => self.ansicpg = 437,
            "pca" if self.ansicpg == 0 => self.ansicpg = 850,
            "deff" => self.deff = pv,
            "f" => {
                self.st.cp = self.font_cps.get(&pv).copied();
                let name = self.fonts.get(&pv).cloned();
                match name {
                    Some(n) if model::is_mono(&n) => {
                        self.st.fmt.code = true;
                        self.st.fmt.font = None;
                    }
                    Some(n) => {
                        self.st.fmt.code = false;
                        self.st.fmt.font = Some(n);
                    }
                    None => {}
                }
            }
            "cf" => self.st.fmt.color = self.colors.get(pv.max(0) as usize).copied().flatten(),
            "highlight" | "chcbpat" | "cb" => self.st.fmt.background = self.colors.get(pv.max(0) as usize).copied().flatten(),
            "v" => self.st.hidden = on,
            "ql" => self.st.align = Some(Align::Left),
            "qc" => self.st.align = Some(Align::Center),
            "qr" => self.st.align = Some(Align::Right),
            "qj" => self.st.align = Some(Align::Justify),
            "qd" => self.st.align = Some(Align::Distribute),
            "s" => self.st.style = pv,
            "outlinelevel" => self.st.outline = Some(pv.clamp(0, 9) as u8),
            "pagebb" => self.st.pagebb = on,
            "intbl" => self.st.intbl = true,
            "ls" => self.st.ls = Some(pv),
            "ilvl" => self.st.ilvl = pv.clamp(0, 8) as u8,
            "brdrb" | "brdrt" | "brdrl" | "brdrr" if self.cell_side.is_none() => {
                if w == "brdrb" && self.st.dest == Dest::Body {
                    self.rule_pending = true;
                }
                self.st.border_side = w.chars().last();
            }
            "trowd" => {
                self.defs.clear();
                self.def_cur = CellDef::default();
                self.row_header = false;
            }
            "trhdr" => self.row_header = true,
            "clvmgf" => self.def_cur.vmerge = 1,
            "clvmrg" => self.def_cur.vmerge = 2,
            "clmgf" => self.def_cur.hmerge = 1,
            "clmrg" => self.def_cur.hmerge = 2,
            "clvertalt" => self.def_cur.valign = Some(VAlign::Top),
            "clvertalc" => self.def_cur.valign = Some(VAlign::Center),
            "clvertalb" => self.def_cur.valign = Some(VAlign::Bottom),
            "clcbpat" => self.def_cur.shading = self.colors.get(pv.max(0) as usize).copied().flatten(),
            "sb" => self.st.sb = Some(pv),
            "sa" => self.st.sa = Some(pv),
            "sl" => self.st.sl = pv,
            "slmult" => self.st.slmult = on,
            "li" | "lin" => self.st.li = Some(pv),
            "ri" | "rin" => self.st.ri = Some(pv),
            "fi" => self.st.fi = Some(pv),
            "clbrdrt" => self.cell_side = Some('t'),
            "clbrdrl" => self.cell_side = Some('l'),
            "clbrdrb" => self.cell_side = Some('b'),
            "clbrdrr" => self.cell_side = Some('r'),
            "brdrs" | "brdrth" | "brdrsh" | "brdrdb" | "brdrdot" | "brdrdash" | "brdrdashsm" | "brdrdashd" | "brdrdashdd" | "brdrhair"
            | "brdrtriple" | "brdrthtnsg" | "brdrtnthsg" | "brdrthtnmg" | "brdrtnthmg" | "brdrwavy" | "brdrwavydb" | "brdrnone" | "brdrnil"
            | "brdrtbl"
                if self.cell_side.is_some() || self.st.border_side.is_some() =>
            {
                use wordcraft_doc::props::{Border, BorderStyle};
                let style = match w {
                    "brdrnone" | "brdrnil" | "brdrtbl" => BorderStyle::None,
                    "brdrth" => BorderStyle::Thick,
                    "brdrdb" | "brdrthtnsg" | "brdrtnthsg" | "brdrthtnmg" | "brdrtnthmg" | "brdrwavydb" => BorderStyle::Double,
                    "brdrdot" => BorderStyle::Dotted,
                    "brdrdash" | "brdrdashsm" => BorderStyle::Dashed,
                    "brdrdashd" | "brdrdashdd" => BorderStyle::DotDash,
                    "brdrtriple" => BorderStyle::Triple,
                    "brdrwavy" => BorderStyle::Wave,
                    _ => BorderStyle::Single,
                };
                let b = (style != BorderStyle::None).then_some(Border { style, ..Border::single(0.5) });
                if let Some(slot) = self.cell_border_slot() {
                    *slot = b;
                }
            }
            "brdrw" if self.cell_side.is_some() || self.st.border_side.is_some() => {
                let width = (pv as f32 / 20.0).clamp(0.25, 6.0);
                if let Some(Some(b)) = self.cell_border_slot() {
                    b.width = width;
                }
            }
            "brdrcf" if self.cell_side.is_some() || self.st.border_side.is_some() => {
                let color = self.colors.get(pv.max(0) as usize).copied().flatten();
                if let Some(Some(b)) = self.cell_border_slot() {
                    b.color = color;
                }
            }
            "paperw" => self.page.w = Some(pv),
            "paperh" => self.page.h = Some(pv),
            "margl" => self.page.left = Some(pv),
            "margr" => self.page.right = Some(pv),
            "margt" => self.page.top = Some(pv),
            "margb" => self.page.bottom = Some(pv),
            "landscape" => self.page.landscape = on,
            _ if self.section_word(w, p) => {}
            "cellx" => {
                self.cell_side = None;
                let mut d = std::mem::take(&mut self.def_cur);
                d.right = pv;
                if self.defs.len() < 63 {
                    self.defs.push(d);
                }
            }
            "field" => self.start_field(),
            "fldrslt" => {
                let link = self.st.field.and_then(|i| self.fields.get(i)).and_then(|s| hyperlink(s));
                if link.is_some() {
                    self.st.fmt.link = link.as_deref().and_then(model::kept_link);
                }
            }
            _ => {}
        }
    }

    fn start_field(&mut self) {
        if self.fields.len() < 100_000 {
            self.fields.push(String::new());
            self.st.field = Some(self.fields.len() - 1);
        }
    }

    fn in_info(&self) -> bool {
        self.info_depth.is_some_and(|d| self.depth() > d)
    }
}

/// The character formatting the paragraph style of `kind` gives its runs, as the writer's
/// stylesheet and paragraph properties set it.
fn style_fmt(kind: Kind) -> Fmt {
    match kind {
        Kind::Heading(n) => Fmt { bold: true, size: HEAD_SIZES.get(usize::from(n.clamp(1, 6)) - 1).copied(), ..Default::default() },
        Kind::Title => Fmt { size: Some(28.0), ..Default::default() },
        Kind::Quote => Fmt { italic: true, ..Default::default() },
        Kind::Code => Fmt { code: true, size: Some(10.0), ..Default::default() },
        Kind::Normal | Kind::Rule => Fmt::default(),
    }
}

/// `f` with what it doesn't set directly taken from its paragraph style's formatting `base`.
fn styled(base: &Fmt, f: &Fmt) -> Fmt {
    let mut e = f.clone();
    e.bold |= base.bold;
    e.italic |= base.italic;
    if e.size.is_none() {
        e.size = base.size;
    }
    if !e.code && e.font.is_none() {
        e.code = base.code;
    }
    e
}

/// A `\stylesheet` entry's character formatting; `None` where the entry doesn't set it.
#[derive(Clone, Copy, Debug, Default)]
struct StyleDef {
    based_on: Option<i32>,
    bold: Option<bool>,
    italic: Option<bool>,
    size: Option<f32>,
    /// A `\fonttbl` index, named when it's used (the table may follow).
    font: Option<i32>,
}

/// Clear from `f` the formatting a paragraph of kind `kind` gets from its style, whose
/// formatting is `base`: a heading's or title's size and bold when they equal the style's, a
/// quote's italic, a code run's font when it's the style's (and, as before, its size and bold).
fn drop_style_formatting(kind: Kind, base: &Fmt, f: &mut Fmt) {
    match kind {
        Kind::Heading(_) | Kind::Title => {
            if f.size.is_some() && f.size == base.size {
                f.size = None;
            }
            if base.bold {
                f.bold = false;
            }
        }
        Kind::Code => {
            f.size = None;
            f.bold = false;
            let same_font = match (&f.font, &base.font) {
                (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                (a, b) => a.is_none() && b.is_none(),
            };
            if f.code == base.code && same_font {
                f.code = false;
                f.font = None;
            }
        }
        Kind::Quote => {
            if base.italic {
                f.italic = false;
            }
        }
        Kind::Normal | Kind::Rule => {}
    }
}

/// A character or paragraph formatting word: what old formatting (`\oldcprops`,
/// `\oldpprops`) may hold.
fn is_format_word(w: &str) -> bool {
    matches!(
        w,
        "plain"
            | "b"
            | "i"
            | "ulnone"
            | "strike"
            | "striked"
            | "super"
            | "up"
            | "sub"
            | "dn"
            | "nosupersub"
            | "fs"
            | "f"
            | "cf"
            | "highlight"
            | "chcbpat"
            | "cb"
            | "v"
            | "pard"
            | "ql"
            | "qc"
            | "qr"
            | "qj"
            | "qd"
            | "s"
            | "outlinelevel"
            | "pagebb"
            | "intbl"
            | "ls"
            | "ilvl"
            | "sb"
            | "sa"
            | "sl"
            | "slmult"
            | "li"
            | "lin"
            | "ri"
            | "rin"
            | "fi"
            | "brdrw"
            | "brdrcf"
    ) || (w.starts_with("ul") && w.len() <= 8)
        || w.starts_with("brdr")
}

/// A character formatting word: what an `\oldcprops` group's words are replayed from.
fn is_char_format_word(w: &str) -> bool {
    matches!(
        w,
        "plain"
            | "b"
            | "i"
            | "strike"
            | "striked"
            | "super"
            | "up"
            | "sub"
            | "dn"
            | "nosupersub"
            | "fs"
            | "f"
            | "cf"
            | "highlight"
            | "chcbpat"
            | "cb"
            | "v"
    ) || (w.starts_with("ul") && w.len() <= 8)
}

/// The spacing and indents in points of the paragraph formatting `st`.
fn spacing(st: &St) -> model::Spacing {
    use wordcraft_doc::props::LineSpacing;
    let pt = |v: i32| (v as f32 / 20.0).clamp(-1584.0, 1584.0);
    let gap = |v: Option<i32>| v.map(|v| pt(v).max(0.0));
    let sl = st.sl;
    let line = if sl == 0 {
        None
    } else if sl < 0 {
        Some(LineSpacing::Exactly(pt(sl.saturating_neg()).max(1.0)))
    } else if st.slmult {
        // \sl240\slmult1 is single spacing.
        Some(LineSpacing::Multiple((sl as f32 / 240.0).clamp(0.06, 132.0)))
    } else {
        Some(LineSpacing::AtLeast(pt(sl)))
    };
    model::Spacing { before: gap(st.sb), after: gap(st.sa), line, left: st.li.map(pt), right: st.ri.map(pt), first: st.fi.map(pt) }
}

/// The target of a `HYPERLINK "url"` (or `HYPERLINK "url" \l "bookmark"`, `HYPERLINK \l "bookmark"`) field instruction.
fn hyperlink(instr: &str) -> Option<String> {
    let t = instr.trim();
    let rest = if t.get(..9).is_some_and(|h| h.eq_ignore_ascii_case("hyperlink")) {
        t[9..].trim()
    } else {
        return None;
    };
    if rest.is_empty() {
        return None;
    }

    let mut url: Option<String> = None;
    let mut local: Option<String> = None;
    let mut switch: Option<&str> = None;

    let mut i = 0;
    let bytes = rest.as_bytes();
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'\\' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'*') {
                i += 1;
            }
            switch = Some(&rest[start..i]);
            continue;
        }
        let arg = if bytes[i] == b'"' {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let val = &rest[start..i];
            if i < bytes.len() && bytes[i] == b'"' {
                i += 1;
            }
            val.to_string()
        } else {
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            rest[start..i].to_string()
        };
        let clean_arg = arg.replace("%22", "\"");
        if switch.take() == Some("\\l") {
            local = Some(clean_arg);
        } else if url.is_none() {
            url = Some(clean_arg);
        }
    }

    match (url, local) {
        (Some(u), Some(l)) if !u.is_empty() && !l.is_empty() => {
            let frag = l.trim_start_matches('#');
            if u.contains('#') { Some(u) } else { Some(format!("{u}#{frag}")) }
        }
        (Some(u), _) if !u.is_empty() => Some(u),
        (None, Some(l)) if !l.is_empty() => {
            let frag = l.trim_start_matches('#');
            Some(format!("#{frag}"))
        }
        _ => None,
    }
}

/// Parse RTF bytes into the flow model.
pub fn parse(bytes: &[u8]) -> Result<Flow, String> {
    parse_with_page(bytes).map(|(f, _, _)| f)
}

/// The flow, the page, and the default font (`\deff`, font 0 when the file names none), which
/// is the font of all text that doesn't select one.
fn parse_with_page(bytes: &[u8]) -> Result<(Flow, PageGeom, Option<String>), String> {
    parse_full(bytes).map(|r| (r.flow, r.page, r.default_font))
}

/// Everything an import takes from an RTF file.
struct Parsed {
    flow: Flow,
    page: PageGeom,
    /// The last section's properties (the others are on the paragraphs that end them).
    last_section: wordcraft_doc::SectionProps,
    default_font: Option<String>,
    /// Each section's headers and footers, in order, the last section's last.
    headers_footers: Vec<Vec<(HeaderFooter, Vec<FBlock>)>>,
}

fn parse_full(bytes: &[u8]) -> Result<Parsed, String> {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0);
    if !bytes.get(start..).is_some_and(|b| b.starts_with(b"{\\rtf")) {
        return Err("not an RTF document (it doesn't start with {\\rtf)".into());
    }
    let mut r = Reader::default();
    let mut i = start;
    let mut first_in_group = false;
    let mut starred = false;
    while let Some(&c) = bytes.get(i) {
        match c {
            b'{' => {
                r.open();
                first_in_group = true;
                starred = false;
                i += 1;
                continue;
            }
            b'}' => {
                r.close();
                if r.info_depth.is_some_and(|d| r.depth() < d) {
                    r.info_depth = None;
                }
                first_in_group = false;
                starred = false;
                i += 1;
                continue;
            }
            b'\\' => {
                let Some(&n) = bytes.get(i + 1) else { break };
                if n.is_ascii_alphabetic() {
                    let mut j = i + 1;
                    while bytes.get(j).is_some_and(u8::is_ascii_alphabetic) && j - i <= 32 {
                        j += 1;
                    }
                    let word = std::str::from_utf8(bytes.get(i + 1..j).unwrap_or(b"")).unwrap_or("");
                    let mut k = j;
                    let neg = bytes.get(k) == Some(&b'-');
                    if neg {
                        k += 1;
                    }
                    let d0 = k;
                    while bytes.get(k).is_some_and(u8::is_ascii_digit) && k - d0 < 10 {
                        k += 1;
                    }
                    let param = if k > d0 {
                        let v = std::str::from_utf8(bytes.get(d0..k).unwrap_or(b"0")).ok().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                        let v = if neg { -v } else { v };
                        // A date (DTTM) is a 32-bit pattern some writers put unsigned.
                        let v = if matches!(word, "revdttm" | "revdttmdel" | "crdate" | "prdate")
                            && v > i64::from(i32::MAX)
                            && v <= i64::from(u32::MAX)
                        {
                            v - (1i64 << 32)
                        } else {
                            v
                        };
                        Some(v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
                    } else {
                        if neg {
                            k -= 1;
                        }
                        None
                    };
                    if bytes.get(k) == Some(&b' ') {
                        k += 1;
                    }
                    i = k;
                    if word == "bin" {
                        let n = param.unwrap_or(0).max(0) as usize;
                        let end = i.saturating_add(n).min(bytes.len());
                        if r.st.dest == Dest::Pict
                            && let Some(data) = bytes.get(i..end)
                            && r.pict_hex.len() + data.len() <= MAX_PICT
                        {
                            r.pict_hex.extend_from_slice(data);
                        }
                        i = end;
                        continue;
                    }
                    if word == "info" {
                        r.info_depth = Some(r.depth());
                    }
                    if r.skip_chars > 0
                        && matches!(r.st.dest, Dest::Body | Dest::Info(_) | Dest::FldInst | Dest::Bookmark | Dest::FontTbl)
                        && !matches!(word, "par" | "pard" | "cell" | "row")
                    {
                        r.skip_chars -= 1;
                        first_in_group = false;
                        continue;
                    }
                    r.word(word, param, starred, first_in_group);
                    first_in_group = false;
                    starred = false;
                    continue;
                }
                match n {
                    b'\'' => {
                        let h = bytes.get(i + 2..i + 4).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                        match h {
                            Some(v) => {
                                if r.st.dest == Dest::Pict {
                                    // Hex digits, two at a time, without a String per picture byte.
                                    const HEX: &[u8; 16] = b"0123456789abcdef";
                                    let h = [HEX[usize::from(v >> 4)], HEX[usize::from(v & 0x0f)]];
                                    if let Ok(s) = std::str::from_utf8(&h) {
                                        r.text(s);
                                    }
                                } else {
                                    r.byte(v);
                                }
                                i += 4;
                            }
                            None => i += 2,
                        }
                    }
                    b'*' => {
                        starred = true;
                        i += 2;
                        continue;
                    }
                    b'~' | b' ' => {
                        r.ch('\u{00A0}');
                        i += 2;
                    }
                    b'-' => {
                        r.ch('\u{00AD}');
                        i += 2;
                    }
                    b'_' => {
                        r.ch('\u{2011}');
                        i += 2;
                    }
                    b'\n' | b'\r' => {
                        r.word("par", None, false, false);
                        i += 2;
                    }
                    b'\\' | b'{' | b'}' => {
                        r.ch(n as char);
                        i += 2;
                    }
                    _ => i += 2,
                }
                first_in_group = false;
            }
            b'\r' | b'\n' | 0 => i += 1,
            _ => {
                // A run of plain text.
                let mut j = i;
                while bytes.get(j).is_some_and(|b| !matches!(b, b'{' | b'}' | b'\\' | b'\r' | b'\n' | 0)) {
                    j += 1;
                }
                let chunk = bytes.get(i..j).unwrap_or(b"");
                if r.st.dest == Dest::Pict && r.skip_chars == 0 && chunk.is_ascii() {
                    // Picture hex is not text: it needs no decoder, so neither a pending lead byte
                    // nor the code page's single-byte path applies to it.
                    if let Ok(s) = std::str::from_utf8(chunk) {
                        r.text(s);
                    }
                } else if r.skip_chars == 0 && chunk.is_ascii() && r.mb_decoder.is_none() {
                    // Plain ASCII with no multibyte run open: append it as it is.
                    if let Ok(s) = std::str::from_utf8(chunk) {
                        r.text(s);
                    }
                } else {
                    r.run(chunk);
                }
                i = j;
                first_in_group = false;
            }
        }
    }
    // A header or footer left open by a truncated file ends here, and so does a multibyte run.
    r.finish_multibyte();
    r.end_header_footer();
    if r.para_has || !r.para.inlines.is_empty() {
        r.end_para(false);
    }
    r.close_table();
    if let Some((y, mo, d, h, mi)) = r.created
        && y > 0
    {
        r.meta.created = format!("{y:04}-{:02}-{:02}T{:02}:{:02}:00Z", mo.clamp(1, 12), d.clamp(1, 31), h.min(23), mi.min(59));
    }
    let mut blocks = std::mem::take(&mut r.body);
    // A file that ends with `\sect` ends its last section there: no empty one follows, and
    // header or footer groups after that `\sect` (a section without text) are dropped.
    let mut last_section = r.sec.props(&r.page);
    let mut headers_footers = std::mem::take(&mut r.section_parts);
    if let Some(FBlock::Para(p)) = blocks.last_mut()
        && let Some(s) = p.section.take()
    {
        last_section = *s;
    } else {
        headers_footers.push(std::mem::take(&mut r.hf_parts));
    }
    if blocks.is_empty() {
        blocks.push(FBlock::Para(Para::default()));
    }
    let default_font = r.fonts.get(&r.deff).filter(|n| !model::is_mono(n)).map(|n| n.to_string());
    Ok(Parsed { flow: Flow { blocks, meta: r.meta }, page: r.page, last_section, default_font, headers_footers })
}

/// The column grid of a table from its rows' cell right edges: every distinct edge starts a grid
/// column, and each cell spans the grid columns between its left and right edges, as Word lays
/// out RTF rows whose cells don't line up. Returns the widths in points, or none (equal columns)
/// when the edges don't make a grid.
fn column_grid(rows: &mut [Vec<Cell>], rights: &[Vec<i32>]) -> Vec<f32> {
    let mut edges: Vec<i32> = rights.iter().flatten().copied().filter(|&x| x > 0).collect();
    edges.sort_unstable();
    edges.dedup();
    let ok = rights.len() == rows.len()
        && rows
            .iter()
            .zip(rights)
            .all(|(r, x)| r.len() == x.len() && x.windows(2).all(|w| w.first() < w.get(1)) && x.first().is_some_and(|&v| v > 0))
        && !edges.is_empty()
        && edges.len() <= 63;
    if !ok {
        return Vec::new();
    }
    for (row, x) in rows.iter_mut().zip(rights) {
        let mut left = 0;
        for (c, &right) in row.iter_mut().zip(x) {
            let span = edges.iter().filter(|&&e| e > left && e <= right).count().max(1);
            c.colspan = span as u32;
            left = right;
        }
    }
    let mut prev = 0;
    edges
        .iter()
        .map(|&e| {
            let w = (e - prev) as f32 / 20.0;
            prev = e;
            w
        })
        .collect()
}

/// Parse RTF bytes into a document.
pub fn import(bytes: &[u8]) -> Result<Document, String> {
    model::begin_import();
    let Parsed { flow, last_section, default_font, headers_footers, .. } = parse_full(bytes)?;
    let mut doc = model::to_doc_in(&flow, last_section);
    // Each section's own headers and footers; those it leaves out stay linked to the previous.
    let mut sets = Vec::with_capacity(headers_footers.len());
    for parts in &headers_footers {
        let (mut headers, mut footers) = (wordcraft_doc::section::HeaderSet::default(), wordcraft_doc::section::HeaderSet::default());
        for (which, blocks) in parts {
            let blocks = model::part_blocks(&mut doc, blocks);
            let id = doc.add_part(if which.footer { wordcraft_doc::PartKind::Footer } else { wordcraft_doc::PartKind::Header }, blocks);
            let set = if which.footer { &mut footers } else { &mut headers };
            match which.pages {
                HfPages::Default => set.default = Some(id),
                HfPages::First => set.first = Some(id),
                HfPages::Even => set.even = Some(id),
            }
        }
        sets.push((headers, footers));
    }
    let mut sets = sets.into_iter();
    for b in &mut doc.body {
        if let wordcraft_doc::Block::Para(p) = &**b
            && p.section.is_some()
            && let wordcraft_doc::Block::Para(p) = std::sync::Arc::make_mut(b)
            && let Some(s) = p.section.as_deref_mut()
            && let Some((h, f)) = sets.next()
        {
            (s.headers, s.footers) = (h, f);
        }
    }
    if let Some((h, f)) = sets.next() {
        (doc.last_section.headers, doc.last_section.footers) = (h, f);
    }
    // RTF's paragraph defaults are single spacing and no space around paragraphs; what a file
    // wants beyond that it states on each paragraph.
    let dp = &mut doc.styles.default_para;
    dp.space_before = Some(0.0);
    dp.space_after = Some(0.0);
    dp.line_spacing = Some(wordcraft_doc::props::LineSpacing::Multiple(1.0));
    if default_font.is_some() {
        doc.styles.default_chr.font = default_font;
    }
    Ok(doc)
}

#[cfg(test)]
mod tests;
