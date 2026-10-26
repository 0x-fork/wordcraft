//! Keyboard: typing, editing keys and command shortcuts (the user's custom keys from Customize
//! Keyboard first, then the registry's `shortcut` fields).

use egui::{Key, Modifiers};
use serde_json::{Value, json};

use crate::WordApp;
use wordcraft_engine::ImeEditOutcome;

/// The most pre-edit text a UI pass will hold and paint. A composition is a word or a phrase;
/// anything larger is a malformed or hostile event, and laying it out every frame would cost.
const MAX_IME_PREEDIT_BYTES: usize = wordcraft_doc::ime_text::MAX_IME_TEXT_BYTES;

/// The largest IME commit inserted in one event. Real commits are short; a multi-megabyte one is
/// refused rather than turned into document text without a bound.
const MAX_IME_COMMIT_BYTES: usize = 64 * 1024;

/// The registry's name for a key.
pub fn key_name(k: Key) -> Option<&'static str> {
    Some(match k {
        Key::ArrowLeft => "Left",
        Key::ArrowRight => "Right",
        Key::ArrowUp => "Up",
        Key::ArrowDown => "Down",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "PageUp",
        Key::PageDown => "PageDown",
        Key::Enter => "Enter",
        Key::Tab => "Tab",
        Key::Backspace => "Backspace",
        Key::Delete => "Delete",
        Key::Escape => "Escape",
        Key::Space => "Space",
        Key::Insert => "Insert",
        Key::Equals | Key::Plus => "=",
        Key::Minus => "-",
        Key::Period => ".",
        Key::Comma => ",",
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        Key::F1 => "F1",
        Key::F2 => "F2",
        Key::F3 => "F3",
        Key::F4 => "F4",
        Key::F5 => "F5",
        Key::F6 => "F6",
        Key::F7 => "F7",
        Key::F8 => "F8",
        Key::F9 => "F9",
        Key::F10 => "F10",
        Key::F11 => "F11",
        Key::F12 => "F12",
        Key::A => "A",
        Key::B => "B",
        Key::C => "C",
        Key::D => "D",
        Key::E => "E",
        Key::F => "F",
        Key::G => "G",
        Key::H => "H",
        Key::I => "I",
        Key::J => "J",
        Key::K => "K",
        Key::L => "L",
        Key::M => "M",
        Key::N => "N",
        Key::O => "O",
        Key::P => "P",
        Key::Q => "Q",
        Key::R => "R",
        Key::S => "S",
        Key::T => "T",
        Key::U => "U",
        Key::V => "V",
        Key::W => "W",
        Key::X => "X",
        Key::Y => "Y",
        Key::Z => "Z",
        _ => return None,
    })
}

/// A key with its modifiers as the registry writes shortcuts (`Mod+Shift+K`).
pub fn combo(m: Modifiers, key: &str, with_shift: bool) -> String {
    let mut s = String::new();
    if m.command {
        s.push_str("Mod+");
    }
    if m.ctrl && !m.command {
        s.push_str("Ctrl+");
    }
    if m.mac_cmd && m.ctrl {
        s.push_str("Ctrl+");
    }
    if m.alt {
        s.push_str("Alt+");
    }
    if m.shift && with_shift {
        s.push_str("Shift+");
    }
    s.push_str(key);
    s
}

/// Shortcuts for interface commands (`ui.*`), which aren't in the engine's keymap: Alt+Q moves
/// to "Tell me" command search, as in Word for Windows (#819). Not on macOS, where Option+Q types
/// a character. On macOS ⌘⌥R collapses or expands the ribbon, as in Word for Mac.
fn ui_shortcut(key: Key, m: Modifiers) -> Option<(&'static str, Value)> {
    let alt_only = m.alt && !m.command && !m.ctrl && !m.mac_cmd && !m.shift;
    let mac_cmd_alt = cfg!(target_os = "macos") && m.mac_cmd && m.alt && !m.ctrl && !m.shift;
    match key {
        Key::Q if alt_only && !cfg!(target_os = "macos") => Some(("ui.dialog", json!({"name": "commands"}))),
        Key::R if mac_cmd_alt => Some(("ui.collapseRibbon", json!({}))),
        // Ctrl+F1 collapses or expands the ribbon, as in Word for Windows (#1678). Not on macOS,
        // where Ctrl+F1 is the system's keyboard-access switch.
        Key::F1 if m.ctrl && !m.alt && !m.shift && !m.mac_cmd && !cfg!(target_os = "macos") => Some(("ui.collapseRibbon", json!({}))),
        _ => None,
    }
}

/// Find and run the command bound to this key. Returns true if handled.
fn dispatch(app: &mut WordApp, key: Key, m: Modifiers) -> bool {
    let reg = app.session.registry.clone();
    let name = key_name(key);
    let bound = name.and_then(|n| app.session.keymap.resolve(&reg, &combo(m, n, true)));
    // Ctrl+F1 collapses the ribbon only when the user hasn't given it to a command (#1678); the
    // other interface shortcuts (Alt+Q, ⌘⌥R) keep their place ahead of the keymap.
    if let Some((id, params)) = ui_shortcut(key, m).filter(|_| key != Key::F1 || bound.is_none()) {
        let _ = app.run(id, params);
        return true;
    }
    if let Some(spec) = bound {
        // Option+Shift+- types an em dash on macOS; Word's Alt+Shift+- (Collapse) only works in
        // Outline view, so elsewhere the key stays typing (#1933).
        if m.alt && !crate::widgets::enabled(app, spec.id) {
            return false;
        }
        let _ = app.run(spec.id, json!({}));
        return true;
    }
    let Some(name) = name else { return false };
    // Shift extends caret movement; Shift+Backspace still erases (#707).
    if m.shift
        && let Some(spec) = app.session.keymap.resolve(&reg, &combo(m, name, false))
        && let Some(params) = shift_variant_params(spec.id)
    {
        let _ = app.run(spec.id, params);
        return true;
    }
    false
}

/// How a command bound to a plain key runs when Shift is also held and nothing is bound to the
/// shifted combo: caret moves extend the selection, Backspace erases as usual (#707).
fn shift_variant_params(id: &str) -> Option<Value> {
    if id.starts_with("caret.") {
        Some(json!({"extend": true}))
    } else if id == "text.backspace" {
        Some(json!({}))
    } else {
        None
    }
}

/// Shift+Delete, which cuts the selection and does nothing without one, as in Word for Windows
/// (#1669). On Windows the windowing layer already sends it as a Cut; elsewhere it arrives as the
/// key, and no shortcut was bound to it.
fn is_shift_delete(key: Key, m: Modifiers) -> bool {
    key == Key::Delete && m.shift && !m.command && !m.ctrl && !m.alt && !m.mac_cmd
}

/// Shift+F10 opens the context menu, as the Menu key does (#821). (The Menu key itself doesn't
/// reach the app: the windowing layer drops it.)
fn is_context_menu_key(key: Key, m: Modifiers) -> bool {
    key == Key::F10 && m.shift && !m.command && !m.alt && !m.ctrl
}

/// F1–F12: shortcuts on these work without the canvas focused.
fn is_function_key(key: Key) -> bool {
    matches!(key, Key::F1 | Key::F2 | Key::F3 | Key::F4 | Key::F5 | Key::F6 | Key::F7 | Key::F8 | Key::F9 | Key::F10 | Key::F11 | Key::F12)
}

/// Keys while editing inside an equation. Returns true when handled.
fn equation_key(app: &mut WordApp, key: Key, m: Modifiers) -> bool {
    if m.command || m.alt || (m.ctrl && !cfg!(target_os = "macos")) {
        return false;
    }
    let (id, params) = match key {
        Key::ArrowLeft => ("equation.move", json!({"dir": "left"})),
        Key::ArrowRight => ("equation.move", json!({"dir": "right"})),
        Key::ArrowUp => ("equation.move", json!({"dir": "up"})),
        Key::ArrowDown => ("equation.move", json!({"dir": "down"})),
        Key::Home => ("equation.move", json!({"dir": "home"})),
        Key::End => ("equation.move", json!({"dir": "end"})),
        Key::Tab => ("equation.move", json!({"dir": if m.shift { "prev" } else { "next" }})),
        Key::Backspace => ("equation.backspace", json!({})),
        Key::Delete => ("equation.delete", json!({})),
        Key::Enter => ("equation.enter", json!({})),
        Key::Escape => ("equation.exit", json!({})),
        _ => return false,
    };
    let _ = app.run(id, params);
    true
}

