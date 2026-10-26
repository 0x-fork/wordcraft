//! WordCraft's egui front end: a Word-style window (title bar with Quick Access Toolbar, ribbon,
//! rulers, page canvas, panes, status bar, Backstage, dialogs) over `wordcraft-engine`.
//!
//! The UI is thin: it reads `Session` state and acts through [`WordApp::run`] (command ids), so
//! the menus, ribbon, shortcuts, control channel and MCP all reach the same behaviour.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// What the operating system says about light and dark. macOS and Windows answer through winit;
/// winit reports nothing on Linux, where the desktop portal does ([`desktop_theme`]). `None` means
/// nobody answered, not a preference.
fn system_theme(ctx: &egui::Context, desktop_dark: Option<bool>) -> Option<egui::Theme> {
    ctx.system_theme().or_else(|| desktop_dark.map(|dark| if dark { egui::Theme::Dark } else { egui::Theme::Light }))
}

/// An English UI string in the current interface language ([`i18n::t`]).
#[macro_export]
macro_rules! tl {
    ($s:expr) => {
        $crate::i18n::t($s)
    };
}

pub mod a11y;
pub mod a11y_pane;
pub mod async_saves;
pub mod autoscroll;
pub mod backstage;
pub mod canvas;
pub mod caret_glide;
pub mod chrome;
pub mod colors;
pub mod content_controls;
pub mod control;
pub mod credits;
pub mod crop;
pub mod desktop_theme;
pub mod dialogs;
pub mod dialogs_controls;
pub mod dialogs_insert;
pub mod dialogs_keyboard;
pub mod dialogs_layout;
pub mod dialogs_lists;
pub mod dialogs_notes;
pub mod dialogs_para;
pub mod dialogs_refs;
pub mod dialogs_review;
pub mod dialogs_ribbon;
pub mod dialogs_smart_art;
pub mod dialogs_table_style;
pub mod dialogs_templates;
pub mod documents;
pub mod equation_tab;
/// The built-in file browser, for desktops without a system file dialog (#576).
#[cfg(not(target_arch = "wasm32"))]
pub mod file_browser;
pub mod file_dialogs;
pub mod format_shape;
pub mod frame;
pub mod guides;
pub mod i18n;
pub mod ibeam;
pub mod icons;
pub mod ink;
pub mod keys;
pub mod keytips;
mod local_time;
pub mod mini_toolbar;
pub mod objects;
pub mod options;
pub mod panes;
pub mod paste_picture;
pub mod previews;
pub mod print_page;
pub mod printing;
pub mod read_aloud;
pub mod ribbon;
pub mod ribbon_fit;
pub mod ruler_tabs;
pub mod scroll;
pub mod system_clipboard;
pub mod table_mouse;
pub mod table_pen;
pub mod table_resize;
pub mod table_ruler;
pub mod theme;
pub mod updates_ui;
pub mod widgets;
pub mod window_close;
pub mod window_geometry;
mod window_ui;
mod windows;
pub mod wordart_menus;
pub mod zotero;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wordcraft_engine::Session;

pub use control::ControlRequest;
pub use dialogs::dialogs_chart;
pub use file_dialogs::{FileDialogAnswer, FileDialogRequest, open_filters};

/// Platform services injected by the host (file dialogs, file I/O).
#[derive(Default)]
pub struct Services {
    /// Desktop: discover configured printers on a worker.
    pub printers: Option<printing::Discover>,
    /// Desktop: submit the PDF with the chosen settings on a worker.
    pub print_native: Option<printing::Submit>,
    /// Pick a file to open; the argument says what for (`document`, `picture`).
    pub pick_open: Option<Box<dyn Fn(&str) -> Option<String>>>,
    /// Pick a path to save to, given a suggested name.
    pub pick_save: Option<Box<dyn Fn(&str) -> Option<String>>>,
    /// Desktop: show a native file dialog without blocking the UI thread (#94); used instead of
    /// `pick_open` / `pick_save` when set. The host sends the picked path (`Ok(None)` when
    /// cancelled) through the returned channel once the user answers, and wakes the UI; a dropped
    /// sender counts as cancelled. An `Err` says no system dialog could be shown, and the app shows
    /// its built-in file browser instead (#576). The app shows one dialog at a time ([`file_dialogs`]).
    pub file_dialog: Option<Box<dyn Fn(FileDialogRequest) -> std::sync::mpsc::Receiver<FileDialogAnswer>>>,
    /// Web: open a file picker; the file arrives later through `inbox`.
    pub open_async: Option<Box<dyn Fn(&str)>>,
    /// Web: files (name, bytes) delivered asynchronously (picker, drag and drop).
    pub inbox: Option<Inbox>,
    /// Web: pictures pasted from the browser's clipboard (the bytes; the name is ignored). Kept
    /// apart from `inbox` because a paste only lands in the document when the page is the edit
    /// target (#1349), where a dropped or opened file always lands.
    pub pasted: Option<Inbox>,
    /// Web: hand bytes to the browser as a download. An error means no download started (the
    /// browser can't tell the page whether the user then kept the file).
    pub download: Option<Box<dyn Fn(&str, &[u8]) -> Result<(), String>>>,
    /// Web: saves that `download` started and that finish later (the browser's save picker,
    /// #1315). The hook starts a ticket for such a save and reports how it ended; until then the
    /// document's save isn't confirmed, and a New/Open held back by the Save Changes prompt waits.
    pub async_saves: Option<async_saves::AsyncSaves>,
    /// Web, framed by a page that keeps the files (`?host=parent`): set alongside `download`,
    /// Save, Save As and Export hand the bytes to that page instead, with whether it was Save As
    /// ([`HostSaveFn`]: true for a `file.saveAs` in a format that keeps everything, which File ›
    /// Export to such a format runs too, and for a Save under a name the page never sent; false
    /// for Save and for copies: the PDF and PNG exports and Save As in a format that doesn't keep
    /// everything). Save then writes the opened file
    /// under its own name and format, as on the desktop, and a Save or Save As that keeps
    /// everything makes that name the document's file, so the next Save goes to it.
    pub host_save: Option<HostSaveFn>,
    /// Web: told whether the document has unsaved changes after each pass, for the browser's
    /// leave-page guard (`beforeunload` runs between frames and can't ask the app).
    pub on_dirty: Option<Box<dyn Fn(bool)>>,
    /// Web: hand PDF bytes to the browser's print dialog (no download or intermediate file).
    /// Desktop uses `print_native` instead. An error means nothing was opened.
    pub print: Option<Box<dyn Fn(&[u8]) -> Result<(), String>>>,
    /// Desktop: the picture on the system clipboard, if any — egui's paste only carries text
    /// (#45). Read when Paste finds no text; see [`paste_picture`].
    pub clipboard_picture: Option<Box<dyn Fn() -> Option<paste_picture::ClipboardPicture>>>,
    /// Whether the system has animation effects on (Windows: Animation effects; macOS: Reduce
    /// motion; Linux and the BSDs: the desktop's setting; browsers: `prefers-reduced-motion`), or
    /// `None` when it doesn't say. What "Use system setting" follows ([`theme::Animations`]).
    /// Asked every frame, so a change in the system's settings shows at once: the host answers
    /// from memory and does any slow asking elsewhere. Without the hook, animation effects stay on.
    pub animations_enabled: Option<Box<dyn Fn() -> Option<bool>>>,
    /// Desktop: the text on the system clipboard, if any. Ctrl+V gets it from egui's paste event;
    /// the ribbon and context-menu Paste read it here (#584).
    pub clipboard_text: Option<Box<dyn Fn() -> Option<String>>>,
    /// Desktop: the HTML on the system clipboard — another program's formatted copy (a Word
    /// document, a web page) — if any. Paste keeps its formatting (#687); see [`system_clipboard`].
    pub clipboard_html: Option<Box<dyn Fn() -> Option<String>>>,
    /// Desktop: put HTML on the system clipboard with its plain-text alternative (html, text, and
    /// for a copy of one equation alone its MathML, else empty), so other programs keep the
    /// formatting (#1523) and equations (#816). The MathML goes in the platform's MathML formats
    /// as well, which Word reads a pasted equation from (#1297). `true` when the HTML and text are
    /// there; otherwise the copy's text goes through egui as usual.
    pub set_clipboard_html: Option<Box<dyn Fn(&str, &str, &str) -> bool>>,
    /// Desktop: the RTF on the system clipboard, where the platform offers it (#687).
    pub clipboard_rtf: Option<Box<dyn Fn() -> Option<String>>>,
    /// Desktop: the names of the formats on the system clipboard (MIME types, Windows format
    /// names), where the platform lists them, so a paste that finds nothing it can use says what
    /// is there instead of "empty" (#833).
    pub clipboard_formats: Option<Box<dyn Fn() -> Vec<String>>>,
    /// Web: keep a font the user added (stored file name, original bytes) for the next visit
    /// (#582). Desktop fonts are stored by the font database itself, in the app's font folder.
    pub store_font: Option<Box<dyn Fn(&str, &[u8])>>,
    /// Web: forget a stored font (its stored file name) once the user removes it.
    pub forget_font: Option<Box<dyn Fn(&str)>>,
    /// Web: request browser to query and load local installed fonts (`window.queryLocalFonts()`).
    pub load_system_fonts: Option<Box<dyn Fn()>>,
    /// Desktop: File › Options › General › Make WordCraft the default for Word documents (#295),
    /// only from that button (never a programmatic command). Windows opens the Settings app's
    /// Default apps page (apps can't set defaults themselves); Linux and BSD set it with
    /// `xdg-mime`. `None` (web, macOS) hides the button; macOS shows how to do it in Finder
    /// instead. An error means nothing changed.
    pub make_default_app: Option<Box<dyn Fn() -> Result<DefaultApp, String>>>,
    /// Desktop: the app's own user templates folder (`Templates` in the settings folder, #2852),
    /// used while File › Options names none. File › New lists personal templates from it and Save
    /// as Word Template starts in it. `None` (web) hides personal templates.
    pub templates_dir: Option<std::path::PathBuf>,
}

/// How Save in the Save Changes prompt went.
enum PromptSave {
    /// Written, keeping everything: the held-back command may run.
    Saved,
    /// Not saved (a format that doesn't keep everything); the command is cancelled.
    NotSaved,
    /// Web: the browser's save picker finishes later; its ticket ([`async_saves`]).
    Later(u64),
}

/// Web: a save the browser's picker hasn't finished ([`async_saves`]).
struct SaveInFlight {
    ticket: u64,
    /// The file name it saves as.
    name: String,
    /// The document it saved ([`Session::document_id`]).
    document: u64,
    /// Whether it marked the document saved (a Save in a format that keeps everything).
    marked_saved: bool,
}

/// Web: the command a Save Changes prompt holds back until its picker save ends.
struct AwaitingSave {
    ticket: u64,
    then: String,
    params: Value,
    /// The document the prompt asked about.
    document: u64,
    /// Its revision once the save started: a later edit isn't in that save.
    rev: u64,
}

/// What [`Services::make_default_app`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultApp {
    /// The system's default-apps settings opened; the user picks WordCraft there (Windows).
    SettingsOpened,
    /// WordCraft is now the default app for Word documents (Linux, BSD).
    MadeDefault,
}

/// Files delivered asynchronously.
pub type Inbox = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

/// [`Services::host_save`]: `(file name, bytes, save_as)`. An error means the page didn't get
/// the file, and the save failed.
pub type HostSaveFn = Box<dyn Fn(&str, &[u8], bool) -> Result<(), String>>;

/// Whether a pasted picture belongs in the document: the page has keyboard focus, and no
/// Backstage or dialog is open (#1349). A focused text field (comment, Find box) means the page
/// does not have focus, so `canvas_focused` already covers it; egui's own "wants keyboard input"
/// is true for the focused page itself and can't be used.
fn paste_target(canvas_focused: bool, backstage: bool, dialog: bool) -> bool {
    canvas_focused && !backstage && !dialog
}

/// UI state that persists between runs (and that agents can read and set).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiState {
    /// The ribbon tab showing. It is written to `ui.json` but not restored at startup: Word opens
    /// on Home every launch, and only going File and back to the editor keeps the tab ([`WordApp::apply_prefs`]).
    pub tab: String,
    pub backstage: bool,
    pub backstage_page: String,
    /// File › Print settings (#641); not saved.
    #[serde(skip)]
    pub print: print_page::PrintSettings,
    pub ribbon_collapsed: bool,
    /// The Quick Access Toolbar shows in the title bar (#1945). Its menu's Hide Quick Access
    /// Toolbar takes it away; Show Quick Access Toolbar (the ribbon's right-click menu, Customize
    /// Ribbon › Quick Access Toolbar) brings it back. Kept between runs.
    pub quick_access_toolbar: bool,
    pub recent: Vec<String>,
    /// Symbols inserted from the Symbol dialog, most recent first (#321).
    pub recent_symbols: Vec<dialogs_insert::RecentSymbol>,
    /// Colours picked in the Colors dialog (More Colors…), most recent first (#606).
    pub recent_colors: Vec<String>,
    /// Fonts applied from the font box, the mini toolbar or the Font dialog, most recent first:
    /// the Recently Used Fonts section at the top of the font list (#2068).
    #[serde(deserialize_with = "lenient_recent_fonts")]
    pub recent_fonts: Vec<String>,
    /// Interface theme: light, dark, or follow the OS appearance (#115).
    pub theme: theme::Appearance,
    /// Interface size (#475, #822): Auto (the display scale the system reports) or an absolute
    /// size in percent. Junk saved values read as Auto; out-of-range ones are clamped to 50–300 %.
    #[serde(rename = "uiScale")]
    pub interface_size: theme::InterfaceSize,
    /// The interface size `ui.json` held before #822, on top of the display scale: read once to
    /// migrate ([`theme::legacy_interface_percent`]), never written. A valid `uiScale` wins.
    #[serde(rename = "interfaceSize", skip_serializing)]
    pub(crate) legacy_interface_size: Option<serde_json::Value>,
    /// Animation effects (smooth scrolling, fades): on, off, or follow the system's setting, which
    /// is the default. An unknown saved value reads as the default.
    pub animations: theme::Animations,
    /// The dark-mode switch `ui.json` held before `theme` (#115): read once to migrate, never written.
    #[serde(rename = "dark", skip_serializing)]
    pub(crate) legacy_dark: Option<bool>,
    pub nav_tab: String,
    /// View › Navigation Pane: open or closed as the person left it, kept between runs (#2526).
    /// The session owns it while running (see [`WordApp::prefs`]).
    pub nav_pane: bool,
    /// Where the Styles pane sits (#1756). The session owns it while running (see [`WordApp::prefs`]).
    #[serde(deserialize_with = "lenient_styles_pane_dock")]
    pub styles_pane_dock: wordcraft_engine::StylesPaneDock,
    /// The floating Styles pane's `[x, y, width, height]`, also kept while it is docked.
    #[serde(deserialize_with = "lenient_float_rect")]
    pub styles_pane_float: Option<[f32; 4]>,
    /// The docked Styles pane's width this run; like the other panes' widths, not kept between runs.
    #[serde(skip)]
    pub styles_pane_width: Option<f32>,
    /// Where each of the other task panes sits, by [`panes::Pane::key`] (#1756).
    #[serde(deserialize_with = "panes::lenient_places")]
    pub pane_places: std::collections::BTreeMap<String, panes::SavedPane>,
    /// The task panes in the order the user put them, by [`panes::Pane::key`]: each edge shows its
    /// panes in this order, the first outermost. Empty: the default order.
    #[serde(deserialize_with = "panes::lenient_order", skip_serializing_if = "Vec::is_empty")]
    pub pane_order: Vec<String>,
    /// Headings whose sub-headings the Navigation pane hides (#1997); per session, not saved.
    #[serde(skip)]
    pub nav_collapsed: std::collections::HashSet<wordcraft_doc::Path>,
    pub show_discord: bool,
    /// User name (File › Options) for comments and tracked changes; empty keeps the default.
    pub author: String,
    /// Desktop: the main window's size and position, restored at the next launch.
    pub window: Option<window_geometry::WindowGeometry>,
    /// Interface language: `auto` (follow the system) or a code from [`i18n::LANGUAGES`].
    pub language: String,
    /// Editing preferences; the session owns them while running (see [`WordApp::prefs`]).
    pub editing: wordcraft_engine::Prefs,
    /// Read Aloud speed (1 = normal) and whether it skips citations and bibliographies.
    pub read_aloud_rate: f32,
    pub read_aloud_skip_citations: bool,
    /// Show ribbon and Quick Access Toolbar KeyTips on Alt/F10 (#620).
    pub keytips_enabled: bool,
    /// Digits entered for a multi-key Quick Access Toolbar badge; never persisted.
    #[serde(skip)]
    pub keytip_prefix: String,
    /// Keytips (Alt) state; not persisted, it resets each session.
    #[serde(skip)]
    pub keytips: crate::keytips::Phase,
    /// Alt is held according to key or modifier events; never persisted.
    #[serde(skip)]
    pub alt_pressed: bool,
    /// Since the last Alt press, another key or a mouse button was pressed: the Alt release
    /// that follows ends a chord (Alt+click, Alt+drag column selection), not a keytip tap.
    #[serde(skip)]
    pub alt_chord_used: bool,
    /// An Alt key is down according to key events. Its modifier can go before the key does:
    /// AltGr on Linux reports the Right Alt key with Level 3 Shift, not Alt (#2388).
    #[serde(skip)]
    pub alt_key_down: bool,
    /// Since the last Alt press the Alt modifier was on, or the key was Left Alt: a Right Alt
    /// that never turns it on is AltGr, which types characters and never opens keytips (#2388).
    #[serde(skip)]
    pub alt_modifier_seen: bool,
    /// View › Switch Modes: show pages dark (white text on black), kept between runs. Only the
    /// pages: the interface follows [`UiState::theme`] (#312).
    pub dark_page: bool,
    /// File › Options › Proofing › Check spelling as you type, kept between runs (#1792).
    pub check_spelling: bool,
    /// File › Options › Advanced › Animation: the caret glides to its new place and its blink
    /// fades, as Word's does on Windows (on, the default), or it jumps and blinks on and off
    /// (off).
    pub caret_animation: bool,
    /// Custom keyboard shortcuts (Customize Keyboard, #368); the session owns them while running.
    pub keyboard: wordcraft_engine::KeyMap,
    /// Custom ribbon tabs and groups, hidden tabs and the Quick Access Toolbar (Customize Ribbon,
    /// #379); the session owns them while running.
    pub ribbon: wordcraft_engine::RibbonLayout,
    /// Caption labels the user added (References › Insert Caption › New Label), kept between
    /// runs like Word keeps them in its Normal template (#402).
    pub caption_labels: Vec<String>,
    /// The label of the last caption inserted, picked again next time (#3120).
    pub last_caption_label: String,
    /// Draw › Add Pen: the pens added to the pen gallery; the session owns them while running.
    /// Entries that aren't pens are dropped on reading.
    #[serde(deserialize_with = "wordcraft_engine::cmd::draw::lenient_pens")]
    pub ink_pens: Vec<wordcraft_engine::cmd::draw::CustomPen>,
    /// File › Options › General: initials for comments; empty = the user name's. The session
    /// owns them while running, like `author`.
    pub initials: String,
    /// The File › Options pane last shown ([`wordcraft_engine::cmd::options::PANES`]).
    pub options_pane: String,
    /// File › Options › Advanced: how many recent documents File › Home and Open list (0–50).
    pub recent_count: usize,
    /// File › Options › General: show the mini toolbar over a selection.
    pub mini_toolbar: bool,
    /// File › Options › Advanced › Editing: moving a picture or shape snaps it to alignment
    /// guides (on by default; Alt moves freely).
    pub alignment_guides: bool,
    /// File › Options › General: start on File › Home (Blank document, templates, Recent) when
    /// the app opens without a document (#766); on, as in Word. See [`WordApp::start_screen`].
    pub start_screen: bool,
    /// File › Options › Display: print the page colour (off, as in Word).
    pub print_backgrounds: bool,
    /// File › Options › Display: update fields (dates, page numbers, SEQ…) before printing.
    pub update_fields_before_print: bool,
    /// The custom dictionary (File › Options › Proofing, Add to Dictionary), kept between runs.
    pub dictionary: Vec<String>,
}

