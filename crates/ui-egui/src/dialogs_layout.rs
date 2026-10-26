//! Layout, Review and Mailings dialogs (#407): Line Numbers (Layout › Line Numbers › Line
//! Numbering Options…), Hyphenation (Hyphenation Options… and Manual…), Language (Review ›
//! Language), Envelopes and Labels (Mailings › Create), Formula (Table Layout › Data, #1201)
//! Sort (Table Layout › Data, #1200), Split Cells (Table Layout › Merge), Convert Text to Table
//! (Insert › Table, #2592), Size (Picture Format › Size, #2573) and Format Page Numbers (Insert ›
//! Header & Footer › Page Number ▸ Format Page Numbers…, #1113). Each ends by running a
//! command (`layout.lineNumbers`, `layout.hyphenation`, `layout.manualHyphenation` with
//! `caret.set` and `text.optionalHyphen`, `review.language`, `mailings.envelopes`,
//! `mailings.labels`, `table.formula`, `table.sort`, `table.split`, `table.fromText`,
//! `picture.size`, `layout.pageNumberFormat`), so scripts and agents get the same result without
//! the dialog. Shown through [`crate::dialogs::Dialog::Layout`].

use egui::{Ui, vec2};
use serde::Serialize;
use serde_json::{Value, json};

use wordcraft_doc::section::LineNumberRestart;
use wordcraft_engine::cmd::mailings::{ENVELOPE_SIZES, LABEL_PRODUCTS};
use wordcraft_engine::cmd::objects::fit_size;

use crate::WordApp;
use crate::options::MeasureExt;
use crate::theme::semibold;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "form", rename_all = "camelCase")]
pub enum LayoutDialog {
    LineNumbers(LineNumbersForm),
    Hyphenation(HyphenationForm),
    ManualHyphenation(ManualForm),
    Language(LanguageForm),
    Envelopes(EnvelopeForm),
    Labels(LabelsForm),
    Formula(FormulaForm),
    Sort(SortForm),
    SplitCells(SplitCellsForm),
    TextToTable(TextToTableForm),
    TableToText(TableToTextForm),
    Size(SizeForm),
    PageNumbers(PageNumbersForm),
}

impl LayoutDialog {
    pub fn name(&self) -> &'static str {
        match self {
            LayoutDialog::LineNumbers(_) => "lineNumbers",
            LayoutDialog::Hyphenation(_) => "hyphenation",
            LayoutDialog::ManualHyphenation(_) => "manualHyphenation",
            LayoutDialog::Language(_) => "language",
            LayoutDialog::Envelopes(_) => "envelopes",
            LayoutDialog::Labels(_) => "labels",
            LayoutDialog::Formula(_) => "tableFormula",
            LayoutDialog::Sort(_) => "tableSort",
            LayoutDialog::SplitCells(_) => "splitCells",
            LayoutDialog::TextToTable(_) => "textToTable",
            LayoutDialog::TableToText(_) => "tableToText",
            LayoutDialog::Size(_) => "pictureSize",
            LayoutDialog::PageNumbers(_) => "pageNumbers",
        }
    }

    /// The window title (English; the caller translates it).
    pub fn title(&self) -> &'static str {
        match self {
            LayoutDialog::LineNumbers(_) => "Line Numbers",
            LayoutDialog::Hyphenation(_) => "Hyphenation",
            LayoutDialog::ManualHyphenation(_) => "Manual Hyphenation",
            LayoutDialog::Language(_) => "Language",
            LayoutDialog::Envelopes(_) => "Envelopes",
            LayoutDialog::Labels(_) => "Labels",
            LayoutDialog::Formula(_) => "Formula",
            LayoutDialog::Sort(_) => "Sort",
            LayoutDialog::SplitCells(_) => "Split Cells",
            LayoutDialog::TextToTable(_) => "Convert Text to Table",
            LayoutDialog::TableToText(_) => "Convert to Text",
            LayoutDialog::Size(_) => "Size",
            LayoutDialog::PageNumbers(_) => "Page Number Format",
        }
    }
}

/// The dialog `ui.dialog` opens by `name`.
pub fn open(name: &str, app: &mut WordApp) -> Option<LayoutDialog> {
    Some(match name {
        "lineNumbers" => LayoutDialog::LineNumbers(LineNumbersForm::read(app)),
        "hyphenation" => LayoutDialog::Hyphenation(HyphenationForm::read(app)),
        "manualHyphenation" => LayoutDialog::ManualHyphenation(ManualForm::start(app)),
        "language" => LayoutDialog::Language(LanguageForm::read(app)),
        "envelopes" => LayoutDialog::Envelopes(EnvelopeForm::read(app)),
        "labels" => LayoutDialog::Labels(LabelsForm::read(app)),
        "tableFormula" => LayoutDialog::Formula(FormulaForm::read(app)?),
        "tableSort" => LayoutDialog::Sort(SortForm::read(app)?),
        "splitCells" => LayoutDialog::SplitCells(SplitCellsForm::read(app)?),
        "textToTable" => LayoutDialog::TextToTable(TextToTableForm::read(app)?),
        "tableToText" => LayoutDialog::TableToText(TableToTextForm::read(app)?),
        "pictureSize" => LayoutDialog::Size(SizeForm::read(app)?),
        "pageNumbers" => LayoutDialog::PageNumbers(PageNumbersForm::read(app)),
        _ => return None,
    })
}

/// Draws the dialog; returns true to close it.
pub fn body(app: &mut WordApp, ui: &mut Ui, d: &mut LayoutDialog) -> bool {
    match d {
        LayoutDialog::LineNumbers(f) => line_numbers_ui(app, ui, f),
        LayoutDialog::Hyphenation(f) => hyphenation_ui(app, ui, f),
        LayoutDialog::ManualHyphenation(f) => manual_ui(app, ui, f),
        LayoutDialog::Language(f) => language_ui(app, ui, f),
        LayoutDialog::Envelopes(f) => envelopes_ui(app, ui, f),
        LayoutDialog::Labels(f) => labels_ui(app, ui, f),
        LayoutDialog::Formula(f) => formula_ui(app, ui, f),
        LayoutDialog::Sort(f) => sort_ui(app, ui, f),
        LayoutDialog::SplitCells(f) => split_cells_ui(app, ui, f),
        LayoutDialog::TextToTable(f) => text_to_table_ui(app, ui, f),
        LayoutDialog::TableToText(f) => table_to_text_ui(app, ui, f),
        LayoutDialog::Size(f) => size_ui(app, ui, f),
        LayoutDialog::PageNumbers(f) => page_numbers_ui(app, ui, f),
    }
}

/// The accent button and Cancel; returns (ok, cancel).
fn buttons(ui: &mut Ui, ok: &str, ok_enabled: bool) -> (bool, bool) {
    let mut r = (false, false);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (okb, cancelb) =
                crate::dialogs::ok_cancel_responses(ui, |ui| ui.add_enabled(ok_enabled, crate::widgets::primary_button(ui, tl!(ok))), tl!("Cancel"));
            if cancelb.clicked() || crate::dialogs::cancel_key_pressed(ui) {
                r.1 = true;
            }
            if okb.clicked() {
                r.0 = true;
            }
        });
    });
    r
}

fn heading(ui: &mut Ui, s: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!(s)).font(semibold(12.5)));
}

/// Runs a dialog's command; a refusal shows in the status bar and keeps the dialog open.
fn run(app: &mut WordApp, id: &str, params: Value) -> bool {
    match app.run(id, params) {
        Ok(_) => true,
        Err(e) => {
            app.status(e);
            false
        }
    }
}

fn unit() -> wordcraft_geom::Unit {
    crate::options::unit()
}