/// Whether releasing `key` with `modifiers` is the plain Paste shortcut (Mod+V or the Paste
/// key) — not Mod+Shift+V (Paste Formatting) or Mod+Alt+V (Paste Special) — so a clipboard
/// picture may be pasted on it (#45).
fn is_paste_release(key: Key, modifiers: egui::Modifiers) -> bool {
    (modifiers.command || key == Key::Paste) && !modifiers.alt && !modifiers.shift
}

/// The modifiers a V release stands for: its own, or, when Mod went up before V and no V press
/// came (egui-winit keeps Mod+V presses), those held with Mod.
fn paste_release_modifiers(released: Modifiers, press_seen: bool, held: Option<Modifiers>) -> Modifiers {
    if released.command || press_seen { released } else { held.unwrap_or(released) }
}

/// Whether `key` is a Shift, Ctrl, Alt or Super key, whose own presses don't end a Mod+V.
fn is_modifier_key(key: Key) -> bool {
    matches!(
        key,
        Key::ShiftLeft | Key::ShiftRight | Key::ControlLeft | Key::ControlRight | Key::AltLeft | Key::AltRight | Key::SuperLeft | Key::SuperRight
    )
}

/// Whether releasing `key` with `modifiers` is Mod+Alt+V (Paste Special).
fn is_paste_special_release(key: Key, modifiers: egui::Modifiers) -> bool {
    key == Key::V && modifiers.command && modifiers.alt && !modifiers.shift
}

/// The adjacent replacement a refused byte-counted equation deletion must discard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EquationReplacementDrop {
    /// No deletion is waiting for a replacement.
    None,
    /// The next event may be the replacement: one `Commit`, or a `Text` starting a pair.
    Awaiting,
    /// The replacement's `Text` was dropped; its immediately following `Commit` also belongs
    /// to that same platform replacement.
    TextDropped,
}