/// The ribbon tab the app opens on, and the one it falls back to when the tab that was showing
/// no longer applies (a contextual tab after the selection moved away, a tab hidden in Customize
/// Ribbon). Word opens on Home every launch; see [`UiState::tab`].
pub const DEFAULT_TAB: &str = "Home";

/// ⌘Q, which only the Mac sends: quit.
const QUIT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(egui::Modifiers::MAC_CMD, egui::Key::Q);
/// Most recent documents kept (File › Options › Advanced offers 0–50, as Word does).
pub const MAX_RECENT: usize = 50;
/// Most custom dictionary words read back from `ui.json`.
pub const MAX_DICTIONARY: usize = 10_000;
/// Fonts kept under Recently Used Fonts, about as many as Word shows.
pub const MAX_RECENT_FONTS: usize = 10;
/// The longest family name Recently Used Fonts keeps, the longest `format.font` accepts.
pub const MAX_FONT_NAME_BYTES: usize = 128;

impl Default for UiState {
    fn default() -> Self {
        UiState {
            tab: DEFAULT_TAB.into(),
            backstage: false,
            backstage_page: "home".into(),
            print: print_page::PrintSettings::default(),
            ribbon_collapsed: false,
            quick_access_toolbar: true,
            recent: Vec::new(),
            recent_symbols: Vec::new(),
            recent_colors: Vec::new(),
            recent_fonts: Vec::new(),
            theme: theme::Appearance::default(),
            interface_size: theme::InterfaceSize::default(),
            legacy_interface_size: None,
            animations: theme::Animations::default(),
            legacy_dark: None,
            nav_tab: "headings".into(),
            nav_pane: false,
            styles_pane_dock: wordcraft_engine::StylesPaneDock::Right,
            styles_pane_float: None,
            styles_pane_width: None,
            pane_places: Default::default(),
            pane_order: Vec::new(),
            nav_collapsed: Default::default(),
            show_discord: true,
            author: String::new(),
            window: None,
            language: i18n::AUTO.into(),
            editing: wordcraft_engine::Prefs::default(),
            read_aloud_rate: 1.0,
            read_aloud_skip_citations: true,
            keytips_enabled: true,
            keytip_prefix: String::new(),
            keytips: crate::keytips::Phase::Off,
            alt_pressed: false,
            alt_chord_used: false,
            alt_key_down: false,
            alt_modifier_seen: false,
            dark_page: false,
            check_spelling: true,
            caret_animation: true,
            keyboard: wordcraft_engine::KeyMap::default(),
            ribbon: wordcraft_engine::RibbonLayout::default(),
            caption_labels: Vec::new(),
            last_caption_label: String::new(),
            ink_pens: Vec::new(),
            initials: String::new(),
            options_pane: "general".into(),
            recent_count: 12,
            mini_toolbar: true,
            alignment_guides: true,
            start_screen: true,
            print_backgrounds: false,
            update_fields_before_print: false,
            dictionary: Vec::new(),
        }
    }
}

impl UiState {
    /// Read `ui.json` leniently: a setting with a value of the wrong type (or a newer or older
    /// version's idea of it) falls back to its default instead of discarding every setting.
    /// `None` only when the file isn't a JSON object.
    pub fn from_json(bytes: &[u8]) -> Option<UiState> {
        let mut v: Value = serde_json::from_slice(bytes).ok()?;
        if !v.is_object() {
            return None;
        }
        migrate_units(&mut v);
        Some(options::lenient(&v))
    }

    /// How many recent documents to list.
    pub fn recent_shown(&self) -> usize {
        self.recent_count.min(MAX_RECENT)
    }
}

/// Put `name` first in the Recently Used Fonts list, dropping an earlier entry for the same
/// family (ignoring case); the list keeps [`MAX_RECENT_FONTS`] names. A blank name, or one longer
/// than [`MAX_FONT_NAME_BYTES`], is not added.
pub fn push_recent_font(list: &mut Vec<String>, name: &str) {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_FONT_NAME_BYTES {
        return;
    }
    let key = name.to_lowercase();
    list.retain(|f| f.to_lowercase() != key);
    list.insert(0, name.to_string());
    list.truncate(MAX_RECENT_FONTS);
}

/// A Recently Used Fonts list from saved `names`: trimmed, without blanks, names longer than
/// [`MAX_FONT_NAME_BYTES`] or repeats (ignoring case); the first [`MAX_RECENT_FONTS`] that remain.
pub fn recent_fonts_from<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    for f in names {
        if out.len() >= MAX_RECENT_FONTS {
            break;
        }
        let f = f.trim();
        if f.is_empty() || f.len() > MAX_FONT_NAME_BYTES {
            continue;
        }
        let key = f.to_lowercase();
        if !keys.contains(&key) {
            keys.push(key);
            out.push(f.to_string());
        }
    }
    out
}

/// An unknown saved position reads as the default.
fn lenient_styles_pane_dock<'de, D: serde::Deserializer<'de>>(d: D) -> Result<wordcraft_engine::StylesPaneDock, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(v.as_str().and_then(wordcraft_engine::StylesPaneDock::parse).unwrap_or_default())
}

/// Anything but four finite numbers reads as no saved rect; `craft_ui::panes` brings a saved one
/// back inside the window.
fn lenient_float_rect<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<[f32; 4]>, D::Error> {
    Ok(panes::float_rect(&Value::deserialize(d)?))
}

/// `recentFonts` as saved, through [`recent_fonts_from`]: an entry that isn't a name is dropped,
/// not the whole list.
fn lenient_recent_fonts<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(v.as_array().map(|a| recent_fonts_from(a.iter().filter_map(Value::as_str))).unwrap_or_default())
}