// ---------------------------------------------------------------------------------------------
// Line Numbers

/// The Line Numbers dialog's fields. `distance` is in the interface unit.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineNumbersForm {
    pub on: bool,
    pub start: u32,
    /// From text: automatic, or `distance`.
    pub auto_distance: bool,
    pub distance: f32,
    pub count_by: u32,
    /// `restartPage`, `restartSection` or `continuous`.
    pub restart: String,
}

impl LineNumbersForm {
    pub fn read(app: &WordApp) -> LineNumbersForm {
        let sp = wordcraft_engine::cmd::page::sect(&app.session);
        let l = sp.line_numbers.clone();
        let cur = l.clone().unwrap_or_default();
        LineNumbersForm {
            on: l.is_some(),
            start: cur.start.max(1),
            auto_distance: cur.distance <= 0.0,
            distance: if cur.distance > 0.0 { cur.distance / unit().pt_per_unit() } else { 0.25 * 72.0 / unit().pt_per_unit() },
            count_by: cur.count_by.max(1),
            restart: match cur.restart {
                LineNumberRestart::Page => "restartPage",
                LineNumberRestart::Section => "restartSection",
                LineNumberRestart::Continuous => "continuous",
            }
            .into(),
        }
    }

    /// `layout.lineNumbers` parameters.
    pub fn params(&self) -> Value {
        if !self.on {
            return json!({"value": "none"});
        }
        let distance = if self.auto_distance { 0.0 } else { (self.distance * unit().pt_per_unit()).max(0.0) };
        json!({"value": self.restart, "start": self.start, "countBy": self.count_by, "distance": distance})
    }
}

fn line_numbers_ui(app: &mut WordApp, ui: &mut Ui, f: &mut LineNumbersForm) -> bool {
    ui.checkbox(&mut f.on, tl!("Add line numbering"));
    ui.add_enabled_ui(f.on, |ui| {
        egui::Grid::new("lnum").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
            ui.label(tl!("Start at:"));
            ui.add(egui::DragValue::new(&mut f.start).range(1..=32_767));
            ui.end_row();
            ui.label(tl!("From text:"));
            ui.horizontal(|ui| {
                ui.checkbox(&mut f.auto_distance, tl!("Auto"));
                ui.add_enabled(
                    !f.auto_distance,
                    egui::DragValue::new(&mut f.distance).speed(0.01).range(0.0..=22.0 * 72.0 / unit().pt_per_unit()).measure(unit()).max_decimals(2),
                );
            });
            ui.end_row();
            ui.label(tl!("Count by:"));
            ui.add(egui::DragValue::new(&mut f.count_by).range(1..=100));
            ui.end_row();
        });
        heading(ui, "Numbering:");
        for (v, l) in [("restartPage", "Restart Each Page"), ("restartSection", "Restart Each Section"), ("continuous", "Continuous")] {
            ui.radio_value(&mut f.restart, v.to_string(), tl!(l));
        }
    });
    let (ok, cancel) = buttons(ui, "OK", true);
    if ok {
        return run(app, "layout.lineNumbers", f.params());
    }
    cancel
}

// ---------------------------------------------------------------------------------------------
// Hyphenation

/// The Hyphenation dialog's fields. `zone` is in the interface unit; `limit` 0 = no limit.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HyphenationForm {
    pub auto: bool,
    pub caps: bool,
    pub zone: f32,
    pub limit: u32,
}

impl HyphenationForm {
    pub fn read(app: &WordApp) -> HyphenationForm {
        let s = &app.session.doc.settings;
        HyphenationForm {
            auto: s.auto_hyphenation,
            caps: s.hyphenate_caps,
            zone: s.hyphenation_zone.max(0.0) / unit().pt_per_unit(),
            limit: s.consecutive_hyphen_limit,
        }
    }

    /// `layout.hyphenation` parameters.
    pub fn params(&self) -> Value {
        json!({"value": self.auto, "caps": self.caps, "zone": (self.zone * unit().pt_per_unit()).max(0.0), "limit": self.limit})
    }
}

fn hyphenation_ui(app: &mut WordApp, ui: &mut Ui, f: &mut HyphenationForm) -> bool {
    ui.checkbox(&mut f.auto, tl!("Automatically hyphenate document"));
    ui.checkbox(&mut f.caps, tl!("Hyphenate words in CAPS"));
    egui::Grid::new("hyph").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
        ui.label(tl!("Hyphenation zone:"));
        ui.add(egui::DragValue::new(&mut f.zone).speed(0.01).range(0.0..=22.0 * 72.0 / unit().pt_per_unit()).measure(unit()).max_decimals(2));
        ui.end_row();
        ui.label(tl!("Limit consecutive hyphens to:"));
        let no_limit = tl!("No limit").to_string();
        ui.add(
            egui::DragValue::new(&mut f.limit).range(0..=99).custom_formatter(move |n, _| if n < 0.5 { no_limit.clone() } else { format!("{n:.0}") }),
        );
        ui.end_row();
    });
    ui.add_space(6.0);
    let manual = ui.button(tl!("Manual…")).clicked();
    let (ok, cancel) = buttons(ui, "OK", true);
    if (ok || manual) && !run(app, "layout.hyphenation", f.params()) {
        return false;
    }
    // Manual… keeps what was set here and moves on to manual hyphenation.
    if manual {
        app.dialog = crate::dialogs::Dialog::open("manualHyphenation", app);
    }
    ok || cancel || manual
}

// ---------------------------------------------------------------------------------------------
// Manual hyphenation

/// One word at a time: where it may be hyphenated and where the hyphen goes.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualForm {
    pub word: String,
    /// Where the word starts.
    pub pos: Value,
    /// Char indices a hyphen may go before.
    pub points: Vec<usize>,
    pub choice: usize,
    /// No more words.
    pub done: bool,
}

impl ManualForm {
    /// From the selection, or from the start of the document.
    pub fn start(app: &mut WordApp) -> ManualForm {
        let from = if app.session.sel.is_collapsed() { Value::Null } else { wordcraft_engine::cmd::pos_json(&app.session.sel.ordered().0) };
        let from = if from.is_null() { json!({"block": 0, "off": 0}) } else { from };
        Self::next(app, from)
    }

    /// The next word from `from`.
    pub fn next(app: &mut WordApp, from: Value) -> ManualForm {
        let c = app.session.run("layout.manualHyphenation", &json!({"from": from})).unwrap_or_else(|_| json!({"done": true}));
        let points: Vec<usize> =
            c.get("points").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|n| n as usize).collect()).unwrap_or_default();
        // The word is selected, so the page shows which one is asked about.
        if let (Some(pos), Some(word)) = (c.get("pos"), c.get("word").and_then(Value::as_str)) {
            let end = with_off(pos, pos.get("off").and_then(Value::as_u64).unwrap_or(0).saturating_add(word.len() as u64));
            let _ = app.session.run("select.range", &json!({"anchor": pos, "focus": end}));
        }
        ManualForm {
            word: c.get("word").and_then(Value::as_str).unwrap_or("").to_string(),
            pos: c.get("pos").cloned().unwrap_or(Value::Null),
            choice: c.get("suggest").and_then(Value::as_u64).map(|n| n as usize).or_else(|| points.last().copied()).unwrap_or(0),
            points,
            done: c.get("done").and_then(Value::as_bool).unwrap_or(false) || c.get("word").is_none(),
        }
    }

    /// The word's position plus `bytes`.
    fn at(&self, bytes: usize) -> Value {
        let off = self.pos.get("off").and_then(Value::as_u64).unwrap_or(0);
        with_off(&self.pos, off.saturating_add(bytes as u64))
    }

    fn byte_of(&self, ch: usize) -> usize {
        self.word.char_indices().nth(ch).map_or(self.word.len(), |(i, _)| i)
    }

    /// Yes: an optional hyphen at the chosen point, then the next word.
    pub fn accept(&mut self, app: &mut WordApp) {
        let at = self.at(self.byte_of(self.choice));
        if run(app, "caret.set", json!({"pos": at})) && run(app, "text.optionalHyphen", json!({})) {
            let after = self.at(self.word.len() + '\u{ad}'.len_utf8());
            *self = Self::next(app, after);
        }
    }

    /// No: leave the word alone and go to the next one.
    pub fn skip(&mut self, app: &mut WordApp) {
        let after = self.at(self.word.len());
        *self = Self::next(app, after);
    }
}