/// Events for the focused canvas: text, editing keys, clipboard, IME. While keytips are showing
/// every key belongs to them (letters pick badges, Escape cancels), so nothing here runs.
pub fn canvas_events(app: &mut WordApp, ctx: &egui::Context) {
    if app.ui.keytips != crate::keytips::Phase::Off {
        return;
    }
    let events = ctx.input(|i| i.events.clone());
    let window = app.windows.shown();
    if app.session.view.read_mode {
        // Read Mode is for navigation and selection, not typing into the document. The ribbon's
        // Edit Document button or Escape returns to Print Layout before editing.
        for e in &events {
            if app.windows.shown() != window {
                break;
            }
            match e {
                // Not while a dialog or menu that takes Escape is open.
                egui::Event::Key { key: Key::Escape, pressed: true, .. } if app.escape_free => {
                    // Focus or Full Screen first (#1678), then Read Mode.
                    let id = crate::chrome::Screen::of(&app.session.view).exit_command().unwrap_or("view.printLayout");
                    let _ = app.run(id, json!({"value": false}));
                    break;
                }
                egui::Event::Key { key: Key::PageDown | Key::ArrowRight, pressed: true, modifiers, .. } if modifiers.is_none() => {
                    crate::canvas::turn_read_page(app, 1.0)
                }
                egui::Event::Key { key: Key::PageUp | Key::ArrowLeft, pressed: true, modifiers, .. } if modifiers.is_none() => {
                    crate::canvas::turn_read_page(app, -1.0)
                }
                egui::Event::Copy => {
                    let selected = app.session.selected_text();
                    if !selected.is_empty() {
                        ctx.copy_text(selected);
                    }
                }
                _ => {}
            }
        }
        return;
    }
    let alt_codes = alt_codes_supported(ctx);
    // The erased text's formatting is carried only for a replacement in this raw batch. The
    // undo join belongs to the deletion transaction instead: the engine's scoped
    // `ime_text_join` survives frames until its matching `text.insert` or any other command,
    // key or new request consumes or clears it, so a platform that delivers the `Commit` in a
    // later batch still shares the deletion's Undo checkpoint.
    app.canvas.ime_join_text = None;
    // A refused byte-counted equation deletion suppresses only the replacement immediately
    // paired with it. egui-winit pushes `DeleteSurroundingBytes` immediately before the
    // `Commit` from the same winit IME event, so one adjacent event is the replacement. Some
    // platforms route the same replacement through a `Text` followed by its `Commit`; that
    // adjacent pair is the only exception. Any other event is fresh input: clearing the state
    // lets a later key, pre-edit or direct commit in this same raw batch apply. This is local,
    // not canvas state: an untagged event in a later frame cannot be told apart from fresh
    // input, so it must never be dropped.
    let mut drop_equation_replacement = EquationReplacementDrop::None;
    // A body/equation replacement may have invalidated a live preedit before this batch (or
    // an earlier event can do it below). Its already queued IME replacement belongs to the old
    // target. This is separate from the outgoing interrupt, which is observed after mutation.
    // Keep the quarantine local to this batch: a later untagged commit can be fresh input.
    let mut interrupted_preedit = std::mem::take(&mut app.canvas.ime_interrupted_preedit);
    let mut interrupted_replacement = EquationReplacementDrop::None;
    // Mod held since an earlier frame (its ModifiersChanged went by then).
    let held = ctx.input(|i| i.modifiers);
    if held.command {
        app.canvas.paste_mods = Some(held);
    }
    for e in events {
        // A key closed the window (Mod+W) or brought another forward: the frame's other keys
        // were typed in this one and are dropped.
        if app.windows.shown() != window {
            break;
        }
        interrupted_preedit |= std::mem::take(&mut app.canvas.ime_interrupted_preedit);
        let stale_composition_event = if interrupted_preedit {
            match &e {
                egui::Event::Ime(egui::ImeEvent::DeleteSurrounding { .. } | egui::ImeEvent::DeleteSurroundingBytes { .. }) => {
                    interrupted_replacement = EquationReplacementDrop::Awaiting;
                    true
                }
                egui::Event::Ime(egui::ImeEvent::Commit(_)) => {
                    interrupted_preedit = false;
                    interrupted_replacement = EquationReplacementDrop::None;
                    true
                }
                egui::Event::Text(_) if matches!(interrupted_replacement, EquationReplacementDrop::Awaiting) => {
                    interrupted_replacement = EquationReplacementDrop::TextDropped;
                    true
                }
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) if text.is_empty() => true,
                // A new marked composition or ordinary input is not the old target's queued
                // replacement. Commit-only input without an interrupted preedit is never gated.
                egui::Event::Ime(egui::ImeEvent::Preedit { .. })
                | egui::Event::Text(_)
                | egui::Event::Paste(_)
                | egui::Event::Key { pressed: true, .. } => {
                    interrupted_preedit = false;
                    interrupted_replacement = EquationReplacementDrop::None;
                    false
                }
                _ => false,
            }
        } else {
            false
        };
        if stale_composition_event {
            app.session.clear_ime_joins();
            app.canvas.ime_join_text = None;
            continue;
        }
        // Which presses reached the canvas, and the modifiers held with Mod, in event order: Mod
        // can go down and up within one frame, leaving the frame's modifiers without it.
        match &e {
            egui::Event::ModifiersChanged(m) if m.command => app.canvas.paste_mods = Some(*m),
            egui::Event::Key { key: Key::V | Key::Paste, pressed: true, .. } => app.canvas.v_pressed = true,
            egui::Event::Key { key, pressed: true, modifiers, .. } if !is_modifier_key(*key) => {
                app.canvas.paste_mods = modifiers.command.then_some(*modifiers);
            }
            egui::Event::WindowFocused(_) => {
                app.canvas.v_pressed = false;
                app.canvas.paste_mods = None;
            }
            _ => {}
        }
        // The replacement is the event immediately after the refused deletion: one `Commit`,
        // or the adjacent `Text` + `Commit` pair some platforms use. Any intervening event is
        // unrelated and clears the state before it is handled.
        let drop_event = match (drop_equation_replacement, &e) {
            (EquationReplacementDrop::None, _) => false,
            (EquationReplacementDrop::Awaiting, egui::Event::Text(_)) => {
                drop_equation_replacement = EquationReplacementDrop::TextDropped;
                true
            }
            (EquationReplacementDrop::Awaiting, egui::Event::Ime(egui::ImeEvent::Commit(_))) => {
                drop_equation_replacement = EquationReplacementDrop::None;
                true
            }
            (EquationReplacementDrop::Awaiting, _) => {
                drop_equation_replacement = EquationReplacementDrop::None;
                false
            }
            (EquationReplacementDrop::TextDropped, egui::Event::Ime(egui::ImeEvent::Commit(_))) => {
                drop_equation_replacement = EquationReplacementDrop::None;
                true
            }
            (EquationReplacementDrop::TextDropped, _) => {
                drop_equation_replacement = EquationReplacementDrop::None;
                false
            }
        };
        if drop_event {
            app.canvas.ime_preedit.clear();
            app.canvas.ime_preedit_range = None;
            app.canvas.ime_interrupt_now = true;
            continue;
        }
        // An Alt shortcut (Alt+Shift+D, Alt+F7…) may also send its key's text; it isn't typed.
        match &e {
            egui::Event::Key { pressed: true, .. } => {
                app.canvas.swallow_text = false;
                app.canvas.alt_code_echo = None;
                app.canvas.ime_join_text = None;
                // A key between a deletion and its text means that text is fresh input, not the
                // replacement the deletion promised (even when the key runs no command).
                app.session.clear_ime_text_join();
            }
            egui::Event::Text(_) if std::mem::take(&mut app.canvas.swallow_text) => continue,
            egui::Event::Text(t) => {
                if let Some(echo) = app.canvas.alt_code_echo.take()
                    && alt_code_echoed(app, &echo, t)
                {
                    continue;
                }
            }
            // Focus elsewhere (another window) ends an Alt code unfinished.
            egui::Event::WindowFocused(false) => app.canvas.alt_code.clear(),
            _ => {}
        }
        if alt_codes && app.session.math.is_none() && alt_code_event(app, &e) {
            continue;
        }
        if let egui::Event::Paste(t) = &e {
            app.canvas.pasted = true;
            // Copied picture files arrive as their paths: insert the pictures (#45).
            if app.session.math.is_none() && !ctx.input(|i| i.modifiers.alt) && crate::paste_picture::paste_named_files(app, t) {
                continue;
            }
        }
        // Desktop: Mod+V with no text on the clipboard sends no paste event, only the key's
        // release; paste the clipboard's picture then (#45). A paste event before the release
        // means it was text. (The web reads pasted pictures itself.) Mod released before V still
        // pastes: a release with no V press seen was Mod+V.
        if let egui::Event::Key { key: key @ (Key::V | Key::Paste), pressed: false, modifiers, .. } = &e {
            let press_seen = std::mem::take(&mut app.canvas.v_pressed);
            let modifiers = &paste_release_modifiers(*modifiers, press_seen, app.canvas.paste_mods.take());
            let paste_keys = is_paste_release(*key, *modifiers);
            let pasted = std::mem::take(&mut app.canvas.pasted);
            if !pasted && paste_keys && app.session.math.is_none() && app.services.clipboard_picture.is_some() {
                let _ = app.run("edit.paste", json!({}));
            }
            // Mod+Alt+V with no text on the clipboard: Paste Special still opens, listing what
            // is there (a picture) or saying what it holds (#687, #833).
            if !pasted && is_paste_special_release(*key, *modifiers) && app.session.math.is_none() && app.services.clipboard_picture.is_some() {
                let _ = app.run("edit.pasteSpecial", json!({}));
            }
            continue;
        }
        // Editing an equation: text and editing keys go into it.
        if app.session.math.is_some() {
            match &e {
                egui::Event::Text(t) => {
                    let m = ctx.input(|i| i.modifiers);
                    let is_cmd = if cfg!(target_os = "macos") { m.command } else { (m.command || m.ctrl) && !m.alt };
                    if !is_cmd && !t.is_empty() && t.chars().all(|c| !c.is_control()) {
                        let _ = app.run("equation.type", json!({"text": t}));
                    } else {
                        // Ignored text is not the replacement a deletion promised.
                        app.session.clear_ime_joins();
                    }
                    continue;
                }
                egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                    app.canvas.ime_preedit.clear();
                    app.canvas.ime_preedit_range = None;
                    if !text.is_empty() {
                        if text.len() > MAX_IME_COMMIT_BYTES || text.chars().count() > wordcraft_engine::MAX_EQUATION_TYPE_CHARS {
                            // Refuse the whole commit (no prefix in either equation mode) and ask
                            // the platform to start the composition afresh. It is not the
                            // replacement for a preceding deletion.
                            log::warn!("ignoring an oversized IME equation commit of {} bytes", text.len());
                            app.session.clear_ime_joins();
                            app.canvas.ime_interrupt_now = true;
                            continue;
                        }
                        let before_stamp = ime_stamp(app);
                        match app.run_ime("equation.type", json!({"text": text})) {
                            Ok(_) => {
                                let outcome = app.session.ime_edit_outcome();
                                let after_stamp = ime_stamp(app);
                                publish_ime_receipt(app, before_stamp, after_stamp, outcome);
                            }
                            Err(_) => {
                                // The equation refused the commit (it may be gone): start the
                                // composition afresh instead of leaving one it believes landed.
                                app.canvas.ime_interrupt_now = true;
                            }
                        }
                    } else {
                        // An empty commit is not the replacement for a preceding deletion.
                        app.session.clear_ime_joins();
                    }
                    continue;
                }
                // A phone keyboard's or macOS replacement inside an equation: erase the counted
                // units on each side with one transactional equation command. The tree cannot
                // provide a surrounding snapshot; an engine revision proves a real commit and the
                // platform is never handed fabricated surroundings.
                egui::Event::Ime(egui::ImeEvent::DeleteSurrounding { before_chars, after_chars }) => {
                    delete_in_equation(app, *before_chars, *after_chars);
                    continue;
                }
                // Byte counts have no reversible mapping into a structured equation, so an
                // exact replacement cannot be performed without guessing at tree units. Refuse
                // the whole request, clear the composition and drop the replacement text that
                // the platform sends for the deletion it believes it made. Inserting it beside
                // the untouched equation would duplicate the old text.
                egui::Event::Ime(egui::ImeEvent::DeleteSurroundingBytes { before_bytes, after_bytes }) => {
                    if *before_bytes != 0 || *after_bytes != 0 {
                        log::debug!("refusing byte delete-surrounding inside an equation");
                        app.canvas.ime_preedit.clear();
                        app.canvas.ime_preedit_range = None;
                        // This refusal is a new request, not the replacement promised by an
                        // earlier equation deletion.
                        app.session.clear_ime_joins();
                        drop_equation_replacement = EquationReplacementDrop::Awaiting;
                        app.canvas.ime_interrupt_now = true;
                    }
                    continue;
                }
                egui::Event::Paste(t) => {
                    if app.run("equation.type", json!({"text": t})).is_err() {
                        app.canvas.ime_interrupt_now = true;
                    }
                    continue;
                }
                egui::Event::Key { key, pressed: true, modifiers, .. } if equation_key(app, *key, *modifiers) => continue,
                // Anything else between the deletion and its replacement (a pre-edit, a key
                // release, focus, a pointer event) means the next equation typing command is a
                // new edit, not the immediate replacement. Drop the promise; a command consumes
                // it in `Session::run` even when it is not a typing command.
                _ => app.session.clear_ime_joins(),
            }
        }
        match e {
            egui::Event::Text(t) => {
                let m = ctx.input(|i| i.modifiers);
                let is_cmd = if cfg!(target_os = "macos") { m.command } else { (m.command || m.ctrl) && !m.alt };
                if is_cmd {
                    continue;
                }
                if t.chars().all(|c| !c.is_control()) && !t.is_empty() {
                    match crate::dialogs_smart_art::type_into(app, &t) {
                        crate::dialogs_smart_art::SmartArtTypeResult::Changed => {
                            note_ime_application_change(app);
                            continue;
                        }
                        crate::dialogs_smart_art::SmartArtTypeResult::ConsumedNoChange => continue,
                        crate::dialogs_smart_art::SmartArtTypeResult::NotHandled => {}
                    }
                    // Typing with a text box selected types into it.
                    crate::objects::enter_text_box(app);
                    type_text(app, &t, false);
                }
            }
            egui::Event::Ime(egui::ImeEvent::DeleteSurrounding { before_chars, after_chars }) => delete_surrounding(app, before_chars, after_chars),
            egui::Event::Ime(egui::ImeEvent::DeleteSurroundingBytes { before_bytes, after_bytes }) => {
                delete_surrounding_bytes(app, before_bytes, after_bytes)
            }
            egui::Event::Ime(egui::ImeEvent::Preedit { text, active_range_chars }) => {
                if text.len() <= MAX_IME_PREEDIT_BYTES {
                    app.canvas.ime_preedit = text;
                    app.canvas.ime_preedit_range = active_range_chars;
                } else {
                    log::warn!("ignoring an IME pre-edit of {} bytes", text.len());
                    app.canvas.ime_preedit.clear();
                    app.canvas.ime_preedit_range = None;
                }
            }
            egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                app.canvas.ime_preedit.clear();
                app.canvas.ime_preedit_range = None;
                if !text.is_empty() {
                    if text.len() <= MAX_IME_COMMIT_BYTES {
                        match crate::dialogs_smart_art::type_into(app, &text) {
                            crate::dialogs_smart_art::SmartArtTypeResult::Changed => {
                                app.session.clear_ime_text_join();
                                app.canvas.ime_join_text = None;
                                note_ime_application_change(app);
                            }
                            crate::dialogs_smart_art::SmartArtTypeResult::ConsumedNoChange => {
                                app.session.clear_ime_text_join();
                                app.canvas.ime_join_text = None;
                            }
                            crate::dialogs_smart_art::SmartArtTypeResult::NotHandled => {
                                type_text(app, &text, true);
                            }
                        }
                    } else {
                        log::warn!("ignoring an IME commit of {} bytes", text.len());
                        app.session.clear_ime_text_join();
                        app.canvas.ime_join_text = None;
                        app.canvas.ime_interrupt_now = true;
                    }
                } else {
                    // An empty commit is not the replacement a preceding deletion promised.
                    app.session.clear_ime_text_join();
                    app.canvas.ime_join_text = None;
                }
            }
            egui::Event::Paste(t) => {
                // Mod+Alt+V arrives as a paste too (the shell reads the clipboard): Paste Special.
                let m = ctx.input(|i| i.modifiers);
                let id = match (m.alt, m.shift) {
                    (true, true) => "edit.pasteText",
                    (true, false) => "edit.pasteSpecial",
                    _ => "edit.paste",
                };
                let _ = app.run(id, json!({"text": t}));
            }
            egui::Event::Copy | egui::Event::Cut => {
                let id = if matches!(e, egui::Event::Cut) { "edit.cut" } else { "edit.copy" };
                // `run` puts the copied text on the system clipboard itself.
                let _ = app.run(id, json!({}));
            }
            egui::Event::Key { key, pressed: true, modifiers, .. } => {
                if key == Key::Escape && app.session.painter.is_some() {
                    app.session.painter = None;
                    continue;
                }
                // The lasso selection and Ink Replay take Delete and Escape first.
                if crate::ink::key(app, key) {
                    continue;
                }
                // Escape puts the pen or eraser down (back to selecting).
                if key == Key::Escape && app.session.view.draw.mode != wordcraft_engine::cmd::draw::DrawMode::Select {
                    app.canvas.ink = None;
                    let _ = app.run("draw.select", json!({}));
                    continue;
                }
                if key == Key::Escape && crate::table_pen::stop(app) {
                    continue;
                }
                // File › Options › Advanced: the Insert key switches overtype only when asked to.
                if key == Key::Insert && modifiers.is_none() {
                    if app.session.prefs.insert_key_overtype {
                        let _ = app.run("text.overtype", json!({}));
                    }
                    continue;
                }
                if crate::objects::key(app, key, modifiers) {
                    continue;
                }
                // Shift+F10, Windows' keyboard way to the context menu (#821).
                if is_context_menu_key(key, modifiers) {
                    app.canvas.context_menu_from_keys = true;
                    continue;
                }
                // Escape in a text box (or WordArt) selects the box itself, so Delete then removes
                // it (#1538). The anchor position is just after the box's object character.
                if key == Key::Escape
                    && crate::canvas::in_text_box(app)
                    && let wordcraft_doc::StoryRef::Part(id) = app.session.sel.focus.story
                    && let Some(end) = app.session.doc.text_box_anchor(id)
                    && let Some(off) = end.off.checked_sub(wordcraft_doc::para::OBJ.len_utf8())
                {
                    let start = wordcraft_doc::Pos { off, ..end.clone() };
                    let _ = app.run("select.range", json!({"anchor": start, "focus": end}));
                    continue;
                }
                if key == Key::Escape && matches!(app.session.sel.focus.story, wordcraft_doc::StoryRef::Part(_)) && app.session.sel.is_collapsed() {
                    let _ = app.run("insert.closeHeader", json!({}));
                    continue;
                }
                if is_shift_delete(key, modifiers) {
                    if !app.session.sel.is_collapsed()
                        && let Ok(r) = app.run("edit.cut", json!({}))
                    {
                        crate::system_clipboard::put_result(app, ctx, &r);
                    }
                    continue;
                }
                if leave_full_screen(app, ctx, key, modifiers, true) {
                    continue;
                }
                app.canvas.swallow_text = dispatch(app, key, modifiers) && modifiers.alt;
            }
            _ => {}
        }
    }
}