/// The family a command applies: `format.font {"name"}` (the font box, the mini toolbar, command
/// search) or `format.set {"props": {"font"}}` (the Font dialog). It joins Recently Used Fonts
/// once the command succeeds.
fn applied_font(id: &str, params: &Value) -> Option<String> {
    let name = match id {
        "format.font" => params.get("name"),
        "format.set" => params.get("props").and_then(|p| p.get("font")),
        _ => None,
    }?;
    let name = name.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Settings saved before #838 kept the measurement unit as `editing.units`, which defaulted to
/// inches; it now lives in `editing.measurementUnits`, and no value there follows the region. A
/// unit other than inches under the old key was the user's own choice, so it carries over (an
/// inch value can't be told from the old default, so it follows the region).
fn migrate_units(v: &mut Value) {
    let Some(editing) = v.get_mut("editing").and_then(Value::as_object_mut) else { return };
    if editing.contains_key("measurementUnits") {
        return;
    }
    if let Some(old) = editing.get("units").and_then(Value::as_str).filter(|u| *u != "inches") {
        let old = Value::String(old.to_string());
        editing.insert("measurementUnits".into(), old);
    }
}

/// The application.
pub struct WordApp {
    pub session: Session,
    pub ui: UiState,
    pub services: Services,
    pub printing: printing::State,
    print_feedback_seen: Option<printing::Feedback>,
    pub canvas: canvas::CanvasState,
    /// Content control frames, check boxes and lists on the page.
    pub controls: content_controls::ControlUi,
    pub dialog: Option<dialogs::Dialog>,
    /// The Colors dialog (More Colors…), shown above any other dialog (#606).
    pub colors: Option<colors::ColorsDialog>,
    /// The Restrict Editing pane (#412).
    pub restrict: dialogs_review::RestrictPane,
    pub status_msg: Option<(String, f64)>,
    pub previews: previews::Previews,
    /// Media key of the picture a pending Change Picture replaces (#147).
    pub change_picture_target: Option<String>,
    /// Web: a recipient list was asked for (Select Recipients › Use an Existing List…), so the
    /// next text file from the picker loads as recipients instead of opening (#240).
    pub recipient_list_pending: bool,
    /// The font database's [`generation`](wordcraft_fonts::FontDb::generation) last seen: when
    /// fonts are added or removed (by any route) the font lists and the layout are refreshed.
    font_generation: u64,
    /// A file picked for the open dialog (Templates and Add-ins › Attach…, Organizer ›
    /// Browse…); the dialog takes it on its next frame.
    pub(crate) dialog_pick: Option<String>,
    /// macOS: the window has no title bar; leave room for the traffic lights.
    pub integrated_titlebar: bool,
    /// macOS: the title bar's height and where its content starts, around the traffic lights.
    pub mac_titlebar: chrome::MacTitleBar,
    /// Focus (#1678): the ribbon is dropped down from the Focus bar, over the page.
    pub(crate) focus_ribbon: bool,
    pub(crate) updates_rt: updates_ui::Runtime,
    control_rx: Option<std::sync::mpsc::Receiver<ControlRequest>>,
    /// Admitted `ui.screenshot` requests: count/byte budgets, retries and completion slots.
    shots: control::Shots,
    /// Monotonic geometry epoch; any size/DPI/zoom/surface/document change increments it,
    /// including a change back to an earlier value.
    capture_epoch: u64,
    /// The surface reported by the last `RawInput` hook, without a document identity.
    surface_geometry: Option<control::SurfaceGeometry>,
    /// The geometry `capture_epoch` describes; `None` before the first logic pass.
    capture_geometry: Option<control::CaptureGeometry>,
    /// Synthetic input not consumed by a real UI pass yet.
    pub(crate) synthetic: Vec<egui::Event>,
    /// The batch offered to eframe is already in its pending RawInput; do not append it again.
    synthetic_pending: bool,
    /// How many events of `synthetic` the pending batch contains.
    synthetic_injected: usize,
    /// macOS: the primary button went down with Control, so it is being read as the secondary.
    control_click: bool,
    /// Badge rects recorded during layout for the current keytip phase (cleared by `logic`, painted by `show`).
    pub(crate) keytip_rects: Vec<(egui::Rect, String)>,
    /// The Navigation pane's search box takes the keyboard on its next frame (Ctrl+F, #712).
    pub(crate) nav_search_focus: bool,
    styled: bool,
    /// The CJK face order the installed UI fonts use (Chinese first, or Japanese first).
    fonts_hans: bool,
    /// Whether the installed UI fonts use the Korean CJK face order.
    fonts_korean: bool,
    /// Whether the installed UI fonts include an installed CJK fallback font (#241).
    fonts_system_cjk: bool,
    /// CJK text is (about to be) on screen in a non-CJK interface, e.g. the language names in
    /// File ▸ Options: load the installed CJK fallback font if no embedded face covers it.
    pub(crate) want_system_cjk: bool,
    /// The document (id, revision) whose comment text [`WordApp::ensure_comment_fonts`] last
    /// looked at.
    comment_fonts_for: Option<(u64, u64)>,
    /// The Recover list's document names [`WordApp::ensure_recover_fonts`] last looked at.
    recover_fonts_for: Option<u64>,
    /// The list of recent document paths [`WordApp::ensure_recent_fonts`] last looked at.
    recent_fonts_for: Vec<String>,
    /// The Find query and result count [`WordApp::ensure_find_fonts`] last looked at.
    find_fonts_for: Option<(String, usize, u64)>,
    fonts_frames: u32,
    /// The effective interface theme last installed in egui ([`theme::apply`]).
    applied_theme: Option<theme::Appearance>,
    /// The display scale (native pixels per point) seen in the last frame, 1.0 before the first;
    /// `ui.interfaceSize` reports the size Auto draws at with it.
    native_scale: f32,
    /// A pre-#822 interface size (percent on top of the display scale) waiting for the first frame
    /// that knows the display's scale, to become the absolute size it drew at
    /// ([`theme::migrate_legacy_interface_size`]).
    legacy_interface_size: Option<f64>,
    /// The desktop's light/dark choice, watched while the app runs (Linux has no winit answer).
    desktop_theme: desktop_theme::DesktopTheme,
    /// The last answer [`desktop_theme`] gave, so a frame that asks costs nothing.
    desktop_dark: Option<bool>,
    /// The interface language and the editing language the session last got for it
    /// ([`i18n::Lang::editing_lang`], or for English the region's English).
    applied_editing: Option<(i18n::Lang, Option<String>)>,
    /// What the system last said about its animation effects ([`Services::animations_enabled`]).
    system_animations: Option<bool>,
    /// Whether egui's own animations were last switched on or off ([`theme::apply_animations`]).
    applied_animations: Option<bool>,
    pub frame_ms: f64,
    /// The window title last sent; a viewport command schedules a repaint, so only send changes.
    sent_title: String,
    /// Full screen as last asked of the window, and as the window last reported it (#980).
    sent_full_screen: bool,
    seen_full_screen: Option<bool>,
    /// Nothing else (a dialog, menu, keytips, the File tab) is open to take Escape this frame, so
    /// Escape may leave full screen. Read before anything handles the frame's keys.
    escape_free: bool,
    pub quit_requested: bool,
    pub autosave: bool,
    pub word_count: (u64, usize),
    last_autosave: f64,
    /// The tab shown before the Equation tab came up (restored when editing ends).
    pub(crate) equation_prev_tab: Option<String>,
    /// SmartArt Text Pane: the item last focused (Promote, Demote and Add Shape act on it) and
    /// the item being typed in (its keystrokes join one undo step).
    pub smart_art_item: Option<usize>,
    /// A Text Pane row has focus (`smart_art_item` remembers the last one, so Tab handling in
    /// [`WordApp::logic`] only fires while a row is focused, not from the canvas).
    pub smart_art_row_focused: bool,
    pub(crate) smart_art_typing: Option<usize>,
    /// The tab shown before Outline view brought up the Outlining tab (restored when it closes).
    pub(crate) outline_prev_tab: Option<String>,
    /// Zotero commands in flight (`ui.zotero.*`).
    pub zotero: zotero::ZoteroLink,
    /// The egui context, once the first frame has run (background work wakes the UI with it).
    pub(crate) ctx: Option<egui::Context>,
    /// Whether the current command was entered by an input method's own transaction. The engine
    /// uses it to keep IME commits from setting the application-change signal.
    pub(crate) ime_origin: bool,
    /// Read Aloud: the start of the sentence the caret was last moved to, and the last error shown.
    pub(crate) read_aloud_at: Option<wordcraft_engine::doc::Pos>,
    pub(crate) read_aloud_error: Option<String>,
    /// The file the document was last explicitly saved to in this session; AutoSave writes only
    /// there. A file that was merely opened isn't rewritten until the user saves it: saving drops
    /// whatever WordCraft can't represent (content controls, charts, macros…).
    autosave_path: Option<std::path::PathBuf>,
    /// Web: saves the browser's picker hasn't finished yet ([`async_saves`], #1315).
    saves_in_flight: Vec<SaveInFlight>,
    /// Web: the command a Save Changes prompt held back until its picker save ends (#1315).
    awaiting_save: Option<AwaitingSave>,
    /// The file dialog the host is showing, and what its answer is for ([`file_dialogs`]).
    file_dialog: Option<file_dialogs::PendingDialog>,
    /// The built-in file browser, when it is open, and the folder it showed last (#576).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) file_browser: file_browser::State,
    /// File › Info: the property field being typed in and the document revision after its last
    /// keystroke, so one visit's keystrokes make a single undo step (and anything else in
    /// between, such as an Undo, starts a new one).
    pub(crate) info_editing: Option<(&'static str, u64)>,
    recovery_seen: bool,
    /// Started on the Start screen with nothing to open (#1129): the id of the placeholder
    /// blank document the session holds until the user creates or opens one. While it is still
    /// the session's document, no document is open: the window shows no page and only the File
    /// tab, Help and settings work. See [`WordApp::has_document`].
    no_document: Option<u64>,
    /// Comment and reply text typed but not yet written to the document (#1348).
    pub(crate) comment_drafts: panes::CommentDrafts,
    /// The document the window's per-document state belongs to; when the session's active
    /// document is another, [`WordApp::sync_document`] puts that state aside and brings the
    /// other's.
    shown_document: u64,
    /// The per-document state of the open documents not shown.
    parked: std::collections::HashMap<u64, documents::Parked>,
    /// The open windows ([`windows`]).
    pub(crate) windows: windows::Windows,
    /// The document the last print job was for: its window reports how the job went.
    print_document: Option<u64>,
}

/// The answer to "Do you want to save changes?" (`ui.saveChanges`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveChoice {
    Save,
    DontSave,
    Cancel,
}

impl SaveChoice {
    pub fn parse(s: &str) -> Option<SaveChoice> {
        Some(match s {
            "save" => SaveChoice::Save,
            "dontSave" => SaveChoice::DontSave,
            "cancel" => SaveChoice::Cancel,
            _ => return None,
        })
    }
}

impl WordApp {
    /// Creates the app: adopts the system locale, then initializes the document session, UI state,
    /// services, printing, canvas, dialogs, previews, control and screenshot state, font and theme
    /// tracking.
    pub fn new(session: Session, services: Services) -> Self {
        // Numeral › System follows the system locale, found here rather than in layout.
        if let Some(tag) = sys_locale::get_locale() {
            wordcraft_engine::set_system_locale(&tag);
        }
        // What the window shows starts as a new window's.
        let window_ui::WindowUi {
            canvas,
            controls,
            dialog,
            colors,
            restrict,
            status_msg,
            previews,
            change_picture_target,
            recipient_list_pending,
            dialog_pick,
            focus_ribbon,
            control_click,
            keytip_rects,
            nav_search_focus,
            comment_fonts_for,
            find_fonts_for,
            sent_title,
            sent_full_screen,
            seen_full_screen,
            escape_free,
            word_count,
            last_autosave,
            equation_prev_tab,
            smart_art_item,
            smart_art_row_focused,
            smart_art_typing,
            outline_prev_tab,
            read_aloud_at,
            awaiting_save,
            file_dialog,
            info_editing,
            shown_document,
            #[cfg(not(target_arch = "wasm32"))]
            file_browser,
            tab,
            backstage,
            backstage_page,
            print,
            nav_collapsed,
            keytip_prefix,
            keytips,
            alt_pressed,
            alt_chord_used,
            alt_key_down,
            alt_modifier_seen,
        } = window_ui::WindowUi::new(session.document_id());
        WordApp {
            session,
            ui: UiState {
                tab,
                backstage,
                backstage_page,
                print,
                nav_collapsed,
                keytip_prefix,
                keytips,
                alt_pressed,
                alt_chord_used,
                alt_key_down,
                alt_modifier_seen,
                ..UiState::default()
            },
            services,
            printing: Default::default(),
            print_feedback_seen: None,
            integrated_titlebar: false,
            mac_titlebar: chrome::MacTitleBar::default(),
            updates_rt: updates_ui::Runtime::default(),
            control_rx: None,
            shots: control::Shots::default(),
            capture_epoch: 0,
            surface_geometry: None,
            capture_geometry: None,
            synthetic: Vec::new(),
            synthetic_pending: false,
            synthetic_injected: 0,
            styled: false,
            fonts_hans: false,
            fonts_korean: false,
            fonts_system_cjk: false,
            want_system_cjk: false,
            recover_fonts_for: None,
            recent_fonts_for: Vec::new(),
            fonts_frames: 0,
            applied_theme: None,
            native_scale: 1.0,
            legacy_interface_size: None,
            // Inert until the app binary calls `watch_desktop_theme`.
            desktop_theme: desktop_theme::DesktopTheme::default(),
            desktop_dark: None,
            applied_editing: None,
            system_animations: None,
            applied_animations: None,
            frame_ms: 0.0,
            quit_requested: false,
            autosave: true,
            zotero: zotero::ZoteroLink::default(),
            ctx: None,
            ime_origin: false,
            read_aloud_error: None,
            autosave_path: None,
            saves_in_flight: Vec::new(),
            font_generation: wordcraft_fonts::FontDb::global().generation(),
            recovery_seen: false,
            no_document: None,
            comment_drafts: Default::default(),
            parked: Default::default(),
            windows: windows::Windows::new(shown_document),
            print_document: None,
            canvas,
            controls,
            dialog,
            colors,
            restrict,
            status_msg,
            previews,
            change_picture_target,
            recipient_list_pending,
            dialog_pick,
            focus_ribbon,
            control_click,
            keytip_rects,
            nav_search_focus,
            comment_fonts_for,
            find_fonts_for,
            sent_title,
            sent_full_screen,
            seen_full_screen,
            escape_free,
            word_count,
            last_autosave,
            equation_prev_tab,
            smart_art_item,
            smart_art_row_focused,
            smart_art_typing,
            outline_prev_tab,
            read_aloud_at,
            awaiting_save,
            file_dialog,
            info_editing,
            shown_document,
            #[cfg(not(target_arch = "wasm32"))]
            file_browser,
        }
    }

    pub fn with_control(mut self, rx: std::sync::mpsc::Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// Start-up with no document to open (#766): show File › Home (Blank document, the templates
    /// and Recent), as Word starts, when the Start screen setting is on (the default). No document
    /// is open until the user picks Blank document or a template or opens a file (#1129): Esc or
    /// the back arrow shows an empty window with the ribbon greyed out, as Word does. Hosts call
    /// this after [`WordApp::apply_prefs`] when they have nothing to open; [`WordApp::new`] never
    /// does, so tests, `ui_shot` and windows opened for a document start in the editor. Returns
    /// whether the Start screen shows.
    pub fn start_screen(&mut self) -> bool {
        if self.ui.start_screen {
            self.ui.backstage = true;
            self.ui.backstage_page = "home".into();
            self.no_document = Some(self.session.document_id());
            self.session.release_untitled_name();
        }
        self.ui.start_screen
    }

    /// Whether a document is open: false from a Start screen launch until the user creates or
    /// opens one (#1129).
    pub fn has_document(&self) -> bool {
        self.no_document != Some(self.session.document_id())
    }

    /// Preferences to save between runs: the UI state plus the current user name and editing
    /// preferences.
    pub fn prefs(&self) -> UiState {
        let mut ui = self.ui.clone();
        ui.author = self.session.author.clone();
        ui.initials = self.session.initials.clone();
        ui.dictionary = wordcraft_engine::proof::user_dictionary();
        ui.editing = self.session.prefs.clone();
        ui.read_aloud_rate = self.session.read_aloud.rate();
        ui.read_aloud_skip_citations = self.session.read_aloud.skip_citations;
        ui.dark_page = self.session.view.dark_mode;
        ui.nav_pane = self.session.view.nav_pane;
        ui.styles_pane_dock = self.session.view.styles_pane_dock;
        ui.check_spelling = self.session.view.proofing;
        ui.keyboard = self.session.keymap.clone();
        ui.ribbon = self.session.ribbon.clone();
        ui.ink_pens = self.session.view.draw.pens.clone();
        ui
    }

    /// Restore preferences saved by [`WordApp::prefs`].
    pub fn apply_prefs(&mut self, ui: UiState) {
        self.ui = ui;
        self.ui.backstage = false;
        // Word opens on Home every launch, so the tab `ui.json` holds is not restored. Going File
        // and back to the editor within the run still returns to the tab that was showing.
        self.ui.tab = DEFAULT_TAB.into();
        let recent_fonts = recent_fonts_from(self.ui.recent_fonts.iter().map(String::as_str));
        self.ui.recent_fonts = recent_fonts;
        // A `ui.json` from before the theme setting: its dark-mode switch picks Dark or Light.
        if let Some(dark) = self.ui.legacy_dark.take() {
            self.ui.theme = if dark { theme::Appearance::Dark } else { theme::Appearance::Light };
        }
        // A `ui.json` from before #822: its interface size was on top of the display scale, so it
        // becomes absolute on the first frame that knows that scale (`logic`). A chosen `uiScale`
        // (the current key) wins; "auto" can't be told from a missing key, so the old size does.
        self.legacy_interface_size =
            self.ui.legacy_interface_size.take().filter(|_| self.ui.interface_size.is_auto()).and_then(|old| theme::legacy_interface_percent(&old));
        // The session owns the name from here on; `prefs` copies it back when saving.
        let author = std::mem::take(&mut self.ui.author);
        if !author.trim().is_empty() {
            self.session.author = author;
        }
        self.session.initials = std::mem::take(&mut self.ui.initials).chars().take(wordcraft_engine::cmd::options::MAX_INITIALS_CHARS).collect();
        for w in std::mem::take(&mut self.ui.dictionary).iter().take(MAX_DICTIONARY) {
            let w = w.trim();
            if !w.is_empty() && w.chars().count() <= wordcraft_engine::cmd::options::MAX_WORD_CHARS {
                wordcraft_engine::proof::add_word(w);
            }
        }
        self.session.prefs = std::mem::take(&mut self.ui.editing);
        // Check spelling as you type follows the saved preference (#1792).
        self.session.view.proofing = self.session.prefs.check_spelling;
        // The saved dictionaries folder (#1063) is searched from the start.
        if let Some(f) = self.session.prefs.dictionaries_folder.as_deref() {
            wordcraft_engine::cmd::options::use_dictionaries_folder(Some(f));
        }
        self.session.read_aloud.set_rate(self.ui.read_aloud_rate);
        self.session.read_aloud.skip_citations = self.ui.read_aloud_skip_citations;
        self.session.view.dark_mode = self.ui.dark_page;
        self.session.view.nav_pane = self.ui.nav_pane;
        self.session.view.styles_pane_dock = self.ui.styles_pane_dock;
        self.session.view.proofing = self.ui.check_spelling;
        // Keys for commands this version doesn't have are dropped.
        self.session.keymap = std::mem::take(&mut self.ui.keyboard);
        let reg = self.session.registry.clone();
        self.session.keymap.retain_known(&reg);
        // So are ribbon and toolbar commands.
        self.session.ribbon = std::mem::take(&mut self.ui.ribbon);
        self.session.ribbon.retain_known(&reg);
        self.session.view.draw.pens = std::mem::take(&mut self.ui.ink_pens);
    }

    /// Select the picture or icon the person just inserted or pasted, so it shows its handles.
    /// `insert.picture` leaves the caret after it, which scripts and agents keep so their next
    /// typing doesn't replace it (#965).
    pub(crate) fn select_inserted_picture(&mut self, r: &Result<Value, String>) {
        if !r.as_ref().is_ok_and(|v| v.get("media").is_some() && v.get("canvasMember").is_none()) || !self.session.sel.is_collapsed() {
            return;
        }
        let end = self.session.sel.focus.clone();
        let Some(off) = end.off.checked_sub(wordcraft_doc::para::OBJ.len_utf8()) else { return };
        let start = wordcraft_doc::Pos { off, ..end.clone() };
        if self.session.doc.para_at(&start).and_then(|p| p.object_at(off)).is_some_and(|o| o.is_drawing()) {
            self.session.sel = wordcraft_engine::Selection { anchor: start, focus: end };
        }
    }

    /// `run` for a command a mouse drag on a table sends: the view stays where it is.
    pub(crate) fn run_keep_view(&mut self, id: &str, params: Value) -> Result<Value, String> {
        self.canvas.keep_view = true;
        let r = self.run(id, params);
        self.canvas.keep_view = false;
        r
    }

    /// Run a command as the input method's own transaction while preserving every normal
    /// [`Self::run`] side effect (save prompts, clipboard, body-epoch tracking, status, …).
    ///
    /// The origin is restored on every return, including errors and nested commands.
    pub(crate) fn run_ime(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let previous = std::mem::replace(&mut self.ime_origin, true);
        let result = self.run(id, params);
        self.ime_origin = previous;
        result
    }

    /// Run a command the user asked for (ribbon, shortcut, Backstage, file drop). New, Open,
    /// Close, Envelopes, Labels and Finish & Merge on a document with unsaved changes first ask
    /// Save / Don't Save / Cancel, and the command runs once that is answered (`ui.saveChanges`).
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        // Comment text still being typed is written first, so a save or export has it and a
        // close, New or Open counts it as a change (#1348). A draft the document refuses is not
        // part of it: saving and exporting go on without it (the refusal stays on the status
        // line), and replacing the document asks Save / Don't Save / Cancel about it.
        let refused = if flushes_comment_drafts(id, &params) { panes::flush_comment_drafts(self).err() } else { None };
        if id == "file.recover" && params.as_object().is_none_or(|p| p.is_empty()) {
            self.ui.backstage = true;
            self.ui.backstage_page = "recover".into();
        }
        // A button whose command needs input the user hasn't given yet opens its dialog (#240).
        if let Some(name) = input_dialog(id, &params)
            && widgets::enabled(self, id)
        {
            self.dialog = dialogs::Dialog::open(name, self);
            return Ok(json!({"pending": name}));
        }
        if (self.session.dirty || self.save_unconfirmed() || refused.is_some())
            && discards_document(id, &params)
            && !self.closes_only_its_window(id, &params)
        {
            let name = self.prompt_name();
            self.dialog = Some(dialogs::Dialog::SaveChanges {
                name,
                then: id.to_string(),
                params,
                document: self.session.document_id(),
                message: String::new(),
            });
            return Ok(json!({"pending": "saveChanges"}));
        }
        // The ribbon and context-menu Paste commands send no text: read the system clipboard's, as
        // Ctrl+V's paste event carries it (#584), and its HTML and RTF, which egui's paste event
        // doesn't carry (#687).
        let mut params = params;
        if let Some(r) = system_clipboard::fill(self, id, &mut params) {
            return r;
        }
        // Paste without system clipboard text: a picture there is newer than our own copy (copying
        // here replaces it with text), so it wins (#45).
        if id == "edit.paste"
            && params.get("text").is_none()
            && let Some(r) = paste_picture::paste(self)
        {
            return r;
        }
        let applied = applied_font(id, &params);
        let r = self.execute_user(id, params);
        // A save error already on the status line is the reason to see; the refusal shows otherwise.
        if let (Some(e), true) = (refused, r.is_ok()) {
            self.status(e);
        }
        if let (Ok(_), Some(name)) = (&r, applied) {
            push_recent_font(&mut self.ui.recent_fonts, &name);
        }
        if matches!(id, "insert.picture" | "insert.icon") {
            self.select_inserted_picture(&r);
        }
        // review.spelling only moves the selection to the next issue; without this, the ribbon's
        // Spelling & Grammar and Editor buttons looked broken — just a selection with no way to
        // see or apply a suggestion (#3340). Reuse the context menu's suggestions, as Shift+F10
        // already does for the word at the caret.
        if id == "review.spelling"
            && let Ok(v) = &r
            && v.get("done").and_then(serde_json::Value::as_bool) != Some(true)
        {
            self.canvas.context_menu_from_keys = true;
        }
        let r = system_clipboard::explain(self, id, r);
        // Check Accessibility shows its findings in the Accessibility pane, kept up to date there.
        if id == "file.accessibility" && r.is_ok() {
            self.session.view.accessibility_pane = true;
        }
        // Match Fields and Check for Errors show what they found.
        if let Ok(v) = &r
            && let Some(d) = dialogs::Dialog::report(id, v)
        {
            self.dialog = Some(d);
        }
        r
    }

    /// Run a command for a script or an agent (control channel, MCP bridge): never asks first.
    /// UI-level commands (`ui.*`) are handled here, the rest by the engine.
    pub fn execute(&mut self, id: &str, params: Value) -> Result<Value, String> {
        // A pending Change Picture only survives until the next action (#147).
        if !matches!(id, "ui.changePicture" | "picture.change" | "insert.picture") {
            self.change_picture_target = None;
        }
        if !matches!(id, "ui.openRecipientList" | "mailings.recipients") {
            self.recipient_list_pending = false;
        }
        // No document open (#1129): only the commands that open or create one, Help, and the
        // settings work.
        if !self.has_document() && !works_without_document(id) {
            let e = tl!("No document is open. Create a document or open one first.").to_string();
            self.status(e.clone());
            return Err(e);
        }
        if let Some(r) = self.ui_command(id, &params) {
            return r;
        }
        if id == "file.close" {
            return self.close_document(&params);
        }
        if id == "file.autosave" {
            return self.set_autosave(&params);
        }
        if let Some(r) = table_pen::toggle(self, id, &params) {
            return r;
        }
        let ctx = self.ctx.clone();
        if let Some(r) = zotero::command(self, id, &params, ctx.as_ref()) {
            if let Err(e) = &r {
                self.status(e.clone());
            }
            return r;
        }
        // Web: saving and exporting become downloads.
        if self.services.download.is_some() && matches!(id, "file.save" | "file.saveAs" | "file.exportPdf" | "file.exportPng") {
            let name = params.get("path").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| self.web_save_name(id));
            let name = if id == "file.exportPdf" && !name.ends_with(".pdf") { format!("{name}.pdf") } else { name };
            // A save with a password encrypts this and later saves (`file.save`'s `password`).
            if matches!(id, "file.save" | "file.saveAs")
                && let Some(pw) = params.get("password").filter(|v| !v.is_null())
            {
                if !wordcraft_engine::io::can_encrypt(&name) || pw.as_str().is_none_or(str::is_empty) {
                    return Err("only Word documents (.docx, .docm, .dotx, .dotm) can be saved with a password, and it can't be empty".into());
                }
                self.session.run("file.encrypt", &json!({"password": pw})).map_err(|e| e.to_string())?;
            }
            // A save is a save, download or not: advance the revision and modified stamp (#262).
            if matches!(id, "file.save" | "file.saveAs") {
                self.session.stamp_save();
            }
            // Word documents are encrypted with the document's password, if it has one.
            let password =
                if matches!(id, "file.save" | "file.saveAs") { self.session.password.as_ref().map(wordcraft_engine::Password::as_str) } else { None };
            let opts = wordcraft_engine::cmd::file::export_options(&self.session, &params);
            let bytes = wordcraft_engine::io::save_bytes_with(&name, &self.session.doc, password, &opts)?;
            // A ticket left over from an earlier call isn't this save's.
            let saves = self.services.async_saves.clone();
            if let Some(saves) = &saves {
                let _ = saves.take_started();
            }
            // An export is a copy, and so is a save in a format that doesn't keep everything (a
            // page image, plain text): the document itself still has unsaved changes.
            let marked_saved = matches!(id, "file.save" | "file.saveAs") && keeps_everything(&name);
            // Save As tells the host page the document is now this file, which it only is when the
            // save marks it saved. A Save under a name the page never sent (an untitled document,
            // an imported .doc saved as .docx) is a new file too, never an overwrite.
            let opened_name = self.session.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string());
            let new_file = id == "file.saveAs" || opened_name.as_deref() != Some(name.as_str());
            let delivered = match (&self.services.host_save, &self.services.download) {
                (Some(host), _) => host(&name, &bytes, marked_saved && new_file),
                (None, Some(download)) => download(&name, &bytes),
                (None, None) => Ok(()),
            };
            if let Err(e) = delivered {
                let msg = i18n::fmt(tl!("Couldn't download {name}: {error}"), &[("name", &name), ("error", &e)]);
                log::error!("download of {name} failed: {e}");
                self.status(msg.clone());
                return Err(msg);
            }
            if marked_saved {
                self.session.dirty = false;
                // The host page keeps the file under this name now, as a desktop save would.
                if self.services.host_save.is_some() {
                    self.session.path = Some(std::path::PathBuf::from(&name));
                }
            }
            // The browser's picker writes the file later (#1315): until it says how that went,
            // the document's save isn't confirmed.
            if let Some(ticket) = saves.as_ref().and_then(async_saves::AsyncSaves::take_started) {
                let document = self.session.document_id();
                self.saves_in_flight.push(SaveInFlight { ticket, name: name.clone(), document, marked_saved });
                return Ok(json!({"downloaded": name, "bytes": bytes.len(), "pendingSave": ticket}));
            }
            return Ok(json!({"downloaded": name, "bytes": bytes.len()}));
        }
        let document = self.session.document_id();
        let body_epoch = self.session.ime_body_epoch();
        let math_session = self.session.math.as_ref().map(|m| m.session);
        let composing_before = !self.canvas.ime_preedit.is_empty();
        let old_path = self.session.path.clone();
        let r = if self.ime_origin {
            self.session.run_ime(id, &params).map_err(|e| e.to_string())
        } else {
            self.session.run(id, &params).map_err(|e| e.to_string())
        };
        if self.session.ime_body_epoch() != body_epoch {
            self.canvas.ime_interrupted_preedit |= composing_before;
            // The body (or the whole document) was replaced under the same id — setText, a
            // version restore, compare, Undo/Redo. A composition belongs to the old content:
            // drop its pre-edit and ask the platform to interrupt it, even when the new bounded
            // window happens to be identical.
            self.canvas.ime_preedit.clear();
            self.canvas.ime_preedit_range = None;
            self.canvas.ime_window_cache = None;
            self.canvas.ime_interrupt_now = true;
        }
        if self.session.math.as_ref().map(|m| m.session) != math_session {
            self.canvas.ime_interrupted_preedit |= composing_before;
            // Entering, leaving or replacing the edited equation changes the IME target's
            // replacement epoch under the same document id. The old composition belongs to a
            // tree this session no longer edits.
            self.canvas.ime_preedit.clear();
            self.canvas.ime_preedit_range = None;
            self.canvas.ime_interrupt_now = true;
        }
        // Copy and Cut put what they took on the system clipboard, whichever control ran them
        // (the ribbon, a context menu, the native menu, or the Cmd+C/X event). Only the key event
        // used to write it, so a Cut button deleted the words without copying them.
        if matches!(id, "edit.copy" | "edit.cut")
            && let Ok(v) = &r
            && let Some(ctx) = &ctx
        {
            // Its HTML too when it has some (an equation as MathML, #816).
            crate::system_clipboard::put_result(self, ctx, v);
        }
        // Another document is shown (not when Open only showed its picker): the window's state
        // for the one it showed is put aside.
        if self.session.document_id() != document {
            // A requested New/Open wins over the asynchronous startup recovery scan.
            // Older copies remain available through File > Recover.
            self.recovery_seen = true;
            self.sync_document();
        }
        // An explicit save to the document's own file lets AutoSave keep writing there.
        if let Ok(v) = &r
            && matches!(id, "file.save" | "file.saveAs")
            && v.get("saved").and_then(Value::as_bool) == Some(true)
            && v.get("path").and_then(Value::as_str).map(std::path::Path::new) == self.session.path.as_deref()
        {
            self.autosave_path = self.session.path.clone();
        }
        // A renamed file is the same file: AutoSave keeps covering it, and Recent drops the old name.
        if r.is_ok()
            && id == "file.rename"
            && let Some(old) = old_path.filter(|old| self.session.path.as_ref() != Some(old))
        {
            if self.autosave_path.as_ref() == Some(&old) {
                self.autosave_path = self.session.path.clone();
            }
            let old = old.to_string_lossy();
            self.ui.recent.retain(|r| *r != old);
        }
        if let Ok(v) = &r {
            self.keep_web_font(id, &params, v);
        }
        self.after_command(id);
        if let Err(e) = &r {
            self.status(e.clone());
        }
        r
    }

    /// Web: store a font added from bytes, or forget the removed ones ([`Services::store_font`]).
    fn keep_web_font(&self, id: &str, params: &Value, result: &Value) {
        match id {
            "font.add" => {
                if let (Some(store), Some(file), Some(data)) =
                    (&self.services.store_font, result.get("file").and_then(Value::as_str), params.get("data").and_then(Value::as_str))
                    && let Some(bytes) = wordcraft_engine::cmd::insert::base64_decode(data)
                {
                    store(file, &bytes);
                }
            }
            "font.remove" => {
                if let Some(forget) = &self.services.forget_font {
                    for f in result.get("removed").and_then(Value::as_array).into_iter().flatten() {
                        if let Some(file) = f.get("file").and_then(Value::as_str) {
                            forget(file);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Fonts were added or removed (a command, the web shell loading stored fonts): lay out
    /// again and refresh the font lists and their previews.
    fn watch_fonts(&mut self, ctx: &egui::Context) {
        let db = wordcraft_fonts::FontDb::global();
        let generation = db.generation();
        if generation == self.font_generation {
            return;
        }
        self.font_generation = generation;
        self.session.relayout();
        self.previews.fonts_changed(ctx, db.user_fonts().iter().flat_map(|f| f.families()));
        ctx.request_repaint();
    }

    /// Answer the Save Changes prompt: Save (through Save As for a new document) and carry on,
    /// carry on without saving, or cancel. A failed or cancelled save cancels too.
    fn answer_save_changes(&mut self, choice: SaveChoice) -> Result<Value, String> {
        let Some(dialogs::Dialog::SaveChanges { then, params, document, .. }) = self.dialog.take() else {
            return Err("no Save Changes prompt is open".into());
        };
        if document != self.session.document_id() {
            return Err("the document the prompt asked about has been replaced".into());
        }
        // Don't Save is the user's say-so to discard the changes, which Close otherwise refuses.
        let mut params = params;
        if choice == SaveChoice::DontSave && then == "file.close" {
            match params.as_object_mut() {
                Some(p) => {
                    p.insert("force".into(), json!(true));
                }
                None => params = json!({"force": true}),
            }
        }
        let go = match choice {
            SaveChoice::Cancel => false,
            SaveChoice::DontSave => true,
            // A new document or an import-only Word file: ask where through Save As. The command
            // runs once that has saved, on a later frame with the non-blocking dialog (#94).
            SaveChoice::Save if (self.session.path.is_none() || self.legacy_word_path()) && self.services.download.is_none() => {
                return match self.save_as(file_dialogs::AfterSave::Continue { then, params, document }) {
                    file_dialogs::Asked::Done(r) => r,
                    file_dialogs::Asked::Pending => Ok(json!({"pending": "saveAs"})),
                    file_dialogs::Asked::Busy => Ok(json!({"done": false})),
                };
            }
            SaveChoice::Save => match self.save_for_prompt() {
                Ok(PromptSave::Saved) => true,
                Ok(PromptSave::NotSaved) => false,
                // The browser's picker finishes later: the command waits for it (#1315).
                Ok(PromptSave::Later(ticket)) => {
                    let rev = self.session.rev();
                    self.awaiting_save = Some(AwaitingSave { ticket, then, params, document, rev });
                    self.status(tl!("Waiting for the save to finish…"));
                    return Ok(json!({"pending": "save"}));
                }
                Err(error) => return Err(self.save_failed(&error, then, params, document)),
            },
        };
        if !go {
            return Ok(json!({"done": false}));
        }
        self.execute_user(&then, params).map(|r| json!({"done": true, "result": r}))
    }

    /// User commands can ask for a supported Save As target or an opening password (#55).
    /// Scripts and agents use [`WordApp::execute`], which never opens these dialogs.
    pub(crate) fn execute_user(&mut self, id: &str, params: Value) -> Result<Value, String> {
        // Legacy Word files are import-only (#2445). Only a user's implicit Save asks for
        // a supported target; scripts and explicitly supplied paths keep their normal errors.
        if id == "file.save"
            && params.get("path").and_then(Value::as_str).is_none()
            && self.has_document()
            && self.legacy_word_path()
            && self.services.download.is_none()
        {
            return match self.save_as(file_dialogs::AfterSave::Nothing) {
                file_dialogs::Asked::Done(r) => r,
                file_dialogs::Asked::Pending => Ok(json!({"pending": "saveAs"})),
                file_dialogs::Asked::Busy => Ok(json!({"saved": false})),
            };
        }
        let retry = (id == "file.open").then(|| params.clone());
        let r = self.execute(id, params);
        if let (Err(e), Some(params)) = (&r, retry)
            && wordcraft_engine::io::needs_password(e)
        {
            let path = params.get("path").and_then(Value::as_str).unwrap_or("");
            let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
            self.dialog = Some(dialogs::Dialog::Password { name, params, password: Default::default(), message: String::new() });
            return Ok(json!({"pending": "password"}));
        }
        r
    }

    /// Save before New/Open/Close (and the mailings that replace the document): true once the
    /// document is safely written. A save in a format that doesn't keep everything (a page
    /// image, plain text) writes a copy and leaves the document unsaved, so it doesn't count:
    /// the command is cancelled and the document stays. A save over the document's own file
    /// (a note.txt that was opened) is a save in any format (#2376). A save that fails (no permission, the
    /// file locked by another app, a missing folder) is the save's error ([`Self::save_failed`]
    /// tells the user).
    /// In the browser the save picker may write the file later ([`PromptSave::Later`], #1315).
    fn save_for_prompt(&mut self) -> Result<PromptSave, String> {
        let v = self.execute("file.save", json!({}))?;
        let saved = v.get("saved").and_then(Value::as_bool) == Some(true) || v.get("downloaded").is_some();
        if !self.safely_saved(saved) {
            return Ok(PromptSave::NotSaved);
        }
        Ok(match v.get("pendingSave").and_then(Value::as_u64) {
            Some(ticket) => PromptSave::Later(ticket),
            None => PromptSave::Saved,
        })
    }

    /// Web: a save of this document is still in the browser's picker (#1315). The document
    /// looks saved, but nothing is written yet, so New/Open ask first.
    fn save_unconfirmed(&self) -> bool {
        let document = self.session.document_id();
        self.saves_in_flight.iter().any(|s| s.marked_saved && s.document == document)
    }

    /// The document's name as the Save Changes prompt shows it: its file's name, else its title.
    fn prompt_name(&self) -> String {
        self.session.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| self.title_stem())
    }

    /// Save (or Save As) from the Save Changes prompt failed (#1445): say so plainly on the status
    /// bar and in the prompt, which comes back for Save again, Don't Save or Cancel, with the
    /// held-back command (`then`) still waiting. Nothing closes and nothing is discarded. Returns
    /// the message, the prompt answer's error.
    pub(crate) fn save_failed(&mut self, error: &str, then: String, params: Value, document: u64) -> String {
        let name = self.prompt_name();
        let message = i18n::fmt(tl!("Couldn't save {name}, so it is still open and nothing was lost: {error}"), &[("name", &name), ("error", error)]);
        log::error!("save from the Save Changes prompt failed: {error}");
        self.prompt_again(message, then, params, document)
    }

    /// The Save Changes prompt comes back with `message` (also on the status bar), the held-back
    /// command still waiting, if the document is still the one it asked about. Returns `message`.
    fn prompt_again(&mut self, message: String, then: String, params: Value, document: u64) -> String {
        self.status(message.clone());
        if document == self.session.document_id() {
            let name = self.prompt_name();
            self.dialog = Some(dialogs::Dialog::SaveChanges { name, then, params, document, message: message.clone() });
        }
        message
    }

    /// Read how the saves that finished later ended (#1315). Called every frame.
    /// A save that didn't write the file leaves the document it was for unsaved again (only that
    /// document: one that has replaced it since is left alone). The command a Save Changes prompt
    /// held back runs once its save is written and closed, if the document is still the same and
    /// unchanged since; a cancelled or failed save brings the prompt back with the reason.
    pub(crate) fn settle_async_saves(&mut self) {
        let Some(saves) = self.services.async_saves.clone() else { return };
        for (ticket, outcome) in saves.take_finished() {
            let current = self.session.document_id();
            if let Some(i) = self.saves_in_flight.iter().position(|s| s.ticket == ticket) {
                let save = self.saves_in_flight.swap_remove(i);
                match &outcome {
                    async_saves::SaveOutcome::Saved => log::info!("saved {} through the save picker", save.name),
                    async_saves::SaveOutcome::Cancelled | async_saves::SaveOutcome::Failed(_) => {
                        if save.marked_saved && save.document == current {
                            self.session.dirty = true;
                        } else if save.marked_saved {
                            // Another document is shown: the one saved is unsaved again where it waits.
                            let _ = self.session.with_document(save.document, |s| s.dirty = true);
                        }
                    }
                }
                if let async_saves::SaveOutcome::Failed(error) = &outcome {
                    let msg = i18n::fmt(tl!("Couldn't download {name}: {error}"), &[("name", &save.name), ("error", error)]);
                    log::error!("the save picker couldn't save {}: {error}", save.name);
                    self.status(msg);
                }
            }
            let Some(wait) = self.awaiting_save.take_if(|w| w.ticket == ticket) else { continue };
            if wait.document != current {
                log::info!("the document a held-back {} was for has been replaced; dropped", wait.then);
                continue;
            }
            match outcome {
                // Only when nothing changed since and no later save of it is still unconfirmed.
                async_saves::SaveOutcome::Saved if !self.session.dirty && self.session.rev() == wait.rev && !self.save_unconfirmed() => {
                    if let Err(e) = self.execute_user(&wait.then, wait.params) {
                        self.status(e);
                    }
                }
                // Edited while the picker was open (those edits may not be written yet): ask again.
                async_saves::SaveOutcome::Saved => {
                    let name = self.prompt_name();
                    self.dialog = Some(dialogs::Dialog::SaveChanges {
                        name,
                        then: wait.then,
                        params: wait.params,
                        document: wait.document,
                        message: String::new(),
                    });
                }
                async_saves::SaveOutcome::Cancelled => {
                    let name = self.prompt_name();
                    let message = i18n::fmt(tl!("Saving {name} was cancelled, so it is still open and nothing was lost."), &[("name", &name)]);
                    self.prompt_again(message, wait.then, wait.params, wait.document);
                }
                async_saves::SaveOutcome::Failed(error) => {
                    self.save_failed(&error, wait.then, wait.params, wait.document);
                }
            }
        }
    }

    /// Whether a save before New/Open/Close counts: it happened, and kept everything.
    fn safely_saved(&mut self, saved: bool) -> bool {
        if saved && self.session.dirty {
            self.status(tl!("That format doesn't keep everything, so the document is still open. Save it as a Word document (.docx) to go on."));
            return false;
        }
        saved
    }

    /// The window's close button (or the system) asked to quit. Returns true to let the window
    /// close; with unsaved changes it asks first and returns false.
    pub fn close_requested(&mut self) -> bool {
        if self.printing.busy() {
            self.status(tl!("Wait for the print job to finish submitting before closing."));
            return false;
        }
        if self.quit_requested {
            return true;
        }
        // Nothing to save, also with no document open in this window (#1129) and none elsewhere.
        if !self.has_unsaved_documents() {
            return true;
        }
        // A comment draft the document refuses is an unsaved change: the prompt can drop it.
        if self.has_document() {
            let _ = panes::flush_comment_drafts(self);
        }
        if !self.has_unsaved_documents() {
            return true;
        }
        // Each document with unsaved work is shown and asked about in turn; Cancel stops.
        self.ask_before_quit();
        self.quit_requested
    }

    /// File › Close (Mod+W, Mod+F4) closes the shown document, or only its window while other
    /// windows show the document (View › New Window), asking nothing then. Its window closes too while other
    /// windows are open, and the one focused before it comes forward (in the web app's one window,
    /// the next document shows). The last window stays, as Word's does (#1712), with no page, as
    /// after Esc on the Start screen (#1129), and File › New or Open goes on from there. `{"quit": true}`,
    /// from the window's close button, goes on quitting: the shown document's prompt has been
    /// answered, so it closes and the next document with unsaved changes is asked about
    /// ([`WordApp::ask_before_quit`]). With no document open there is nothing to close. Unsaved
    /// changes are never discarded unasked: a dirty document closes only with `{"force": true}`
    /// (Don't Save in the prompt passes it).
    fn close_document(&mut self, params: &Value) -> Result<Value, String> {
        if self.closes_only_its_window("file.close", params) && self.close_window_only() {
            return Ok(json!({"closed": true, "window": true}));
        }
        if let Some(e) = control::close_refused(self, "file.close", params) {
            return Err(e.into());
        }
        let last = self.session.documents().len() <= 1;
        if params.get("quit").and_then(Value::as_bool) == Some(true) {
            if last {
                // Don't Save: the document stays open until the app exits, but its changes,
                // and so its recovery copy, are discarded.
                if params.get("force").and_then(Value::as_bool) == Some(true) {
                    self.session.discard_recovery_copy();
                }
                self.quit_requested = true;
                return Ok(json!({"quit": true}));
            }
            self.close_shown_document()?;
            return Ok(self.ask_before_quit());
        }
        if !self.has_document() {
            return Ok(json!({"closed": false}));
        }
        self.close_shown_document()?;
        self.recovery_seen = true;
        if last {
            self.no_document = Some(self.session.document_id());
            self.session.release_untitled_name();
        }
        // The window that stays leaves the File page; another window that came forward keeps
        // its own.
        if last || !self.windows.native() {
            self.ui.backstage = false;
        }
        Ok(json!({"closed": true}))
    }

    fn after_command(&mut self, id: &str) {
        options::sync(self);
        self.canvas.caret_visible_since = now_ms();
        if id == "view.outline" {
            self.ui.nav_tab = "headings".into();
        }
        if matches!(id, "view.readColumnWidth" | "view.readTextSize" | "view.readLineSpacing") {
            self.canvas.read_force_reflow = true;
        }
        if id == "view.readMode" {
            self.canvas.read_first_visible = None;
        }
        // Drawing leaves the caret where it was: following it would pull the view off the ink.
        // A mouse drag on a table (`run_keep_view`) does the same (#1738). A caret in an empty
        // story (a footer just made) also spans the whole story: it isn't a selection of it, and
        // the view follows it. Saving, exporting, printing, the document's properties and File ›
        // Options move neither the caret nor the text, so the view stays where the user is
        // reading (#3864); AutoSave runs its save on the session and never gets here.
        let file_keeps_view = matches!(
            id,
            "file.save"
                | "file.saveAs"
                | "file.saveTemplate"
                | "file.exportPdf"
                | "file.exportPng"
                | "file.printPdf"
                | "file.print"
                | "file.printMarkup"
                | "file.properties"
                | "file.info"
                | "file.rename"
                | "file.privacy"
                | "file.privacyOptions"
                | "file.setAuthor"
                | "file.compatibility"
                | "file.encrypt"
                | "file.share"
                | "file.options"
        );
        if (self.session.selects_whole_story() && !self.session.sel.is_collapsed()) || id == "select.all" || self.canvas.keep_view {
            self.canvas.scroll_to_caret = false;
        } else if !file_keeps_view && !["ui.", "view.", "document.", "format.state", "draw."].iter().any(|p| id.starts_with(p)) {
            self.canvas.scroll_to_caret = true;
        }
        for req in std::mem::take(&mut self.session.ui_requests) {
            self.handle_request(&req);
        }
        if let Some(p) = self.session.path.as_ref().map(|p| p.to_string_lossy().to_string())
            && self.ui.recent.first() != Some(&p)
        {
            self.ui.recent.retain(|r| *r != p);
            self.ui.recent.insert(0, p);
            self.ui.recent.truncate(MAX_RECENT);
        }
        if !self.session.status.is_empty() {
            // Engine messages are English; the catalog translates the ones it knows.
            let s = std::mem::take(&mut self.session.status);
            let s = tl!(&s).to_string();
            self.status(s);
        }
        if matches!(id, "table.deleteTable")
            || (self.canvas.table_tool == Some(crate::table_pen::TableTool::Erase) && self.session.sel.focus.path.cell().is_none())
        {
            crate::table_pen::stop(self);
        }
    }

    /// Requests commands make of the UI (open a dialog…).
    pub(crate) fn handle_request(&mut self, req: &Value) {
        // Made for a document that isn't shown (an agent's command run there): its window handles
        // it when it draws.
        if let Some(document) = req.get("document").and_then(Value::as_u64)
            && document != self.session.document_id()
        {
            let app_wide = req.get("interfaceTheme").is_some() || req.get("toggle").and_then(Value::as_str) == Some("interfaceTheme");
            if app_wide || req.get("close").is_some() || !self.queue_for_window(document, req) {
                self.handle_request_for(document, req);
            }
            return;
        }
        // Ctrl+F: type straight into the Navigation pane's search box (#712).
        if req.get("focus").and_then(Value::as_str) == Some("navSearch") {
            self.nav_search_focus = true;
            self.ui.nav_tab = "results".into();
        }
        // #1861: the registered Interface Theme command drives the same setting as Options.
        if let Some(name) = req.get("interfaceTheme").and_then(Value::as_str)
            && let Some(a) = theme::Appearance::from_code(name)
        {
            self.ui.theme = a;
        }
        if req.get("toggle").and_then(Value::as_str) == Some("interfaceTheme") {
            self.ui.theme = if self.ui_is_dark() { theme::Appearance::Light } else { theme::Appearance::Dark };
        }
        if let Some(what) = req.get("open").and_then(Value::as_str) {
            match what {
                "openFile" => self.open_dialog(),
                "saveAs" => {
                    self.save_as_dialog();
                }
                "exportPdf" => self.export_dialog("pdf"),
                "exportPng" => self.export_dialog("png"),
                "insertPicture" => self.pick_picture(),
                "addFont" => self.pick_font(),
                "loadSystemFonts" => self.load_system_fonts(),
                "print" => {
                    self.ui.backstage = true;
                    self.ui.backstage_page = "print".into();
                }
                "options" => {
                    self.ui.backstage = true;
                    self.ui.backstage_page = "options".into();
                    if let Some(pane) = req.get("pane").and_then(Value::as_str) {
                        self.ui.options_pane = pane.into();
                    }
                }
                "pasteSpecial" => self.dialog = Some(dialogs::Dialog::paste_special(self, req)),
                "newWindow" => self.new_window(),
                other => self.dialog = dialogs::Dialog::open(other, self),
            }
        }
        if req.get("close").is_some() {
            self.quit_requested = true;
        }
    }

    /// A request made for document `document`, which isn't shown and which no other window
    /// handles: the interface theme is the app's and changes; Close closes that document if it
    /// has nothing unsaved; the rest (dialogs, panes, focus, scrolling) are about a window's page
    /// and are dropped, as no window shows the document.
    fn handle_request_for(&mut self, document: u64, req: &Value) {
        let theme_only = req.get("interfaceTheme").is_some() || req.get("toggle").and_then(Value::as_str) == Some("interfaceTheme");
        if theme_only {
            let mut req = req.clone();
            if let Some(o) = req.as_object_mut() {
                o.remove("document");
            }
            self.handle_request(&req);
            return;
        }
        if req.get("close").is_some() {
            let dirty = self.session.documents().iter().any(|d| d.id == document && d.dirty);
            if dirty {
                log::warn!("not closing document {document}: it has unsaved changes");
            } else if let Err(e) = self.session.close_document(document) {
                log::warn!("closing document {document}: {e}");
            }
            self.sync_document();
            return;
        }
        log::warn!("dropped a request for document {document}, which no window shows: {req}");
    }

    /// Follow the desktop's light/dark choice while the app runs (Linux, where winit reports no
    /// system theme; see [`desktop_theme`]). The desktop app calls this once at start-up; unit
    /// tests and `ui_shot` don't, so what they draw doesn't depend on the host's desktop theme.
    pub fn watch_desktop_theme(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        self.desktop_theme = desktop_theme::DesktopTheme::start(move || ctx.request_repaint());
    }

    /// Whether the interface theme setting currently resolves to dark (`System` asks the OS).
    pub fn ui_is_dark(&self) -> bool {
        self.ui.theme.is_dark(self.ctx.as_ref().and_then(|ctx| system_theme(ctx, self.desktop_dark)))
    }

    /// Whether animation effects are on: the Animation effects setting, where `System` follows
    /// what the system last reported (on when it reports nothing).
    pub fn animations_on(&self) -> bool {
        self.ui.animations.enabled(self.system_animations)
    }

    /// What the system last said about its animation effects (`None`: it doesn't say).
    pub fn system_animations(&self) -> Option<bool> {
        self.system_animations
    }

    /// Ask the host for the system's animation setting.
    fn read_system_animations(&mut self) {
        self.system_animations = self.services.animations_enabled.as_ref().and_then(|ask| ask());
    }

    /// Commands that live in the UI layer.
    fn ui_command(&mut self, id: &str, p: &Value) -> Option<Result<Value, String>> {
        let s = |k: &str| p.get(k).and_then(Value::as_str);
        Some(Ok(match id {
            "ui.checkForUpdates" | "help.checkForUpdates" => {
                crate::updates_ui::check(self);
                Value::Null
            }
            "ui.tab" => {
                let tab = s("tab").unwrap_or("Home").to_string();
                if tab == "File" {
                    self.ui.backstage = true;
                } else {
                    self.ui.tab = tab;
                    self.ui.backstage = false;
                }
                json!({"tab": self.ui.tab})
            }
            "ui.backstage" => {
                self.ui.backstage = p.get("value").and_then(Value::as_bool).unwrap_or(!self.ui.backstage);
                if let Some(pg) = s("page") {
                    self.ui.backstage_page = pg.into();
                }
                if let Some(pane) = s("pane") {
                    self.ui.options_pane = pane.into();
                }
                json!({"backstage": self.ui.backstage})
            }
            "ui.dialog" if s("name") == Some("colors") => {
                // The font colour's More Colors… (#606); `tab`: standard or custom.
                let target = colors::Target::cmd("format.color").remember(colors::Remember::FontColor);
                let current =
                    self.session.run("format.state", &json!({})).ok().and_then(|st| st.get("color").and_then(Value::as_str).and_then(colors::parse));
                let mut d = colors::ColorsDialog::new(current, target);
                if s("tab") == Some("custom") {
                    d.tab = colors::ColorsTab::Custom;
                }
                self.colors = Some(d);
                json!({"dialog": "colors"})
            }
            "ui.dialog" => {
                self.dialog = dialogs::Dialog::open(s("name").unwrap_or(""), self);
                // New / Modify Table Style can open on a part (`"part": "firstRow"`…).
                if let Some(dialogs::Dialog::TableStyle { region, .. }) = &mut self.dialog
                    && let Some(part) = s("part").and_then(wordcraft_engine::cmd::table_style::part_named)
                {
                    *region = dialogs_table_style::part_index(part);
                }
                json!({"dialog": self.dialog.as_ref().map(|d| d.name())})
            }
            "ui.closeDialog" => {
                self.dialog = None;
                self.colors = None;
                json!({})
            }
            "ui.changePicture" => {
                let Some(media) = self.selected_picture_media() else {
                    return Some(Err("select a picture first".into()));
                };
                self.change_picture_target = Some(media);
                self.pick_picture();
                json!({"pending": self.change_picture_target.is_some()})
            }
            // The page's context menu at the caret, as Shift+F10 or the Menu key opens it (#821).
            "ui.contextMenu" => {
                self.canvas.context_menu_from_keys = true;
                self.canvas.want_focus = true;
                json!({"pending": "contextMenu"})
            }
            "ui.paneDock" => return Some(panes::dock_command(self, p)),
            "ui.collapseRibbon" => {
                self.ui.ribbon_collapsed = !self.ui.ribbon_collapsed;
                json!({"collapsed": self.ui.ribbon_collapsed})
            }
            "ui.focusRibbon" => {
                // Focus's "⋯" (#1678): `value` true drops the ribbon down over the page, false
                // puts it away; no value toggles it. Outside Focus there is no ribbon to drop:
                // it stays put away.
                let focus = chrome::Screen::of(&self.session.view) == chrome::Screen::Focus;
                match p.get("value") {
                    None => self.focus_ribbon = focus && !self.focus_ribbon,
                    Some(Value::Bool(on)) => self.focus_ribbon = focus && *on,
                    Some(_) => return Some(Err("`value` must be true or false".into())),
                }
                json!({"focusRibbon": self.focus_ribbon})
            }
            "ui.quickAccessToolbar" => {
                // Hide Quick Access Toolbar (its menu) and Show Quick Access Toolbar (the ribbon's
                // right-click menu, Customize Ribbon), #1945: `value` true shows the toolbar in
                // the title bar, false hides it; no value toggles it.
                match p.get("value") {
                    None => self.ui.quick_access_toolbar = !self.ui.quick_access_toolbar,
                    Some(Value::Bool(on)) => self.ui.quick_access_toolbar = *on,
                    Some(_) => return Some(Err("`value` must be true or false".into())),
                }
                json!({"quickAccessToolbar": self.ui.quick_access_toolbar})
            }
            "ui.keytips" => {
                if let Some(v) = p.get("value") {
                    let Some(enabled) = v.as_bool() else { return Some(Err("`value` must be a boolean".into())) };
                    self.ui.keytips_enabled = enabled;
                    if !enabled {
                        self.ui.keytips = keytips::Phase::Off;
                        self.ui.keytip_prefix.clear();
                    }
                }
                json!({"enabled": self.ui.keytips_enabled})
            }
            "ui.dark" => {
                // A manual Light/Dark switch (toggles what is shown when no value is given).
                let dark = p.get("value").and_then(Value::as_bool).unwrap_or(!self.ui_is_dark());
                self.ui.theme = if dark { theme::Appearance::Dark } else { theme::Appearance::Light };
                json!({"dark": dark, "theme": self.ui.theme.code()})
            }
            "ui.theme" => {
                // `light`, `dark` or `system` (follow the OS appearance); no value reads it.
                if let Some(v) = s("value") {
                    match theme::Appearance::from_code(v) {
                        Some(a) => self.ui.theme = a,
                        None => return Some(Err(format!("unknown theme {v:?}; use light, dark or system"))),
                    }
                }
                json!({"theme": self.ui.theme.code(), "dark": self.ui_is_dark()})
            }
            "ui.interfaceSize" => {
                // `auto` (the display scale the system reports) or an absolute size in percent,
                // 50–300 (clamped); no value reads it. `percent` in the result is the size the
                // interface is drawn at. Document zoom is `view.zoom` and stays as it is.
                if let Some(v) = p.get("percent").or_else(|| p.get("value")) {
                    let size = match v.as_str() {
                        Some(a) if a.trim().eq_ignore_ascii_case("auto") => Some(theme::InterfaceSize::AUTO),
                        Some(t) => t.trim().trim_end_matches('%').trim().parse::<f64>().ok().and_then(theme::InterfaceSize::from_percent),
                        None => v.as_f64().and_then(theme::InterfaceSize::from_percent),
                    };
                    match size {
                        Some(size) => {
                            self.ui.interface_size = size;
                            self.legacy_interface_size = None;
                        }
                        None => {
                            return Some(Err(format!(
                                "`percent` must be \"auto\" or a number from {} to {}",
                                theme::InterfaceSize::MIN,
                                theme::InterfaceSize::MAX
                            )));
                        }
                    }
                }
                let size = self.ui.interface_size;
                json!({"auto": size.is_auto(), "percent": size.drawn_percent(self.native_scale)})
            }
            "ui.animations" => {
                // `system` (follow the system's animation setting), `on` or `off`; no value reads
                // it. `enabled` is what that comes to now, `system` what the system reports.
                if let Some(v) = s("value") {
                    match theme::Animations::from_code(v) {
                        Some(a) => self.ui.animations = a,
                        None => return Some(Err(format!("unknown animations setting {v:?}; use system, on or off"))),
                    }
                }
                self.read_system_animations();
                json!({"animations": self.ui.animations.code(), "enabled": self.animations_on(), "system": self.system_animations})
            }
            "ui.language" => {
                // `auto` (follow the system) or a language code; anything else is an error.
                if let Some(v) = s("value") {
                    match i18n::normalize_pref(v) {
                        Some(code) => self.ui.language = code.to_string(),
                        None => {
                            let codes: Vec<&str> = i18n::Lang::all().map(i18n::Lang::code).collect();
                            return Some(Err(format!("unknown language `{v}`; use auto or one of {}", codes.join(", "))));
                        }
                    }
                }
                let lang = i18n::Lang::from_pref(&self.ui.language);
                json!({"language": self.ui.language, "effective": lang.code(), "available": i18n::Lang::all().map(|l| json!({"code": l.code(), "name": l.name()})).collect::<Vec<_>>()})
            }
            "ui.recoveryDiscard" => {
                if let Some(answer) = s("answer") {
                    let Some(dialogs::Dialog::DiscardRecovery { id, .. }) = self.dialog.clone() else {
                        return Some(Err("no recovery discard is pending".into()));
                    };
                    match answer {
                        "cancel" => {
                            self.dialog = None;
                            json!({"discarded": false})
                        }
                        "discard" => {
                            let result = self.execute("file.recover", json!({"discard": id}));
                            if result.is_ok() {
                                self.dialog = None;
                            }
                            return Some(result);
                        }
                        _ => return Some(Err("`answer` must be discard or cancel".into())),
                    }
                } else {
                    let Some(id) = s("id") else {
                        return Some(Err("`id` is required".into()));
                    };
                    let Some(info) = self.session.recovery_documents().into_iter().find(|d| d.id == id) else {
                        return Some(Err("no such recovery copy".into()));
                    };
                    self.dialog = Some(dialogs::Dialog::DiscardRecovery { id: info.id, name: info.name });
                    json!({"pending": "discardRecovery"})
                }
            }
            "ui.saveChanges" => {
                let Some(choice) = s("answer").and_then(SaveChoice::parse) else {
                    return Some(Err("`answer` must be save, dontSave or cancel".into()));
                };
                return Some(self.answer_save_changes(choice));
            }
            "ui.openFileDialog" => {
                self.open_dialog();
                json!({})
            }
            // File › Save As › Save as Word Template (#2852): the Save dialog for a .dotx, in the
            // user templates folder. Opens system UI, like `ui.openFileDialog`.
            "ui.saveAsTemplate" => {
                let dir = self.save_as_template_dialog();
                json!({"dir": dir})
            }
            // The built-in file browser (#576), for when the system's file dialog doesn't show up
            // (desktop only): `purpose` is document (default), picture, recipients or save. A system
            // dialog still waiting for an answer is given up as cancelled.
            #[cfg(not(target_arch = "wasm32"))]
            "ui.fileBrowser" => {
                self.browse_builtin(s("purpose").unwrap_or("document"));
                json!({"open": self.file_browser.open.is_some()})
            }
            // Mailings › Select Recipients › Use an Existing List…: pick a CSV/TSV/text file and
            // load it as the mail-merge recipients. Opens system UI, like `ui.openFileDialog`.
            "ui.openRecipientList" => {
                self.pick_recipient_list();
                json!({"pending": self.recipient_list_pending})
            }
            // Native printing submits the selected queue on a worker; the browser owns its dialog.
            "ui.printSettings" => return Some(printing::settings(&mut self.printing, p, self.services.printers.as_ref())),
            "ui.print" | "ui.printSubmit" => return Some(self.print_to_system(p)),
            // File › Options › General › Make WordCraft the default… changes the user's system
            // default apps (Linux) or opens system settings (Windows), so only a click on that
            // button does it ([`WordApp::make_default_app`]); scripts, the control channel and
            // MCP get an error.
            "ui.makeDefaultApp" => {
                return Some(Err(
                    "making WordCraft the default app needs a click in File › Options › General; scripts and agents can't change system defaults"
                        .into(),
                ));
            }
            "ui.discord" => {
                self.canvas.open_url = Some("https://discord.gg/artcraft".into());
                json!({})
            }
            "ui.caretAnimation" => {
                // File › Options › Advanced › Animation: `value` true or false turns the caret's
                // glide and fading blink on or off; no value reads it.
                match p.get("value") {
                    None => {}
                    Some(Value::Bool(on)) => self.ui.caret_animation = *on,
                    Some(_) => return Some(Err("`value` must be true or false".into())),
                }
                json!({"caretAnimation": self.ui.caret_animation})
            }
            "ui.alignmentGuides" => {
                // File › Options › Advanced › Editing: `value` true or false turns snapping to
                // alignment guides while moving pictures and shapes on or off; no value reads it.
                match p.get("value") {
                    None => {}
                    Some(Value::Bool(on)) => self.ui.alignment_guides = *on,
                    Some(_) => return Some(Err("`value` must be true or false".into())),
                }
                json!({"alignmentGuides": self.ui.alignment_guides})
            }
            "ui.startScreen" => {
                // File › Options › General › Start-up (#766): `value` true or false turns showing
                // File › Home at start-up on or off; no value reads it.
                match p.get("value") {
                    None => {}
                    Some(Value::Bool(on)) => self.ui.start_screen = *on,
                    Some(_) => return Some(Err("`value` must be true or false".into())),
                }
                json!({"startScreen": self.ui.start_screen})
            }
            // Shape Format › Text › Create Link: the next click on an empty text box links the
            // selected box to it (`shape.link` with that box as the target); Esc cancels.
            // Picture Format › Size › Crop: crop mode on the selected picture, or off (`crop`).
            "ui.crop" => match crop::toggle(self) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            },
            "ui.linkTextBox" => {
                let from = wordcraft_engine::cmd::objects::text_box_target(&self.session).filter(|_| widgets::enabled(self, "shape.link"));
                let Some((pos, _)) = from else { return Some(Err("select a text box that isn't linked yet first".into())) };
                self.canvas.link_from = Some(pos);
                self.status(tl!("Click the empty text box the text should continue in (Esc cancels)"));
                json!({"pending": "link"})
            }
            _ => return None,
        }))
    }

    /// Submit the selected print job; the implementation lives beside its localized controls.
    fn print_to_system(&mut self, p: &Value) -> Result<Value, String> {
        self.print_document = Some(self.session.document_id());
        printing::submit(self, p)
    }

    /// File › Options › General › Make WordCraft the default…: hand over to the host's
    /// [`Services::make_default_app`] and say in the status bar what happened. An error when the
    /// host has no hook (web, macOS). Only the button calls this: it is deliberately not a
    /// command, so nothing programmatic (control channel, MCP, macros) can change system defaults.
    pub(crate) fn make_default_app(&mut self) -> Result<Value, String> {
        let Some(hook) = self.services.make_default_app.as_ref() else {
            return Err("WordCraft can't change the default apps here; use your system's settings".into());
        };
        match hook() {
            Ok(DefaultApp::SettingsOpened) => {
                self.status(tl!("In Default apps, choose WordCraft for the document types you want it to open."));
                Ok(json!({"settingsOpened": true}))
            }
            Ok(DefaultApp::MadeDefault) => {
                self.status(tl!("WordCraft is now the default app for Word documents."));
                Ok(json!({"default": true}))
            }
            Err(e) => {
                log::warn!("make default app: {e}");
                let msg = i18n::fmt(tl!("Couldn't make WordCraft the default: {error}"), &[("error", &e)]);
                self.status(msg.clone());
                Err(msg)
            }
        }
    }

    pub fn status(&mut self, s: impl Into<String>) {
        self.status_msg = Some((s.into(), now_ms()));
    }

    /// Media key of the selected picture, if any.
    pub(crate) fn selected_picture_media(&self) -> Option<String> {
        match wordcraft_engine::cmd::objects::selected(&self.session) {
            Some((_, wordcraft_doc::para::InlineObject::Image { media, .. })) => Some(media),
            _ => None,
        }
    }

    /// Inserting a picture selects it, and the next insert would replace it. When several images
    /// arrive in one drop, put the caret after the picture placed by the previous file (#1592).
    fn caret_after_placed_picture(&mut self, placed: bool) {
        if placed {
            self.session.sel.anchor = self.session.sel.focus.clone();
        }
    }

    /// Whether `r` placed a picture in the text (and selected it). One taken into a selected
    /// Drawing Canvas leaves the canvas selected, so the next file of the drop joins it too.
    fn placed_in_text(r: &Result<Value, String>) -> bool {
        r.as_ref().is_ok_and(|v| v.get("canvasMember").is_none())
    }

    /// A picked image replaces the pending Change Picture target, else it is inserted.
    fn insert_or_change_picture(&mut self, params: Value) -> Result<Value, String> {
        let target = std::mem::take(&mut self.change_picture_target);
        match (target, self.selected_picture_media()) {
            (Some(t), Some(m)) if t == m => self.run("picture.change", params),
            _ => self.run("insert.picture", params),
        }
    }

    /// Drop a pending Change Picture once the selection moved to another picture.
    fn clear_stale_change_picture(&mut self) {
        let stale = match (&self.change_picture_target, self.selected_picture_media()) {
            (Some(t), Some(m)) => t != &m,
            (Some(_), None) => true,
            _ => false,
        };
        if stale {
            self.change_picture_target = None;
        }
    }

    /// Pick a recipient list (desktop: through the file dialog hook; web: it arrives through the
    /// inbox).
    fn pick_recipient_list(&mut self) {
        if let Some(f) = &self.services.open_async {
            f("recipients");
            self.recipient_list_pending = true;
            return;
        }
        let _ = self.ask_file(file_dialogs::FileDialogRequest::Open { purpose: "recipients".into() }, file_dialogs::AfterPick::Recipients);
    }

    /// Load mail-merge recipients and say how many there are. A workbook with several sheets
    /// shows Select Table to choose one first (#334).
    pub(crate) fn load_recipients(&mut self, params: Value) -> Result<Value, String> {
        let r = self.run("mailings.recipients", params.clone());
        if let Ok(v) = &r
            && v.get("chooseSheet").and_then(Value::as_bool) == Some(true)
        {
            let sheets = v
                .get("sheets")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            self.dialog = Some(dialogs::Dialog::SelectTable { sheets, selected: 0, headers: true, message: String::new(), params });
        } else if let Ok(v) = &r {
            let records = v.get("records").and_then(Value::as_u64).unwrap_or(0).to_string();
            self.status(i18n::fmt(tl!("Recipients: {count}"), &[("count", &records)]));
        }
        r
    }

    /// These paths have readers but no writer, so an interactive save needs another format.
    fn legacy_word_path(&self) -> bool {
        self.session
            .path
            .as_ref()
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("doc") || e.eq_ignore_ascii_case("dot"))
    }

    /// The name a browser Save or Export without a name uses. An export is named after the
    /// document (`report.pdf`, not `report.docx.pdf`). A host page that keeps the files
    /// ([`Services::host_save`]) gets Save back under the opened file's own name and format, as
    /// the desktop app writes it, except an import-only Word 97–2003 file (#2445).
    fn web_save_name(&self, id: &str) -> String {
        match id {
            "file.exportPdf" => return format!("{}.pdf", self.title_stem()),
            "file.exportPng" => return format!("{}.png", self.title_stem()),
            _ => {}
        }
        if id == "file.save"
            && self.services.host_save.is_some()
            && !self.legacy_word_path()
            && let Some(name) = self.session.path.as_ref().and_then(|p| p.file_name())
        {
            return name.to_string_lossy().to_string();
        }
        self.default_save_name()
    }

    /// The name Save suggests: the open file's own Word format (so a .docm keeps its macros),
    /// otherwise .docx.
    fn default_save_name(&self) -> String {
        let ext = self.session.path.as_ref().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_string());
        match ext {
            Some(e) if wordcraft_engine::io::is_word_package(&e) => format!("{}.{e}", self.title_stem()),
            _ => format!("{}.docx", self.title_stem()),
        }
    }

    /// The window caption is painted with the semibold interface face (Inter). A file name in a
    /// script those faces lack would otherwise be drawn as `?`. Borrow the document fonts'
    /// fallback faces, as IME composition does (#565). A CJK name uses the CJK UI font instead:
    /// those collections are too large to add here.
    fn ensure_caption_fonts(&mut self) {
        let stem = self.title_stem();
        if stem.chars().any(wordcraft_fonts::is_cjk) {
            self.want_system_cjk = true;
        }
        let new = theme::faces_for_preedit(&stem, &self.canvas.ime_faces, theme::caption_char_covered, |c| {
            wordcraft_fonts::FontDb::global().fallback_for(c, u32::MAX)
        });
        if !new.is_empty() {
            self.canvas.ime_faces.extend(new);
            // Fonts are already installed for this frame's language; load the new faces too.
            self.styled = false;
        }
    }

    /// Comment balloons and the Comments pane draw comment text and author names with the
    /// interface faces, which have no Arabic, Hebrew, Thai …: each such letter was drawn as a box.
    /// Borrow the document fonts' fallback faces for them, as the caption does. Looked at once per
    /// document revision; CJK text uses the CJK UI font instead.
    fn ensure_comment_fonts(&mut self) {
        let key = (self.session.document_id(), self.session.rev());
        if self.comment_fonts_for == Some(key) {
            return;
        }
        self.comment_fonts_for = Some(key);
        let doc = &self.session.doc;
        let mut chars = std::collections::BTreeSet::new();
        for c in doc.comments.values() {
            chars.extend(c.author.chars());
            chars.extend(doc.plain_text(wordcraft_doc::StoryRef::Part(c.part)).chars());
        }
        if chars.iter().any(|c| wordcraft_fonts::is_cjk(*c)) {
            self.want_system_cjk = true;
        }
        let text: String = chars.into_iter().collect();
        let new = theme::faces_for_preedit(&text, &self.canvas.ime_faces, theme::caption_char_covered, |c| {
            wordcraft_fonts::FontDb::global().fallback_for(c, u32::MAX)
        });
        if !new.is_empty() {
            self.canvas.ime_faces.extend(new);
            self.styled = false;
        }
    }

    /// File › Recover draws the recovered documents' names with the interface faces too: a
    /// Chinese name was a row of boxes, an Arabic or Hebrew one likewise. The same treatment as
    /// the caption (and as the Recent list, `ensure_recent_fonts`): CJK loads the CJK UI font,
    /// other scripts borrow the document fallback faces. Looked at again only when the names change.
    fn ensure_recover_fonts(&mut self) {
        use std::hash::{Hash, Hasher};
        let docs = self.session.recovery_documents();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        docs.iter().for_each(|d| d.name.hash(&mut hasher));
        let key = hasher.finish();
        if self.recover_fonts_for == Some(key) {
            return;
        }
        self.recover_fonts_for = Some(key);
        let chars: std::collections::BTreeSet<char> = docs.iter().flat_map(|d| d.name.chars()).collect();
        if chars.iter().any(|c| wordcraft_fonts::is_cjk(*c)) {
            self.want_system_cjk = true;
        }
        let text: String = chars.into_iter().collect();
        let new = theme::faces_for_preedit(&text, &self.canvas.ime_faces, theme::caption_char_covered, |c| {
            wordcraft_fonts::FontDb::global().fallback_for(c, u32::MAX)
        });
        if !new.is_empty() {
            self.canvas.ime_faces.extend(new);
            self.styled = false;
        }
    }

    /// Recent documents in Backstage (and the Start screen) show file names and paths with the
    /// interface faces. Borrow document fallback faces for characters in non-Latin scripts (#2067),
    /// as the caption and comments do.
    fn ensure_recent_fonts(&mut self) {
        if self.recent_fonts_for.as_slice() == self.ui.recent.as_slice() {
            return;
        }
        self.recent_fonts_for = self.ui.recent.clone();
        for p in &self.ui.recent {
            if p.chars().any(wordcraft_fonts::is_cjk) {
                self.want_system_cjk = true;
            }
            let new = theme::faces_for_preedit(p, &self.canvas.ime_faces, theme::caption_char_covered, |c| {
                wordcraft_fonts::FontDb::global().fallback_for(c, u32::MAX)
            });
            if !new.is_empty() {
                for face in new {
                    if !self.canvas.ime_faces.iter().any(|f| f.id() == face.id()) {
                        self.canvas.ime_faces.push(face);
                    }
                }
                self.styled = false;
            }
        }
    }

    /// The Find box and the Navigation pane's Results list draw the typed query and document
    /// excerpts around each match with the interface faces, which have no Georgian, Armenian,
    /// Thai … (#3248): each such letter was drawn as a box. Borrow the document fonts' fallback
    /// faces for them, as comments and the caption do. Looked at once per query/result-count/
    /// revision change (an edit can change a result's surrounding text — adding Georgian beside
    /// an existing Latin match — without changing the query or how many matches there are, #3257
    /// review); CJK text uses the CJK UI font instead.
    fn ensure_find_fonts(&mut self) {
        let key = (self.session.find.query.clone(), self.session.find.results.len(), self.session.rev());
        if self.find_fonts_for.as_ref() == Some(&key) {
            return;
        }
        self.find_fonts_for = Some(key);
        let mut text = self.session.find.query.clone();
        for (a, b) in self.session.find.results.iter().take(500) {
            let Some(p) = self.session.doc.para_at(a) else { continue };
            let from = p.clamp(a.off.saturating_sub(30));
            let to = p.clamp((b.off + 40).min(p.len()));
            if let Some(s) = p.text.get(from..to) {
                text.push_str(s);
            }
        }
        if text.chars().any(wordcraft_fonts::is_cjk) {
            self.want_system_cjk = true;
        }
        let new = theme::faces_for_preedit(&text, &self.canvas.ime_faces, theme::caption_char_covered, |c| {
            wordcraft_fonts::FontDb::global().fallback_for(c, u32::MAX)
        });
        if !new.is_empty() {
            self.canvas.ime_faces.extend(new);
            self.styled = false;
        }
    }

    /// Document title for the title bar (and the suggested file name). An untitled document is
    /// named in the interface language, as `Document1` is in English.
    pub fn title_stem(&self) -> String {
        match &self.session.path {
            Some(p) => p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| tl!("Document1").into()),
            None => match &self.session.file_name {
                Some(name) => name.clone(),
                None if self.session.doc.core.title.is_empty() => {
                    documents::untitled_title(&self.session.resolved_filenames().0).unwrap_or_else(|| tl!("Document1").into())
                }
                None => self.session.doc.core.title.clone(),
            },
        }
    }

    /// Terminal feedback also reaches the normal window when printing starts through control.
    pub fn poll_printing(&mut self, ctx: &egui::Context) {
        self.printing.poll(ctx);
        self.print_feedback();
    }

    /// Say how the print job went, once, in the window showing the printed document.
    fn print_feedback(&mut self) {
        if self.printing.message == self.print_feedback_seen {
            return;
        }
        self.print_feedback_seen = self.printing.message.clone();
        let Some(message @ (printing::Feedback::Submitted(_) | printing::Feedback::Failed(_) | printing::Feedback::WorkerStopped)) =
            self.printing.message.clone()
        else {
            return;
        };
        let text = printing::feedback(&message, |label| i18n::t(label).to_owned());
        self.status(text);
        // Word returns to the document once a print job is away, the same as after Save; a
        // failure stays on the Print page so the error (and the settings to fix it) are visible
        // (#3210: the Print page used to stay up after a successful submission).
        if matches!(message, printing::Feedback::Submitted(_)) && self.ui.backstage && self.ui.backstage_page == "print" {
            self.ui.backstage = false;
            self.canvas.want_focus = true;
        }
    }

    /// Per-frame logic before layout: control requests, screenshots, shortcuts, file drops.
    pub fn logic(&mut self, ctx: &egui::Context) {
        updates_ui::tick(self, ctx);
        let lang = i18n::Lang::from_pref(&self.ui.language);
        i18n::set_current(lang);
        // New documents are written in the interface's language when WordCraft can proof it
        // (a Polish interface: Polish spelling, grammar and hyphenation from the first word).
        // An English interface writes the region's English (English (UK) on a British system,
        // #1045), a Spanish one the region's Spanish (Spanish (Uruguay) on a Uruguayan system,
        // Spain's elsewhere, #3360).
        let editing = match lang.editing_lang() {
            Some(t) => Some(t.to_string()),
            None if lang.code() == "en" => self.session.region_english(),
            None if lang.code() == "es" => Some(self.session.region_spanish()),
            None => None,
        };
        if self.applied_editing.as_ref().is_none_or(|(l, e)| *l != lang || *e != editing) {
            self.session.set_editing_language(editing.as_deref());
            self.applied_editing = Some((lang, editing));
        }
        options::sync(self);
        self.settle_async_saves();
        // Chinese text wants the Chinese face before the Japanese one (one glyph style per line).
        // An installed CJK font is read only once CJK text is shown (#241): a CJK interface, the
        // language names in Options, or a file name the caption is drawing. A file opened later
        // in this frame (dialog, drop, control channel) is picked up again at the end.
        self.ensure_caption_fonts();
        self.ensure_comment_fonts();
        self.ensure_recover_fonts();
        self.ensure_recent_fonts();
        self.ensure_find_fonts();
        self.install_ui_fonts(ctx, lang);
        if let Some(answer) = self.desktop_theme.take_change() {
            self.desktop_dark = Some(answer);
        }
        // Both palettes are installed and egui picks one: with `System` it follows an OS appearance
        // change live (winit reports it and repaints, #311). Where winit is silent (Linux) the
        // desktop portal's answer fills in instead. The interface follows the Interface theme
        // setting alone; Dark page only changes how pages are drawn (#312).
        let effective = match self.ui.theme {
            theme::Appearance::System if ctx.system_theme().is_none() => match self.desktop_dark {
                Some(true) => theme::Appearance::Dark,
                Some(false) => theme::Appearance::Light,
                None => theme::Appearance::System,
            },
            other => other,
        };
        if self.applied_theme != Some(effective) {
            theme::apply(ctx, effective);
            self.applied_theme = Some(effective);
        }
        // Interface size (#475, #822): egui's zoom factor multiplies the display scale the system
        // reports (native pixels per point). Auto keeps that scale; a chosen size divides it back
        // out, so "100%" is one screen pixel per point even where the system's scale is a guess.
        // Recomputed every frame, so a move to a monitor with another scale keeps a chosen size.
        // Keyboard zoom is off (above), so nothing else changes the factor; `set_zoom_factor`
        // repaints only on change.
        self.native_scale = theme::usable_scale(ctx.native_pixels_per_point().unwrap_or(1.0));
        if let Some(native) = ctx.native_pixels_per_point()
            && let Some(old) = self.legacy_interface_size.take()
        {
            self.ui.interface_size = theme::migrate_legacy_interface_size(old, native);
        }
        let zoom = self.ui.interface_size.zoom_factor(self.native_scale);
        if ctx.zoom_factor() != zoom {
            ctx.set_zoom_factor(zoom);
        }
        // Animation effects: the host is asked every frame what the system says (it answers from
        // memory), so a change made in the system's settings is followed without a restart.
        self.read_system_animations();
        let animate = self.animations_on();
        if self.applied_animations != Some(animate) {
            theme::apply_animations(ctx, animate);
            self.applied_animations = Some(animate);
        }
        if self.ctx.is_none() {
            self.ctx = Some(ctx.clone());
        }
        // Capture geometry/window/document identity is current before control admission and
        // completion.
        self.refresh_capture_geometry(ctx);
        let native = self.windows.native();
        if native {
            self.printing.poll(ctx);
        } else {
            self.poll_printing(ctx);
        }
        self.drain_control(ctx);
        // The active window's own work; with native windows each window does it again as it
        // draws, for itself.
        self.window_work(ctx);
        self.drain_inbox();
        self.drain_pasted();
        self.watch_fonts(ctx);
        // A script, an agent or a later result may have made another document the active one.
        self.sync_document();
        let document = self.session.document_id();
        self.session.recovery_tick();
        if self.session.document_id() != document {
            self.ui.backstage = false;
            self.sync_document();
        }
        if !self.recovery_seen && self.session.recovery_ready() {
            self.recovery_seen = true;
            if !self.session.recovery_documents().is_empty() && !self.session.dirty {
                self.ui.backstage = true;
                self.ui.backstage_page = "recover".into();
            }
        }
        if let Some(wait) = self.session.recovery_wait() {
            ctx.request_repaint_after(wait);
        }
        if !native {
            self.window_input(ctx);
        }
        self.send_control_input(ctx);
        // Screenshot deadlines, geometry and completions belong to `logic`; issuing the one
        // callback command belongs to `ui` (`flush_shot_and_synthetic`). A queued/settling capture
        // needs a real UI pass, and so does an injected synthetic step, even while the window is
        // hidden.
        control::drain_screenshots(self, ctx);
        if self.shots.in_flight() || self.synthetic_pending || !self.synthetic.is_empty() {
            let viewport = self.active_viewport();
            if ctx.input_for(viewport, |i| i.viewport().visible()) != Some(true) {
                ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::RequestPaintWhileHidden);
            }
            ctx.request_repaint_of(viewport);
            ctx.request_repaint();
        }
        // The open above may have changed the file name the caption draws.
        self.ensure_caption_fonts();
        self.ensure_comment_fonts();
        self.ensure_recover_fonts();
        self.ensure_recent_fonts();
        self.ensure_find_fonts();
        self.install_ui_fonts(ctx, lang);
        self.report_dirty();
    }

    /// A window's own work, with its state in place: print feedback for its document, Zotero,
    /// Read Aloud, its file dialog, AutoSave of its document. Runs in [`WordApp::logic`] for the
    /// active window and, with native windows, as each window draws.
    fn window_work(&mut self, ctx: &egui::Context) {
        if self.windows.native() {
            let doc = self.session.document_id();
            if self.print_document.is_none_or(|d| d == doc || !self.session.is_open(d)) {
                self.print_feedback();
            }
        }
        zotero::poll(self, ctx);
        read_aloud::poll(self, ctx);
        self.clear_stale_change_picture();
        let _ = self.poll_file_dialog();
        self.autosave_tick(now_ms());
        if self.autosaves() && self.session.dirty {
            ctx.request_repaint_after(std::time::Duration::from_millis(1000));
        }
    }

    /// A window's own input before it draws: ⌘Q. Runs in [`WordApp::logic`] with one window,
    /// else as each window draws (each native window has its own input). The SmartArt Text
    /// Pane's Tab keys and dropped files are read in the first UI pass
    /// ([`WordApp::handle_ui_input_events`]).
    fn window_input(&mut self, ctx: &egui::Context) {
        if self.windows.native() && ctx.input_mut(|i| i.consume_shortcut(&QUIT)) {
            self.quit();
        }
        // The drawn window's caption, comments and Find results borrow faces it may not have.
        if self.windows.native() {
            self.ensure_caption_fonts();
            self.ensure_comment_fonts();
            self.ensure_find_fonts();
        }
    }

    /// Files dropped on the native window. Raw input is taken only in the first UI pass: a
    /// hidden `run_logic` tick leaves the previous shown frame's events stale, and replaying a
    /// drop would open or insert it again.
    #[cfg(not(target_arch = "wasm32"))]
    fn open_dropped_files(&mut self, files: Vec<egui::DroppedFileHandle>) {
        let before = self.session.document_id();
        let mut placed_picture = false;
        for f in &files {
            let path = f.path().to_string_lossy().to_string();
            let lp = path.to_ascii_lowercase();
            let img = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".wmf", ".emf"].iter().any(|e| lp.ends_with(e));
            let sheet = wordcraft_engine::cmd::mailings::is_spreadsheet_name(&lp);
            let _ = if img {
                self.caret_after_placed_picture(placed_picture);
                let r = self.run("insert.picture", json!({"path": path}));
                placed_picture = Self::placed_in_text(&r);
                r
            } else if sheet {
                // A spreadsheet isn't a document: it's a recipient list (#334).
                self.load_recipients(json!({"path": path}))
            } else if wordcraft_fonts::is_font_file_name(&lp) {
                self.run("font.add", json!({"path": path}))
            } else {
                self.open_file(&path, None)
            };
        }
        if self.session.document_id() != before {
            self.autosave_path = None;
            self.dialog = None;
            self.ui.backstage = false;
            self.canvas.want_focus = true;
        }
        if !files.is_empty() {
            self.ensure_caption_fonts();
            self.ensure_comment_fonts();
            self.ensure_recover_fonts();
            self.ensure_recent_fonts();
            self.ensure_find_fonts();
        }
    }

    /// Install the interface fonts when the language, the CJK fallback, or a caption fallback
    /// face has changed since the last install.
    fn install_ui_fonts(&mut self, ctx: &egui::Context, lang: i18n::Lang) {
        let system_cjk = self.fonts_system_cjk || self.want_system_cjk || lang.uses_cjk();
        if !self.styled || lang.prefers_hans() != self.fonts_hans || lang.prefers_korean() != self.fonts_korean || system_cjk != self.fonts_system_cjk
        {
            // Every window's borrowed faces: the fonts are the app's.
            let mut faces = self.canvas.ime_faces.clone();
            for w in self.other_windows() {
                for face in &w.canvas.ime_faces {
                    if !faces.iter().any(|f| f.id() == face.id()) {
                        faces.push(face.clone());
                    }
                }
            }
            theme::install_fonts_language_extra(ctx, lang.prefers_hans(), lang.prefers_korean(), system_cjk, &faces);
            self.fonts_hans = lang.prefers_hans();
            self.fonts_korean = lang.prefers_korean();
            self.fonts_system_cjk = system_cjk;
            // Mod with -, = and 0 are Word shortcuts (optional hyphen, subscript, paragraph spacing);
            // egui's keyboard zoom would also scale the whole window on them. Zoom is View › Zoom.
            // egui quits on Command-Q, which is Ctrl+Q off the Mac: Word's Reset Paragraph
            // Formatting there (#1218). Keep ⌘Q, which only the Mac sends. egui would close only the
            // window it was pressed in, so with native windows the app quits itself
            // ([`WordApp::window_logic_before`]).
            let quit = (!self.windows.native()).then_some(QUIT).into_iter().collect();
            ctx.options_mut(|o| {
                o.zoom_with_keyboard = false;
                o.quit_shortcuts = quit;
            });
            self.styled = true;
        }
    }

    /// Tell the host whether the document has unsaved changes ([`Services::on_dirty`]).
    fn report_dirty(&self) {
        if let Some(f) = &self.services.on_dirty {
            f(self.session.dirty || panes::has_comment_drafts(self));
        }
    }

    /// Whether the document's file is one the user saved to in this session.
    pub fn saved_here(&self) -> bool {
        self.session.path.is_some() && self.session.path == self.autosave_path
    }

    /// Why AutoSave can't cover the document, or `None` when it can: it writes only a file the
    /// user saved to in this session, in a format that keeps everything, and never in the browser
    /// (a page can't write to the user's files; saving there is a download, #176).
    pub fn autosave_block(&self) -> Option<AutoSaveBlock> {
        if self.services.download.is_some() {
            return Some(AutoSaveBlock::Browser);
        }
        let Some(path) = &self.session.path else { return Some(AutoSaveBlock::Unsaved) };
        if !keeps_everything(&path.to_string_lossy()) {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            return Some(AutoSaveBlock::Format { name });
        }
        if !self.saved_here() {
            return Some(AutoSaveBlock::NotSavedHere);
        }
        None
    }

    /// Whether AutoSave covers the document: it is on and nothing blocks it ([`Self::autosave_block`]).
    pub fn autosaves(&self) -> bool {
        self.autosave && self.autosave_block().is_none()
    }

    /// `file.autosave` (the AutoSave switch, and scripts): `value` turns AutoSave on or off;
    /// without it, it toggles what the switch shows. Turning it on for a document AutoSave can't
    /// cover is an error that says why, instead of a switch that is on but saves nothing (#196).
    fn set_autosave(&mut self, params: &Value) -> Result<Value, String> {
        let on = params.get("value").and_then(Value::as_bool).unwrap_or(!self.autosaves());
        if on && let Some(block) = self.autosave_block() {
            return Err(block.reason());
        }
        self.autosave = on;
        self.session.autosave = on;
        Ok(json!({"value": on}))
    }

    /// AutoSave: write a saved document a couple of seconds after the last change. A failure is
    /// shown in the status bar and turns AutoSave off for the file until the user saves it again.
    fn autosave_tick(&mut self, now: f64) {
        if self.autosaves() && self.session.dirty && now - self.last_autosave > 2500.0 && now - self.canvas.caret_visible_since > 1500.0 {
            self.last_autosave = now;
            if let Err(e) = self.session.run("file.save", &json!({})) {
                log::warn!("AutoSave failed: {e}");
                self.autosave_path = None;
                self.status(i18n::fmt(tl!("AutoSave failed: {error}. Save the document to turn AutoSave back on."), &[("error", &e.to_string())]));
            }
        }
    }

    /// Read a Control-click as the right-click on macOS, then offer at most one synthetic gesture
    /// step to the host. eframe carries an unconsumed RawInput into the next pass and into hidden
    /// logic ticks, so offering it again would duplicate input. The step stays queued until a
    /// final (not discarded) UI pass acknowledges it, and a press/release pair stays intact.
    pub fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // Capture the surface from the raw input itself. `InputState::viewport_rect` and the
        // last painted screen rect are stale in a hidden `run_logic` tick; RawInput is the host's
        // current answer, including minimized/occluded and a Wayland `inner_rect: None`.
        if raw.viewport_id == egui::ViewportId::ROOT
            && let Some(geometry) = control::SurfaceGeometry::from_raw(raw, ctx)
        {
            // A hidden resize and change back still invalidate an older capture, even if
            // neither intermediate geometry reaches a drawing or logic pass.
            if self.surface_geometry != Some(geometry) {
                self.capture_epoch = self.capture_epoch.wrapping_add(1);
            }
            self.surface_geometry = Some(geometry);
        }
        if cfg!(target_os = "macos") {
            control_click_as_secondary(&mut raw.events, &mut self.control_click);
        }
        if self.synthetic_pending || self.synthetic.is_empty() || self.fonts_frames < 2 || !self.control_input_is_roots(ctx) {
            return;
        }
        let Some((events, n)) = self.peek_synthetic() else { return };
        if events.iter().any(|e| matches!(e, egui::Event::Key { .. } | egui::Event::Text(_))) {
            // Control input belongs to this window's remembered widget even while another app
            // is active. Give only this input frame egui focus, without requesting OS activation.
            raw.focused = true;
            if let (Some(ctx), Some((id, _))) = (&self.ctx, self.canvas.widget_id) {
                // Canvas keys run before painting updates last frame's native focus flag.
                self.canvas.focused = ctx.memory(|m| m.has_focus(id));
            }
        }
        raw.events.extend(events);
        self.synthetic_pending = true;
        self.synthetic_injected = n;
    }

    /// The control channel's next input frame, taken off the queue.
    fn next_synthetic(&mut self) -> Option<Vec<egui::Event>> {
        let (events, n) = self.peek_synthetic()?;
        self.synthetic.drain(..n);
        Some(events)
    }

    /// The control channel's next input frame: one pointer event (after a move to it), or keys
    /// and text up to the next key release; with how many queued events it takes.
    fn peek_synthetic(&self) -> Option<(Vec<egui::Event>, usize)> {
        if self.synthetic.is_empty() {
            return None;
        }
        let n = match self.synthetic.first() {
            Some(egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. }) => 1,
            _ => self.synthetic.iter().position(|e| matches!(e, egui::Event::Key { pressed: false, .. })).map_or(self.synthetic.len(), |i| i + 1),
        };
        let mut events = Vec::new();
        if let Some(egui::Event::PointerMoved(p) | egui::Event::PointerButton { pos: p, .. }) = self.synthetic.first() {
            events.push(egui::Event::PointerMoved(*p));
        }
        let n = n.min(self.synthetic.len());
        events.extend(self.synthetic.iter().take(n).cloned());
        Some((events, n))
    }

    /// Lay out the windows: on the desktop each open document's in its own native window
    /// ([`windows`]), on the web the one window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if self.windows.native() {
            self.windows_ui(ui);
        } else {
            self.window_ui(ui);
        }
    }

    /// Lay out the drawn window. A command that brings another window's state in (New, Open,
    /// Switch Windows) ends this window's drawing for the frame: the rest would be the other
    /// window's.
    fn window_ui(&mut self, ui: &mut egui::Ui) {
        let window = self.windows.shown();
        let moved = |app: &WordApp| app.windows.shown() != window;
        let t0 = now_ms();
        let ctx = ui.ctx().clone();
        i18n::set_current(i18n::Lang::from_pref(&self.ui.language));
        // Animation effects off: egui's scroll areas get a wheel notch whole, not eased in.
        if !self.animations_on() {
            let opts = ctx.options(|o| o.input_options);
            ctx.input_mut(|i| i.smooth_scroll_delta = scroll::unsmoothed_scroll(i, &opts));
        }
        self.handle_ui_input_events(&ctx);
        // Fonts installed by `logic` take effect on the next frame.
        if self.fonts_frames < 2 {
            self.fonts_frames += 1;
            ctx.request_repaint();
            return;
        }
        let t = theme::Tokens::get(&ctx);
        self.escape_free = self.dialog.is_none() && !self.ui.backstage && self.ui.keytips == keytips::Phase::Off && !egui::Popup::is_any_open(&ctx);
        // Keytips are read before layout so the frame paints the new state; badges paint after
        // the ribbon so they sit on top of it.
        crate::keytips::logic(self, &ctx);
        // A failed Recent open, or a failed Save As, is news only until File closes (#2938).
        if !self.ui.backstage {
            backstage::forget_open_error(&ctx);
            backstage::forget_save_as_error(&ctx);
        }
        self.paint_window(ui, &ctx, t);
        if !moved(self) {
            self.show_overlays(ui, &ctx);
        }
        if !moved(self) {
            self.sync_title_and_frame(&ctx, t0);
        }
        // The synthetic step this frame took is done even when drawing ended early.
        self.flush_shot_and_synthetic(&ctx);
    }

    /// Raw input read in the known UI phase, first pass only: dropped files and the SmartArt Text
    /// Pane's Tab/Shift+Tab, before the font warm-up return. `logic` also runs hidden, where its
    /// input events are from the previous shown frame.
    fn handle_ui_input_events(&mut self, ctx: &egui::Context) {
        // eframe does not preserve dropped files for a later frame, so a drop in the first two
        // passes would otherwise disappear.
        #[cfg(not(target_arch = "wasm32"))]
        if ctx.current_pass_index() == 0 {
            let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
            if !dropped.is_empty() {
                self.open_dropped_files(dropped);
                let lang = i18n::Lang::from_pref(&self.ui.language);
                self.install_ui_fonts(ctx, lang);
            }
        }
        // SmartArt Text Pane (#2701): Tab demotes and Shift+Tab promote the focused item, as
        // Word's pane does. This is an input action, so it runs in the first UI pass only; hidden
        // `run_logic` ticks still see the previous shown frame's keys.
        if ctx.current_pass_index() == 0 && self.session.view.smart_art_pane && self.smart_art_row_focused && self.smart_art_item.is_some() {
            let shift_tab = ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab));
            let tab = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
            if shift_tab || tab {
                ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
                let _ = self.run(if shift_tab { "smartArt.promote" } else { "smartArt.demote" }, json!({"index": self.smart_art_item.unwrap_or(0)}));
            }
        }
    }

    /// Draw the File page, the empty-window chrome, or the normal document window.
    fn paint_window(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, t: theme::Tokens) {
        let window = self.windows.shown();
        let moved = |app: &WordApp| app.windows.shown() != window;
        if self.ui.backstage {
            backstage::show(self, ui);
        } else if !self.has_document() {
            // No document open (#1129): the ribbon greyed out but for Help, and no page.
            chrome::title_bar(self, ui);
            ribbon::show(self, ui);
            chrome::status_bar(self, ui);
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.canvas)).show(ui, |_| {});
        } else {
            let screen = chrome::Screen::of(&self.session.view);
            // Focus and Full Screen (#1678) take the whole screen, with only their slim bar.
            self.focus_ribbon &= screen == chrome::Screen::Focus;
            if screen != chrome::Screen::Normal {
                chrome::screen_bar(self, ui, screen);
            } else {
                chrome::title_bar(self, ui);
                if moved(self) {
                    return;
                }
                ribbon::show(self, ui);
                if moved(self) {
                    return;
                }
                chrome::status_bar(self, ui);
                panes::show(self, ui);
            }
            if moved(self) {
                return;
            }
            let fill = if screen == chrome::Screen::Focus { chrome::FOCUS_CANVAS } else { t.canvas };
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(fill)).show(ui, |ui| {
                canvas::show(self, ui);
            });
            if self.focus_ribbon && !moved(self) {
                chrome::focus_ribbon(self, ctx);
            }
        }
    }

    /// Dialogs, panes and other overlays drawn over the window. A command that brings another
    /// window's state in ends the drawing: the rest would be the other window's.
    fn show_overlays(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let window = self.windows.shown();
        let moved = |app: &WordApp| app.windows.shown() != window;
        content_controls::popup(self, ctx);
        dialogs::show(self, ctx);
        if moved(self) {
            return;
        }
        updates_ui::show(self, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        file_browser::show(self, ctx);
        colors::show(self, ctx);
        zotero::show_alert(self, ctx);
        read_aloud::show(self, ctx);
        crate::keytips::show(self, ctx, ui);
        if moved(self) {
            return;
        }
        keys::global_shortcuts(self, ctx);
        if moved(self) {
            return;
        }
        self.refocus_after_chrome_click(ctx);
        if let Some(url) = self.canvas.open_url.take() {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
    }

    /// Keep the window title and full-screen state in sync, then record this frame's time.
    fn sync_title_and_frame(&mut self, ctx: &egui::Context, t0: f64) {
        // With native windows the viewport driver gives the window its title.
        let title = self.window_title();
        if title != self.sent_title {
            if !self.windows.driven() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            }
            self.sent_title = title;
        }
        self.sync_full_screen(ctx);
        self.frame_ms = now_ms() - t0;
        // Typing and formatting land here, after `logic` has reported.
        self.report_dirty();
    }

    /// One callback command per admitted request, and the synthetic-input queue's final pass.
    fn flush_shot_and_synthetic(&mut self, ctx: &egui::Context) {
        // This is in `ui` so the captured frame is the layout just drawn; a discarded pass is not
        // a layout anyone will see. The shot's `issued` flag makes repeated final passes harmless.
        if !ctx.will_discard() {
            control::issue_screenshots(self, ctx);
        }
        // The raw input was consumed by this host frame's passes; only a final UI pass proves
        // the step is done. A discarded pass keeps the queue for the next UI frame.
        if self.synthetic_pending && !ctx.will_discard() {
            let n = self.synthetic_injected.min(self.synthetic.len());
            self.synthetic.drain(..n);
            self.synthetic_pending = false;
            self.synthetic_injected = 0;
            if !self.synthetic.is_empty() {
                ctx.request_repaint();
            }
        }
    }

    /// Carry View › Full Screen (`view.fullScreen`, F11), Focus and Full Screen View (#1678) to
    /// the window, and follow the window when the system takes it in or out of full screen (the
    /// macOS title bar button, the window manager): in is Full Screen, out ends all three. Leaving
    /// full screen gives the window back its earlier size and place: the system keeps them, and
    /// [`window_geometry`] never saves a full-screen size (#980).
    fn sync_full_screen(&mut self, ctx: &egui::Context) {
        let v = &self.session.view;
        let want = v.full_screen || v.focus_mode || v.full_screen_view;
        let seen = ctx.input(|i| i.viewport().fullscreen);
        if seen != self.seen_full_screen {
            self.seen_full_screen = seen;
            if let Some(on) = seen
                && on != want
            {
                let v = &mut self.session.view;
                v.full_screen = on;
                if !on {
                    v.focus_mode = false;
                    v.full_screen_view = false;
                }
                self.sent_full_screen = on;
            }
        }
        let v = &self.session.view;
        let want = v.full_screen || v.focus_mode || v.full_screen_view;
        if want != self.sent_full_screen {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(want));
            self.sent_full_screen = want;
        }
    }

    /// A click on the window's chrome (ribbon tabs and empty ribbon space, galleries, check
    /// boxes, the status bar, panes) hands keyboard focus back to the document, as in Word, so the
    /// caret keeps blinking and typing continues where it was (#1093). egui drops the page's focus
    /// on any click elsewhere; only a click that left nothing focused counts, so a text field
    /// (font size, zoom, command search) keeps the focus it took, and clicks on a dialog, menu or
    /// popup, or while one is open, leave focus alone. Clicks on the page are the canvas's own.
    fn refocus_after_chrome_click(&mut self, ctx: &egui::Context) {
        if self.ui.backstage || self.dialog.is_some() || self.colors.is_some() {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.file_browser.open.is_some() {
            return;
        }
        let Some(pos) = ctx.input(|i| if i.pointer.any_click() { i.pointer.interact_pos() } else { None }) else { return };
        // The ribbon dropped over the page in Focus (#1678) is chrome, though it lies over the page.
        let on_focus_ribbon = self.focus_ribbon && ctx.layer_id_at(pos).is_some_and(|l| l.id == egui::Id::unique("focus_ribbon"));
        // The page handles its own clicks (comment balloons there take the keyboard themselves).
        if !on_focus_ribbon && (self.canvas.balloon.is_some() || self.canvas.viewport.is_some_and(|v| v.contains(pos))) {
            return;
        }
        let on_chrome = on_focus_ribbon || ctx.layer_id_at(pos).is_none_or(|l| l.order == egui::Order::Background);
        if on_chrome && ctx.memory(|m| m.focused().is_none()) && !egui::Popup::is_any_open(ctx) {
            self.canvas.want_focus = true;
            // The page has already been drawn, so it only sees `want_focus` next frame, and its own
            // `focused` flag (which gates the keys) isn't recomputed until then either. Claim both
            // now, or the click swallows the first keystroke.
            self.canvas.focused = true;
            // Only a page this pass drew. The File page's back arrow closes the File page in the
            // pass that drew it instead of the page, and focus on a widget missing from the pass
            // names a node AccessKit doesn't have: with accessibility on, AccessKit ends the program
            // over that. `want_focus` hands the keyboard over next frame instead.
            if let Some((id, pass)) = self.canvas.widget_id
                && pass == ctx.cumulative_pass_nr()
            {
                ctx.memory_mut(|m| m.request_focus(id));
            }
            // The canvas reads keys before it takes focus back (#1107), so it counts as focused
            // now: a keystroke in the very next frame reaches the page instead of being dropped.
            self.canvas.focused = true;
            ctx.request_repaint();
        }
    }

    /// Files that arrived asynchronously (web picker, drops): documents open, pictures insert,
    /// recipient lists (CSV/TSV, spreadsheets, or a text file asked for as one) load as recipients.
    fn drain_inbox(&mut self) {
        let Some(inbox) = self.services.inbox.clone() else { return };
        let files = std::mem::take(&mut *inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        let mut placed_picture = false;
        for (name, bytes) in files {
            let lower = name.to_ascii_lowercase();
            let img = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".wmf", ".emf"].iter().any(|e| lower.ends_with(e));
            // Any recipient file ends the pending picker request, so a later .txt drop opens normally.
            let asked = std::mem::take(&mut self.recipient_list_pending);
            let list = lower.ends_with(".csv") || lower.ends_with(".tsv") || (lower.ends_with(".txt") && asked);
            let sheet = wordcraft_engine::cmd::mailings::is_spreadsheet_name(&lower);
            self.recipient_list_pending = asked && !list && !sheet;
            let r = if sheet {
                self.load_recipients(json!({"path": name, "data": wordcraft_engine::cmd::insert::base64_encode(&bytes)}))
            } else if wordcraft_fonts::is_font_file_name(&lower) {
                self.run("font.add", json!({"name": name, "data": wordcraft_engine::cmd::insert::base64_encode(&bytes)}))
            } else if list {
                self.load_recipients(json!({"csv": wordcraft_engine::io::decode_text(&bytes)}))
            } else if img {
                self.caret_after_placed_picture(placed_picture);
                let r = self.insert_or_change_picture(json!({"data": wordcraft_engine::cmd::insert::base64_encode(&bytes)}));
                placed_picture = Self::placed_in_text(&r);
                r
            } else {
                self.run("file.open", json!({"path": name, "data": wordcraft_engine::cmd::insert::base64_encode(&bytes)}))
            };
            if r.is_ok() {
                self.ui.backstage = false;
            }
        }
    }

    /// Pictures pasted in the browser: inserted once when the page is the edit target, dropped
    /// otherwise so the focused field's own paste applies (a text field ignores a picture).
    fn drain_pasted(&mut self) {
        let Some(pasted) = self.services.pasted.clone() else { return };
        let pictures = std::mem::take(&mut *pasted.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        let target = paste_target(self.canvas.focused, self.ui.backstage, self.dialog.is_some() || self.file_dialog_open());
        for (_, bytes) in pictures {
            if !target {
                log::debug!("pasted picture ignored: the document is not the edit target");
                continue;
            }
            let _ = self.insert_or_change_picture(json!({"data": wordcraft_engine::cmd::insert::base64_encode(&bytes)}));
        }
    }

    /// Make the capture geometry the active window's surface, viewport and document. The epoch is
    /// monotonic: resizing back to an earlier size is still a new epoch.
    pub(crate) fn refresh_capture_geometry(&mut self, ctx: &egui::Context) {
        let (viewport, surface) = self.active_surface_geometry(ctx);
        let geometry = surface.map(|surface| surface.with_target(viewport, self.session.document_id()));
        if self.capture_geometry != geometry {
            self.capture_geometry = geometry;
            self.capture_epoch = self.capture_epoch.wrapping_add(1);
        }
    }

    /// Drains queued control requests, dispatches each through `control::handle`, sends its reply or
    /// admits a screenshot, then puts the receiver back.
    fn drain_control(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.control_rx.take() else { return };
        while let Ok(req) = rx.try_recv() {
            let reply = req.reply.clone();
            match control::handle(self, ctx, &req) {
                control::Outcome::Done(v) => {
                    let _ = reply.send(v);
                }
                control::Outcome::Screenshot { path } => control::admit_screenshot(self, ctx, path, reply),
            }
        }
        self.control_rx = Some(rx);
    }
}

/// Why AutoSave can't cover the document ([`WordApp::autosave_block`]); the switch is greyed out
/// and its tooltip gives [`AutoSaveBlock::reason`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutoSaveBlock {
    /// The web app: saving is a download, there is no file to keep writing to.
    Browser,
    /// A new document that has never been saved.
    Unsaved,
    /// A file in a format that can't keep everything (Markdown, plain text, HTML…), by file name.
    Format { name: String },
    /// A file that was opened but not yet saved in this session.
    NotSavedHere,
}

