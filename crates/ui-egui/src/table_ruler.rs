//! A table on the rulers, like Word's: with the caret in a table, the horizontal ruler marks the
//! caret row's column borders and the vertical ruler the bottom of each of the table's rows on
//! the page. Dragging a marker drags that border as on the page (`table_resize::grab`), except
//! that on the horizontal ruler only the column left of the border changes and the columns right
//! of it move along. A paragraph's indents and tabs in a cell are measured from the cell's text
//! area (`text_area`).

use egui::{Rect, Sense, Stroke, Ui, pos2, vec2};
use wordcraft_layout::{DocLayout, Placed};

use crate::WordApp;
use crate::theme::Tokens;

/// The cells of the caret's table drawn on `page`: (row, cell, box in page points).
fn cells(app: &WordApp, layout: &DocLayout, page: usize) -> Vec<(usize, usize, wordcraft_geom::Rect)> {
    let story = app.session.sel.focus.story;
    let (Some((tp, _, _)), Some(pg)) = (app.session.sel.focus.path.cell(), layout.pages.get(page)) else { return Vec::new() };
    pg.items
        .iter()
        .filter_map(|it| match it {
            Placed::Cell { rect, table, row, cell, story: st, .. } if *table == tp && *st == story => Some((*row, *cell, *rect)),
            _ => None,
        })
        .collect()
}

/// The caret cell's text area on `page`, as (left edge, width) in page points: its box less its
/// left and right margins. `None` when the caret isn't in a table cell drawn there.
pub(crate) fn text_area(app: &WordApp, layout: &DocLayout, page: usize) -> Option<(f32, f32)> {
    let story = app.session.sel.focus.story;
    let (tp, r, c) = app.session.sel.focus.path.cell()?;
    let (.., b) = cells(app, layout, page).into_iter().find(|(row, cell, _)| *row == r && *cell == c)?;
    let m = wordcraft_layout::cell_margins(&app.session.doc, app.session.doc.table(story, &tp)?, r, c);
    let (left, right) = (m[1].max(0.0), m[3].max(0.0));
    Some((b.x + left, (b.w - left - right).max(0.0)))
}

/// A marker on a ruler: a small grip across the bar at `r`.
fn marker(painter: &egui::Painter, r: Rect, t: &Tokens) {
    painter.rect_filled(r, 1.0, t.ruler_margin);
    painter.rect_stroke(r, 1.0, Stroke::new(1.0, t.text_dim), egui::StrokeKind::Inside);
    let c = r.center();
    let (along, across) = if r.height() > r.width() { (vec2(0.0, 3.0), vec2(1.5, 0.0)) } else { (vec2(3.0, 0.0), vec2(0.0, 1.5)) };
    for k in [-1.0, 0.0, 1.0] {
        let m = c + along * k;
        painter.line_segment([m - across, m + across], Stroke::new(1.0, t.text_dim));
    }
}

/// Marker `i`'s drag zone on the column (or row) ruler, named for screen readers and with its
/// tooltip: a press starts dragging the border at `at` (page, and page point on the border).
fn grip(app: &mut WordApp, ui: &Ui, layout: &DocLayout, zone: Rect, column: bool, i: usize, at: (usize, f32, f32)) {
    let (name, label, cursor) = if column {
        ("ruler_column", tl!("Table Column Width"), egui::CursorIcon::ResizeColumn)
    } else {
        ("ruler_row", tl!("Table Row Height"), egui::CursorIcon::ResizeRow)
    };
    let r = ui.interact(zone, ui.id().with((name, i)), Sense::drag()).on_hover_cursor(cursor);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, label));
    let r = if r.dragged() { r } else { r.on_hover_text(label) };
    if r.drag_started() {
        crate::table_resize::grab(app, layout, at.0, at.1, at.2);
    }
}

/// The horizontal ruler's markers (`bar`; the page's left edge at screen x `x0`, `scale` pixels per
/// point): one on each column border of the caret's row past the table's left edge.
pub(crate) fn columns(app: &mut WordApp, ui: &Ui, layout: &DocLayout, page: usize, bar: Rect, x0: f32, scale: f32) {
    let Some((_, r, _)) = app.session.sel.focus.path.cell() else { return };
    let t = Tokens::get(ui.ctx());
    let painter = ui.painter_at(bar.expand(1.0));
    let shift = ui.input(|i| i.modifiers.shift);
    for (i, (.., b)) in cells(app, layout, page).into_iter().filter(|(row, ..)| *row == r).enumerate() {
        // Where it is, or while a border of the table is dragged, where it's going.
        let sx = x0 + crate::table_resize::moved(app, page, shift, true, b.right()) * scale;
        marker(&painter, Rect::from_center_size(pos2(sx, bar.center().y), vec2(7.0, bar.height() - 2.0)), &t);
        let zone = Rect::from_center_size(pos2(sx, bar.center().y), vec2(8.0, bar.height()));
        grip(app, ui, layout, zone, true, i, (page, b.right(), b.y + b.h / 2.0));
    }
}

/// The vertical ruler's markers (`bar`; the page's top at screen y `y0`, `scale` pixels per
/// point): one on the bottom of each of the caret's table's rows on the page.
pub(crate) fn rows(app: &mut WordApp, ui: &Ui, layout: &DocLayout, page: usize, bar: Rect, y0: f32, scale: f32) {
    // Each row's bottom, and the cell it's taken from (a press in the middle of it grabs the
    // bottom): its shortest box, as a vertically merged cell's box spans all its rows (#640), and
    // of those the leftmost.
    let mut rows: Vec<(usize, wordcraft_geom::Rect)> = Vec::new();
    for (r, _, b) in cells(app, layout, page) {
        match rows.iter_mut().find(|(row, _)| *row == r) {
            Some((_, first)) if b.bottom() < first.bottom() - 0.01 || (b.bottom() <= first.bottom() + 0.01 && b.x < first.x) => *first = b,
            Some(_) => {}
            None => rows.push((r, b)),
        }
    }
    let t = Tokens::get(ui.ctx());
    let painter = ui.painter_at(bar.expand(1.0));
    for (r, b) in rows {
        let sy = y0 + crate::table_resize::moved(app, page, false, false, b.bottom()) * scale;
        marker(&painter, Rect::from_center_size(pos2(bar.center().x, sy), vec2(bar.width() - 2.0, 7.0)), &t);
        let zone = Rect::from_center_size(pos2(bar.center().x, sy), vec2(bar.width(), 8.0));
        grip(app, ui, layout, zone, false, r, (page, b.x + b.w / 2.0, b.bottom()));
    }
}