/// Escape leaves Focus or Full Screen View (#1678), then Full Screen (#980), once nothing nearer
/// took it: a pen, a text box, a header, a dialog, a menu or a popup, a drag on the page, and in
/// the document (`canvas`) a selection, which Escape collapses first (`select.collapse`), as in a
/// normal window.
fn leave_full_screen(app: &mut WordApp, ctx: &egui::Context, key: Key, m: Modifiers, canvas: bool) -> bool {
    let screen = crate::chrome::Screen::of(&app.session.view).exit_command();
    let Some(id) = screen.or(app.session.view.full_screen.then_some("view.fullScreen")) else { return false };
    if key != Key::Escape || !m.is_none() || !app.escape_free {
        return false;
    }
    // Escape with a button held (dragging a task pane's title bar, say) cancels the drag only.
    // egui has already dropped the drag itself, so the held button is what is left of it.
    if ctx.input(|i| i.pointer.any_down()) {
        return false;
    }
    let c = &app.canvas;
    if c.obj_drag.is_some() || c.table_resize.is_some() || c.table_drag.is_some() || c.link_from.is_some() {
        return false;
    }
    if canvas && !app.session.sel.is_collapsed() {
        return false;
    }
    let _ = app.run(id, json!({"value": false}));
    true
}

/// Alt+numpad character codes are a Windows convention; on macOS Option+digit types its own
/// characters (¡ ™ £…), so they're off there (also in a browser on a Mac).
fn alt_codes_supported(ctx: &egui::Context) -> bool {
    !cfg!(target_os = "macos") && ctx.os() != egui::os::OperatingSystem::Mac
}

/// The digit a key types for an Alt code.
///
/// Word counts only numeric-keypad digits. egui 0.36 reports keypad and top-row digits alike
/// (`Key::Num0`…`Num9`, with the same `physical_key`), and eframe passes no winit key location
/// on, so the top-row digits compose codes too (they type nothing else with Alt alone; Ctrl+Alt+1
/// and keytip digits are untouched).
fn alt_code_digit(key: Key) -> Option<char> {
    let n = match key {
        Key::Num0 => 0,
        Key::Num1 => 1,
        Key::Num2 => 2,
        Key::Num3 => 3,
        Key::Num4 => 4,
        Key::Num5 => 5,
        Key::Num6 => 6,
        Key::Num7 => 7,
        Key::Num8 => 8,
        Key::Num9 => 9,
        _ => return None,
    };
    char::from_digit(n, 10)
}