impl AutoSaveBlock {
    /// What the user (or a script) is told, in the interface language.
    pub fn reason(&self) -> String {
        match self {
            AutoSaveBlock::Browser => {
                tl!("AutoSave isn't available in the browser: it can't write to your files. Save downloads a copy.").to_string()
            }
            AutoSaveBlock::Unsaved => tl!("Save the document to turn on AutoSave").to_string(),
            AutoSaveBlock::Format { name } => i18n::fmt(
                tl!("AutoSave only keeps .docx, .odt and .rtf files; {name} can't keep all formatting. Save as .docx to turn on AutoSave."),
                &[("name", name)],
            ),
            AutoSaveBlock::NotSavedHere => {
                tl!("Save the document to turn on AutoSave (a file you have only opened isn't rewritten until you save it)").to_string()
            }
        }
    }

    /// Whether Save As can lift the block (everywhere but the browser).
    pub fn save_as_helps(&self) -> bool {
        !matches!(self, AutoSaveBlock::Browser)
    }
}

/// Whether saving to `name` keeps the whole document (the formats `file.save` treats as the
/// document's own file, including macro-enabled documents and templates); anything else is a
/// copy that leaves it unsaved, unless it is the file the document was opened from (#2376).
/// AutoSave writes only these formats.
pub fn keeps_everything(name: &str) -> bool {
    let ext = std::path::Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "docx" | "docm" | "dotx" | "dotm" | "odt" | "rtf" | "json" | "xml")
}

