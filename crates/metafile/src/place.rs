//! Placing a picture on a target (a page or a device): the fitted transform, paths as kurbo paths in
//! target units and bitmaps as target rectangles. The rasteriser and the PDF writer only paint the result.

use kurbo::{Affine, BezPath, Rect, Shape};

use crate::{Item, Picture, Seg};

/// Largest bitmap side the renderers draw: within the PDF image limit and the rasteriser's 16-bit sides.
pub const MAX_BITMAP_SIDE: u32 = 30_000;
/// Placed coordinates past this (target units) are far off any page; such items are left out.
const MAX_COORD: f64 = 1e6;
/// Smallest placed bitmap side (target units), so the stretch onto it is never singular.
const MIN_BITMAP_SIDE: f64 = 1e-6;
/// The least of a source axis a crop keeps (0.1%). Files crop pictures to slivers (a header bar
/// that is 2.7% of a page-sized metafile, #1899); this only keeps the stretch finite.
const MIN_VISIBLE: f64 = 0.001;

/// The part of a picture's source that `crop` (left, top, right, bottom fractions of the source)
/// keeps, as fractions of the source: [left, top, width, height], bitmap or metafile alike. Each
/// side is taken as 0–1 (junk as 0). When the two sides of an axis leave less than 0.1% of it,
/// the left (top) side stands and 0.1% shows.
pub fn crop_window(crop: [f32; 4]) -> [f64; 4] {
    let [l, t, r, b] = crop.map(|v| if v.is_finite() { f64::from(v.clamp(0.0, 1.0)) } else { 0.0 });
    let axis = |lo: f64, hi: f64| {
        let lo = lo.min(1.0 - MIN_VISIBLE);
        (lo, (1.0 - lo - hi).max(MIN_VISIBLE))
    };
    let ((x, w), (y, h)) = (axis(l, r), axis(t, b));
    [x, y, w, h]
}

/// One item of a placed picture, in target units.
pub enum PlacedItem {
    /// A filled and/or stroked path. The stroke width is in target units; 0 is a cosmetic hairline that
    /// the renderer draws at its minimum width.
    Path { path: BezPath, fill: Option<[u8; 4]>, stroke: Option<([u8; 4], f64)>, even_odd: bool },
    /// `Picture::items[index]`, a bitmap, stretched into `rect`.
    Bitmap { index: usize, rect: Rect },
}

impl Picture {
    /// Whether the picture has a finite, positive size, so it can be fitted into a frame.
    pub fn has_size(&self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }

    /// The transform that fits the picture into `r` with the crop fractions `crop` (left, top, right,
    /// bottom) removed first: the visible part fills `r`. None for a degenerate size.
    pub fn fit(&self, r: Rect, crop: [f32; 4]) -> Option<Affine> {
        let [cl, ct, vw, vh] = crop_window(crop);
        let (w, h) = (f64::from(self.width), f64::from(self.height));
        let sx = r.width() / (w * vw);
        let sy = r.height() / (h * vh);
        (sx.is_finite() && sy.is_finite() && sx > 0.0 && sy > 0.0)
            .then(|| Affine::translate((r.x0 - cl * w * sx, r.y0 - ct * h * sy)) * Affine::scale_non_uniform(sx, sy))
    }

    /// The items placed into `r` with `crop` applied. Items that are not finite or reach past ±1e6
    /// target units are left out (they are far off any page), as are bitmaps with no area. Stroke
    /// widths are scaled to target units (along the mean axis scale, so a non-uniform fit keeps
    /// strokes even).
    pub fn place(&self, r: Rect, crop: [f32; 4]) -> Vec<PlacedItem> {
        let Some(tf) = self.fit(r, crop) else { return Vec::new() };
        let c = tf.as_coeffs();
        let k = (c[0].abs() + c[3].abs()) / 2.0;
        let mut out = Vec::with_capacity(self.items.len());
        for (index, item) in self.items.iter().enumerate() {
            match item {
                Item::Path { segs, fill, stroke, even_odd } => {
                    let path = tf * bez(segs);
                    if in_range(path.bounding_box()) {
                        let stroke = stroke.map(|(c, w)| (c, f64::from(w) * k));
                        out.push(PlacedItem::Path { path, fill: *fill, stroke, even_odd: *even_odd });
                    }
                }
                Item::Bitmap { rect: [x, y, w, h], .. } => {
                    let (x0, y0) = (f64::from(*x), f64::from(*y));
                    let rect = tf.transform_rect_bbox(Rect::new(x0, y0, x0 + f64::from(*w), y0 + f64::from(*h)));
                    if in_range(rect) && rect.width() > MIN_BITMAP_SIDE && rect.height() > MIN_BITMAP_SIDE {
                        out.push(PlacedItem::Bitmap { index, rect });
                    }
                }
            }
        }
        out
    }
}

impl Item {
    /// Takes a bitmap's pixels out of the item (its geometry stays) and returns them as (width, height,
    /// RGBA) when a renderer can draw them: both sides in 1..=[`MAX_BITMAP_SIDE`] and exactly that many
    /// pixels. None for paths and for bitmaps that cannot be drawn.
    pub fn take_pixels(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        let Item::Bitmap { width, height, rgba, .. } = self else { return None };
        let (w, h, px) = (*width, *height, std::mem::take(rgba));
        let sides = (1..=MAX_BITMAP_SIDE).contains(&w) && (1..=MAX_BITMAP_SIDE).contains(&h);
        (sides && px.len() == (w as usize).saturating_mul(h as usize).saturating_mul(4)).then_some((w, h, px))
    }
}

/// Segments as a kurbo path. A segment after a close opens a new subpath at the last start point.
fn bez(segs: &[Seg]) -> BezPath {
    let mut p = BezPath::new();
    let mut start = (0.0, 0.0);
    let mut open = false;
    for s in segs {
        match *s {
            Seg::Move(x, y) => {
                start = (f64::from(x), f64::from(y));
                p.move_to(start);
                open = true;
            }
            Seg::Line(x, y) => {
                if !open {
                    p.move_to(start);
                }
                p.line_to((f64::from(x), f64::from(y)));
                open = true;
            }
            Seg::Cubic(a, b, c, d, e, f) => {
                if !open {
                    p.move_to(start);
                }
                p.curve_to((f64::from(a), f64::from(b)), (f64::from(c), f64::from(d)), (f64::from(e), f64::from(f)));
                open = true;
            }
            Seg::Close => {
                if open {
                    p.close_path();
                }
                open = false;
            }
        }
    }
    p
}

/// Whether a placed box is finite and within ±[`MAX_COORD`].
fn in_range(b: Rect) -> bool {
    [b.x0, b.y0, b.x1, b.y1].iter().all(|v| v.is_finite() && v.abs() < MAX_COORD)
}