/// Alt+numpad entry (#961): digits typed with Alt alone held collect; releasing Alt types the
/// character their code names (`text.typeCode`: 0233 → é, 233 → Θ in code page 437). Returns
/// whether the event was part of it (and isn't handled otherwise).
fn alt_code_event(app: &mut WordApp, e: &egui::Event) -> bool {
    let alt_only = |m: &Modifiers| m.alt && !m.ctrl && !m.shift && !m.command && !m.mac_cmd;
    match e {
        egui::Event::Key { key, pressed: true, repeat, modifiers, .. } if alt_only(modifiers) => match alt_code_digit(*key) {
            // A held digit's auto-repeat adds nothing (as on Windows).
            Some(_) if *repeat => {
                app.canvas.swallow_text = true;
                true
            }
            Some(d) => {
                // Codes beyond seven digits name nothing; stop collecting rather than grow.
                if app.canvas.alt_code.len() < 8 {
                    app.canvas.alt_code.push(d);
                }
                // The digit's text (Linux sends it even with Alt held) isn't typed.
                app.canvas.swallow_text = true;
                true
            }
            None => {
                app.canvas.alt_code.clear();
                false
            }
        },
        egui::Event::Key { key: Key::AltLeft | Key::AltRight, pressed: false, .. } => {
            finish_alt_code(app);
            false
        }
        egui::Event::ModifiersChanged(m) if !m.alt => {
            finish_alt_code(app);
            false
        }
        egui::Event::Key { pressed: true, .. } => {
            // Another key with Ctrl/Shift held (or without Alt) ends the entry unfinished.
            app.canvas.alt_code.clear();
            false
        }
        _ => false,
    }
}

/// What an Alt code typed, for the platform's own composed character that may follow.
#[derive(Clone, Debug)]
pub(crate) struct AltCodeEcho {
    /// The character typed.
    typed: String,
    /// The system composes Alt codes itself (Windows): whatever it sends is the code's character.
    system_composes: bool,
    /// The code named a byte of the system's code pages (1–255), so the system's character wins
    /// over ours: it knows the code pages (DBCS ones too) better than our locale guess.
    byte_code: bool,
}

/// Alt released: type the collected code's character, if any.
fn finish_alt_code(app: &mut WordApp) {
    let code = std::mem::take(&mut app.canvas.alt_code);
    if code.is_empty() {
        return;
    }
    crate::objects::enter_text_box(app);
    let system_composes = cfg!(target_os = "windows");
    let mut params = json!({"code": code});
    if system_composes {
        // Windows: the code pages of the system locale (Polish 1250/852, Russian 1251/866…).
        let (ansi, oem) = wordcraft_engine::cmd::text::system_code_pages();
        params = json!({"code": code, "ansi": ansi, "oem": oem});
        // The system's composed character comes as text after the digits' swallowed text.
        app.canvas.swallow_text = false;
    }
    if let Ok(r) = app.run("text.typeCode", params) {
        let byte_code = code.trim_start_matches('0').parse::<u32>().is_ok_and(|v| v <= 255);
        app.canvas.alt_code_echo = r.get("char").and_then(Value::as_str).map(|c| AltCodeEcho { typed: c.to_string(), system_composes, byte_code });
    }
}

/// Typed text (a key's text or an input method's commit). Text replacing what an input method's
/// `DeleteSurrounding` just erased takes the erased text's formatting, never overtypes what
/// follows, and stays in the typing undo group (#1480).
///
/// `ime` says this text came from an accepted IME commit. Only then is a receipt taken, and only
/// around the insertion itself: a transformation the engine performs (AutoCorrect, AutoFormat)
/// makes the frame [`egui::ImeChangeCause::Other`] instead.
fn type_text(app: &mut WordApp, text: &str, ime: bool) {
    let before = ime.then(|| ime_stamp(app));
    // Where the text lands, so a quote can be checked against what the engine actually inserted.
    let start = app.session.sel.ordered().0;
    let pending_before = app.session.pending.clone();
    let typed = if let Some(props) = app.canvas.ime_join_text.take() {
        // The replacement takes the erased text's formatting. `Session::run` does not restore
        // `pending` on failure (it is not part of the document snapshot), so keep it here: a
        // refused replacement must not leave its formatting waiting for the next ordinary key.
        // The undo join itself is the engine's scoped body-replacement promise, consumed by
        // `Session::run` only for this `text.insert` at the promised selection.
        app.session.pending = Some(props);
        let overtype = std::mem::replace(&mut app.session.prefs.overtype, false);
        let typed =
            if ime { app.run_ime("text.insert", json!({"text": text})).is_ok() } else { app.run("text.insert", json!({"text": text})).is_ok() };
        app.session.prefs.overtype = overtype;
        if typed {
            app.session.continue_typing();
        } else {
            app.session.pending = pending_before;
            // The platform already applied its own deletion to the text it believed was there.
            // Its replacement was refused, so the surrounding snapshot is stale and the
            // deletion's join must not reach a later composition.
            app.session.clear_ime_text_join();
            app.canvas.ime_interrupt_now = true;
        }
        typed
    } else if ime {
        app.run_ime("text.insert", json!({"text": text})).is_ok()
    } else {
        app.run("text.insert", json!({"text": text})).is_ok()
    };
    if !typed {
        return;
    }
    let mut outcome = app.session.ime_edit_outcome();
    // The engine's own outcome is the per-command provenance: `text.insert` compares the paragraph
    // with the one it typed and marks AutoCorrect/list/dash rewrites `transformed`. A smart quote
    // is also checked against the actual inserted text, so smart quotes off stays the IME's own
    // edit. Never infer this from an undo label, which may belong to an earlier step. A refused
    // insert has no provenance at all: it must not fabricate an AutoFormat change or receipt.
    let quote = text == "\"" || text == "'";
    outcome.transformed |= quote && !inserted_literal_at(app, text, &start);
    note_automatic_change(app, outcome);
    if let Some(before) = before {
        let after = ime_stamp(app);
        publish_ime_receipt(app, before, after, outcome);
    }
}

/// Whether the text at `start` is exactly `text` (the engine inserted it unchanged).
///
/// A smart quote, an AutoCorrect replacement or a relocated insert makes this false: the edit is
/// AutoFormat work, not the input method's own change.
fn inserted_literal_at(app: &WordApp, text: &str, start: &wordcraft_doc::Pos) -> bool {
    let Some(para) = app.session.doc.para_at(start) else { return false };
    let Some(end) = start.off.checked_add(text.len()) else { return false };
    para.text.get(start.off..end) == Some(text)
}

/// The bounded visible-logical window around the body selection, cached by `(revision, cursor,
/// anchor)` so an idle pass does not walk a long paragraph again. Every edit bumps `rev`.
fn ime_window(app: &mut WordApp) -> Option<wordcraft_doc::ime_text::ImeWindow> {
    let rev = app.session.rev();
    let cursor = app.session.sel.focus.clone();
    let anchor = app.session.sel.anchor.clone();
    if let Some((r, c, a, w)) = &app.canvas.ime_window_cache
        && *r == rev
        && *c == cursor
        && *a == anchor
    {
        return w.clone();
    }
    let window = app.session.doc.ime_surrounding(&cursor, &anchor, wordcraft_doc::ime_text::MAX_IME_TEXT_BYTES);
    app.canvas.ime_window_cache = Some((rev, cursor, anchor, window.clone()));
    window
}

/// Observe the settled editor at the end of a UI pass and publish its IME descriptor.
///
/// The tracker is scoped by native viewport. A closed viewport releases its state; a pass without
/// a visible focused editor reports an unfocused observation so a later focus starts afresh.
pub(crate) fn observe_ime(app: &mut WordApp, ctx: &egui::Context, geometry: Option<(egui::Rect, egui::Rect)>, interrupt: bool) {
    let viewport = ctx.viewport_id();
    // Drain an application-origin change before the final observation. The signal survives later
    // commands until here (or until a receipt publishes), so an edit after the receipt still
    // makes the frame `Other`.
    if app.session.take_pending_ime_application_change() {
        note_ime_application_change(app);
    }
    if app.canvas.ime_viewport != Some(viewport) {
        if let Some(old) = app.canvas.ime_viewport {
            app.canvas.ime_tracker.end_viewport(old);
        }
        app.canvas.ime_viewport = Some(viewport);
    }
    let frame = ctx.cumulative_frame_nr_for(viewport);
    let pass = ctx.cumulative_pass_nr_for(viewport);
    let focused = app.canvas.focused && geometry.is_some();
    let update = if focused {
        let target = ime_target(app);
        if app.session.math.is_some() {
            // An equation's structured tree has no reversible text mapping yet: report an
            // explicit Unavailable, never fabricated surroundings. Its stable session token is
            // the replacement epoch, so caret motion keeps one context. The engine revision lets
            // an unavailable receipt prove a committed edit.
            let mut observation = egui::ImeObservation::new(frame, pass, target);
            if let Some(rev) = app.session.ime_state_revision() {
                observation = observation.unavailable_revision(rev);
            }
            app.canvas.ime_tracker.observe(viewport, Some(observation))
        } else {
            let window = ime_window(app);
            if let Some(w) = &window {
                let committed = Some(egui::CommittedText::new(&w.text, w.cursor, w.anchor));
                app.canvas.ime_tracker.observe(viewport, Some(egui::ImeObservation::new(frame, pass, target).committed(committed)))
            } else {
                // A wide selection or cross-story caret has no single surrounding window. Its
                // engine revision orders unavailable observations; no text is fabricated.
                let mut observation = egui::ImeObservation::new(frame, pass, target);
                if let Some(rev) = app.session.ime_state_revision() {
                    observation = observation.unavailable_revision(rev);
                }
                app.canvas.ime_tracker.observe(viewport, Some(observation))
            }
        }
    } else {
        // No editor has the keyboard: a pre-edit painted from the old focus must not linger.
        app.canvas.ime_preedit.clear();
        app.canvas.ime_preedit_range = None;
        app.canvas.ime_tracker.observe(viewport, None)
    };
    if let (Some(update), Some((rect, cursor_rect))) = (update, geometry) {
        ctx.output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                owner: update.owner,
                context_generation: update.context_generation,
                purpose: update.purpose,
                rect,
                cursor_rect,
                should_interrupt_composition: interrupt,
                reset_request: None,
                change_cause: update.change_cause,
                surrounding_text: update.surrounding_text,
            });
        });
    }
}

