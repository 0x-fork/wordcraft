//! Picture Format, Shape Format and Arrange: commands on the selected inline object.
//!
//! The selected object is the first picture/shape inside the selection (or right before the
//! caret). Picture adjustments re-encode the bitmap (originals are kept for Reset Picture).

use serde_json::{Value, json};
use wordcraft_doc::graphic::GraphicKind;
use wordcraft_doc::para::{Float, InlineObject, Wrap, WrapText};
use wordcraft_doc::props::{Rgb, TextVert, VAlign};
use wordcraft_doc::{Pos, StoryRef, TextBody};
use wordcraft_geom::Spin;

use super::sel_result;
use crate::{CmdError, CmdResult, CommandSpec, Selection, Session, p};

fn has_picture(s: &Session) -> Option<&'static str> {
    match selected(s) {
        Some((_, InlineObject::Image { .. })) => None,
        _ => Some("select a picture first"),
    }
}
fn has_object(s: &Session) -> Option<&'static str> {
    if selected(s).is_some() { None } else { Some("select a picture or shape first") }
}
fn has_floating(s: &Session) -> Option<&'static str> {
    match selected(s) {
        Some((_, InlineObject::Graphic { .. })) => Some(FROZEN),
        Some((_, o)) if o.is_floating() => None,
        _ => Some("select a floating picture or shape first"),
    }
}
/// Stories whose objects can be moved, resized and arranged: the body and headers/footers.
/// Objects in table cells (deeper paths), notes and text boxes stay where their text is.
fn movable_story(s: &Session, story: StoryRef) -> bool {
    match story {
        StoryRef::Body => true,
        StoryRef::Part(id) => {
            s.doc.parts.get(&id).is_some_and(|p| matches!(p.kind, wordcraft_doc::PartKind::Header | wordcraft_doc::PartKind::Footer))
        }
    }
}
/// Charts and diagrams are not written back on save yet, so their size, position and wrapping stay
/// as imported: they can be selected and deleted, not moved, resized or arranged.
const FROZEN: &str = "charts and diagrams can't be moved or resized yet";
/// `has_movable` for Size: charts and SmartArt WordCraft can edit are laid out again at their
/// new size.
fn has_sizable(s: &Session) -> Option<&'static str> {
    match selected(s) {
        Some((_, InlineObject::Graphic { graphic, .. })) if graphic.chart.is_some() || graphic.smart_art.is_some() => None,
        _ => has_movable(s),
    }
}
/// `has_object` for the geometry commands: a chart or diagram is frozen.
fn has_movable(s: &Session) -> Option<&'static str> {
    match selected(s) {
        Some((_, InlineObject::Graphic { .. })) => Some(FROZEN),
        _ => has_object(s),
    }
}
/// Word's own ink in a group is kept in the form a group member takes; it can't stand on its own
/// yet, so such a group stays whole.
const INK_GROUP: &str = "a group holding Word ink can't be ungrouped yet";
/// Does group `o` hold a member that can't stand on its own (Word's ink)?
fn holds_ink(o: &InlineObject) -> bool {
    matches!(o, InlineObject::Group { children, .. } if children.iter().any(|c| matches!(&c.obj, InlineObject::Graphic { graphic, .. } if graphic.kind == GraphicKind::Ink)))
}
fn has_group(s: &Session) -> Option<&'static str> {
    match selected(s) {
        Some((_, o)) if o.is_canvas() => Some("a drawing canvas can't be ungrouped"),
        Some((_, o)) if holds_ink(&o) => Some(INK_GROUP),
        Some((_, InlineObject::Group { .. })) => None,
        _ => Some("select a group first"),
    }
}
/// Shape Fill and Shape Outline: a shape, or a Drawing Canvas's background and border.
pub(crate) fn has_shape_or_canvas(s: &Session) -> Option<&'static str> {
    match selected(s) {
        Some((_, o)) if o.is_canvas() => None,
        _ => has_shape(s),
    }
}
fn can_group(s: &Session) -> Option<&'static str> {
    if picked_objects(s).len() >= 2 { None } else { Some("select two or more pictures or shapes first (Shift+click)") }
}
fn has_shape(s: &Session) -> Option<&'static str> {
    match selected(s) {
        Some((_, InlineObject::Shape { .. })) => None,
        _ => Some("select a shape first"),
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("picture.size", "Size", "Picture Format › Size", size)
            .params(
                r#"{"width"?: pt, "height"?: pt, "lockAspect"?: bool, "scale"?: percent, "original"?: bool} (scale: of the current size, or with original of the picture's original size, its pixels at 96 ppi less what is cropped; original alone restores that size; one side alone keeps the proportions unless lockAspect is false; 1–4000 pt: when the proportions are kept, a side past 4000 pt shrinks the other with it)"#,
            )
            .when(has_sizable),
        CommandSpec::new("picture.crop", "Crop", "Picture Format › Size", crop)
            .params(
                r#"{"left"?, "top"?, "right"?, "bottom"? (fractions of the source 0–0.95; the two sides of an axis keep at least 5% between them; a side left out is uncropped), "width"?: pt, "height"?: pt (the new display size, in the same undo step; a floating picture also moves by what a left/top crop trims, so the visible part stays put)}"#,
            )
            .when(has_picture),
        // Kept for scripts written before every object had alt text: `object.altText` on a picture.
        CommandSpec::new("picture.altText", "Alt Text", "Picture Format › Accessibility", |s, v| {
            p::req_str(v, "text")?;
            alt_text(s, v)
        })
        .params(r#"{"text": string, "title"?: string, "decorative"?: bool}"#)
        .when(has_picture),
        CommandSpec::new("object.altText", "Alt Text", "Shape Format › Accessibility", alt_text)
            .params(r#"{"text"?: string, "title"?: string, "decorative"?: bool} (the selected picture, shape, text box, group or chart)"#)
            .when(has_object),
        CommandSpec::new("picture.corrections", "Corrections", "Picture Format › Adjust", |s, v| {
            let b = p::f32(v, "brightness").unwrap_or(0.0).clamp(-100.0, 100.0);
            let c = p::f32(v, "contrast").unwrap_or(0.0).clamp(-100.0, 100.0);
            let sharpen = p::f32(v, "sharpen").unwrap_or(0.0).clamp(-100.0, 100.0);
            // Absolute from the original (like transparency): re-setting all three to 0
            // restores the original pixels instead of stacking on the last pass.
            let (_, o) = selected(s).ok_or_else(|| CmdError::Disabled("no picture".into()))?;
            let InlineObject::Image { media, .. } = o else { return Err(CmdError::Disabled("not a picture".into())) };
            let base = s.originals.get(&media).cloned().unwrap_or(media.clone());
            adjust_media(s, &base, move |img| {
                let mut img = img;
                if b != 0.0 {
                    img = image::DynamicImage::ImageRgba8(image::imageops::colorops::brighten(&img, (b * 2.55) as i32));
                }
                if c != 0.0 {
                    img = image::DynamicImage::ImageRgba8(image::imageops::colorops::contrast(&img, c));
                }
                if sharpen > 0.0 {
                    img = img.unsharpen(1.0 + sharpen / 50.0, 2);
                } else if sharpen < 0.0 && (-sharpen / 25.0).is_normal() {
                    // `blur` asserts on a subnormal width.
                    img = img.blur(-sharpen / 25.0);
                }
                Ok(img)
            })
        })
        .params(r#"{"brightness"?: -100..100, "contrast"?: -100..100, "sharpen"?: -100..100}"#)
        .when(has_picture),
        CommandSpec::new("picture.color", "Color", "Picture Format › Adjust", |s, v| {
            let mode = p::str(v, "mode").unwrap_or("grayscale").to_string();
            let sat = p::f32(v, "saturation").unwrap_or(100.0);
            // Absolute from the original (like transparency): saturation 100 restores it.
            let (_, o) = selected(s).ok_or_else(|| CmdError::Disabled("no picture".into()))?;
            let InlineObject::Image { media, .. } = o else { return Err(CmdError::Disabled("not a picture".into())) };
            let base = s.originals.get(&media).cloned().unwrap_or(media.clone());
            adjust_media(s, &base, move |img| Ok(recolor(img, &mode, sat)))
        })
        .params(r#"{"mode": "grayscale|sepia|washout|blackAndWhite|saturation|tint", "saturation"?: 0..400}"#)
        .when(has_picture),
        CommandSpec::new("picture.iconColor", "Icon Color", "Picture Format › Adjust", |s, v| {
            let c = p::str(v, "color").and_then(Rgb::parse).ok_or_else(|| CmdError::Params("`color` (RRGGBB) is required".into()))?;
            // One colour everywhere, transparency kept: recolours an inserted icon or other
            // one-colour line art without redrawing it.
            adjust(s, move |img| {
                let mut rgba = img.to_rgba8();
                for px in rgba.pixels_mut() {
                    px.0 = [c.0, c.1, c.2, px.0[3]];
                }
                Ok(image::DynamicImage::ImageRgba8(rgba))
            })
        })
        .params(r#"{"color": "RRGGBB"}"#)
        .when(has_picture),
        CommandSpec::new("picture.effects", "Artistic Effects", "Picture Format › Adjust", |s, v| {
            // One effect at a time, as in Word: a second one replaces the first (#2052).
            let e = p::str(v, "effect").unwrap_or("blur").to_string();
            relook(s, move |l| l.effect = (e != "none").then_some(e))
        })
        .params(r#"{"effect": "blur|sharpen|invert|posterize|pixelate|none"} (replaces the picture's effect)"#)
        .when(has_picture),
        CommandSpec::new("picture.transparency", "Transparency", "Picture Format › Adjust", |s, v| {
            let t = p::f32(v, "percent").unwrap_or(50.0).clamp(0.0, 100.0);
            // Absolute, not compounding: 0% is always the original pixels fully opaque.
            // The base is the pre-edit original (kept for Reset Picture), so dragging a
            // slider back and forth never fades what a previous pass already faded.
            // (Combining with other pixel edits then re-setting transparency drops those
            // edits; a full live-effect stack is future work.)
            let (_, o) = selected(s).ok_or_else(|| CmdError::Disabled("no picture".into()))?;
            let InlineObject::Image { media, .. } = o else { return Err(CmdError::Disabled("not a picture".into())) };
            let base = s.originals.get(&media).cloned().unwrap_or(media.clone());
            adjust_media(s, &base, move |img| {
                let mut rgba = img.to_rgba8();
                for px in rgba.pixels_mut() {
                    px.0[3] = (px.0[3] as f32 * (1.0 - t / 100.0)) as u8;
                }
                Ok(image::DynamicImage::ImageRgba8(rgba))
            })
        })
        .params(r#"{"percent": 0..100}"#)
        .when(has_picture),
        CommandSpec::new("picture.removeBackground", "Remove Background", "Picture Format › Adjust", |s, v| {
            let tol = p::f32(v, "tolerance").unwrap_or(30.0).clamp(0.0, 255.0);
            adjust(s, move |img| Ok(remove_background(img, tol)))
        })
        .params(r#"{"tolerance"?: 0..255}"#)
        .when(has_picture),
        CommandSpec::new("picture.compress", "Compress Pictures", "Picture Format › Adjust", |s, v| {
            let max = p::u64(v, "maxPixels").unwrap_or(1600).clamp(32, 20000) as u32;
            adjust(s, move |img| Ok(if img.width().max(img.height()) > max { img.resize(max, max, image::imageops::FilterType::Lanczos3) } else { img }))
        })
        .params(r#"{"maxPixels"?: n}"#)
        .when(has_picture),
        CommandSpec::new("picture.reset", "Reset Picture", "Picture Format › Adjust", reset).when(has_picture),
        CommandSpec::new("picture.change", "Change Picture", "Picture Format › Adjust", |s, v| {
            let bytes = if let Some(d) = p::str(v, "data") {
                super::insert::base64_decode(d).ok_or_else(|| CmdError::Params("bad base64".into()))?
            } else if let Some(path) = p::str(v, "path") {
                crate::io::read_file(path).map_err(CmdError::Failed)?
            } else {
                return Err(CmdError::Params("`path` or `data` required".into()));
            };
            wordcraft_geom::picture::probe(&bytes).map_err(|e| match e {
                wordcraft_geom::picture::Refusal::Unsupported(why) => CmdError::Failed(why.into()),
                _ => CmdError::Failed("not a supported image".into()),
            })?;
            let key = s.doc.add_media(bytes.clone(), super::insert::image_ext(&bytes));
            with_obj(s, |o| {
                if let InlineObject::Image { media, .. } = o {
                    *media = key.clone();
                }
            })
        })
        .params(r#"{"path"?: string, "data"?: base64}"#)
        .when(has_picture),
        CommandSpec::new("picture.style", "Picture Styles", "Picture Format › Picture Styles", |s, v| {
            // A style replaces the picture's style and border, as Word's gallery does (#2052).
            let style = p::str(v, "style").unwrap_or("simpleFrame").to_string();
            relook(s, move |l| {
                l.style = Some(style);
                l.border = None;
            })
        })
        .params(r#"{"style": "simpleFrame|thickFrame|rounded|softEdge|shadow"} (replaces the picture's style and border)"#)
        .when(has_picture),
        CommandSpec::new("picture.border", "Picture Border", "Picture Format › Picture Styles", |s, v| {
            let c = p::str(v, "color").and_then(Rgb::parse).unwrap_or(Rgb::BLACK);
            let w = p::u64(v, "width").unwrap_or(4).clamp(1, 200) as u32;
            // Replaces the picture's border, keeping its style (#2052).
            relook(s, move |l| l.border = Some((c, w)))
        })
        .params(r#"{"color"?: "RRGGBB", "width"?: px} (replaces the picture's border)"#)
        .when(has_picture),
        CommandSpec::new("arrange.rotate", "Rotate", "Layout › Arrange", |s, v| {
            // Turns and flips are the object's angle and mirroring (DrawingML `a:xfrm`): the
            // picture's pixels are never rewritten. A flip mirrors what is on the page, so a
            // rotated object's angle reverses too.
            let by = match p::str(v, "direction").unwrap_or("right") {
                "left" => Spin::new(270.0, false, false),
                "flipHorizontal" | "flipH" => Spin::new(0.0, true, false),
                "flipVertical" | "flipV" => Spin::new(0.0, false, true),
                "right" => Spin::new(90.0, false, false),
                other => return Err(CmdError::Params(format!("unknown direction `{other}`"))),
            };
            with_float(s, |f| f.set_spin(f.spin().within(by)))
        })
        .params(r#"{"direction": "right|left|flipVertical|flipHorizontal"}  (turns 90° or mirrors the selected picture, shape or text box; its pixels are kept)"#)
        .when(has_movable),
        CommandSpec::new("arrange.rotation", "Rotation", "Layout › Arrange", |s, v| {
            let deg = p::req_f32(v, "degrees")?;
            let (fh, fv) = (p::bool(v, "flipH"), p::bool(v, "flipV"));
            with_float(s, |f| {
                let cur = f.spin();
                f.set_spin(Spin::new(deg, fh.unwrap_or(cur.flip_h), fv.unwrap_or(cur.flip_v)));
            })
        })
        .params(r#"{"degrees": number, "flipH"?: bool, "flipV"?: bool}  (the exact angle, clockwise, of the selected picture, shape or text box; normalised to 0–360)"#)
        .when(has_movable),
        CommandSpec::new("arrange.wrap", "Wrap Text", "Layout › Arrange", |s, v| {
            let wrap: Wrap = serde_json::from_value(v.get("wrap").cloned().unwrap_or(json!("square"))).map_err(|e| CmdError::Params(e.to_string()))?;
            with_float(s, |f| f.wrap = wrap)
        })
        .params(r#"{"wrap": "inline|square|tight|through|topAndBottom|behindText|inFrontOfText"}"#)
        .when(has_movable),
        CommandSpec::new("arrange.wrapText", "Wrap Text Sides", "Layout › Arrange", |s, v| {
            let side = p::str(v, "side").unwrap_or("bothSides");
            let side = WrapText::from_ooxml(side).ok_or_else(|| CmdError::Params(format!("unknown side {side:?}")))?;
            with_float(s, |f| f.wrap_text = side)
        })
        .params(r#"{"side": "bothSides|left|right|largest"}  (the sides of the selected Square, Tight or Through object text flows on; largest: the wider side, line by line)"#)
        .when(has_movable),
        CommandSpec::new("arrange.moveWithText", "Move with Text", "Layout › Arrange › Wrap Text", |s, v| move_with_text(s, p::bool(v, "value").unwrap_or(true)))
            .params(r#"{"value": bool}  (true: Move with Text, the floating object's vertical position follows its paragraph; false: Fix Position on Page. It stays where it is)"#)
            .when(has_floating),
        CommandSpec::new("arrange.position", "Position", "Layout › Arrange", |s, v| {
            let preset = p::str(v, "preset").unwrap_or("middleCenter").to_string();
            let (x, y) = (p::f32(v, "x"), p::f32(v, "y"));
            let sect = super::page::sect(s);
            let size = selected(s).map(|(_, o)| obj_size(&o)).unwrap_or((72.0, 72.0));
            with_float(s, |f| {
                if f.wrap == Wrap::Inline {
                    f.wrap = Wrap::Square;
                }
                f.h_rel = wordcraft_doc::para::Anchor::Margin;
                f.v_rel = wordcraft_doc::para::Anchor::Margin;
                (f.h_align, f.v_align) = (None, None);
                let (tw, th) = (sect.text_width(), sect.text_height());
                let col = if preset.ends_with("Left") { 0.0 } else if preset.ends_with("Right") { tw - size.0 } else { (tw - size.0) / 2.0 };
                let row = if preset.starts_with("top") { 0.0 } else if preset.starts_with("bottom") { th - size.1 } else { (th - size.1) / 2.0 };
                f.x = x.unwrap_or(col);
                f.y = y.unwrap_or(row);
            })
        })
        .params(r#"{"preset"?: "topLeft|topCenter|topRight|middleLeft|middleCenter|middleRight|bottomLeft|bottomCenter|bottomRight", "x"?: pt, "y"?: pt}"#)
        .when(has_movable),
        CommandSpec::new("arrange.bringForward", "Bring Forward", "Layout › Arrange", |s, _| with_float(s, |f| f.wrap = Wrap::InFrontOfText)).when(has_movable),
        CommandSpec::new("arrange.sendBackward", "Send Backward", "Layout › Arrange", |s, _| with_float(s, |f| f.wrap = Wrap::BehindText)).when(has_movable),
        CommandSpec::new("arrange.align", "Align", "Layout › Arrange", |s, v| {
            let h = p::str(v, "value").unwrap_or("center").to_string();
            let tw = super::page::sect(s).text_width();
            let w = selected(s).map(|(_, o)| obj_size(&o).0).unwrap_or(72.0);
            with_float(s, |f| {
                if f.wrap == Wrap::Inline {
                    f.wrap = Wrap::Square;
                }
                f.h_rel = wordcraft_doc::para::Anchor::Margin;
                f.h_align = None;
                f.x = match h.as_str() {
                    "left" => 0.0,
                    "right" => tw - w,
                    _ => (tw - w) / 2.0,
                };
            })
        })
        .params(r#"{"value": "left|center|right"}"#)
        .when(has_movable),
        CommandSpec::new("arrange.selectionPane", "Selection Pane", "Layout › Arrange", objects_list).pure(),
        CommandSpec::new("arrange.bounds", "Move or Resize", "Layout › Arrange", bounds)
            .params(r#"{"width"?: pt, "height"?: pt, "x"?: pt, "y"?: pt, "page"?: n, "to"?: Pos}  (x/y: the top-left on page `page` (0-based, default its page); moving an inline object by x/y floats it. to: an In Line with Text object moves to that text position and stays inline, as a drag does)"#)
            .when(has_movable),
        CommandSpec::new("arrange.copyTo", "Copy to", "Layout › Arrange", copy_to)
            .params(r#"{"x": pt, "y": pt, "page"?: n} | {"to": Pos}  (Ctrl+drag: a copy of the selected object lands with its top-left there, or an In Line with Text one at the text position `to`, and is selected; the original and the clipboard stay as they were)"#)
            .when(has_movable),
        CommandSpec::new("arrange.nudge", "Nudge", "Layout › Arrange", |s, v| {
            let d = |k| p::f32(v, k).unwrap_or(0.0).clamp(-MAX_OFFSET, MAX_OFFSET);
            let (dx, dy) = (d("dx"), d("dy"));
            unalign(s)?;
            with_float(s, |f| {
                f.x = (f.x + dx).clamp(-MAX_OFFSET, MAX_OFFSET);
                f.y = (f.y + dy).clamp(-MAX_OFFSET, MAX_OFFSET);
            })
        })
        .params(r#"{"dx"?: pt, "dy"?: pt}"#)
        .when(has_floating),
        CommandSpec::new("arrange.group", "Group", "Layout › Arrange", group)
            .params(r#"{}  (groups the selected pictures, shapes and text boxes: the selected one plus those added with select.addObject)"#)
            .when(can_group),
        CommandSpec::new("arrange.ungroup", "Ungroup", "Layout › Arrange", ungroup).when(has_group),
        CommandSpec::new("select.addObject", "Add Object to Selection", "Home › Editing › Select", add_object)
            .params(r#"{"pos"?: Pos, "index"?: n, "objects"?: [Pos | n]}  (n: its Selection Pane index; with no object selected yet, the first one becomes the selection)"#)
            .pure(),
        CommandSpec::new("select.objects", "Select Objects", "Home › Editing › Select", |s, v| {
            let n = p::u64(v, "index").unwrap_or(0) as usize;
            let list = all_objects(s);
            let (pos, _) = list.into_iter().nth(n).ok_or_else(|| CmdError::Failed("no such object".into()))?;
            let end = Pos { off: pos.off + wordcraft_doc::para::OBJ.len_utf8(), ..pos.clone() };
            s.sel = Selection { anchor: pos, focus: end };
            sel_result(s)
        })
        .params(r#"{"index"?: n}"#)
        .pure(),
        CommandSpec::new("shape.outline", "Shape Outline", "Shape Format › Shape Styles", |s, v| {
            let c = p::str(v, "color").and_then(Rgb::parse);
            let w = p::f32(v, "width").map(|w| w.clamp(0.0, 72.0));
            with_obj(s, |o| {
                if let InlineObject::Shape { stroke, stroke_width, extra, .. } = o {
                    *stroke = c;
                    // A colour chosen here replaces an outline gradient from a file.
                    extra.outline_fill = None;
                    if let Some(w) = w {
                        *stroke_width = w;
                    }
                } else if let Some(style) = super::canvas::style_mut(o) {
                    style.stroke = c;
                    if let Some(w) = w {
                        style.stroke_width = w;
                    }
                }
            })
        })
        .params(r#"{"color": "RRGGBB" | null, "width"?: pt}  (a selected drawing canvas: its border)"#)
        .when(has_shape_or_canvas),
        CommandSpec::new("shape.effects", "Shape Effects", "Shape Format › Shape Styles", shape_effects)
            .params(
                r#"{"shadow"?: preset|{"preset"?, "color"?: "RRGGBB", "transparency"?: %, "blur"?: pt, "distance"?: pt, "angle"?: deg}|null, "glow"?: pt|{"color"?: "RRGGBB", "size"?: pt, "transparency"?: %}|null, "softEdge"?: pt|null}  (shadow presets: offsetBottomRight, offsetBottom, offsetBottomLeft, offsetRight, offsetCenter, offsetLeft, offsetTopRight, offsetTop, offsetTopLeft; an omitted key is left as it is; applies to every shape in the selection)"#,
            )
            .when(has_shape),
        CommandSpec::new("shape.change", "Change Shape", "Shape Format › Insert Shapes", |s, v| {
            let kind: wordcraft_doc::para::ShapeKind = serde_json::from_value(v.get("kind").cloned().unwrap_or(json!("rectangle"))).map_err(|e| CmdError::Params(e.to_string()))?;
            with_obj(s, |o| {
                if let InlineObject::Shape { kind: k, .. } = o {
                    *k = kind;
                }
            })
        })
        .params(r#"{"kind": string}"#)
        .when(has_shape),
        CommandSpec::new("shape.textDirection", "Text Direction", "Shape Format › Text", |s, v| {
            let dir = p::str(v, "dir").unwrap_or("horz");
            let vert = match dir {
                "horz" | "horizontal" => TextVert::Horz,
                "vert" | "rotate90" => TextVert::Vert,
                "vert270" | "rotate270" => TextVert::Vert270,
                "wordArtVert" | "stacked" => TextVert::Stacked,
                other => return Err(CmdError::Params(format!("unknown text direction `{other}`"))),
            };
            set_text_body(s, |b| b.vert = vert)
        })
        .params(r#"{"dir": "horz|vert|vert270|wordArtVert"}  (horizontal, rotate all text 90°, 270°, stacked; the selected text box or the one the caret is in)"#)
        .when(has_text_box),
        CommandSpec::new("shape.alignText", "Align Text", "Shape Format › Text", |s, v| {
            let anchor = match p::str(v, "anchor").unwrap_or("t") {
                "t" | "top" => VAlign::Top,
                "ctr" | "middle" | "center" => VAlign::Center,
                "b" | "bottom" => VAlign::Bottom,
                other => return Err(CmdError::Params(format!("unknown text alignment `{other}`"))),
            };
            set_text_body(s, |b| b.anchor = anchor)
        })
        .params(r#"{"anchor": "t|ctr|b"}  (top, middle, bottom of the box: for turned text, from the side its lines start on)"#)
        .when(has_text_box),
        CommandSpec::new("shape.link", "Create Link", "Shape Format › Text", link_text_box)
            .params(r#"{"target": Pos | n}  (the empty text box the selected box's text continues in: its position, or its Selection Pane index)"#)
            .when(can_link),
        CommandSpec::new("shape.breakLink", "Break Link", "Shape Format › Text", |s, _| {
            let (pos, id) = text_box_target(s).ok_or_else(|| CmdError::Disabled(NO_TEXT_BOX.into()))?;
            let part = s.doc.parts.get_mut(&id).ok_or_else(|| CmdError::Failed("text box vanished".into()))?;
            if part.body.next.take().is_none() {
                return Err(CmdError::Disabled("this text box isn't linked to another".into()));
            }
            edit_obj(s, &pos, |_| {})?;
            Ok(json!({"textBox": id}))
        })
        .params(r#"{}  (the text stops at the end of the selected box; the boxes after it go back to their own text)"#)
        .when(|s| match text_box_target(s) {
            Some((_, id)) if s.doc.parts.get(&id).is_some_and(|p| p.body.next.is_some()) => None,
            _ => Some("select a text box linked to another first"),
        }),
    ]
}

const NO_TEXT_BOX: &str = "select a text box first";

fn has_text_box(s: &Session) -> Option<&'static str> {
    if text_box_target(s).is_some() { None } else { Some(NO_TEXT_BOX) }
}

fn is_text_box(s: &Session, id: u32) -> bool {
    s.doc.parts.get(&id).is_some_and(|p| p.kind == wordcraft_doc::PartKind::TextBox)
}

/// The text box the Shape Format › Text commands work on: the selected one, or the one whose
/// text the caret is in. Its U+FFFC's position and its story.
pub fn text_box_target(s: &Session) -> Option<(Pos, u32)> {
    if let Some((pos, InlineObject::Shape { story: Some(id), .. })) = selected(s)
        && is_text_box(s, id)
    {
        return Some((pos, id));
    }
    let StoryRef::Part(id) = s.sel.focus.story else { return None };
    if !is_text_box(s, id) {
        return None;
    }
    let after = s.doc.text_box_anchor(id)?;
    let off = after.off.checked_sub(wordcraft_doc::para::OBJ.len_utf8())?;
    Some((Pos { off, ..after }, id))
}

/// Change the target text box's text direction, alignment or link.
pub(crate) fn set_text_body(s: &mut Session, f: impl Fn(&mut TextBody)) -> CmdResult {
    let (pos, id) = text_box_target(s).ok_or_else(|| CmdError::Disabled(NO_TEXT_BOX.into()))?;
    let part = s.doc.parts.get_mut(&id).ok_or_else(|| CmdError::Failed("text box vanished".into()))?;
    f(&mut part.body);
    let body = part.body;
    // Its paragraph is laid out again.
    edit_obj(s, &pos, |_| {})?;
    Ok(json!({"textBox": id, "body": body}))
}

fn can_link(s: &Session) -> Option<&'static str> {
    match text_box_target(s) {
        None => Some(NO_TEXT_BOX),
        Some((_, id)) if s.doc.parts.get(&id).is_some_and(|p| p.body.next.is_some()) => Some("this text box is already linked: break the link first"),
        Some(_) => None,
    }
}

/// `shape.link`: the selected text box's text continues in the target box, which must be an
/// empty text box no other box links to, and not one of those the text already runs through.
fn link_text_box(s: &mut Session, v: &Value) -> CmdResult {
    let (pos, from) = text_box_target(s).ok_or_else(|| CmdError::Disabled(NO_TEXT_BOX.into()))?;
    let target = v.get("target").ok_or_else(|| CmdError::Params("`target`: the text box to link to".into()))?;
    let at = match target.as_u64() {
        Some(n) => all_objects(s).into_iter().nth(usize::try_from(n).unwrap_or(usize::MAX)).map(|(p, _)| p),
        None => super::parse_pos(target),
    }
    .ok_or_else(|| CmdError::Params("no object there".into()))?;
    let to = match s.doc.para_at(&at).and_then(|q| q.object_at(at.off)) {
        Some(InlineObject::Shape { story: Some(id), .. }) if is_text_box(s, *id) => *id,
        _ => return Err(CmdError::Params("the target isn't a text box".into())),
    };
    if s.doc.parts.get(&from).is_some_and(|p| p.body.next.is_some()) {
        return Err(CmdError::Failed("this text box is already linked: break the link first".into()));
    }
    if to == from || s.doc.text_box_chain(from).is_some_and(|c| c.contains(&to)) {
        return Err(CmdError::Failed("a text box can't link to itself or to a box its text already runs through".into()));
    }
    if s.doc.text_box_chains().iter().any(|c| c.iter().skip(1).any(|id| *id == to)) {
        return Err(CmdError::Failed("another text box already links to that one".into()));
    }
    if !s.doc.plain_text(StoryRef::Part(to)).trim().is_empty() {
        return Err(CmdError::Failed("the text box to link to must be empty".into()));
    }
    if let Some(p) = s.doc.parts.get_mut(&from) {
        p.body.next = Some(to);
    }
    edit_obj(s, &pos, |_| {})?;
    edit_obj(s, &at, |_| {})?;
    Ok(json!({"textBox": from, "next": to}))
}

/// The selection when it is exactly one picture, shape or text box (its U+FFFC), rather than
/// text: what clicking an object selects.
pub fn object_selection(s: &Session) -> Option<(Pos, &InlineObject)> {
    let (a, b) = s.sel.ordered();
    if a.story != b.story || a.path != b.path || b.off != a.off + wordcraft_doc::para::OBJ.len_utf8() {
        return None;
    }
    let o = s.doc.para_at(&a)?.object_at(a.off)?;
    o.is_drawing().then_some((a, o))
}

/// The first picture/shape in the selection, or just before a collapsed caret.
pub fn selected(s: &Session) -> Option<(Pos, InlineObject)> {
    selected_all(s, 1).into_iter().next()
}

/// The pictures and shapes in the selection (at most `max`), or the one just before a collapsed
/// caret.
pub fn selected_all(s: &Session, max: usize) -> Vec<(Pos, InlineObject)> {
    let mut out = Vec::new();
    let (a, b) = s.sel.ordered();
    let max = if a == b { 1 } else { max };
    let story = a.story;
    let paths = if a == b { vec![a.path.clone()] } else { s.doc.paths_between(&a, &b) };
    for path in paths {
        let Some(p) = s.doc.para(story, &path) else { continue };
        for off in p.object_offsets() {
            let inside = if a == b {
                off + wordcraft_doc::para::OBJ.len_utf8() == a.off || off == a.off
            } else {
                (path != a.path || off >= a.off) && (path != b.path || off < b.off)
            };
            if !inside {
                continue;
            }
            if let Some(o) = p.object_at(off).filter(|o| o.is_drawing()) {
                out.push((Pos { story, path: path.clone(), off }, o.clone()));
                if out.len() >= max {
                    return out;
                }
            }
        }
    }
    out
}

/// Most shapes one `shape.effects` changes.
const MAX_EFFECT_SHAPES: usize = 10_000;

/// `shape.effects`: set or clear the shadow, glow and soft edges of the selected shapes.
fn shape_effects(s: &mut Session, v: &Value) -> CmdResult {
    use wordcraft_doc::effects::{Glow, Shadow, ShapeEffects};
    let color = |o: &Value| p::str(o, "color").and_then(Rgb::parse);
    // Each change is `None` (leave as it is) or `Some(new value)`.
    let shadow: Option<Option<Shadow>> = match v.get("shadow") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::Bool(b)) => Some(b.then(Shadow::default)),
        Some(Value::String(id)) if id == "none" => Some(None),
        Some(Value::String(id)) => Some(Some(Shadow::preset(id).ok_or_else(|| CmdError::Params(format!("unknown shadow preset `{id}`")))?)),
        Some(o @ Value::Object(_)) => {
            let mut sh = match p::str(o, "preset") {
                Some(id) => Shadow::preset(id).ok_or_else(|| CmdError::Params(format!("unknown shadow preset `{id}`")))?,
                None => Shadow::default(),
            };
            sh.color = color(o).unwrap_or(sh.color);
            sh.transparency = p::f32(o, "transparency").unwrap_or(sh.transparency);
            sh.blur = p::f32(o, "blur").unwrap_or(sh.blur);
            sh.distance = p::f32(o, "distance").unwrap_or(sh.distance);
            sh.angle = p::f32(o, "angle").unwrap_or(sh.angle);
            Some(Some(sh.sanitized()))
        }
        Some(_) => return Err(CmdError::Params("`shadow`: a preset name, an object or null".into())),
    };
    let glow: Option<Option<Glow>> = match v.get("glow") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(id)) if id == "none" => Some(None),
        Some(n @ Value::Number(_)) => Some(Some(Glow { size: n.as_f64().unwrap_or(0.0) as f32, ..Glow::default() }.sanitized())),
        Some(o @ Value::Object(_)) => {
            let d = Glow::default();
            Some(Some(
                Glow {
                    color: color(o).unwrap_or(d.color),
                    size: p::f32(o, "size").unwrap_or(d.size),
                    transparency: p::f32(o, "transparency").unwrap_or(d.transparency),
                }
                .sanitized(),
            ))
        }
        Some(_) => return Err(CmdError::Params("`glow`: a size in points, an object or null".into())),
    };
    let soft: Option<Option<f32>> = match v.get("softEdge") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(n @ Value::Number(_)) => Some(n.as_f64().map(|x| x as f32)),
        Some(_) => return Err(CmdError::Params("`softEdge`: a radius in points or null".into())),
    };
    // The shapes in the selection and those added to it (Shift+click).
    let mut targets: Vec<Pos> =
        selected_all(s, MAX_EFFECT_SHAPES).into_iter().filter(|(_, o)| matches!(o, InlineObject::Shape { .. })).map(|(pos, _)| pos).collect();
    for pos in picked_objects(s) {
        if !targets.contains(&pos) && matches!(s.doc.para_at(&pos).and_then(|q| q.object_at(pos.off)), Some(InlineObject::Shape { .. })) {
            targets.push(pos);
        }
    }
    if targets.is_empty() {
        return Err(CmdError::Disabled("no shape selected".into()));
    }
    let mut last = ShapeEffects::default();
    for pos in &targets {
        let o = edit_obj(s, pos, |o| {
            if let InlineObject::Shape { effects, float, .. } = o {
                let mut e = *effects;
                if let Some(x) = shadow {
                    e.shadow = x;
                }
                if let Some(x) = glow {
                    e.glow = x;
                }
                if let Some(x) = soft {
                    e.soft_edge = x;
                }
                *effects = e.sanitized();
                // Room around the shape for its shadow and glow, as Word's effect extent.
                float.effect = effects.extent();
            }
        })?;
        if let InlineObject::Shape { effects, .. } = o {
            last = effects;
        }
    }
    Ok(json!({"shapes": targets.len(), "effects": serde_json::to_value(last).unwrap_or(Value::Null)}))
}

/// Smallest width/height an object can be resized to, points (a text box keeps room for a line).
pub fn min_size(o: &InlineObject) -> f32 {
    if matches!(o, InlineObject::Shape { story: Some(_), .. }) { 18.0 } else { 4.0 }
}

/// Largest object offset or move, points.
const MAX_OFFSET: f32 = 4000.0;

/// Resize and/or move the selected object (dragging its frame or handles).
fn bounds(s: &mut Session, v: &Value) -> CmdResult {
    let (pos, obj) = selected(s).ok_or_else(|| CmdError::Disabled("no picture or shape selected".into()))?;
    let to = match (p::f32(v, "x"), p::f32(v, "y")) {
        (Some(x), Some(y)) => Some((x, y)),
        (None, None) => None,
        _ => return Err(CmdError::Params("`x` and `y` go together".into())),
    };
    let in_text = text_target(v, &obj)?;
    if in_text.is_some() && to.is_some() {
        return Err(CmdError::Params("`to` or `x`/`y`, not both".into()));
    }
    let (w0, h0) = obj_size(&obj);
    let min = min_size(&obj);
    let w = p::f32(v, "width").unwrap_or(w0).clamp(min, MAX_OFFSET);
    let h = p::f32(v, "height").unwrap_or(h0).clamp(min, MAX_OFFSET);
    if (w, h) != (w0, h0) {
        with_obj(s, |o| o.set_size(w, h))?;
    }
    let pos = match (to, in_text) {
        (Some((x, y)), _) => move_object(s, pos, p::u64(v, "page"), x, y, (w, h))?,
        (None, Some(at)) => move_in_text(s, pos, &at)?,
        (None, None) => pos,
    };
    select_object(s, &pos);
    let o = s.doc.para_at(&pos).and_then(|p| p.object_at(pos.off)).cloned();
    Ok(json!({"object": serde_json::to_value(o).unwrap_or(Value::Null), "pos": super::pos_json(&pos)}))
}

/// Select the object whose character is at `pos`.
fn select_object(s: &mut Session, pos: &Pos) {
    s.sel = Selection { anchor: pos.clone(), focus: Pos { off: pos.off + wordcraft_doc::para::OBJ.len_utf8(), ..pos.clone() } };
}

/// The text position in `v`'s `to`, for an In Line with Text `obj` (a floating one is placed by
/// `x`/`y`); None without one.
fn text_target(v: &Value, obj: &InlineObject) -> Result<Option<Pos>, CmdError> {
    let Some(to) = v.get("to").filter(|t| !t.is_null()) else { return Ok(None) };
    if obj.is_floating() {
        return Err(CmdError::Params("`to` places an In Line with Text object; move a floating one by `x` and `y`".into()));
    }
    super::parse_pos(to).map(Some).ok_or_else(|| CmdError::Params("bad `to`".into()))
}

/// The text position `to` an In Line with Text object at `pos` can land at: in the object's own
/// story (a table cell there too), not inside a field's code or such (it's clamped to a
/// character).
fn landing(s: &Session, pos: &Pos, to: &Pos) -> Result<Pos, CmdError> {
    if pos.path.depth() > 0 || !movable_story(s, pos.story) {
        return Err(CmdError::Disabled("only objects in the body text, headers and footers can be moved".into()));
    }
    if to.story != pos.story || s.doc.para_at(to).is_none() {
        return Err(CmdError::Params("`to` must be a position in the object's own text".into()));
    }
    Ok(s.doc.clamp(to))
}

/// Move the In Line with Text object at `pos` to the text position `to`, where it stays In Line
/// with Text, keeping its character formatting: Word's drag of an inline picture, which lands
/// where its dashed insertion cursor was (#2233, #2234). Dropped on itself, nothing changes.
/// Returns the object's position afterwards.
fn move_in_text(s: &mut Session, pos: Pos, to: &Pos) -> Result<Pos, CmdError> {
    let to = landing(s, &pos, to)?;
    let len = wordcraft_doc::para::OBJ.len_utf8();
    let same_para = to.path == pos.path;
    if same_para && (to.off == pos.off || to.off == pos.off + len) {
        return Ok(pos);
    }
    let para = s.doc.para_at(&pos).ok_or_else(|| CmdError::Failed("object vanished".into()))?;
    let obj = para.object_at(pos.off).cloned().ok_or_else(|| CmdError::Failed("object vanished".into()))?;
    let props = para.props_of_char(pos.off).clone();
    s.doc.delete_range(&pos, &Pos { off: pos.off + len, ..pos.clone() })?;
    let mut at = to;
    if same_para && at.off > pos.off {
        at.off = at.off.saturating_sub(len);
    }
    let at = s.doc.clamp(&at);
    s.doc.insert_object(&at, obj, &props)?;
    Ok(at)
}

/// Ctrl+drag: copy the selected object (a text box gets its own story) and move the copy to
/// (x, y), or for an In Line with Text object put it at the text position `to`, leaving the
/// original in place and the clipboard alone. The copy is selected.
fn copy_to(s: &mut Session, v: &Value) -> CmdResult {
    let (pos, obj) = selected(s).ok_or_else(|| CmdError::Disabled("no picture or shape selected".into()))?;
    let len = wordcraft_doc::para::OBJ.len_utf8();
    let end = Pos { off: pos.off + len, ..pos.clone() };
    if let Some(to) = text_target(v, &obj)? {
        if p::f32(v, "x").is_some() || p::f32(v, "y").is_some() {
            return Err(CmdError::Params("`to` or `x`/`y`, not both".into()));
        }
        let at = landing(s, &pos, &to)?;
        let frag = s.doc.copy_range(&pos, &end);
        s.sel = Selection::caret(at.clone());
        super::paste::insert(s, frag)?;
        // The copy starts where it was pasted.
        let copy = s.doc.clamp(&at);
        select_object(s, &copy);
        let o = s.doc.para_at(&copy).and_then(|p| p.object_at(copy.off)).cloned();
        return Ok(json!({"object": serde_json::to_value(o).unwrap_or(Value::Null), "pos": super::pos_json(&copy)}));
    }
    if p::f32(v, "x").is_none() || p::f32(v, "y").is_none() {
        return Err(CmdError::Params("`x` and `y` are required".into()));
    }
    let frag = s.doc.copy_range(&pos, &end);
    s.sel = Selection::caret(end.clone());
    super::paste::insert(s, frag)?;
    s.touch(); // lay out the copy before moving it
    // The copy sits right after the original; select it and move it like a drag would.
    s.sel = Selection { anchor: end.clone(), focus: Pos { off: end.off + len, ..end } };
    bounds(s, v)
}

/// Put the object at `pos` with its top-left at (x, y) on `page` (default: where it is), as a
/// drag does in Word: it floats (an inline object gets Square wrapping) and anchors to the
/// paragraph under its top edge, positioned relative to that paragraph's column and top. So the
/// text it lands on flows around it, and it moves with that text. On a page where no paragraph
/// starts, it anchors to the nearest line and is positioned on the page. Returns the object's
/// position afterwards.
///
/// A drop across pages can be thrown back: while the object sits on its old page it wraps text
/// away from the paragraph it is anchored to, so once it has moved, that paragraph can move back
/// onto the old page itself — and the object goes with it. Dropping near the top of the next
/// page does it every time (the anchor is that page's first paragraph, which fits on the old page
/// again once the object's wrap is gone). So a cross-page drop is anchored again until the object
/// really stays on the page asked for: the later passes see the pagination as it now is. A
/// same-page move (a resize, a nudge) places once, and a drop that still won't stay keeps its
/// last place rather than failing.
fn move_object(s: &mut Session, pos: Pos, page: Option<u64>, x: f32, y: f32, (w, h): (f32, f32)) -> Result<Pos, CmdError> {
    let cur = s.layout().object(&pos, s.page_hint).ok_or_else(|| CmdError::Failed("the object isn't laid out".into()))?.page;
    let target = page.map_or(cur, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let tries = if target == cur { 1 } else { 3 };
    let mut at = pos;
    for _ in 0..tries {
        at = move_object_once(s, &at, target, x, y, (w, h))?;
        s.touch();
        if s.layout().object(&at, target).is_some_and(|hit| hit.page == target) {
            break;
        }
    }
    Ok(at)
}

/// [`move_object`]'s one placement: move the object's character to the anchor under (x, y) on
/// `page` and offset it from there.
fn move_object_once(s: &mut Session, pos: &Pos, page: usize, x: f32, y: f32, (w, h): (f32, f32)) -> Result<Pos, CmdError> {
    use wordcraft_doc::para::Anchor;
    if pos.path.depth() > 0 || !movable_story(s, pos.story) {
        return Err(CmdError::Disabled("only objects in the body text, headers and footers can be moved".into()));
    }
    let story = pos.story;
    let layout = s.layout();
    let pg = layout.pages.get(page).ok_or_else(|| CmdError::Params(format!("no page {page}")))?;
    // Keep a corner of it on the page.
    let x = x.max(12.0 - w).min(pg.w - 12.0);
    let y = y.max(12.0 - h).min(pg.h.min(MAX_OFFSET) - 12.0);
    // The paragraph under the middle of its top edge: in the column it mostly lands in.
    let (anchor, on_page) = match anchor_paragraph(&s.doc, &layout, page, (x + w / 2.0, y), story) {
        Some(path) => (Pos { story, path, off: 0 }, false),
        None => (nearest_line(&layout, page, y, story).ok_or_else(|| CmdError::Params("no text on that page to anchor to".into()))?, true),
    };
    // Move its character to the anchor (unless it's already in that paragraph).
    let at = if anchor.story == pos.story && anchor.path == pos.path && !on_page {
        pos.clone()
    } else {
        let obj = s.doc.para_at(pos).and_then(|p| p.object_at(pos.off)).cloned().ok_or_else(|| CmdError::Failed("object vanished".into()))?;
        let len = wordcraft_doc::para::OBJ.len_utf8();
        s.doc.delete_range(pos, &Pos { off: pos.off + len, ..pos.clone() })?;
        let mut at = anchor;
        if at.story == pos.story && at.path == pos.path && at.off > pos.off {
            at.off = at.off.saturating_sub(len);
        }
        let at = s.doc.clamp(&at);
        s.doc.insert_object(&at, obj, &wordcraft_doc::CharProps::default())?;
        at
    };
    // First on the page where it was dropped, so the text wraps around it as it will in the end.
    edit_float(s, &at, |f| {
        if f.wrap == Wrap::Inline {
            f.wrap = Wrap::Square;
        }
        f.h_rel = Anchor::Page;
        f.v_rel = Anchor::Page;
        f.h_align = None;
        f.v_align = None;
        f.x = x.clamp(-MAX_OFFSET, MAX_OFFSET);
        f.y = y.clamp(-MAX_OFFSET, MAX_OFFSET);
    })?;
    if on_page {
        return Ok(at);
    }
    // Then offsets from its paragraph, as laid out with the object there.
    s.touch();
    let origin = s.layout().object(&at, page).ok_or_else(|| CmdError::Failed("the object isn't laid out".into()))?.origin;
    edit_float(s, &at, |f| {
        f.h_rel = Anchor::Column;
        f.v_rel = Anchor::Paragraph;
        f.x = (x - origin.x).clamp(-MAX_OFFSET, MAX_OFFSET);
        f.y = (y - origin.y).clamp(-MAX_OFFSET, MAX_OFFSET);
    })?;
    Ok(at)
}

/// Turn the selected floating object's alignment (left, centred, …) into an offset from its
/// column / paragraph that keeps it where it is, so it can be moved by an offset.
fn unalign(s: &mut Session) -> Result<(), CmdError> {
    use wordcraft_doc::para::Anchor;
    let (pos, obj) = selected(s).ok_or_else(|| CmdError::Disabled("no picture or shape selected".into()))?;
    let Some((_, _, float)) = obj.frame() else { return Ok(()) };
    if float.h_align.is_none() && float.v_align.is_none() {
        return Ok(());
    }
    let hit = s.layout().object(&pos, s.page_hint).ok_or_else(|| CmdError::Failed("the object isn't laid out".into()))?;
    let (ox, oy) = (hit.rect.x - hit.origin.x, hit.rect.y - hit.origin.y);
    edit_float(s, &pos, |f| {
        if f.h_align.is_some() {
            f.h_align = None;
            f.h_rel = Anchor::Column;
            f.x = ox.clamp(-MAX_OFFSET, MAX_OFFSET);
        }
        if f.v_align.is_some() {
            f.v_align = None;
            f.v_rel = Anchor::Paragraph;
            f.y = oy.clamp(-MAX_OFFSET, MAX_OFFSET);
        }
    })?;
    Ok(())
}

/// Whether a floating object moves with its text (Word's "Move with Text"): its vertical position
/// follows its paragraph (as layout places any anchor but the page and its margins), rather than
/// being fixed on the page ("Fix Position on Page").
pub fn moves_with_text(f: &Float) -> bool {
    use wordcraft_doc::para::Anchor;
    !matches!(f.v_rel, Anchor::Page | Anchor::Margin | Anchor::TopMargin | Anchor::BottomMargin | Anchor::InsideMargin | Anchor::OutsideMargin)
}

/// Arrange › Wrap Text › Move with Text / Fix Position on Page: anchor the selected floating
/// object's vertical position to its paragraph or to the page, keeping it where it is now.
fn move_with_text(s: &mut Session, with_text: bool) -> CmdResult {
    use wordcraft_doc::para::Anchor;
    let (pos, obj) = selected(s).ok_or_else(|| CmdError::Disabled("no picture or shape selected".into()))?;
    let Some((_, _, float)) = obj.frame() else { return Err(CmdError::Disabled("select a floating picture or shape first".into())) };
    if moves_with_text(float) == with_text {
        return Ok(json!({"object": serde_json::to_value(&obj).unwrap_or(Value::Null)}));
    }
    let hit = s.layout().object(&pos, s.page_hint).ok_or_else(|| CmdError::Failed("the object isn't laid out".into()))?;
    // Paragraph offsets start at the paragraph's top (the hit's origin), page offsets at the page's.
    let y = if with_text { hit.rect.y - hit.origin.y } else { hit.rect.y };
    let o = edit_float(s, &pos, |f| {
        f.v_rel = if with_text { Anchor::Paragraph } else { Anchor::Page };
        f.v_align = None;
        f.y = y.clamp(-MAX_OFFSET, MAX_OFFSET);
    })?;
    Ok(json!({"object": serde_json::to_value(o).unwrap_or(Value::Null)}))
}

/// The top-level paragraph of `story` under `(x, y)` on `page`: in the column under `x` (from
/// the section's column geometry), the last one starting at or above `y` (else the first
/// starting in that column; else, with no paragraph starting in that column, the last starting
/// at or above `y` anywhere on the page, else the first). Body paragraphs come from the page's
/// items; header/footer ones from its header and footer. The column matters (#884): layout
/// places a floating object when it lays out its anchor, so text laid out before that (an
/// earlier column) can't wrap around it. (Nor do lines continuing a paragraph from the column
/// before: layout breaks them once, in the column the paragraph starts in.)
pub(crate) fn anchor_paragraph(
    doc: &wordcraft_doc::Document,
    layout: &wordcraft_layout::DocLayout,
    page: usize,
    (x, y): (f32, f32),
    story: StoryRef,
) -> Option<wordcraft_doc::Path> {
    let pg = layout.pages.get(page)?;
    let items: Box<dyn Iterator<Item = &wordcraft_layout::Placed> + '_> = match story {
        StoryRef::Body => Box::new(pg.items.iter()),
        StoryRef::Part(_) => Box::new(pg.header.iter().chain(pg.footer.iter())),
    };
    // Top-level text on the page: (path, left, top, starts the paragraph).
    let pieces: Vec<(&wordcraft_doc::Path, f32, f32, bool)> = items
        .filter_map(|it| match it {
            wordcraft_layout::Placed::Lines { story: s, path, l0, x, y: top, .. } if *s == story && path.depth() == 0 => {
                Some((path, *x, *top, *l0 == 0))
            }
            _ => None,
        })
        .collect();
    let starts: Vec<(&wordcraft_doc::Path, f32)> = pieces.iter().filter(|p| p.3).map(|p| (p.0, p.2)).collect();
    // The page's columns (left, right), from its section's column geometry (headers and footers
    // span the text area); the column under `x` is the rightmost one starting at or left of it.
    let boxes = match story {
        StoryRef::Body => doc.sections().get(pg.section).map(|(_, sp)| sp.column_boxes()).unwrap_or_default(),
        StoryRef::Part(_) => Vec::new(),
    };
    let mut cols: Vec<(f32, f32)> =
        boxes.iter().map(|(cx, w)| (pg.body.x + cx, pg.body.x + cx + w)).filter(|c| c.0.is_finite() && c.1.is_finite()).collect();
    cols.sort_by(|a, b| a.0.total_cmp(&b.0));
    let col_of = |px: f32| cols.iter().rposition(|c| c.0 <= px + 0.5).unwrap_or(0);
    let col = col_of(x);
    let in_col: Vec<(&wordcraft_doc::Path, f32)> = pieces.iter().filter(|p| p.3 && col_of(p.1) == col).map(|p| (p.0, p.2)).collect();
    let pick = |v: &[(&wordcraft_doc::Path, f32)]| v.iter().rev().find(|(_, top)| *top <= y + 0.5).or(v.first()).map(|(p, _)| (*p).clone());
    pick(&in_col).or_else(|| pick(&starts))
}

/// The start of the top-level line of `story` on `page` nearest to `y`.
pub(crate) fn nearest_line(layout: &wordcraft_layout::DocLayout, page: usize, y: f32, story: StoryRef) -> Option<Pos> {
    let lines = wordcraft_layout::hit::page_lines(layout.pages.get(page)?, story);
    let best = lines.iter().filter(|l| l.path.depth() == 0).min_by(|a, b| {
        let d = |l: &wordcraft_layout::hit::LineHit| if y < l.top { l.top - y } else { (y - l.bottom).max(0.0) };
        d(a).total_cmp(&d(b))
    })?;
    let off = best.para.lines.get(best.li)?.start;
    Some(Pos { story, path: best.path.clone(), off })
}

fn obj_size(o: &InlineObject) -> (f32, f32) {
    o.frame().map_or((0.0, 0.0), |(w, h, _)| (w, h))
}

/// Alt Text: the selected object's description, title and decorative mark, in one undo step.
fn alt_text(s: &mut Session, v: &Value) -> CmdResult {
    let (text, title, decorative) = (p::str(v, "text"), p::str(v, "title"), p::bool(v, "decorative"));
    if text.is_none() && title.is_none() && decorative.is_none() {
        return Err(CmdError::Params("`text`, `title` or `decorative` is required".into()));
    }
    with_obj(s, |o| {
        if let Some(t) = text {
            o.set_alt_text(t);
        }
        if let Some(f) = o.float_mut() {
            if let Some(t) = title {
                f.alt.title = wordcraft_doc::para::cap_alt(t);
            }
            if let Some(d) = decorative {
                f.alt.decorative = d;
            }
        }
    })
}

fn with_obj(s: &mut Session, f: impl Fn(&mut InlineObject)) -> CmdResult {
    let (pos, _) = selected(s).ok_or_else(|| CmdError::Disabled("no picture or shape selected".into()))?;
    let o = edit_obj(s, &pos, f)?;
    Ok(json!({"object": serde_json::to_value(o).unwrap_or(Value::Null)}))
}

/// Change the object at `pos`; returns it afterwards.
pub(crate) fn edit_obj(s: &mut Session, pos: &Pos, f: impl Fn(&mut InlineObject)) -> Result<InlineObject, CmdError> {
    let para = s.doc.para_mut(pos.story, &pos.path)?;
    let o = para.object_at_mut(pos.off).ok_or_else(|| CmdError::Failed("object vanished".into()))?;
    f(o);
    let o = o.clone();
    para.touch();
    Ok(o)
}

fn edit_float(s: &mut Session, pos: &Pos, f: impl Fn(&mut Float)) -> Result<InlineObject, CmdError> {
    edit_obj(s, pos, |o| {
        if let Some(float) = o.float_mut() {
            f(float);
        }
    })
}

fn with_float(s: &mut Session, f: impl Fn(&mut Float)) -> CmdResult {
    with_obj(s, |o| {
        if let Some(float) = o.float_mut() {
            let was_inline = float.wrap == Wrap::Inline;
            f(float);
            // Word's distance from text for a newly wrapped object: 0.125" at the sides.
            if was_inline && float.wrap != Wrap::Inline && float.dist == 0.0 {
                float.dist = 9.0;
            }
        }
    })
}

/// `picture.crop`: the crop, and optionally the new display size, as one edit. With a size, a
/// floating picture positioned by offsets moves by what the crop on its left/top side on the page
/// trims (or restores) — for a flipped picture that's the source's right/bottom — so what stays
/// visible keeps its place on the page: a crop handle drop is one command.
fn crop(s: &mut Session, v: &Value) -> CmdResult {
    let mut c = [p::f32(v, "left"), p::f32(v, "top"), p::f32(v, "right"), p::f32(v, "bottom")].map(|x| x.unwrap_or(0.0).clamp(0.0, 0.95));
    // The two sides of an axis keep at least 5% of the source between them, as the renderer
    // draws it: the second side stops where the first one already left less than that.
    c[2] = c[2].min(0.95 - c[0]);
    c[3] = c[3].min(0.95 - c[1]);
    let size = |k: &str| p::f32(v, k).map(|x| x.clamp(1.0, 4000.0));
    let (nw, nh) = (size("width"), size("height"));
    with_obj(s, |o| {
        if let InlineObject::Image { crop, w, h, float, .. } = o {
            // The source's full size at the new display size (the visible part is 1-lo-hi of it).
            let full = |len: f32, lo: f32, hi: f32| len / (1.0 - lo - hi).max(0.05);
            // Flipped, the source's right (bottom) edge is the one on the page's left (top).
            let (left, top) = (if float.flip_h { 2 } else { 0 }, if float.flip_v { 3 } else { 1 });
            if let Some(nw) = nw {
                if float.wrap != Wrap::Inline && float.h_align.is_none() {
                    float.x += (c[left] - crop[left]) * full(nw, c[0], c[2]);
                }
                *w = nw;
            }
            if let Some(nh) = nh {
                if float.wrap != Wrap::Inline && float.v_align.is_none() {
                    float.y += (c[top] - crop[top]) * full(nh, c[1], c[3]);
                }
                *h = nh;
            }
            *crop = c;
        }
    })
}

/// A picture's original size: its image's pixels at 96 ppi (the size Insert › Pictures gives it)
/// or a metafile's own frame, before any crop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OriginalSize {
    /// Points.
    pub w: f32,
    pub h: f32,
    /// A bitmap's pixels (None for a metafile).
    pub pixels: Option<(u32, u32)>,
}

/// The original size of the picture showing `media`, read from the image its Picture Format
/// adjustments started from (Compress Pictures may have resampled the current one); None when the
/// image can't be read.
pub fn original_size(s: &Session, media: &str) -> Option<OriginalSize> {
    let key = s.originals.get(media).map_or(media, String::as_str);
    let bytes = s.doc.media.get(key).or_else(|| s.doc.media.get(media))?;
    if let Some((pw, ph)) = wordcraft_render::image_size(bytes) {
        return Some(OriginalSize { w: pw as f32 * 0.75, h: ph as f32 * 0.75, pixels: Some((pw, ph)) });
    }
    let (w, h, _) = wordcraft_render::metafile_size(bytes)?;
    Some(OriginalSize { w, h, pixels: None })
}

/// What a picture's crop leaves of its original size: the size Reset gives back and what a scale
/// of the original counts from. None for anything but a picture whose image can be read.
pub fn cropped_original_size(s: &Session, o: &InlineObject) -> Option<(f32, f32)> {
    let InlineObject::Image { media, crop, .. } = o else { return None };
    let orig = original_size(s, media)?;
    let part = |lo: f32, hi: f32| (1.0 - lo - hi).max(0.05);
    Some((orig.w * part(crop[0], crop[2]), orig.h * part(crop[1], crop[3])))
}

/// Size: `scale` is a percentage of the current size, or with `original` of what the crop leaves
/// of the original size (`original` alone gives that back: the Size dialog's Reset); then `width`
/// and `height`, where one alone keeps the proportions unless `lockAspect` is false.
fn size(s: &mut Session, v: &Value) -> CmdResult {
    let (_, o) = selected(s).ok_or_else(|| CmdError::Disabled("no object".into()))?;
    let lock = p::bool(v, "lockAspect").unwrap_or(true);
    let (mut w, mut h) = obj_size(&o);
    let scale = p::f32(v, "scale");
    if p::bool(v, "original").unwrap_or(false) {
        let (ow, oh) =
            cropped_original_size(s, &o).ok_or_else(|| CmdError::Disabled("only a picture whose image can be read has an original size".into()))?;
        let k = scale.unwrap_or(100.0) / 100.0;
        (w, h) = (ow * k, oh * k);
    } else if let Some(sc) = scale {
        w *= sc / 100.0;
        h *= sc / 100.0;
    }
    let (nw, nh) = (p::f32(v, "width"), p::f32(v, "height"));
    match (nw, nh) {
        (Some(nw), Some(nh)) => {
            w = nw;
            h = nh;
        }
        (Some(nw), None) => {
            if lock && w > 0.0 {
                h *= nw / w;
            }
            w = nw;
        }
        (None, Some(nh)) => {
            if lock && h > 0.0 {
                w *= nh / h;
            }
            h = nh;
        }
        _ => {}
    }
    // A size that keeps the proportions shrinks on both sides to fit, rather than flattening.
    let keep = match (nw, nh) {
        (Some(_), Some(_)) => false,
        (None, None) => true,
        _ => lock,
    };
    let (w, h) = fit_size(w, h, keep);
    // Any drawing: a group's members scale with its frame (its child space is kept); a chart or
    // SmartArt graphic lays itself out again at the new size.
    let theme = s.doc.settings.theme_colors.clone();
    with_obj(s, |o| {
        o.set_size(w, h);
        super::chart::redraw(o, &theme);
        super::smart_art::redraw(o, &theme);
    })
}

/// An object size within 1–4000 pt. With `keep`, both sides scale by one factor, the one nearest
/// 1 that brings them within the limits, so the proportions hold; a shape too long and thin for any
/// such factor is clamped side by side.
pub fn fit_size(w: f32, h: f32, keep: bool) -> (f32, f32) {
    let side = |x: f32| if x.is_nan() { 1.0 } else { x.clamp(1.0, 4000.0) };
    if keep && w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 {
        let (lo, hi) = ((1.0 / w).max(1.0 / h), (4000.0 / w).min(4000.0 / h));
        if lo <= hi {
            let k = if lo > 1.0 {
                lo
            } else if hi < 1.0 {
                hi
            } else {
                1.0
            };
            return (side(w * k), side(h * k));
        }
    }
    (side(w), side(h))
}

/// Apply an image operation to the selected picture's bitmap (stored as a new PNG).
fn adjust(s: &mut Session, f: impl Fn(image::DynamicImage) -> Result<image::DynamicImage, CmdError>) -> CmdResult {
    let (_, o) = selected(s).ok_or_else(|| CmdError::Disabled("no picture".into()))?;
    let InlineObject::Image { media, .. } = o else { return Err(CmdError::Disabled("not a picture".into())) };
    adjust_media(s, &media, f)
}

/// Like [`adjust`], but decodes `base` (a media key, usually the pre-edit original) instead
/// of the picture's current bitmap. The result still chains onto the originals map, so Reset
/// Picture keeps working.
fn adjust_media(s: &mut Session, base: &str, f: impl Fn(image::DynamicImage) -> Result<image::DynamicImage, CmdError>) -> CmdResult {
    let bytes = s.doc.media.get(base).cloned().ok_or_else(|| CmdError::Failed("picture data missing".into()))?;
    // Within the budget pictures are drawn within, checked from the header first (#3644, #3647).
    let img = wordcraft_geom::picture::decode(&bytes, wordcraft_geom::MAX_DECODED_BYTES).map_err(|e| match e {
        wordcraft_geom::picture::Refusal::TooLarge { .. } => too_large(),
        e => CmdError::Failed(format!("can't decode the picture: {e}")),
    })?;
    let out = f(img)?;
    let mut png = Vec::new();
    out.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| CmdError::Failed(e.to_string()))?;
    let key = s.doc.add_media(png, "png");
    // Chain onto the ultimate original (the base's, or the current picture's when the base
    // is itself derived), so Reset Picture always finds it.
    let cur = selected(s).and_then(|(_, o)| match o {
        InlineObject::Image { media, .. } => Some(media),
        _ => None,
    });
    let orig = s
        .originals
        .get(base)
        .cloned()
        .or_else(|| cur.as_ref().and_then(|m| s.originals.get(m).cloned()))
        .unwrap_or_else(|| cur.clone().unwrap_or_else(|| key.clone()));
    s.originals.entry(key.clone()).or_insert(orig);
    with_obj(s, |o| {
        if let InlineObject::Image { media, .. } = o {
            *media = key.clone();
        }
    })
}

fn too_large() -> CmdError {
    CmdError::Failed("picture is too large to edit".into())
}

/// A `w` × `h` bitmap of `fill` for an edit to draw on, refused when the picture it makes would be
/// too large to draw (two RGBA buffers within the decode budget), before it is allocated (#3611).
fn canvas(w: u32, h: u32, fill: [u8; 4]) -> Result<image::RgbaImage, CmdError> {
    if !wordcraft_geom::decode_fits(w, h, wordcraft_geom::MAX_DECODED_BYTES) {
        return Err(too_large());
    }
    Ok(image::RgbaImage::from_pixel(w, h, image::Rgba(fill)))
}

/// `w` and `h` grown by `pad` on every side, None past `u32`.
fn grown(w: u32, h: u32, pad: u32) -> Option<(u32, u32)> {
    let twice = pad.checked_mul(2)?;
    Some((w.checked_add(twice)?, h.checked_add(twice)?))
}

/// What Artistic Effects, Picture Styles and Picture Border drew on a picture's bitmap: the
/// bitmap they started from and the effect, style and border on it, so the next one draws again
/// from that bitmap instead of over the last result. Picking a second style replaces the first,
/// as in Word, instead of framing the frame (#2052). Kept in [`Session::picture_looks`] under the
/// media key of the drawn bitmap.
#[derive(Clone, Debug, Default)]
pub struct PictureLook {
    /// The media key of the bitmap before any of them.
    pub base: String,
    pub effect: Option<String>,
    pub style: Option<String>,
    pub border: Option<(Rgb, u32)>,
    /// The drawn bitmap: a media key that names other bytes later (undo then a new picture
    /// reusing the key) is not this look's.
    pub drawn: std::sync::Arc<Vec<u8>>,
}

/// Change the selected picture's effect, style or border and draw it again from the bitmap
/// under them (effect, then style, then border).
fn relook(s: &mut Session, change: impl FnOnce(&mut PictureLook)) -> CmdResult {
    let (_, o) = selected(s).ok_or_else(|| CmdError::Disabled("no picture".into()))?;
    let InlineObject::Image { media, .. } = o else { return Err(CmdError::Disabled("not a picture".into())) };
    let cur = s.doc.media.get(&media).cloned();
    let mut look = s
        .picture_looks
        .get(&media)
        .filter(|l| cur.as_ref().is_some_and(|c| std::sync::Arc::ptr_eq(c, &l.drawn)) && s.doc.media.contains_key(&l.base))
        .cloned()
        .unwrap_or_else(|| PictureLook { base: media.clone(), ..Default::default() });
    change(&mut look);
    if look.effect.is_none() && look.style.is_none() && look.border.is_none() {
        // Nothing left on it: the bitmap as it was.
        let base = look.base.clone();
        return with_obj(s, |o| {
            if let InlineObject::Image { media, .. } = o {
                *media = base.clone();
            }
        });
    }
    let l = look.clone();
    let out = adjust_media(s, &look.base, move |img| {
        let mut img = match &l.effect {
            Some(e) => artistic_effect(img, e),
            None => img,
        };
        if let Some(st) = &l.style {
            img = picture_style(img, st)?;
        }
        if let Some((c, w)) = l.border {
            img = frame(img, w, [c.0, c.1, c.2, 255])?;
        }
        Ok(img)
    })?;
    if let Some((_, InlineObject::Image { media, .. })) = selected(s)
        && let Some(drawn) = s.doc.media.get(&media).cloned()
    {
        s.picture_looks.insert(media, PictureLook { drawn, ..look });
    }
    Ok(out)
}

fn artistic_effect(img: image::DynamicImage, effect: &str) -> image::DynamicImage {
    match effect {
        "blur" => img.blur(3.0),
        "sharpen" => img.unsharpen(2.0, 2),
        "invert" => {
            let mut i = img;
            i.invert();
            i
        }
        "posterize" => {
            let mut rgba = img.to_rgba8();
            for px in rgba.pixels_mut() {
                for c in px.0.iter_mut().take(3) {
                    *c = (*c / 64) * 85;
                }
            }
            image::DynamicImage::ImageRgba8(rgba)
        }
        "pixelate" => {
            let (w, h) = (img.width().max(1), img.height().max(1));
            img.resize_exact((w / 12).max(1), (h / 12).max(1), image::imageops::FilterType::Nearest).resize_exact(
                w,
                h,
                image::imageops::FilterType::Nearest,
            )
        }
        _ => img,
    }
}

fn reset(s: &mut Session, _: &Value) -> CmdResult {
    let (_, o) = selected(s).ok_or_else(|| CmdError::Disabled("no picture".into()))?;
    let InlineObject::Image { media, .. } = o else { return Err(CmdError::Disabled("not a picture".into())) };
    let orig = s.originals.get(&media).cloned().unwrap_or(media);
    with_obj(s, |o| {
        if let InlineObject::Image { media, crop, .. } = o {
            *media = orig.clone();
            *crop = [0.0; 4];
        }
    })
}

fn recolor(img: image::DynamicImage, mode: &str, sat: f32) -> image::DynamicImage {
    let mut rgba = img.to_rgba8();
    for px in rgba.pixels_mut() {
        let [r, g, b, a] = px.0;
        let (rf, gf, bf) = (r as f32, g as f32, b as f32);
        let l = 0.299 * rf + 0.587 * gf + 0.114 * bf;
        let out = match mode {
            "grayscale" => [l, l, l],
            "sepia" => [
                (0.393 * rf + 0.769 * gf + 0.189 * bf).min(255.0),
                (0.349 * rf + 0.686 * gf + 0.168 * bf).min(255.0),
                (0.272 * rf + 0.534 * gf + 0.131 * bf).min(255.0),
            ],
            "washout" => [rf * 0.35 + 255.0 * 0.65, gf * 0.35 + 255.0 * 0.65, bf * 0.35 + 255.0 * 0.65],
            "blackAndWhite" => {
                let v = if l > 127.0 { 255.0 } else { 0.0 };
                [v, v, v]
            }
            "tint" => [l * 0.6 + 0.4 * 0x3B as f32, l * 0.6 + 0.4 * 0x5B as f32, l * 0.6 + 0.4 * 0xDB as f32],
            _ => {
                let k = sat / 100.0;
                [l + (rf - l) * k, l + (gf - l) * k, l + (bf - l) * k]
            }
        };
        px.0 = [out[0].clamp(0.0, 255.0) as u8, out[1].clamp(0.0, 255.0) as u8, out[2].clamp(0.0, 255.0) as u8, a];
    }
    image::DynamicImage::ImageRgba8(rgba)
}

/// Flood the border-connected region close to the corner colour with transparency.
fn remove_background(img: image::DynamicImage, tol: f32) -> image::DynamicImage {
    let mut rgba = img.to_rgba8();
    clear_background(&mut rgba, tol);
    image::DynamicImage::ImageRgba8(rgba)
}

/// [`remove_background`] on `rgba`, a row span at a time: besides the border's seeds, the stack
/// holds one for each run of matching pixels beside a filled span, instead of four for every
/// pixel filled (#3647). Returns the most seeds it held.
fn clear_background(rgba: &mut image::RgbaImage, tol: f32) -> usize {
    let (w, h) = rgba.dimensions();
    let Some(bg) = rgba.get_pixel_checked(0, 0).map(|p| p.0) else { return 0 };
    let close = |p: [u8; 4]| (0..3).map(|i| (p[i] as f32 - bg[i] as f32).abs()).fold(0.0f32, f32::max) <= tol;
    let at = |x: u32, y: u32| y as usize * w as usize + x as usize;
    // Not cleared yet, and close to the corner colour.
    let open = |rgba: &image::RgbaImage, seen: &[bool], x: u32, y: u32| {
        !seen.get(at(x, y)).copied().unwrap_or(true) && rgba.get_pixel_checked(x, y).is_some_and(|p| close(p.0))
    };
    let mut seen = vec![false; (w as usize) * (h as usize)];
    let mut stack = border_pixels(w, h);
    let mut most = stack.len();
    while let Some((x, y)) = stack.pop() {
        if !open(rgba, &seen, x, y) {
            continue;
        }
        let (x0, x1) = run_span(x, y, w, |xi, yi| open(rgba, &seen, xi, yi));
        for xi in x0..=x1 {
            if let Some(s) = seen.get_mut(at(xi, y)) {
                *s = true;
            }
            if let Some(p) = rgba.get_pixel_mut_checked(xi, y) {
                p.0[3] = 0;
            }
        }
        for ny in [y.checked_sub(1), y.checked_add(1).filter(|&n| n < h)].into_iter().flatten() {
            push_open_runs(&mut stack, x0, x1, ny, |xi, yi| open(rgba, &seen, xi, yi));
        }
        most = most.max(stack.len());
    }
    most
}

/// Every pixel on the image's edge, the flood-fill seeds.
fn border_pixels(w: u32, h: u32) -> Vec<(u32, u32)> {
    let mut stack = Vec::new();
    for x in 0..w {
        stack.push((x, 0));
        stack.push((x, h - 1));
    }
    for y in 0..h {
        stack.push((0, y));
        stack.push((w - 1, y));
    }
    stack
}

/// The horizontal run of open pixels around `(x, y)`, both ends included.
fn run_span(x: u32, y: u32, w: u32, open: impl Fn(u32, u32) -> bool) -> (u32, u32) {
    let (mut x0, mut x1) = (x, x);
    while x0 > 0 && open(x0 - 1, y) {
        x0 -= 1;
    }
    while x1 + 1 < w && open(x1 + 1, y) {
        x1 += 1;
    }
    (x0, x1)
}

/// Queues the first pixel of each open run in row `ny` between `x0` and `x1`.
fn push_open_runs(stack: &mut Vec<(u32, u32)>, x0: u32, x1: u32, ny: u32, open: impl Fn(u32, u32) -> bool) {
    let mut xi = x0;
    while xi <= x1 {
        if open(xi, ny) {
            stack.push((xi, ny));
            while xi <= x1 && open(xi, ny) {
                xi += 1;
            }
        } else {
            xi += 1;
        }
    }
}

fn frame(img: image::DynamicImage, border: u32, color: [u8; 4]) -> Result<image::DynamicImage, CmdError> {
    let (w, h) = grown(img.width(), img.height(), border).ok_or_else(too_large)?;
    let mut out = canvas(w, h, color)?;
    image::imageops::overlay(&mut out, &img.to_rgba8(), border as i64, border as i64);
    Ok(image::DynamicImage::ImageRgba8(out))
}

fn picture_style(img: image::DynamicImage, style: &str) -> Result<image::DynamicImage, CmdError> {
    let m = (img.width().max(img.height()) / 40).max(4);
    Ok(match style {
        "thickFrame" => frame(frame(img, m * 2, [255, 255, 255, 255])?, m / 2, [40, 40, 40, 255])?,
        "rounded" | "softEdge" => {
            let mut rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            let r = (w.min(h) as f32) * if style == "rounded" { 0.12 } else { 0.0 };
            let feather = if style == "softEdge" { (w.min(h) as f32 * 0.08).max(2.0) } else { 1.5 };
            for y in 0..h {
                for x in 0..w {
                    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let dx = (r - fx).max(fx - (w as f32 - r)).max(0.0);
                    let dy = (r - fy).max(fy - (h as f32 - r)).max(0.0);
                    let edge = (fx.min(w as f32 - fx)).min(fy.min(h as f32 - fy));
                    let inside = if r > 0.0 && dx > 0.0 && dy > 0.0 { r - (dx * dx + dy * dy).sqrt() } else { edge };
                    let k = (inside / feather).clamp(0.0, 1.0);
                    let p = rgba.get_pixel_mut(x, y);
                    p.0[3] = (p.0[3] as f32 * k) as u8;
                }
            }
            image::DynamicImage::ImageRgba8(rgba)
        }
        "shadow" => {
            let (w, h) = (img.width(), img.height());
            let off = m;
            let (pw, ph) = grown(w, h, off).ok_or_else(too_large)?;
            let mut out = canvas(pw, ph, [0, 0, 0, 0])?;
            // A rectangle of alpha 90 the picture's size, one and a half margins in, blurred. Its
            // blur is the product of its sides' blurs along each axis, so it is drawn from two
            // profiles, not by blurring every pixel with a kernel that grows with the picture (#3647).
            let (sigma, start) = (off as f32 / 2.0, off + off / 2);
            let (xs, ys) = (blurred_span(pw, start, w, sigma), blurred_span(ph, start, h, sigma));
            for (x, y, p) in out.enumerate_pixels_mut() {
                let k = xs.get(x as usize).copied().unwrap_or(0.0) * ys.get(y as usize).copied().unwrap_or(0.0);
                p.0[3] = (90.0 * k).round() as u8;
            }
            image::imageops::overlay(&mut out, &img.to_rgba8(), 0, 0);
            image::DynamicImage::ImageRgba8(out)
        }
        _ => frame(img, m, [255, 255, 255, 255])?,
    })
}

/// How much of a span `len` long from `start` covers each of `n` positions once blurred with the
/// Gaussian `image::imageops::blur` uses for `sigma` (outside the `n` positions counts as clear):
/// 0 to 1. Each position is one difference of the kernel's running sums.
fn blurred_span(n: u32, start: u32, len: u32, sigma: f32) -> Vec<f32> {
    let kernel = gaussian_kernel(sigma);
    let mut sums = vec![0.0f32];
    for k in &kernel {
        sums.push(sums.last().copied().unwrap_or(0.0) + k);
    }
    let (size, r) = (kernel.len() as i64, (kernel.len() / 2) as i64);
    let sum_to = |k: i64| sums.get(k.clamp(0, size) as usize).copied().unwrap_or(0.0);
    let (start, end) = (i64::from(start), i64::from(start) + i64::from(len));
    // Position i takes weight k from i + k - r, inside the span for k in [start - i + r, end - i + r).
    (0..i64::from(n)).map(|i| sum_to(end - i + r) - sum_to(start - i + r)).collect()
}

/// The normalized kernel `image::imageops::blur` (0.25) uses for `sigma`: odd, about 6.7 σ wide.
fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let size = ((((sigma - 0.8) / 0.3 + 1.0) * 2.0 + 1.0).max(3.0) as usize) | 1;
    let mean = (size / 2) as f32;
    let weights: Vec<f32> = (0..size).map(|x| (-0.5 * ((x as f32 - mean) / sigma).powi(2)).exp()).collect();
    let total: f32 = weights.iter().sum();
    weights.into_iter().map(|v| v / total).collect()
}

/// The selected pictures, shapes, text boxes and groups: the one the selection holds, then
/// those added to it ([`Session::also_selected`]), each once, in the body.
pub fn picked_objects(s: &Session) -> Vec<Pos> {
    let Some((first, _)) = object_selection(s) else { return Vec::new() };
    let mut out = vec![first];
    for p in &s.also_selected {
        if !out.contains(p) && s.doc.para_at(p).and_then(|q| q.object_at(p.off)).is_some_and(InlineObject::is_drawing) {
            out.push(p.clone());
        }
    }
    out
}

/// The object a `select.addObject` item names: a position, or a Selection Pane index.
fn object_ref(s: &Session, v: &Value) -> Result<Pos, CmdError> {
    let pos = match v.as_u64() {
        Some(n) => all_objects(s).into_iter().nth(usize::try_from(n).unwrap_or(usize::MAX)).map(|(p, _)| p),
        None => super::parse_pos(v),
    };
    pos.filter(|p| movable_story(s, p.story) && s.doc.para_at(p).and_then(|q| q.object_at(p.off)).is_some_and(InlineObject::is_drawing))
        .ok_or_else(|| CmdError::Params("no picture or shape there".into()))
}

/// Add objects to the selection (Shift+click): the first one is selected if no object is yet;
/// one already selected is taken out again.
fn add_object(s: &mut Session, v: &Value) -> CmdResult {
    let mut items: Vec<Value> = match v.get("objects").and_then(Value::as_array) {
        Some(a) => a.iter().take(1000).cloned().collect(),
        None => Vec::new(),
    };
    if let Some(p) = v.get("pos") {
        items.push(p.clone());
    }
    if let Some(i) = v.get("index") {
        items.push(i.clone());
    }
    if items.is_empty() {
        return Err(CmdError::Params("`pos`, `index` or `objects` required".into()));
    }
    for it in &items {
        let pos = object_ref(s, it)?;
        match object_selection(s).map(|(p, _)| p) {
            None => {
                s.also_selected.clear();
                s.sel = Selection { anchor: pos.clone(), focus: Pos { off: pos.off + wordcraft_doc::para::OBJ.len_utf8(), ..pos } };
            }
            Some(first) if first == pos => {}
            Some(_) => {
                if let Some(i) = s.also_selected.iter().position(|p| *p == pos) {
                    s.also_selected.remove(i);
                } else {
                    s.also_selected.push(pos);
                }
            }
        }
    }
    Ok(json!({"selected": picked_objects(s).iter().map(super::pos_json).collect::<Vec<_>>()}))
}

/// Group the selected objects (Layout › Arrange › Group): one floating group object, anchored
/// where the first of them was, in the box around them all; they keep their places and sizes.
/// A group among them is merged in (its members join the new group).
fn group(s: &mut Session, _: &Value) -> CmdResult {
    use wordcraft_doc::para::GroupChild;
    let mut picks = picked_objects(s);
    if picks.len() < 2 {
        return Err(CmdError::Disabled("select two or more pictures or shapes first".into()));
    }
    picks.sort();
    let layout = s.layout();
    let mut found = Vec::new();
    let story = picks.first().cloned().map(|p| p.story).ok_or_else(|| CmdError::Failed("nothing to group".into()))?;
    if picks.iter().any(|p| p.story != story) {
        return Err(CmdError::Disabled("objects in different stories (body, header, footer) can't be grouped".into()));
    }
    for pos in &picks {
        if pos.path.depth() > 0 || !movable_story(s, pos.story) {
            return Err(CmdError::Disabled("only objects in the body text, headers and footers (not in tables) can be grouped".into()));
        }
        let obj = s.doc.para_at(pos).and_then(|p| p.object_at(pos.off)).cloned().ok_or_else(|| CmdError::Failed("object vanished".into()))?;
        if matches!(obj, InlineObject::Graphic { .. }) {
            return Err(CmdError::Disabled(FROZEN.into()));
        }
        if obj.is_canvas() {
            return Err(CmdError::Disabled("a drawing canvas can't be grouped".into()));
        }
        if !obj.is_floating() {
            return Err(CmdError::Disabled("objects in line with text can't be grouped: choose a text wrapping for them first".into()));
        }
        let hit = layout.object(pos, s.page_hint).ok_or_else(|| CmdError::Failed("the object isn't laid out".into()))?;
        found.push((obj, hit));
    }
    let page = found.first().map(|(_, h)| h.page).unwrap_or(0);
    if found.iter().any(|(_, h)| h.page != page) {
        return Err(CmdError::Disabled("objects on different pages can't be grouped".into()));
    }
    // The members as they show on the page: each one's unturned frame and turn (a group among
    // them gives its members, its turn taken into theirs).
    let mut shown: Vec<([f32; 4], Spin, &InlineObject)> = Vec::new();
    for (obj, hit) in &found {
        let r = hit.rect;
        match obj {
            InlineObject::Group { .. } => shown.extend(shown_members(obj, [r.x, r.y, r.w, r.h])),
            _ => shown.push(([r.x, r.y, r.w, r.h], hit.spin, obj)),
        }
    }
    // The group's box takes in what they cover, turned.
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for ([x, y, w, h], spin, _) in &shown {
        let b = spin.bounds(wordcraft_geom::Rect::new(*x, *y, *w, *h));
        (x0, y0) = (x0.min(b.x), y0.min(b.y));
        (x1, y1) = (x1.max(b.right()), y1.max(b.bottom()));
    }
    if shown.is_empty() {
        return Err(CmdError::Failed("nothing to group".into()));
    }
    // `shown` already has the members of any picked group merged in, so this counts the
    // flattened members. Readers stop at the cap and would drop the rest.
    if shown.len() > wordcraft_doc::para::MAX_GROUP_CHILDREN {
        return Err(CmdError::Disabled(format!("too many objects to group (at most {})", wordcraft_doc::para::MAX_GROUP_CHILDREN)));
    }
    let (w, h) = ((x1 - x0).clamp(1.0, MAX_OFFSET), (y1 - y0).clamp(1.0, MAX_OFFSET));
    // Each member keeps its turn: the group itself starts unturned.
    let children: Vec<GroupChild> = shown
        .into_iter()
        .map(|([x, y, cw, ch], spin, obj)| {
            let mut obj = obj.clone();
            obj.set_size(cw, ch);
            if let Some(f) = obj.float_mut() {
                // Each member keeps its own alt text.
                let alt = std::mem::take(&mut f.alt);
                *f = Float { alt, ..Float::default() };
                f.set_spin(spin);
            }
            GroupChild { x: x - x0, y: y - y0, obj }
        })
        .collect();
    // The group takes the first member's placement; its members keep their own alt text.
    let mut float = found.first().and_then(|(o, _)| o.frame()).map(|(_, _, f)| Float { alt: Default::default(), ..f.clone() }).unwrap_or_default();
    float.set_spin(Spin::default());
    let grouped = InlineObject::Group { w, h, float, ch_w: w, ch_h: h, children, canvas: None };
    // Take the members out, last first so the earlier positions hold, and put the group where
    // the first one was.
    let len = wordcraft_doc::para::OBJ.len_utf8();
    for pos in picks.iter().rev() {
        s.doc.delete_range(pos, &Pos { off: pos.off + len, ..pos.clone() })?;
    }
    let first = picks.first().cloned().ok_or_else(|| CmdError::Failed("nothing to group".into()))?;
    let at = s.doc.clamp(&first);
    s.doc.insert_object(&at, grouped, &wordcraft_doc::CharProps::default())?;
    s.touch();
    let pos = move_object(s, at, Some(page as u64), x0, y0, (w, h))?;
    s.also_selected.clear();
    s.sel = Selection { anchor: pos.clone(), focus: Pos { off: pos.off + len, ..pos.clone() } };
    let o = s.doc.para_at(&pos).and_then(|p| p.object_at(pos.off)).cloned();
    Ok(json!({"object": serde_json::to_value(o).unwrap_or(Value::Null), "pos": super::pos_json(&pos)}))
}

/// Split the selected group back into its pictures and shapes (Layout › Arrange › Ungroup), in
/// the places and sizes the group shows them, anchored where the group was. They stay
/// selected, so Group puts them back together.
fn ungroup(s: &mut Session, _: &Value) -> CmdResult {
    let (pos, obj) = selected(s).ok_or_else(|| CmdError::Disabled("select a group first".into()))?;
    let InlineObject::Group { w, h, float, .. } = &obj else { return Err(CmdError::Disabled("select a group first".into())) };
    if holds_ink(&obj) {
        return Err(CmdError::Disabled(INK_GROUP.into()));
    }
    // Its offsets, explicit: the members are placed from the same anchor.
    let pos = if float.wrap == Wrap::Inline {
        if pos.path.depth() > 0 || !movable_story(s, pos.story) {
            return Err(CmdError::Disabled("only groups in the body text, headers and footers can be ungrouped in line with text".into()));
        }
        let hit = s.layout().object(&pos, s.page_hint).ok_or_else(|| CmdError::Failed("the object isn't laid out".into()))?;
        move_object(s, pos, Some(hit.page as u64), hit.rect.x, hit.rect.y, (*w, *h))?
    } else {
        unalign(s)?;
        pos
    };
    let obj = s.doc.para_at(&pos).and_then(|p| p.object_at(pos.off)).cloned().ok_or_else(|| CmdError::Failed("object vanished".into()))?;
    let Some((w, h, float)) = obj.frame() else { return Err(CmdError::Failed("object vanished".into())) };
    let float = float.clone();
    // Each member where the group shows it: the group's turn moves its centre about the group's
    // centre and adds to its own turn.
    let members: Vec<InlineObject> = shown_members(&obj, [0.0, 0.0, w, h])
        .into_iter()
        .map(|([x, y, cw, ch], spin, c)| {
            let mut c = c.clone();
            let (mw, mh) = (cw.max(min_size(&c)), ch.max(min_size(&c)));
            c.set_size(mw, mh);
            // Grown to its smallest size about its centre, so it stays centred where it was.
            let (x, y) = (x - (mw - cw) / 2.0, y - (mh - ch) / 2.0);
            if let Some(f) = c.float_mut() {
                // Each member keeps its own alt text; the group's placement is shared out.
                let alt = std::mem::take(&mut f.alt);
                *f = Float { x: (float.x + x).clamp(-MAX_OFFSET, MAX_OFFSET), y: (float.y + y).clamp(-MAX_OFFSET, MAX_OFFSET), alt, ..float.clone() };
                f.set_spin(spin);
            }
            c
        })
        .collect();
    if members.is_empty() {
        return Err(CmdError::Failed("the group is empty".into()));
    }
    let len = wordcraft_doc::para::OBJ.len_utf8();
    s.doc.delete_range(&pos, &Pos { off: pos.off + len, ..pos.clone() })?;
    let mut at = s.doc.clamp(&pos);
    let mut placed = Vec::new();
    for m in members {
        placed.push(at.clone());
        at = s.doc.insert_object(&at, m, &wordcraft_doc::CharProps::default())?;
    }
    let first = placed.first().cloned().unwrap_or(pos);
    s.sel = Selection { anchor: first.clone(), focus: Pos { off: first.off + len, ..first } };
    s.also_selected = placed.into_iter().skip(1).collect();
    sel_result(s)
}

/// A group's members as it shows them in its frame `[x, y, w, h]`: each member's unturned frame
/// and the turn it is drawn with (its own inside the group's). The group's turn, about its
/// centre, moves each member's centre. Empty for anything but a group.
fn shown_members(group: &InlineObject, [x, y, w, h]: [f32; 4]) -> Vec<([f32; 4], Spin, &InlineObject)> {
    let outer = group.frame().map(|(_, _, f)| f.spin()).unwrap_or_default();
    let [x, y, w, h] = [x, y, w, h].map(wordcraft_geom::finite);
    let (gx, gy) = (x + w / 2.0, y + h / 2.0);
    group
        .group_rects(x, y, w, h)
        .into_iter()
        .map(|([mx, my, mw, mh], c)| {
            let own = c.frame().map(|(_, _, f)| f.spin()).unwrap_or_default();
            let (cx, cy) = outer.apply(gx, gy, mx + mw / 2.0, my + mh / 2.0);
            ([cx - mw / 2.0, cy - mh / 2.0, mw, mh], own.within(outer), c)
        })
        .collect()
}

pub(crate) fn all_objects(s: &Session) -> Vec<(Pos, InlineObject)> {
    let mut v = Vec::new();
    for path in s.doc.para_paths(StoryRef::Body) {
        let Some(p) = s.doc.para(StoryRef::Body, &path) else { continue };
        for off in p.object_offsets() {
            if let Some(o) = p.object_at(off).filter(|o| o.is_drawing()) {
                v.push((Pos { story: StoryRef::Body, path: path.clone(), off }, o.clone()));
            }
        }
    }
    v
}

fn objects_list(s: &mut Session, _: &Value) -> CmdResult {
    Ok(Value::Array(
        all_objects(s)
            .into_iter()
            .enumerate()
            .map(|(i, (pos, o))| {
                let (kind, name) = match &o {
                    InlineObject::Image { alt, .. } => ("picture", if alt.is_empty() { format!("Picture {}", i + 1) } else { alt.clone() }),
                    InlineObject::Shape { kind, .. } => ("shape", format!("{kind:?} {}", i + 1)),
                    InlineObject::Graphic { graphic, .. } => ("graphic", format!("{:?} {}", graphic.kind, i + 1)),
                    InlineObject::Group { .. } => ("group", format!("Group {}", i + 1)),
                    _ => ("object", format!("Object {}", i + 1)),
                };
                let hidden = o.frame().is_some_and(|(_, _, f)| f.hidden);
                json!({"index": i, "kind": kind, "name": name, "pos": super::pos_json(&pos), "size": obj_size(&o), "hidden": hidden})
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests;