/// A position (`{"story", "path", "off"}`) at another offset in the same paragraph.
fn with_off(pos: &Value, off: u64) -> Value {
    let mut p = pos.as_object().cloned().unwrap_or_default();
    p.insert("off".into(), json!(off));
    Value::Object(p)
}

fn manual_ui(app: &mut WordApp, ui: &mut Ui, f: &mut ManualForm) -> bool {
    if f.done {
        ui.label(tl!("Manual hyphenation is complete."));
        ui.add_space(8.0);
        return ui.button(tl!("Close")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter));
    }
    ui.label(tl!("Hyphenate at:"));
    // The word with every possible break; the chosen one is the hyphen that will be inserted.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut from = 0;
        for pt in f.points.clone() {
            ui.label(egui::RichText::new(f.word.chars().skip(from).take(pt.saturating_sub(from)).collect::<String>()).size(16.0));
            if ui.selectable_label(f.choice == pt, egui::RichText::new("-").size(16.0)).clicked() {
                f.choice = pt;
            }
            from = pt;
        }
        ui.label(egui::RichText::new(f.word.chars().skip(from).collect::<String>()).size(16.0));
    });
    ui.add_space(8.0);
    let mut close = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tl!("Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
            if ui.button(tl!("No")).clicked() {
                f.skip(app);
            }
            if ui.add(crate::widgets::primary_button(ui, tl!("Yes"))).clicked() {
                f.accept(app);
            }
        });
    });
    close
}

// ---------------------------------------------------------------------------------------------
// Language

/// Proofing languages offered, by BCP 47 tag, with their English names (translated for display).
pub const LANGUAGES: [(&str, &str); 38] = [
    ("en-US", "English (United States)"),
    ("en-GB", "English (United Kingdom)"),
    ("en-AU", "English (Australia)"),
    ("en-CA", "English (Canada)"),
    ("en-IN", "English (India)"),
    ("en-IE", "English (Ireland)"),
    ("en-NZ", "English (New Zealand)"),
    ("en-ZA", "English (South Africa)"),
    ("de-DE", "German (Germany)"),
    ("fr-FR", "French (France)"),
    ("es-ES", "Spanish (Spain)"),
    ("es-MX", "Spanish (Mexico)"),
    ("es-419", "Spanish (Latin America)"),
    ("es-UY", "Spanish (Uruguay)"),
    ("it-IT", "Italian"),
    ("pt-BR", "Portuguese (Brazil)"),
    ("pt-PT", "Portuguese (Portugal)"),
    ("nl-NL", "Dutch"),
    ("sv-SE", "Swedish"),
    ("da-DK", "Danish"),
    ("nb-NO", "Norwegian (Bokmål)"),
    ("fi-FI", "Finnish"),
    ("et-EE", "Estonian"),
    ("pl-PL", "Polish"),
    ("cs-CZ", "Czech"),
    ("uk-UA", "Ukrainian"),
    ("ru-RU", "Russian"),
    ("sr-Cyrl-RS", "Serbian (Cyrillic)"),
    ("sr-Latn-RS", "Serbian (Latin)"),
    ("el-GR", "Greek"),
    ("tr-TR", "Turkish"),
    ("ja-JP", "Japanese"),
    ("zh-CN", "Chinese (Simplified)"),
    ("zh-TW", "Chinese (Traditional)"),
    ("ko-KR", "Korean"),
    ("id-ID", "Indonesian"),
    ("ar-SA", "Arabic"),
    ("he-IL", "Hebrew"),
];

/// The Language dialog's fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageForm {
    pub lang: String,
    pub no_proof: bool,
    pub detect: bool,
}

impl LanguageForm {
    pub fn read(app: &WordApp) -> LanguageForm {
        let s = &app.session;
        let style = s.doc.para_at(&s.sel.focus).and_then(|p| p.props.style.as_deref());
        let here = s.doc.styles.resolve_char(style, &s.typing_props());
        let lang = here.lang.unwrap_or_else(|| "en-US".into());
        LanguageForm { lang, no_proof: here.no_proof, detect: s.prefs.detect_language }
    }

    /// `review.language` parameters for OK.
    pub fn params(&self) -> Value {
        json!({"lang": self.lang, "noProof": self.no_proof, "detect": self.detect})
    }
}

/// A language's name in the interface language (the tag itself when we don't know it).
fn language_name(tag: &str) -> String {
    LANGUAGES.iter().find(|(t, _)| t.eq_ignore_ascii_case(tag)).map(|(_, n)| tl!(n).to_string()).unwrap_or_else(|| tag.to_string())
}

fn language_ui(app: &mut WordApp, ui: &mut Ui, f: &mut LanguageForm) -> bool {
    ui.label(tl!("Mark selected text as:"));
    let mut list: Vec<(String, String)> = LANGUAGES.iter().map(|(t, _)| (t.to_string(), language_name(t))).collect();
    if !LANGUAGES.iter().any(|(t, _)| t.eq_ignore_ascii_case(&f.lang)) {
        list.push((f.lang.clone(), f.lang.clone()));
    }
    list.sort_by_key(|a| a.1.to_lowercase());
    egui::Frame::NONE.stroke(egui::Stroke::new(1.0, crate::theme::Tokens::get(ui.ctx()).border_strong)).inner_margin(4.0).show(ui, |ui| {
        egui::ScrollArea::vertical().max_height(220.0).min_scrolled_height(220.0).show(ui, |ui| {
            ui.set_min_width(300.0);
            for (tag, name) in &list {
                let on = f.lang.eq_ignore_ascii_case(tag);
                let r = ui.selectable_label(on, name.as_str());
                if r.clicked() {
                    f.lang = tag.clone();
                }
            }
        });
    });
    ui.add_space(4.0);
    ui.checkbox(&mut f.no_proof, tl!("Do not check spelling or grammar"));
    ui.checkbox(&mut f.detect, tl!("Detect language automatically"));
    ui.add_space(4.0);
    let set_default = ui.button(tl!("Set As Default")).clicked();
    if set_default && run(app, "review.language", json!({"lang": f.lang, "default": true, "detect": f.detect})) {
        app.status(crate::i18n::fmt(tl!("{language} is now the document's default language."), &[("language", &language_name(&f.lang))]));
    }
    let (ok, cancel) = buttons(ui, "OK", true);
    if ok {
        return run(app, "review.language", f.params());
    }
    cancel
}

// ---------------------------------------------------------------------------------------------
// Envelopes

/// The Envelopes dialog's fields. `size` indexes [`ENVELOPE_SIZES`].
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvelopeForm {
    pub delivery: String,
    pub ret: String,
    pub omit_return: bool,
    pub size: usize,
}

impl EnvelopeForm {
    /// The delivery address starts as the selected text (an address in the letter).
    pub fn read(app: &WordApp) -> EnvelopeForm {
        let sel = if app.session.sel.is_collapsed() { String::new() } else { app.session.selected_text() };
        EnvelopeForm { delivery: sel.trim().replace("\r\n", "\n").replace('\r', "\n"), ret: String::new(), omit_return: false, size: 0 }
    }