/// The immutable IME owner for the editor that currently has the keyboard.
///
/// The document identity and story are stable across edits; an equation adds its editing-session
/// token as the replacement epoch because its document position moves with the text.
fn ime_target(app: &WordApp) -> egui::ImeTarget {
    let document = app.session.document_id();
    if let Some(math) = &app.session.math {
        egui::ImeTarget::new(egui::Id::unique(("wordcraft-ime-equation", document)), math.session)
    } else {
        // A body replaced under the same document id (setText, version restore, compare,
        // Undo/Redo) advances the epoch, so the platform clears the old composition.
        egui::ImeTarget::new(egui::Id::unique(("wordcraft-ime-body", document, app.session.sel.focus.story)), app.session.ime_body_epoch())
    }
}

/// The bounded committed snapshot used by IME receipts.
///
/// An equation has no reversible tree-to-text mapping, and a selection across blocks has no
/// single visible paragraph, so both are honestly unavailable rather than fabricated.
fn ime_stamp(app: &mut WordApp) -> egui::ImeMutationStamp {
    let unavailable = |app: &mut WordApp| match app.session.ime_state_revision() {
        Some(rev) => egui::ImeMutationStamp::unavailable_at_revision(rev),
        None => egui::ImeMutationStamp::unavailable(),
    };
    if app.session.math.is_some() {
        return unavailable(app);
    }
    match ime_window(app) {
        Some(w) => egui::ImeMutationStamp::from_committed(&w.text, w.cursor, w.anchor, None),
        None => unavailable(app),
    }
}

/// Record a successful accepted IME commit/delete for the current frame and viewport.
fn note_ime_receipt(app: &mut WordApp, before: egui::ImeMutationStamp, after: egui::ImeMutationStamp) {
    let Some(ctx) = app.ctx.clone() else { return };
    let viewport = ctx.viewport_id();
    let frame = ctx.cumulative_frame_nr_for(viewport);
    let target = ime_target(app);
    app.canvas.ime_tracker.note_input_method_apply(viewport, frame, target, before, after);
}

/// Record an application-origin edit in the current frame; `Other` dominates the frame.
fn note_ime_application_change(app: &mut WordApp) {
    let Some(ctx) = app.ctx.clone() else { return };
    let viewport = ctx.viewport_id();
    let frame = ctx.cumulative_frame_nr_for(viewport);
    let target = ime_target(app);
    app.canvas.ime_tracker.note_application_change(viewport, frame, target);
}

/// Whether the stamp pair proves the transition an IME receipt needs.
/// The receipt decision, kept pure so tests can exercise every revision/availability case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImeProof {
    /// Classified no-op with an unchanged stamp: leave an earlier receipt as the frame's cause.
    NoMutation,
    /// A committed change the stamps cannot prove (or an automatic rewrite): `Other`.
    UnprovenMutation,
    /// A classified content change whose exact stamp transition is proven.
    Certified,
}

/// Wraps egui's unavailable-revision accessor for an IME mutation stamp.
fn unavailable_revision(stamp: &egui::ImeMutationStamp) -> Option<u64> {
    stamp.unavailable_revision()
}

/// Whether a pair of IME mutation stamps proves a transition: changed surrounding text, an
/// increase between unavailable revisions, or a mixed transition whose unavailable endpoint
/// carries a revision.
fn stamp_transition_proven(before: &egui::ImeMutationStamp, after: &egui::ImeMutationStamp) -> bool {
    use egui::output::IMESurroundingText;
    match (before.surrounding_text(), after.surrounding_text()) {
        (IMESurroundingText::Available(a), IMESurroundingText::Available(b)) => a != b,
        (IMESurroundingText::Unavailable, IMESurroundingText::Unavailable) => {
            matches!((unavailable_revision(before), unavailable_revision(after)), (Some(a), Some(b)) if a < b)
        }
        // A mixed transition has one unavailable endpoint; only that endpoint carries a revision.
        (IMESurroundingText::Unavailable, IMESurroundingText::Available(_)) | (IMESurroundingText::Available(_), IMESurroundingText::Unavailable) => {
            let unavailable = if matches!(before.surrounding_text(), IMESurroundingText::Unavailable) { before } else { after };
            unavailable_revision(unavailable).is_some()
        }
    }
}

/// Classifies an IME outcome and stamp pair as no mutation, unproven mutation or certified. Only a
/// classified, untransformed content change with a proven stamp transition is certified.
fn classify_ime_proof(outcome: ImeEditOutcome, before: &egui::ImeMutationStamp, after: &egui::ImeMutationStamp) -> ImeProof {
    if !outcome.classified {
        return if outcome.state_changed { ImeProof::UnprovenMutation } else { ImeProof::NoMutation };
    }
    if outcome.transformed {
        return ImeProof::UnprovenMutation;
    }
    if outcome.state_changed && !outcome.content_changed {
        return ImeProof::UnprovenMutation;
    }
    if outcome.content_changed {
        return if stamp_transition_proven(before, after) { ImeProof::Certified } else { ImeProof::UnprovenMutation };
    }
    if before == after { ImeProof::NoMutation } else { ImeProof::UnprovenMutation }
}

/// Publish at most one receipt for a successful IME command, or force `Other` when the engine
/// outcome and the bounded stamps cannot prove a pure content change.
fn publish_ime_receipt(app: &mut WordApp, before: egui::ImeMutationStamp, after: egui::ImeMutationStamp, outcome: ImeEditOutcome) {
    if app.ctx.is_none() {
        return;
    }
    // A successful application-origin command since the last observation poisons the frame, even
    // when the available text stamp is unchanged (formatting-only changes).
    if app.session.take_pending_ime_application_change() {
        note_ime_application_change(app);
        return;
    }
    match classify_ime_proof(outcome, &before, &after) {
        ImeProof::NoMutation => {}
        ImeProof::UnprovenMutation => note_ime_application_change(app),
        ImeProof::Certified => note_ime_receipt(app, before, after),
    }
}

/// AutoCorrect or AutoFormat just changed the text around the caret. The web's text agent still
/// holds the text as typed and counts its `DeleteSurrounding` from that, so it is asked to start
/// afresh (`canvas::ime_reset`). The caller passes the engine's per-command outcome; an undo label
/// from an earlier step cannot make an unrelated literal commit look transformed.
fn note_automatic_change(app: &mut WordApp, outcome: ImeEditOutcome) {
    if outcome.transformed {
        app.canvas.ime_reset = true;
    }
}

/// An input method's `DeleteSurrounding`: erase `before` characters (Unicode scalar values, as
/// the input method counts them) before the caret and `after` after it, as Backspace does over a
/// selection. The web build's text agent (eframe's hidden `<input>`) sends it, then the new
/// text, whenever its value changes other than by an addition at its end (#1480): Backspace
/// and corrections from phone keyboards, and Chrome's caret jump. The desktop builds never get
/// it (egui-winit 0.36 doesn't send it).
///
/// The counts are over the visible logical text: tracked deletions are hidden and inline objects
/// contribute their plain-text result. They exclude the selection, which is preserved, and any
/// pre-edit (which never lives in the model here). Erasure joins an open typing group and the
/// text that follows joins it too ([`type_text`]), so one Undo takes back the typing with its
/// correction. Neither path runs inside an equation, whose text is not the document's.
fn delete_surrounding(app: &mut WordApp, before: usize, after: usize) {
    delete_surrounding_common(app, before, after, wordcraft_doc::ime_text::DeleteUnit::Chars);
}