/// The dialog a user-run command opens when it lacks the input it needs (scripts and agents get
/// the command's own error or default instead): Select Recipients and Edit Recipient List without
/// data, Insert Merge Field without a field, Find Recipient without text, the If and Skip
/// Record If rules (or Rules with no rule at all) without a field, Finish & Merge and Print
/// Documents without records, Table Properties without settings, Symbol and Field without a
/// character or code, and Enclose Characters without a shape.
fn input_dialog(id: &str, params: &Value) -> Option<&'static str> {
    let has = |k: &str| params.get(k).is_some_and(|v| !v.is_null());
    let rule = params.get("rule").and_then(Value::as_str).map(str::to_ascii_uppercase);
    match id {
        "file.rename" if !has("name") => Some("renameDocument"),
        "mailings.rules" if !has("field") => match rule.as_deref() {
            None | Some("IF") => Some("ruleIf"),
            Some("SKIPIF") => Some("ruleSkipIf"),
            _ => None,
        },
        "mailings.recipients" if !["csv", "path", "data", "rows"].into_iter().any(has) => Some("recipientList"),
        "mailings.editRecipients" if !has("rows") => Some("recipientList"),
        "mailings.insertField" if !has("field") => Some("insertMergeField"),
        "mailings.findRecipient" if !has("text") => Some("findRecipient"),
        // Finish & Merge and Print Documents without records ask which (#3028).
        "mailings.finish" if !["path", "records", "from", "to"].into_iter().any(has) => Some("mergeRecords"),
        "mailings.printDocuments" if !["records", "from", "to"].into_iter().any(has) => Some("printRecords"),
        // Symbol without a character and Field without a code show their dialogs (#321).
        "insert.symbol" if !has("char") => Some("symbol"),
        "insert.field" if !has("instr") => Some("field"),
        // Edit Field Code shows the selected field's code (#339).
        "fields.setCode" if !has("code") => Some("editFieldCode"),
        // SmartArt without a layout or items shows the layout picker.
        "insert.smartArt" if !has("layout") && !has("items") => Some("insertSmartArt"),
        // Chart without a type shows the gallery; Change Chart Type and Edit Data their dialogs.
        "insert.chart" if !has("type") && !has("data") => Some("insertChart"),
        "chart.type" if !has("type") => Some("changeChartType"),
        "chart.editData" if !["data", "categories", "series"].into_iter().any(has) => Some("chartData"),
        // Table Properties without settings shows the dialog (with settings it applies them).
        "table.properties" if params.as_object().is_none_or(|m| m.is_empty()) => Some("tableProperties"),
        // Cell Margins without settings shows the dialog; values continue through the same command.
        "table.cellMargins" if params.as_object().is_none_or(|m| m.is_empty()) => Some("tableCellMargins"),
        // Size without a size (Picture Format › Size's launcher) shows the Size dialog (#2573).
        "picture.size" if params.as_object().is_none_or(|m| m.is_empty()) => Some("pictureSize"),
        // Formula without one asks for it, suggested from the numbers next to the cell (#1201).
        "table.formula" if !has("formula") => Some("tableFormula"),
        // Sort without settings asks for the column, order and header row (#1200).
        "table.sort" if params.as_object().is_none_or(|m| m.is_empty()) => Some("tableSort"),
        // Split Cells and Convert Text to Table without settings ask for them (#2592).
        "table.split" if params.as_object().is_none_or(|m| m.is_empty()) => Some("splitCells"),
        "table.fromText" if params.as_object().is_none_or(|m| m.is_empty()) => Some("textToTable"),
        // Convert to Text without a separator asks for one.
        "table.toText" if params.as_object().is_none_or(|m| m.is_empty()) => Some("tableToText"),
        // Tabs, Borders and Shading, and Page Borders without settings show their dialogs (#320).
        "para.tabs" if params.as_object().is_none_or(|m| m.is_empty()) => Some("tabs"),
        "para.borders" if params.as_object().is_none_or(|m| m.is_empty()) => Some("borders"),
        "design.pageBorders" if params.as_object().is_none_or(|m| m.is_empty()) => Some("pageBorders"),
        // `null` is an answer here: it removes the password.
        "file.encrypt" if params.get("password").is_none() => Some("encryptPassword"),
        // Define New Multilevel List and Track Changes Options without settings show their dialogs.
        "list.define" if params.get("levels").is_none() => Some("defineList"),
        "review.trackingOptions" if params.as_object().is_none_or(|m| m.is_empty()) => Some("trackChangesOptions"),
        // Compare / Combine without a revised document, AutoCorrect Options without settings and
        // Restrict Editing without settings show their dialogs or pane (#412).
        "review.compare" if !has("path") && !has("text") => Some("compare"),
        "review.combine" if !has("path") && !has("text") => Some("combine"),
        "tools.autocorrect" if params.as_object().is_none_or(|m| m.is_empty()) => Some("autoCorrect"),
        "review.restrict" | "file.protect" if params.as_object().is_none_or(|m| m.is_empty()) => Some("restrictEditing"),
        "insert.bookmark" | "insert.deleteBookmark" if !has("name") => Some("bookmark"),
        // Word Count from the keyboard or command search shows its dialog, as the ribbon does.
        "review.wordCount" if params.as_object().is_none_or(|m| m.is_empty()) => Some("wordCount"),
        // Content control Properties without settings shows the dialog.
        "developer.properties" if params.as_object().is_none_or(|m| m.is_empty()) => Some("controlProperties"),
        // Enclose Characters without a shape (null is one: it takes the enclosure away).
        "format.enclose" if params.get("shape").is_none() => Some("enclose"),
        // Footnote and Endnote without settings shows its dialog (#385).
        "references.noteOptions" if params.as_object().is_none_or(|m| m.is_empty()) => Some("noteOptions"),
        // References dialogs (#402): Insert Caption, Insert Index, Insert Table of Figures (#3013),
        // Mark Entry, Manage Sources and Insert Citation without a source.
        "references.caption" | "references.index" | "references.tableOfFigures" if params.as_object().is_none_or(|m| m.is_empty()) => {
            Some(match id {
                "references.caption" => "caption",
                "references.index" => "index",
                _ => "tableOfFigures",
            })
        }
        "references.markEntry" if !has("entry") => Some("markEntry"),
        // Cross-reference without a target shows its dialog (#916); scripts get the target list.
        "insert.crossReference" if !has("target") => Some("crossReference"),
        "references.sources" if params.as_object().is_none_or(|m| m.is_empty()) => Some("sourceManager"),
        "references.citation" if !has("tag") && !has("source") => Some("createSource"),
        // Templates and Add-ins, and the Organizer without a file to list (#377).
        "tools.templates" => Some("templates"),
        "styles.organizer" if !has("path") => Some("organizer"),
        // Language, Envelopes and Labels without settings show their dialogs (#407).
        "review.language" if params.as_object().is_none_or(|m| m.is_empty()) => Some("language"),
        "mailings.envelopes" if params.as_object().is_none_or(|m| m.is_empty()) => Some("envelopes"),
        "mailings.labels" if params.as_object().is_none_or(|m| m.is_empty()) => Some("labels"),
        _ => None,
    }
}