    /// `mailings.envelopes` parameters; `add` puts the envelope at the start of this document.
    pub fn params(&self, add: bool) -> Value {
        let size = ENVELOPE_SIZES.get(self.size).map_or("Envelope #10", |s| s.0);
        let ret = if self.omit_return { "" } else { self.ret.as_str() };
        json!({"delivery": self.delivery, "return": ret, "size": size, "add": add})
    }
}

fn envelopes_ui(app: &mut WordApp, ui: &mut Ui, f: &mut EnvelopeForm) -> bool {
    ui.label(tl!("Delivery address:"));
    ui.add(egui::TextEdit::multiline(&mut f.delivery).desired_rows(4).desired_width(340.0));
    ui.horizontal(|ui| {
        ui.label(tl!("Return address:"));
        ui.checkbox(&mut f.omit_return, tl!("Omit"));
    });
    ui.add_enabled(!f.omit_return, egui::TextEdit::multiline(&mut f.ret).desired_rows(3).desired_width(340.0));
    ui.horizontal(|ui| {
        ui.label(tl!("Envelope size:"));
        let label = |i: usize| ENVELOPE_SIZES.get(i).map(|(n, w, h)| envelope_label(n, *w, *h)).unwrap_or_default();
        egui::ComboBox::from_id_salt("env_size").selected_text(label(f.size)).width(220.0).show_ui(ui, |ui| {
            for i in 0..ENVELOPE_SIZES.len() {
                ui.selectable_value(&mut f.size, i, label(i));
            }
        });
    });
    let ready = !f.delivery.trim().is_empty();
    ui.add_space(8.0);
    let (mut add, mut new, mut cancel) = (false, false, false);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            cancel = ui.button(tl!("Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            new = ui.add_enabled(ready, egui::Button::new(tl!("New Document"))).clicked();
            add = ui.add_enabled(ready, crate::widgets::primary_button(ui, tl!("Add to Document"))).clicked();
        });
    });
    if add || new {
        return run(app, "mailings.envelopes", f.params(add));
    }
    cancel
}

/// `DL   110 × 220 mm` / `#10   4.13 × 9.5 in`: metric sizes in millimetres, the others in
/// inches. The pattern is translated (the unit's name, the order) and the numbers use the
/// region's decimal separator.
pub(crate) fn envelope_label(name: &str, w: f32, h: f32) -> String {
    let short = name.strip_prefix("Envelope ").unwrap_or(name);
    let (template, w, h) = if short.starts_with('#') || short == "Monarch" {
        let i = |pt: f32| ((pt / 72.0) * 100.0).round() / 100.0;
        (tl!("{name}   {height} × {width} in"), i(w), i(h))
    } else {
        let mm = |pt: f32| (pt * 25.4 / 72.0).round();
        (tl!("{name}   {height} × {width} mm"), mm(w), mm(h))
    };
    crate::i18n::fmt(template, &[("name", short), ("height", &with_decimal(h.to_string())), ("width", &with_decimal(w.to_string()))])
}

/// A number written with `.` rewritten with the region's decimal separator (`2,625`).
fn with_decimal(n: String) -> String {
    let sep = crate::options::decimal();
    if sep == '.' { n } else { n.replace('.', sep.encode_utf8(&mut [0; 4])) }
}

// ---------------------------------------------------------------------------------------------
// Labels

/// The Labels dialog's fields. `product` indexes [`LABEL_PRODUCTS`]; `row`/`col` count from 1.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelsForm {
    pub text: String,
    pub product: usize,
    pub single: bool,
    pub row: usize,
    pub col: usize,
    /// One label per recipient (when a recipient list is loaded).
    pub recipients: bool,
}

impl LabelsForm {
    pub fn read(app: &WordApp) -> LabelsForm {
        let sel = if app.session.sel.is_collapsed() { String::new() } else { app.session.selected_text() };
        LabelsForm {
            text: sel.trim().replace("\r\n", "\n").replace('\r', "\n"),
            product: 0,
            single: false,
            row: 1,
            col: 1,
            recipients: !app.session.merge.rows.is_empty(),
        }
    }

    /// `mailings.labels` parameters.
    pub fn params(&self) -> Value {
        let id = LABEL_PRODUCTS.get(self.product).map_or("letter-3x10", |p| p.id);
        let mut v = json!({"text": self.text, "product": id, "fromRecipients": self.recipients && !self.single});
        if self.single {
            v["single"] = json!({"row": self.row, "col": self.col});
        }
        v
    }
}

/// `Address labels, 3 × 10, 2.625" × 1" (Letter)`.
fn product_label(i: usize) -> String {
    let Some(p) = LABEL_PRODUCTS.get(i) else { return String::new() };
    let kind = match p.kind {
        "shipping" => tl!("Shipping labels"),
        "return" => tl!("Return address labels"),
        _ => tl!("Address labels"),
    };
    // Inches on Letter, millimetres on A4, as the sheets are sold.
    let size = if p.paper == "A4" {
        let mm = |pt: f32| with_decimal(format!("{:.1}", pt * 25.4 / 72.0));
        crate::i18n::fmt(tl!("{width} × {height} mm"), &[("width", &mm(p.width)), ("height", &mm(p.height))])
    } else {
        let i = |pt: f32| with_decimal((((pt / 72.0) * 1000.0).round() / 1000.0).to_string());
        format!("{}\" × {}\"", i(p.width), i(p.height))
    };
    let (columns, rows) = (p.cols.to_string(), p.rows.to_string());
    crate::i18n::fmt(
        tl!("{kind}, {columns} × {rows}, {size} ({paper})"),
        &[("kind", kind), ("columns", &columns), ("rows", &rows), ("size", &size), ("paper", tl!(p.paper))],
    )
}

fn labels_ui(app: &mut WordApp, ui: &mut Ui, f: &mut LabelsForm) -> bool {
    let has_recipients = !app.session.merge.rows.is_empty();
    if has_recipients {
        ui.checkbox(&mut f.recipients, tl!("One label per recipient"));
    }
    ui.add_enabled_ui(!(has_recipients && f.recipients && !f.single), |ui| {
        ui.label(tl!("Address:"));
        ui.add(egui::TextEdit::multiline(&mut f.text).desired_rows(4).desired_width(340.0));
    });
    ui.horizontal(|ui| {
        ui.label(tl!("Label:"));
        egui::ComboBox::from_id_salt("label_product").selected_text(product_label(f.product)).width(300.0).show_ui(ui, |ui| {
            for i in 0..LABEL_PRODUCTS.len() {
                ui.selectable_value(&mut f.product, i, product_label(i));
            }
        });
    });
    let (rows, cols) = LABEL_PRODUCTS.get(f.product).map_or((1, 1), |p| (p.rows, p.cols));
    heading(ui, "Print");
    ui.radio_value(&mut f.single, false, tl!("Full page of the same label"));
    ui.horizontal(|ui| {
        ui.radio_value(&mut f.single, true, tl!("Single label"));
        ui.add_enabled_ui(f.single, |ui| {
            ui.label(tl!("Row:"));
            ui.add(egui::DragValue::new(&mut f.row).range(1..=rows));
            ui.label(tl!("Column:"));
            ui.add(egui::DragValue::new(&mut f.col).range(1..=cols));
        });
    });
    f.row = f.row.clamp(1, rows.max(1));
    f.col = f.col.clamp(1, cols.max(1));
    let ready = (has_recipients && f.recipients && !f.single) || !f.text.trim().is_empty();
    let (ok, cancel) = buttons(ui, "New Document", ready);
    if ok {
        return run(app, "mailings.labels", f.params());
    }
    cancel
}