/// An input method's byte-counted `DeleteSurroundingBytes` (macOS and Wayland): erase the UTF-8
/// range it names, then the following `Commit` replaces it. Both counts round inward at a
/// character boundary (a start forward, an end backward), so a count that ends inside a
/// character leaves that character whole. The selection and paragraph marks stay outside the
/// range.
fn delete_surrounding_bytes(app: &mut WordApp, before: usize, after: usize) {
    delete_surrounding_common(app, before, after, wordcraft_doc::ime_text::DeleteUnit::Bytes);
}

/// Refuse a body surrounding request and discard the composition it invalidates.
///
/// The platform has already changed the text it believed was around the caret, so its snapshot is
/// stale: interrupt it, unpaint the local pre-edit, and drop every replacement promise (body and
/// equation) that could otherwise join a later edit into an earlier deletion.
fn refuse_ime_request(app: &mut WordApp) {
    app.canvas.ime_preedit.clear();
    app.canvas.ime_preedit_range = None;
    app.canvas.ime_join_text = None;
    app.session.clear_ime_joins();
    app.canvas.ime_interrupt_now = true;
}

/// A signed stored-length change: negative when the paragraph shrank, positive only when a
/// restored content-control placeholder grew in place of the erased text.
fn signed_len_delta(before: usize, after: usize) -> Option<i64> {
    let after = i64::try_from(after).ok()?;
    let before = i64::try_from(before).ok()?;
    after.checked_sub(before)
}

/// Move a stored UTF-8 offset by a signed paragraph-length delta.
fn shift_offset(off: usize, shift: i64) -> Option<usize> {
    let off = i64::try_from(off).ok()?;
    usize::try_from(off.checked_add(shift)?).ok()
}

/// The end of a placeholder `restore_placeholder` just inserted at or before `deleted_start`.
///
/// `restore_placeholder` sets the caret to the control's content start before inserting. Finding
/// that exact state proves the growth came from a placeholder insertion at a position before the
/// erased range, so the stored suffix really moved by the measured signed delta.
fn restored_placeholder_end(s: &wordcraft_engine::Session, deleted_start: usize) -> Option<usize> {
    let at = &s.sel.focus;
    let r = s.doc.control_at(at)?;
    if !r.control.showing_placeholder || r.content_start() != *at || r.start.path != r.end.path {
        return None;
    }
    let start = r.content_start().off;
    if start > deleted_start {
        return None;
    }
    let placeholder = r.control.placeholder_shown();
    let end = start.checked_add(placeholder.len())?;
    // The caret alone is not proof: a control that was already showing its placeholder and had
    // that text deleted also leaves the caret at the content start. Require the restored
    // placeholder text itself to occupy the content from the start.
    let para = s.doc.para_at(at)?;
    (!placeholder.is_empty() && para.text.get(start..end) == Some(placeholder)).then_some(end)
}

/// The exact state a refused IME delete-surrounding transaction restores.
struct ImeRollback {
    snapshot: Option<wordcraft_engine::EditSnapshot>,
    original: wordcraft_engine::Selection,
    pending: Option<wordcraft_doc::props::CharProps>,
    goal_x: Option<f32>,
}

impl ImeRollback {
    fn apply(&mut self, app: &mut WordApp) {
        let Some(snapshot) = self.snapshot.take() else {
            return;
        };
        app.session.restore(snapshot);
        app.session.sel = self.original.clone();
        app.session.pending = self.pending.clone();
        app.session.goal_x = self.goal_x;
        refuse_ime_request(app);
    }
}

/// Whether the edit is a classified content and state change that is not a transform.
fn ime_edit_satisfied(app: &mut WordApp) -> bool {
    let outcome = app.session.ime_edit_outcome();
    outcome.classified && outcome.content_changed && outcome.state_changed && !outcome.transformed
}

/// Delete one IME-surrounding range. Returns the paragraph's signed length change and the end of
/// a placeholder the deletion restored, or `None` after rolling back the whole transaction.
fn ime_delete_side(
    app: &mut WordApp,
    rollback: &mut ImeRollback,
    cursor: &wordcraft_doc::Pos,
    start: usize,
    end: usize,
    join_undo: bool,
) -> Option<(i64, Option<usize>)> {
    if start >= end {
        rollback.apply(app);
        return None;
    }
    let Some(len_before) = app.session.doc.para_at(cursor).map(|p| p.len()) else {
        rollback.apply(app);
        return None;
    };
    // A control already showing its placeholder cannot have received a new one from this
    // deletion; its unchanged text must not be mistaken for a restored placeholder.
    let showing_before =
        app.session.doc.control_at(&wordcraft_doc::Pos { off: end, ..cursor.clone() }).is_some_and(|r| r.control.showing_placeholder);
    app.session.sel = wordcraft_engine::Selection {
        anchor: wordcraft_doc::Pos { off: start, ..cursor.clone() },
        focus: wordcraft_doc::Pos { off: end, ..cursor.clone() },
    };
    if join_undo {
        app.session.join_next_undo();
    }
    if app.run_ime("text.backspace", json!({})).is_err() || !ime_edit_satisfied(app) {
        rollback.apply(app);
        return None;
    }
    // A tracked delete of text this author inserted removes it from the stored paragraph;
    // other tracked deletions only mark it. A content control emptied by the deletion gains
    // its placeholder again, which can make the paragraph longer. Measure the signed change
    // and require proof for growth: only a placeholder restored at or before the erased
    // range can move the suffix right.
    let Some(len_after) = app.session.doc.para_at(cursor).map(|p| p.len()) else {
        rollback.apply(app);
        return None;
    };
    let Some(delta) = signed_len_delta(len_before, len_after) else {
        rollback.apply(app);
        return None;
    };
    let restored = restored_placeholder_end(&app.session, start).filter(|_| !showing_before);
    if delta > 0 && restored.is_none() {
        // Growth that cannot be attributed to a placeholder insertion at/before the erased
        // range has no provable suffix mapping: refuse the whole transaction.
        rollback.apply(app);
        return None;
    }
    // Keep the restored placeholder's end: for a caret it is the provable insertion point,
    // also when a tracked deletion keeps the erased bytes in the stored paragraph.
    Some((delta, restored))
}

/// The final caret/selection offsets after one or two surrounding deletions. A restored
/// placeholder end is the only provable position when one side inserted it; otherwise both
/// stored offsets move by the paragraph's signed length change.
fn ime_surrounding_offsets(
    del_anchor: usize,
    del_focus: usize,
    shift: i64,
    first_placeholder_end: Option<usize>,
    second_placeholder_end: Option<usize>,
) -> Option<(usize, usize)> {
    if let Some(end) = second_placeholder_end {
        // A selection spanning the emptied control cannot be remapped through a placeholder
        // insertion without guessing; only a caret has a provable position.
        return (del_anchor == del_focus).then_some((end, end));
    }
    if let (Some(end), true) = (first_placeholder_end, del_anchor == del_focus) {
        // The first side restored the placeholder; its end is the caret's provable position even
        // while an after-side deletion is applied beyond the control.
        return Some((end, end));
    }
    Some((shift_offset(del_anchor, shift)?, shift_offset(del_focus, shift)?))
}