/// Commands that work while no document is open (#1129): opening or creating one, closing the
/// window, Help, adding fonts (a font file dropped on the empty window), and the settings (File ›
/// Options, the view and interface preferences it sets, Customize Ribbon and Keyboard).
/// Everything else needs a document, `ui.*` included: only the interface commands listed here
/// (tabs, Backstage, dialogs, appearance, opening files) work without one.
pub(crate) fn works_without_document(id: &str) -> bool {
    matches!(
        id,
        "file.new"
            | "file.newFromTemplate"
            | "file.listTemplates"
            | "file.open"
            | "file.close"
            | "file.options"
            | "file.setAuthor"
            | "font.add"
            | "font.list"
            | "font.remove"
            | "readAloud.speed"
            | "readAloud.skipCitations"
            | "tools.customizeKeyboard"
            | "tools.customizeRibbon"
            | "view.darkMode"
            | "view.marks"
            | "view.ruler"
            | "view.numeral"
            | "ui.tab"
            | "ui.backstage"
            | "ui.dialog"
            | "ui.closeDialog"
            | "ui.collapseRibbon"
            | "ui.quickAccessToolbar"
            | "ui.keytips"
            | "ui.dark"
            | "ui.theme"
            | "ui.interfaceSize"
            | "ui.animations"
            | "ui.language"
            | "ui.saveChanges"
            | "ui.openFileDialog"
            | "ui.fileBrowser"
            | "ui.makeDefaultApp"
            | "ui.discord"
            | "ui.caretAnimation"
            | "ui.startScreen"
            | "ui.checkForUpdates"
    ) || id.starts_with("help.")
}