// ---------------------------------------------------------------------------------------------
// Formula

/// The functions `table.formula` works out, offered by Paste function.
pub const FORMULA_FUNCTIONS: [&str; 6] = ["AVERAGE", "COUNT", "MAX", "MIN", "PRODUCT", "SUM"];

/// The table Formula dialog's field: the formula, suggested as Word does from the numbers next
/// to the caret's cell (`=SUM(ABOVE)`, else `=SUM(LEFT)`, else a bare `=`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormulaForm {
    pub formula: String,
}

impl FormulaForm {
    /// The caret's cell; none outside a table.
    pub fn read(app: &WordApp) -> Option<FormulaForm> {
        let s = &app.session;
        let (tp, r, c) = s.sel.focus.path.cell()?;
        let t = s.doc.table(s.sel.focus.story, &tp)?;
        let lang = wordcraft_engine::cmd::default_lang(s);
        let number = |row: Option<usize>, col: Option<usize>| {
            let cell = t.rows.get(row?)?.cells.get(col?)?;
            let text = cell.blocks.iter().filter_map(|b| b.as_para()).map(|p| p.plain_text()).collect::<Vec<_>>().join(" ");
            wordcraft_doc::locale_number::parse_number(text.trim(), lang.as_deref())
        };
        let formula = if number(r.checked_sub(1), Some(c)).is_some() {
            "=SUM(ABOVE)"
        } else if number(Some(r), c.checked_sub(1)).is_some() {
            "=SUM(LEFT)"
        } else {
            "="
        };
        Some(FormulaForm { formula: formula.into() })
    }

    /// Paste function: `NAME()` at the end of the formula.
    pub fn paste(&mut self, name: &str) {
        self.formula.push_str(name);
        self.formula.push_str("()");
    }

    /// `table.formula` parameters for OK.
    pub fn params(&self) -> Value {
        json!({"formula": self.formula.trim()})
    }
}

fn formula_ui(app: &mut WordApp, ui: &mut Ui, f: &mut FormulaForm) -> bool {
    ui.label(tl!("Formula:"));
    ui.add(egui::TextEdit::singleline(&mut f.formula).desired_width(300.0));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(tl!("Paste function:"));
        let mut picked = None;
        egui::ComboBox::from_id_salt("formula_paste").selected_text("").width(140.0).show_ui(ui, |ui| {
            for name in FORMULA_FUNCTIONS {
                if ui.selectable_label(false, name).clicked() {
                    picked = Some(name);
                }
            }
        });
        if let Some(name) = picked {
            f.paste(name);
        }
    });
    let ready = f.formula.trim().starts_with('=') && f.formula.trim().len() > 1;
    let (ok, cancel) = buttons(ui, "OK", ready);
    if ok {
        return run(app, "table.formula", f.params());
    }
    cancel
}

// ---------------------------------------------------------------------------------------------
// Sort

/// The table Sort dialog's fields: the column to sort by, the order and whether the first row is
/// a header (kept in place). `headers` is the first row's text, which names the columns when it
/// is a header.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SortForm {
    pub column: usize,
    pub descending: bool,
    pub header: bool,
    pub headers: Vec<String>,
}

impl SortForm {
    /// The caret's table; none outside a table.
    pub fn read(app: &WordApp) -> Option<SortForm> {
        let s = &app.session;
        let (tp, _, _) = s.sel.focus.path.cell()?;
        let first = s.doc.table(s.sel.focus.story, &tp)?.rows.first()?;
        let headers = first
            .cells
            .iter()
            .map(|c| c.blocks.iter().filter_map(|b| b.as_para()).map(|p| p.plain_text()).collect::<Vec<_>>().join(" ").trim().to_string())
            .collect();
        Some(SortForm { column: 0, descending: false, header: first.props.header, headers })
    }

    /// A column's name in Sort by: its header text, or `Column n`.
    pub fn column_label(&self, i: usize) -> String {
        match self.headers.get(i).filter(|h| self.header && !h.is_empty()) {
            Some(h) => h.clone(),
            None => crate::i18n::fmt(tl!("Column {n}"), &[("n", &(i + 1).to_string())]),
        }
    }

    /// `table.sort` parameters for OK.
    pub fn params(&self) -> Value {
        json!({"column": self.column, "descending": self.descending, "header": self.header})
    }
}

fn sort_ui(app: &mut WordApp, ui: &mut Ui, f: &mut SortForm) -> bool {
    heading(ui, "Sort by");
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("sort_column").selected_text(f.column_label(f.column)).width(180.0).show_ui(ui, |ui| {
            for i in 0..f.headers.len() {
                let label = f.column_label(i);
                ui.selectable_value(&mut f.column, i, label);
            }
        });
        ui.vertical(|ui| {
            ui.radio_value(&mut f.descending, false, tl!("Ascending"));
            ui.radio_value(&mut f.descending, true, tl!("Descending"));
        });
    });
    heading(ui, "My list has");
    ui.horizontal(|ui| {
        ui.radio_value(&mut f.header, true, tl!("Header row"));
        ui.radio_value(&mut f.header, false, tl!("No header row"));
    });
    let (ok, cancel) = buttons(ui, "OK", true);
    if ok {
        return run(app, "table.sort", f.params());
    }
    cancel
}

// ---------------------------------------------------------------------------------------------
// Split Cells

/// The Split Cells dialog's fields. As in Word, the defaults follow the selection: twice its
/// columns and its rows. Merge cells before split is offered, and on, when several cells (or one
/// cell merged across columns) are selected (`several`). A merged selection splits into at most
/// `max_rows` rows, a vertically merged cell alone into at most its `height`; more gets Word's
/// message ([`SplitCellsForm::check`]). Without merging each cell splits into columns on its own
/// and the row count stays 1.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitCellsForm {
    pub columns: u32,
    pub rows: u32,
    pub merge_first: bool,
    pub several: bool,
    /// The selected cells, a vertically merged cell counted once.
    pub cells: u32,
    /// The rows a single selected cell covers (more than 1 when it is merged down).
    pub height: u32,
    /// The selected rows: the default row count and its limit while merging.
    pub max_rows: u32,
    /// The default column count: twice the most cells selected in a row.
    pub default_columns: u32,
    /// Word's message for a row count out of range, shown until OK is pressed again.
    #[serde(skip)]
    pub error: Option<String>,
}

impl SplitCellsForm {
    /// The selected cells; none outside a table.
    pub fn read(app: &WordApp) -> Option<SplitCellsForm> {
        let s = &app.session;
        let (tp, _, _) = s.sel.focus.path.cell()?;
        let mut cells = wordcraft_engine::cmd::table::selected_cells(s);
        cells.sort_unstable();
        cells.dedup();
        let mut rows: Vec<usize> = cells.iter().map(|&(r, _)| r).collect();
        rows.dedup();
        let per_row = rows.iter().map(|&r| cells.iter().filter(|c| c.0 == r).count()).max().unwrap_or(1);
        // A vertically merged cell is one cell, however many rows it covers.
        let targets = wordcraft_engine::cmd::table::split_targets(s);
        let t = s.doc.table(s.sel.focus.story, &tp);
        let (span, height) = match (targets.as_slice(), t) {
            ([(top, g)], Some(t)) => t
                .cell_at_grid(*top, *g)
                .map(|ci| {
                    (t.rows.get(*top).and_then(|row| row.cells.get(ci)).map_or(1, |c| c.span()), t.vertical_extent(*top, ci).map_or(1, |(_, k)| k))
                })
                .unwrap_or((1, 1)),
            _ => (1, 1),
        };
        // Word offers Merge for one cell merged across columns too.
        let several = targets.len() > 1 || span > 1;
        let max_rows = if targets.len() > 1 { rows.len().clamp(1, 63) as u32 } else { 1 };
        let default_columns = per_row.saturating_mul(2).clamp(2, 63) as u32;
        Some(SplitCellsForm {
            columns: default_columns,
            rows: max_rows,
            merge_first: several,
            several,
            cells: targets.len().clamp(1, 63 * 63) as u32,
            height: height.clamp(1, 32_767) as u32,
            max_rows,
            default_columns,
            error: None,
        })
    }