/// Both delete-surrounding paths: one visible-logical mapping for the query and the deletion.
fn delete_surrounding_common(app: &mut WordApp, before: usize, after: usize, unit: wordcraft_doc::ime_text::DeleteUnit) {
    if before == 0 && after == 0 {
        return;
    }
    // A new surrounding request supersedes any replacement promised by an earlier one.
    app.session.clear_ime_joins();
    if app.session.math.is_some() {
        refuse_ime_request(app);
        return;
    }
    let cursor = app.session.sel.focus.clone();
    let anchor = app.session.sel.anchor.clone();
    if cursor.story != anchor.story || cursor.path != anchor.path {
        refuse_ime_request(app);
        return;
    }
    let Some(del) = app.session.doc.ime_delete_surrounding(&cursor, &anchor, before, after, unit) else {
        refuse_ime_request(app);
        return;
    };
    if del.before.is_none() && del.after.is_none() {
        refuse_ime_request(app);
        return;
    }
    let join = app.session.typing_open();
    let props = app.session.typing_props();
    let original = app.session.sel.clone();
    let pending_before = app.session.pending.clone();
    let goal_x_before = app.session.goal_x;
    let before_stamp = ime_stamp(app);
    // Both sides are one IME request: if the second half is refused (Restrict Editing, a
    // content control, an already-deleted tracked range, a classified no-op…), the first half
    // must go back too. The document, selection, undo stack, typing group, dirty flag, pending
    // formatting, `goal_x` and every pending join are restored or cleared. Deliberately not
    // restored: `last_command`, macro recording, Go Back marks, `also_selected`, `typing`/
    // `autocap` and canvas preedit (a refusal clears it).
    let snapshot = app.session.edit_snapshot();
    let mut rollback = ImeRollback { snapshot: Some(snapshot), original, pending: pending_before, goal_x: goal_x_before };
    // Stored offsets of the suffix (and of the original selection) move by the paragraph's
    // signed length change. A placeholder can make that positive; subtracting a removed-byte
    // count cannot describe that, and would leave the second range and the caret inside the
    // placeholder.
    let mut shift = 0i64;
    let mut deleted = false;
    let mut first_placeholder_end = None;
    if let Some(range) = del.before.clone() {
        let Some((delta, restored)) = ime_delete_side(app, &mut rollback, &cursor, range.start, range.end, join) else {
            return;
        };
        deleted = true;
        first_placeholder_end = restored;
        shift = delta;
    }
    let mut second_placeholder_end = None;
    if let Some(range) = del.after {
        let Some(start) = shift_offset(range.start, shift) else {
            rollback.apply(app);
            return;
        };
        let Some(end) = shift_offset(range.end, shift) else {
            rollback.apply(app);
            return;
        };
        let in_bounds = app
            .session
            .doc
            .para_at(&cursor)
            .is_some_and(|p| start < end && end <= p.len() && p.text.is_char_boundary(start) && p.text.is_char_boundary(end));
        if !in_bounds {
            rollback.apply(app);
            return;
        }
        // The mapped end is in the paragraph as the first side left it; a control already
        // showing its placeholder there is not one this side restores.
        let Some((_, restored)) = ime_delete_side(app, &mut rollback, &cursor, start, end, deleted || join) else {
            return;
        };
        deleted = true;
        second_placeholder_end = restored;
    }
    let Some((anchor_off, focus_off)) = ime_surrounding_offsets(del.anchor, del.focus, shift, first_placeholder_end, second_placeholder_end) else {
        rollback.apply(app);
        return;
    };
    let valid =
        app.session.doc.para_at(&cursor).is_some_and(|p| {
            anchor_off <= p.len() && focus_off <= p.len() && p.text.is_char_boundary(anchor_off) && p.text.is_char_boundary(focus_off)
        });
    if !valid {
        rollback.apply(app);
        return;
    }
    app.session.sel = wordcraft_engine::Selection {
        anchor: wordcraft_doc::Pos { off: anchor_off, ..cursor.clone() },
        focus: wordcraft_doc::Pos { off: focus_off, ..cursor },
    };
    if deleted {
        // Only the next `text.insert` at this exact selection may join the deletion; any
        // intervening ordinary command consumes and discards the promise and owns its own Undo
        // checkpoint.
        app.session.join_next_ime_text_replacement();
        app.canvas.ime_join_text = Some(props);
        let after_stamp = ime_stamp(app);
        let outcome = ImeEditOutcome { classified: true, content_changed: true, state_changed: true, transformed: false, selection_changed: true };
        publish_ime_receipt(app, before_stamp, after_stamp, outcome);
    }
}

/// An input method's `DeleteSurrounding` inside an equation: one bounded transactional command.
///
/// The structured tree has no surrounding snapshot; the engine revision on the unavailable stamp
/// proves the commit. Any refusal leaves the document untouched and interrupts the composition.
/// `Session::run` has already consumed the pending undo joins before dispatch and may have
/// recorded the attempted no-op command in the macro/Repeat; those generic side effects are not
/// rolled back.
fn delete_in_equation(app: &mut WordApp, before: usize, after: usize) {
    if app.session.math.is_none() {
        return;
    }
    // A new surrounding request supersedes any earlier equation deletion's promised join,
    // whether or not this one can run. Clear the painted composition before every refusal,
    // too: it belongs to a snapshot the interrupt is about to discard.
    app.session.clear_ime_joins();
    app.canvas.ime_preedit.clear();
    app.canvas.ime_preedit_range = None;
    if before > wordcraft_engine::MAX_EQUATION_DELETE_UNITS || after > wordcraft_engine::MAX_EQUATION_DELETE_UNITS {
        app.canvas.ime_interrupt_now = true;
        return;
    }
    if before == 0 && after == 0 {
        return;
    }
    if app.session.typing_open() {
        app.session.join_next_undo();
    }
    let before_stamp = ime_stamp(app);
    match app.run_ime("equation.deleteSurrounding", json!({"before": before, "after": after})) {
        Ok(_) => {
            // The replacement the platform commits next belongs to the same edit as the
            // deletion, exactly like the body path's `ime_join_text`. This is an equation-only
            // promise: an unrelated command consumes it without joining, so it cannot merge
            // into this deletion's undo step.
            app.session.join_next_equation_edit();
            let outcome = app.session.ime_edit_outcome();
            let after_stamp = ime_stamp(app);
            publish_ime_receipt(app, before_stamp, after_stamp, outcome);
        }
        Err(_) => {
            app.canvas.ime_interrupt_now = true;
        }
    }
}
/// Text arriving right after an Alt code typed `echo` (no key pressed between): whether it was
/// the platform's composed character and is handled. The same character isn't typed twice. On
/// Windows the text is always the composed character: for a code up to 255 it replaces ours (the
/// system knows its code pages), above 255 ours stays (Windows reduces it to a byte; Word types
/// the Unicode character).
fn alt_code_echoed(app: &mut WordApp, echo: &AltCodeEcho, text: &str) -> bool {
    if echo.typed == text {
        return true;
    }
    if !echo.system_composes {
        return false;
    }
    if echo.byte_code && text.chars().count() == 1 {
        let s = &app.session;
        let f = s.sel.focus.clone();
        let start = f.off.checked_sub(echo.typed.len());
        let ours =
            start.filter(|_| s.sel.is_collapsed()).filter(|st| s.doc.para_at(&f).and_then(|p| p.text.get(*st..f.off)) == Some(echo.typed.as_str()));
        if let Some(start) = ours {
            app.session.sel = wordcraft_engine::Selection { anchor: wordcraft_doc::Pos { off: start, ..f.clone() }, focus: f };
            let _ = app.run("text.insert", json!({"text": text}));
        }
    }
    true
}

/// Whether a key runs its shortcut with the canvas unfocused (after a ribbon click, say): Mod and
/// Alt shortcuts (Alt+Q reaches Search from anywhere, as in Word, #819) and the function keys.
fn is_global_shortcut(key: Key, m: Modifiers) -> bool {
    m.command || m.alt || is_function_key(key)
}

/// Command shortcuts when the canvas isn't focused and no text field is: nothing has the
/// keyboard, or a button or a pane's title bar does. A focused widget in an open dialog or popup
/// keeps the keys, so Ctrl+Z on a dialog button can't undo the document behind it.
pub fn global_shortcuts(app: &mut WordApp, ctx: &egui::Context) {
    // Customize Keyboard records the keys pressed; they never run commands meanwhile.
    if app.canvas.focused
        || ctx.text_edit_focused()
        || (ctx.egui_wants_keyboard_input() && crate::panes::overlay_open(app, ctx))
        || matches!(app.dialog, Some(crate::dialogs::Dialog::CustomizeKeyboard { .. }))
    {
        return;
    }
    let events = ctx.input(|i| i.events.clone());
    let window = app.windows.shown();
    for e in events {
        // The rest of the keys were pressed in a window that closed or went to the back.
        if app.windows.shown() != window {
            break;
        }
        if let egui::Event::Key { key, pressed: true, modifiers, .. } = e {
            if leave_full_screen(app, ctx, key, modifiers, false) {
                continue;
            }
            if is_global_shortcut(key, modifiers) {
                dispatch(app, key, modifiers);
            }
        }
    }
}

#[cfg(test)]
mod tests;