/// Commands that save, export, print, send or replace the document: comment text still being
/// typed is written before them (#1348).
fn flushes_comment_drafts(id: &str, params: &Value) -> bool {
    matches!(
        id,
        "file.save"
            | "file.saveAs"
            | "file.saveTemplate"
            | "file.open"
            | "file.exportPdf"
            | "file.exportPng"
            | "file.print"
            | "file.printPdf"
            | "file.share"
            | "ui.print"
            | "ui.printSubmit"
    ) || discards_document(id, params)
}

/// User commands that close the shown document: Close. New, Open, restoring a recovered copy,
/// Envelopes, Labels, Finish & Merge and Print Documents open their document beside it (Open and restoring
/// replace only an untouched blank document, which has nothing to lose).
pub(crate) fn discards_document(id: &str, _params: &Value) -> bool {
    id == "file.close"
}

/// Monotonic milliseconds for deadlines, settling and animation. `web_time::Instant` is a real
/// monotonic clock on native (`Instant::now`) and in the browser (`performance.now()`), so a wall
/// clock or NTP adjustment cannot make a screenshot deadline or autosave fire early or late.
pub fn now_ms() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<web_time::Instant> = OnceLock::new();
    START.get_or_init(web_time::Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// macOS reads a Control-click as the right-click (winit reports the primary button with Control
/// held), so menus open on it as on a two-finger click. The release follows its press even if
/// Control was let go in between.
fn control_click_as_secondary(events: &mut [egui::Event], held: &mut bool) {
    for e in events {
        if let egui::Event::PointerButton { button: button @ egui::PointerButton::Primary, pressed, modifiers, .. } = e {
            if *pressed {
                *held = modifiers.ctrl && !modifiers.mac_cmd;
            }
            if *held {
                *button = egui::PointerButton::Secondary;
            }
            if !*pressed {
                *held = false;
            }
        }
    }
}

/// Every `.rs` file under `dir`, subdirectories included, in a fixed order: tests that scan the
/// sources must keep seeing code that moves into a module directory.
#[cfg(test)]
pub(crate) fn rust_sources(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = vec![];
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests;