    /// Ticks or unticks Merge cells before split; unticked, each cell splits into two columns
    /// in its own row, as Word resets the boxes.
    pub fn set_merge(&mut self, on: bool) {
        self.merge_first = on && self.several;
        (self.columns, self.rows) = if self.merge_first { (self.default_columns, self.max_rows) } else { (2, 1) };
    }

    /// Word's message when the row count is out of range: a merged selection splits into 1 to
    /// its rows, a merged cell into 1 to its height.
    pub fn check(&self) -> Option<String> {
        let limit = if self.cells > 1 && self.merge_first {
            self.max_rows
        } else if self.cells <= 1 && self.height > 1 {
            self.height
        } else {
            return None;
        };
        if self.rows > limit {
            return Some(crate::i18n::fmt(tl!("The number must be between 1 and {max}."), &[("max", &limit.to_string())]));
        }
        // A merged cell splits into a divisor of its height, as in Word (the engine refuses the rest).
        let divides = self.cells <= 1 && self.height > 1 && self.rows > 1 && !self.height.is_multiple_of(self.rows);
        divides.then(|| crate::i18n::fmt(tl!("The number must be a divisor of {max}."), &[("max", &self.height.to_string())]))
    }

    /// `table.split` parameters for OK.
    pub fn params(&self) -> Value {
        json!({"columns": self.columns, "rows": self.rows, "mergeFirst": self.several && self.merge_first})
    }
}

/// An alert line under a dialog's fields, in the error colour.
fn alert(ui: &mut Ui, text: &Option<String>) {
    if let Some(e) = text {
        ui.colored_label(ui.visuals().error_fg_color, e.as_str());
    }
}

fn split_cells_ui(app: &mut WordApp, ui: &mut Ui, f: &mut SplitCellsForm) -> bool {
    // Several cells split one by one keep their rows.
    let rows_on = f.cells <= 1 || f.merge_first;
    egui::Grid::new("split_cells").num_columns(2).show(ui, |ui| {
        ui.label(tl!("Number of columns:"));
        ui.add(egui::DragValue::new(&mut f.columns).range(1..=63));
        ui.end_row();
        ui.label(tl!("Number of rows:"));
        ui.add_enabled(rows_on, egui::DragValue::new(&mut f.rows).range(1..=63));
        ui.end_row();
    });
    let mut merge = f.merge_first;
    if ui.add_enabled(f.several, egui::Checkbox::new(&mut merge, tl!("Merge cells before split"))).changed() {
        f.set_merge(merge);
    }
    alert(ui, &f.error);
    let (ok, cancel) = buttons(ui, "OK", true);
    if ok {
        // Out of range: Word's message, and the dialog stays open.
        f.error = f.check();
        return f.error.is_none() && run(app, "table.split", f.params());
    }
    cancel
}

// ---------------------------------------------------------------------------------------------
// Convert Text to Table

/// The Convert Text to Table dialog's fields: the selected paragraphs (`lines`), where to
/// separate them (`paragraph`, `comma`, `tab` or `other` with `other_char`), the column count
/// (guessed from the text for the separator, as Word does) and the AutoFit behaviour: an initial
/// column width (`width`, "Auto" or a measurement up to 22 in), or AutoFit to contents or to the
/// window. `rows` is the resulting row count, shown read-only.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextToTableForm {
    #[serde(skip)]
    pub lines: Vec<String>,
    pub columns: u32,
    pub rows: usize,
    pub autofit: String,
    /// Initial column width as typed: "Auto" (or 0) shares the text width out.
    pub width: String,
    pub separator: String,
    pub other_char: String,
    /// Word's message for a width out of range, shown until OK is pressed again.
    #[serde(skip)]
    pub error: Option<String>,
}

impl TextToTableForm {
    /// The selected paragraphs; none when the selection can't be converted (a table inside it).
    pub fn read(app: &WordApp) -> Option<TextToTableForm> {
        let lines = wordcraft_engine::cmd::table::text_table_lines(&app.session).ok()?;
        // Tabs when the text has them, else paragraphs (Word doesn't guess commas).
        let separator = if lines.iter().any(|l| l.contains('\t')) { "tab" } else { "paragraph" };
        let mut f = TextToTableForm {
            lines,
            columns: 1,
            rows: 0,
            autofit: "initial".into(),
            width: tl!("Auto").into(),
            separator: separator.into(),
            other_char: "-".into(),
            error: None,
        };
        f.guess_columns();
        Some(f)
    }

    fn sep(&self) -> Option<char> {
        match self.separator.as_str() {
            "comma" => Some(','),
            "tab" => Some('\t'),
            "other" => Some(self.other_char.chars().next().unwrap_or('-')),
            _ => None,
        }
    }

    /// The column count the text gives with the chosen separator.
    pub fn guess_columns(&mut self) {
        let cols = wordcraft_engine::cmd::table::text_table_cells(&self.lines, self.sep(), None).1;
        self.columns = cols.clamp(1, 63) as u32;
        self.count_rows();
    }

    /// The row count for the chosen separator and columns.
    pub fn count_rows(&mut self) {
        self.rows = wordcraft_engine::cmd::table::text_table_cells(&self.lines, self.sep(), Some(self.columns as usize)).0.len();
    }

    /// The initial column width in points: 0 for Auto, `None` when it isn't a measurement from
    /// 0 to 22 in.
    fn width_pt(&self) -> Option<f32> {
        let text = self.width.trim();
        if text.is_empty() || text.eq_ignore_ascii_case("auto") || text == tl!("Auto") {
            return Some(0.0);
        }
        wordcraft_geom::parse_measure(text, unit()).filter(|pt| (0.0..=22.0 * 72.0).contains(pt))
    }

    /// Word's message when the initial column width is out of range (only with that option).
    pub fn check(&self) -> Option<String> {
        if self.autofit != "initial" || self.width_pt().is_some() {
            return None;
        }
        let sep = crate::options::decimal();
        let (min, max) = (unit().format_with(0.0, sep), unit().format_with(22.0 * 72.0, sep));
        Some(crate::i18n::fmt(tl!("The measurement must be between {min} and {max}."), &[("min", &min), ("max", &max)]))
    }

    /// `table.fromText` parameters for OK.
    pub fn params(&self) -> Value {
        let mut v = json!({"separator": self.separator, "columns": self.columns, "autofit": self.autofit});
        if self.separator == "other" {
            v["char"] = json!(self.other_char);
        }
        if self.autofit == "initial"
            && let Some(w) = self.width_pt().filter(|&w| w > 0.0)
        {
            v["width"] = json!(w);
        }
        v
    }
}

fn text_to_table_ui(app: &mut WordApp, ui: &mut Ui, f: &mut TextToTableForm) -> bool {
    let before = (f.separator.clone(), f.other_char.clone(), f.columns);
    heading(ui, "Table size");
    egui::Grid::new("text_to_table").num_columns(2).show(ui, |ui| {
        ui.label(tl!("Number of columns:"));
        ui.add(egui::DragValue::new(&mut f.columns).range(1..=63));
        ui.end_row();
        ui.label(tl!("Number of rows:"));
        ui.label(f.rows.to_string());
        ui.end_row();
    });
    heading(ui, "AutoFit behavior");
    ui.horizontal(|ui| {
        ui.radio_value(&mut f.autofit, "initial".to_string(), tl!("Initial column width:"));
        ui.add_enabled(f.autofit == "initial", egui::TextEdit::singleline(&mut f.width).desired_width(72.0));
    });
    ui.radio_value(&mut f.autofit, "contents".to_string(), tl!("AutoFit to contents"));
    ui.radio_value(&mut f.autofit, "window".to_string(), tl!("AutoFit to window"));
    heading(ui, "Separate text at");
    egui::Grid::new("text_to_table_sep").num_columns(2).show(ui, |ui| {
        ui.radio_value(&mut f.separator, "paragraph".to_string(), tl!("Paragraphs"));
        ui.radio_value(&mut f.separator, "comma".to_string(), tl!("Commas"));
        ui.end_row();
        ui.radio_value(&mut f.separator, "tab".to_string(), tl!("Tabs"));
        ui.horizontal(|ui| {
            ui.radio_value(&mut f.separator, "other".to_string(), tl!("Other:"));
            let r = ui.add(egui::TextEdit::singleline(&mut f.other_char).char_limit(1).desired_width(24.0));
            if r.changed() {
                f.separator = "other".into();
            }
        });
        ui.end_row();
    });
    if (f.separator.as_str(), f.other_char.as_str()) != (before.0.as_str(), before.1.as_str()) {
        f.guess_columns();
    } else if f.columns != before.2 {
        f.count_rows();
    }
    alert(ui, &f.error);
    let ready = f.separator != "other" || !f.other_char.is_empty();
    let (ok, cancel) = buttons(ui, "OK", ready);
    if ok {
        // Out of range: Word's message, and the dialog stays open.
        f.error = f.check();
        return f.error.is_none() && run(app, "table.fromText", f.params());
    }
    cancel
}

// ---------------------------------------------------------------------------------------------
// Convert to Text

/// The Convert to Text dialog (Table Layout › Data): what sets the cells of a row apart in the
/// text (`paragraph`, `tab`, `comma` or `other` with `other_char`). It opens on Tabs.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableToTextForm {
    pub separator: String,
    pub other_char: String,
}

impl TableToTextForm {
    /// None when the caret isn't in a table.
    pub fn read(app: &WordApp) -> Option<TableToTextForm> {
        app.session.sel.focus.path.cell()?;
        Some(TableToTextForm { separator: "tab".into(), other_char: "-".into() })
    }

    /// `table.toText` parameters for OK.
    pub fn params(&self) -> Value {
        let mut v = json!({"separator": self.separator});
        if self.separator == "other" {
            v["char"] = json!(self.other_char);
        }
        v
    }
}

fn table_to_text_ui(app: &mut WordApp, ui: &mut Ui, f: &mut TableToTextForm) -> bool {
    heading(ui, "Separate text with");
    ui.radio_value(&mut f.separator, "paragraph".to_string(), tl!("Paragraph marks"));
    ui.radio_value(&mut f.separator, "tab".to_string(), tl!("Tabs"));
    ui.radio_value(&mut f.separator, "comma".to_string(), tl!("Commas"));
    ui.horizontal(|ui| {
        ui.radio_value(&mut f.separator, "other".to_string(), tl!("Other:"));
        let r = ui.add(egui::TextEdit::singleline(&mut f.other_char).char_limit(1).desired_width(24.0));
        if r.changed() {
            f.separator = "other".into();
        }
    });
    let ready = f.separator != "other" || !f.other_char.is_empty();
    let (ok, cancel) = buttons(ui, "OK", ready);
    if ok {
        return run(app, "table.toText", f.params());
    }
    cancel
}

// ---------------------------------------------------------------------------------------------
// Size

/// The Size dialog's fields (Picture Format › Size launcher, #2573) for the selected picture or
/// shape. Lengths are in points; the fields show them in the interface unit.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeForm {
    /// Where the object sized is (the selected one when the dialog opened).
    pub at: wordcraft_doc::Pos,
    pub width: f32,
    pub height: f32,
    /// Changing one side changes the other in proportion (on for pictures, as in Word).
    pub lock: bool,
    /// The scale counts from the original size; otherwise from the size the dialog opened with.
    pub relative: bool,
    /// The size the dialog opened with.
    pub start: (f32, f32),
    /// What the crop leaves of the picture's original size: Reset's size. None for a shape, or a
    /// picture whose image can't be read.
    pub original: Option<(f32, f32)>,
    /// The original size before the crop, and a bitmap's pixels, as shown.
    pub uncropped: Option<(f32, f32)>,
    pub pixels: Option<(u32, u32)>,
    /// What the form was read from (see [`seen`]).
    #[serde(skip)]
    seen: Seen,
}

/// An object's picture, crop and size, bit for bit: what [`SizeForm::follow`] compares.
type Seen = (String, [u32; 4], [u32; 2]);

fn seen(o: &wordcraft_doc::para::InlineObject) -> Seen {
    let (w, h) = o.frame().map_or((0.0, 0.0), |(w, h, _)| (w, h));
    match o {
        wordcraft_doc::para::InlineObject::Image { media, crop, .. } => (media.clone(), crop.map(f32::to_bits), [w.to_bits(), h.to_bits()]),
        _ => (String::new(), [0; 4], [w.to_bits(), h.to_bits()]),
    }
}

impl SizeForm {
    /// The selected picture's or shape's size; None when nothing that has one is selected.
    pub fn read(app: &WordApp) -> Option<SizeForm> {
        use wordcraft_doc::para::InlineObject;
        let (at, o) = wordcraft_engine::cmd::objects::selected(&app.session)?;
        let (w, h, _) = o.frame()?;
        let full = match &o {
            InlineObject::Image { media, .. } => wordcraft_engine::cmd::objects::original_size(&app.session, media),
            _ => None,
        };
        let original = wordcraft_engine::cmd::objects::cropped_original_size(&app.session, &o);
        Some(SizeForm {
            at,
            width: w,
            height: h,
            lock: matches!(o, InlineObject::Image { .. }),
            relative: original.is_some(),
            start: (w, h),
            original,
            uncropped: full.map(|f| (f.w, f.h)),
            pixels: full.and_then(|f| f.pixels),
            seen: seen(&o),
        })
    }

    /// Follow the selection: dialogs aren't modal, so a click on the page may pick another object,
    /// and the ribbon or the handles may crop, change or resize this one. The form then starts over
    /// on what is selected, or there is nothing to size (false: close the dialog). OK and Reset
    /// never work from numbers read from another object or an older state of this one.
    pub fn follow(&mut self, app: &WordApp) -> bool {
        match wordcraft_engine::cmd::objects::selected(&app.session) {
            Some((at, o)) if at == self.at && seen(&o) == self.seen => true,
            Some(_) => SizeForm::read(app).map(|f| *self = f).is_some(),
            None => false,
        }
    }

    /// What the scale counts from.
    fn base(&self) -> (f32, f32) {
        match self.original {
            Some(o) if self.relative => o,
            _ => self.start,
        }
    }

    /// The width and height as percentages of [`Self::base`].
    pub fn scale(&self) -> (f32, f32) {
        let (bw, bh) = self.base();
        (self.width / bw.max(0.01) * 100.0, self.height / bh.max(0.01) * 100.0)
    }

    /// A new width; locked, the height follows in proportion, both kept within 1–4000 pt by one
    /// factor (as `picture.size` does), so the limit never changes the shape.
    pub fn set_width(&mut self, w: f32) {
        if self.lock && self.width > 0.0 {
            (self.width, self.height) = fit_size(w, self.height * w / self.width, true);
        } else {
            self.width = fit_size(w, self.height, false).0;
        }
    }

    /// A new height; locked, the width follows (see [`Self::set_width`]).
    pub fn set_height(&mut self, h: f32) {
        if self.lock && self.height > 0.0 {
            (self.width, self.height) = fit_size(self.width * h / self.height, h, true);
        } else {
            self.height = fit_size(self.width, h, false).1;
        }
    }

    /// The width as a percentage of [`Self::base`].
    pub fn set_scale_width(&mut self, percent: f32) {
        self.set_width(self.base().0 * percent / 100.0);
    }

    /// The height as a percentage of [`Self::base`].
    pub fn set_scale_height(&mut self, percent: f32) {
        self.set_height(self.base().1 * percent / 100.0);
    }

    /// Reset: the picture's original size, less what is cropped; one past 4000 pt (a large photo)
    /// shrinks as a whole.
    pub fn reset(&mut self) {
        if let Some((w, h)) = self.original {
            (self.width, self.height) = fit_size(w, h, true);
        }
    }

    /// `picture.size` parameters: both sides, as set here.
    pub fn params(&self) -> Value {
        json!({"width": self.width, "height": self.height, "lockAspect": false})
    }
}

/// The Size dialog; returns true to close it.
fn size_ui(app: &mut WordApp, ui: &mut Ui, f: &mut SizeForm) -> bool {
    if !f.follow(app) {
        return true;
    }
    let unit = unit();
    let k = unit.pt_per_unit();
    // A length in the interface unit that takes any typed unit (`300 px` too), applied once typed
    // (Enter, Tab or a click elsewhere, OK included) so a half-typed number never resizes the other
    // side; `None` until its value changes (while
    // typing, the field reports each keystroke as a change). The range bounds what is typed or
    // dragged only: a value already outside it (a 1 px picture is 0.75 pt) is shown as it is, not
    // clamped, which egui would report as a change and so apply unasked.
    let length = |ui: &mut Ui, label: &str, pt: f32| {
        let name = ui.label(tl!(label));
        let was = pt / k;
        let mut v = was;
        let field =
            egui::DragValue::new(&mut v).speed(1.0 / k).range(1.0 / k..=4000.0 / k).clamp_existing_to_range(false).measure(unit).max_decimals(2);
        let changed = ui.add(field.update_while_editing(false)).labelled_by(name.id).changed();
        (changed && v != was).then_some(v * k)
    };
    let percent = |ui: &mut Ui, label: &str, was: f32| {
        let name = ui.label(tl!(label));
        let mut v = was;
        let field = egui::DragValue::new(&mut v).speed(1.0).range(1.0..=10_000.0).clamp_existing_to_range(false).suffix(" %").max_decimals(1);
        let changed = ui.add(field.update_while_editing(false)).labelled_by(name.id).changed();
        (changed && v != was).then_some(v)
    };
    heading(ui, "Size");
    egui::Grid::new("size_lengths").num_columns(4).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
        if let Some(h) = length(ui, "Height:", f.height) {
            f.set_height(h);
        }
        if let Some(w) = length(ui, "Width:", f.width) {
            f.set_width(w);
        }
        ui.end_row();
    });
    heading(ui, "Scale");
    let (sw, sh) = f.scale();
    egui::Grid::new("size_scale").num_columns(4).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
        if let Some(p) = percent(ui, "Height:", sh) {
            f.set_scale_height(p);
        }
        if let Some(p) = percent(ui, "Width:", sw) {
            f.set_scale_width(p);
        }
        ui.end_row();
    });
    ui.checkbox(&mut f.lock, tl!("Lock aspect ratio"));
    ui.add_enabled(f.original.is_some(), egui::Checkbox::new(&mut f.relative, tl!("Relative to original picture size")));
    if let Some((w, h)) = f.uncropped {
        heading(ui, "Original size");
        ui.horizontal(|ui| {
            ui.label(format!("{} {}", tl!("Height:"), crate::options::length(h)));
            ui.add_space(12.0);
            ui.label(format!("{} {}", tl!("Width:"), crate::options::length(w)));
            if let Some((pw, ph)) = f.pixels {
                ui.add_space(12.0);
                let (pw, ph) = (pw.to_string(), ph.to_string());
                ui.weak(crate::i18n::fmt(tl!("{width} × {height} pixels"), &[("width", &pw), ("height", &ph)]));
            }
            ui.add_space(12.0);
            if ui.button(tl!("Reset")).clicked() {
                f.reset();
            }
        });
    }
    let (ok, cancel) = buttons(ui, "OK", true);
    if ok {
        return run(app, "picture.size", f.params());
    }
    cancel
}

// ---------------------------------------------------------------------------------------------
// Format Page Numbers

/// The Format Page Numbers dialog's fields (#1113): the number format, and where numbering
/// starts — `cont` numbers on from the section before (no `w:start`), else the pages start at
/// `start`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageNumbersForm {
    /// The section the fields were read from (see [`Self::follow`]).
    pub section: usize,
    /// OOXML number format (`decimal`, `upperRoman`, …).
    pub format: String,
    pub cont: bool,
    pub start: u32,
}

impl PageNumbersForm {
    /// The section's numbering as it is at the caret.
    pub fn read(app: &WordApp) -> PageNumbersForm {
        let sp = wordcraft_engine::cmd::page::sect(&app.session);
        PageNumbersForm {
            section: wordcraft_engine::cmd::page::section_index_of_focus(&app.session),
            format: sp.page_num_format.ooxml().to_string(),
            cont: sp.page_num_start.is_none(),
            start: sp.page_num_start.unwrap_or(1).clamp(1, 100_000),
        }
    }

    /// Follow the caret: dialogs aren't modal, so a click on the page may move it to another
    /// section. The form then shows that section's numbering, so OK never writes numbers read
    /// from another one (the Size dialog follows its object the same way).
    pub fn follow(&mut self, app: &WordApp) {
        if self.section != wordcraft_engine::cmd::page::section_index_of_focus(&app.session) {
            *self = PageNumbersForm::read(app);
        }
    }

    /// `layout.pageNumberFormat` parameters: no `start` (null) when the pages number on from the
    /// section before.
    pub fn params(&self) -> Value {
        let start = if self.cont { Value::Null } else { json!(self.start) };
        json!({"format": self.format, "start": start})
    }
}

fn page_numbers_ui(app: &mut WordApp, ui: &mut Ui, f: &mut PageNumbersForm) -> bool {
    f.follow(app);
    ui.add_space(4.0);
    ui.label(tl!("Number format"));
    let label = crate::dialogs_notes::FORMATS.iter().find(|(v, _)| *v == f.format.as_str()).map(|(_, l)| *l).unwrap_or_default();
    egui::ComboBox::from_id_salt("pnum_format").selected_text(label).width(150.0).show_ui(ui, |ui| {
        for (v, l) in crate::dialogs_notes::FORMATS {
            ui.selectable_value(&mut f.format, v.to_string(), l);
        }
    });
    heading(ui, "Page numbering");
    ui.radio_value(&mut f.cont, true, tl!("Continue from previous section"));
    ui.horizontal(|ui| {
        ui.radio_value(&mut f.cont, false, tl!("Start at:"));
        ui.add_enabled(!f.cont, egui::DragValue::new(&mut f.start).range(1..=100_000));
    });
    let (ok, cancel) = buttons(ui, "OK", true);
    if ok {
        return run(app, "layout.pageNumberFormat", f.params());
    }
    cancel
}

#[cfg(test)]
mod tests;
