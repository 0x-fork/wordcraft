use super::export::Fit;
use super::glyphs::{baseline, cluster_ends, glyph_outlines, left_to_right, split_clusters, visual_order};
use super::options::{PdfError, PdfOptions};
use super::paint::{append_path, fill, grown, norm, ok, paint_fill, pdf_line_cap, rect_path, shape_effects, shape_path};
use super::picture::{Vector, build_vector, load_image, paint_of, vector_picture};
use super::tags::{Role, TagEntry, figure_alt, graphic_alt, holds_picture};
use super::*;

pub(crate) struct Exporter<'a> {
    pub(crate) doc: &'a Document,
    pub(crate) lay: &'a DocLayout,
    pub(crate) opts: &'a PdfOptions,
    /// Layout page index → output page index.
    pub(crate) out_index: HashMap<usize, usize>,
    /// Embedded fonts by face id (`None`: krilla can't read it).
    pub(crate) fonts: HashMap<u32, (FaceRef, Option<Font>)>,
    pub(crate) font_error: Option<PdfError>,
    /// Faces whose glyphs are drawn as outlines because krilla can't subset them.
    pub(crate) outlined: &'a HashSet<u32>,
    pub(crate) cmaps: HashMap<u32, Arc<HashMap<u32, char>>>,
    pub(crate) images: HashMap<String, Option<Image>>,
    /// Parsed metafiles by media key (`None`: not readable).
    pub(crate) vectors: HashMap<String, Option<Arc<Vector>>>,
    pub(crate) tags: Vec<TagEntry>,
    pub(crate) tag_index: HashMap<(StoryRef, DocPath), usize>,
    /// Link rectangles on the current page: (x0, y0, x1, y1, target).
    pub(crate) links: Vec<(f32, f32, f32, f32, String)>,
    /// Bookmark positions by name (the first of a name wins), gathered once: looking each internal
    /// link up by walking the whole document made a long document with a linked table of contents
    /// take seconds to export.
    /// Bookmarks by exact name, and by ASCII-lowercased name for the case-blind fallback; the
    /// first in document order wins either way.
    pub(crate) bookmarks: std::cell::OnceCell<(HashMap<String, wordcraft_doc::Pos>, HashMap<String, wordcraft_doc::Pos>)>,
}

impl Exporter<'_> {
    fn font(&mut self, face: &FaceRef) -> Option<Font> {
        if self.outlined.contains(&face.id()) {
            return None;
        }
        let font_error = &mut self.font_error;
        self.fonts
            .entry(face.id())
            .or_insert_with(|| {
                let variations = face.is_variable().then_some(face.coords.as_slice());
                let font = match write::font(face.data(), face.index(), variations) {
                    Ok(font) => Some(font),
                    // Keep WordCraft's existing policy: unreadable faces are skipped.
                    Err(WriteError::InvalidFont) => None,
                    Err(error) => {
                        // Allocation, validation or backend failures must fail the export,
                        // rather than return a PDF with missing text.
                        font_error.get_or_insert_with(|| error.into());
                        None
                    }
                };
                (*face, font)
            })
            .1
            .clone()
    }

    fn cmap(&mut self, face: &FaceRef) -> Arc<HashMap<u32, char>> {
        self.cmaps
            .entry(face.id())
            .or_insert_with(|| {
                let mut m = HashMap::new();
                for (c, g) in face.chars() {
                    m.entry(g).or_insert(c);
                }
                Arc::new(m)
            })
            .clone()
    }

    /// The metafile picture for a media key, parsed once. None when the bytes are not a metafile
    /// (the raster path takes them) or do not parse (the raster path then draws the grey box).
    fn vector(&mut self, key: &str) -> Option<Arc<Vector>> {
        let doc = self.doc;
        let bytes = doc.media.get(key)?;
        if !wordcraft_metafile::is_metafile(bytes) {
            return None;
        }
        if let Some(v) = self.vectors.get(key) {
            return v.clone();
        }
        let v = build_vector(bytes).map(Arc::new);
        if v.is_none() {
            log::warn!("PDF: metafile `{key}` could not be read");
        }
        self.vectors.insert(key.to_string(), v.clone());
        v
    }

    fn image(&mut self, key: &str) -> Option<Image> {
        if let Some(i) = self.images.get(key) {
            return i.clone();
        }
        let img = self.doc.media.get(key).and_then(load_image);
        if img.is_none() {
            log::warn!("PDF: picture `{key}` could not be decoded");
        }
        self.images.insert(key.to_string(), img.clone());
        img
    }

    /// Run `f` inside a marked-content section for `role`.
    fn tagged(&mut self, s: &mut Surface, role: Role, alt: Option<&str>, f: impl FnOnce(&mut Self, &mut Surface)) {
        if !self.opts.tagged {
            f(self, s);
            return;
        }
        match role {
            Role::Artifact(kind) => {
                s.start_tagged(ContentTag::Artifact(Artifact::new(kind, None)));
                f(self, s);
                s.end_tagged();
            }
            Role::Para(i) => {
                let id = s.start_tagged(ContentTag::Span(SpanTag::empty()));
                f(self, s);
                s.end_tagged();
                if let Some(e) = self.tags.get_mut(i) {
                    e.children.push(Node::Leaf(id));
                }
            }
            Role::Figure => {
                let id: Identifier = s.start_tagged(ContentTag::Other);
                f(self, s);
                s.end_tagged();
                let alt = alt.map(str::to_string).filter(|a| !a.is_empty()).or_else(|| Some("picture".to_string()));
                let g = TagGroup::with_children(Tag::Figure(alt), vec![Node::Leaf(id)]);
                self.tags.push(TagEntry { kind: Tag::Div.into(), children: vec![Node::Group(g)] });
            }
        }
    }

    /// A rotated inline picture or group: turned as a whole, with each picture in it tagged on its
    /// own, so a decorative picture is an artifact while the others stay figures with their alt
    /// text. A described shape's figure keeps its description; anything else is an artifact.
    fn rotated_pictures(&mut self, s: &mut Surface, d: &Draw) {
        match d {
            Draw::Rotated { cx, cy, spin, items } => {
                if !ok(*cx) || !ok(*cy) {
                    return;
                }
                let [a, b, c, d, e, f] = spin.matrix(*cx, *cy);
                s.push_transform(&Transform::from_row(a, b, c, d, e, f));
                for it in items {
                    self.rotated_pictures(s, it);
                }
                s.pop();
            }
            Draw::Image { decorative: true, .. } => self.tagged(s, Role::Artifact(ArtifactType::Other), None, |me, s| me.draw(s, d)),
            Draw::Image { alt, .. } => self.tagged(s, Role::Figure, Some(alt), |me, s| me.draw(s, d)),
            Draw::Figure { alt, draws, .. } => self.tagged(s, Role::Figure, Some(alt), |me, s| {
                for d in draws {
                    me.draw(s, d);
                }
            }),
            _ => self.tagged(s, Role::Artifact(ArtifactType::Other), None, |me, s| me.draw(s, d)),
        }
    }

    pub(crate) fn page(&mut self, s: &mut Surface, page: &Page, w: f32, h: f32) {
        if let Some(c) = self.doc.settings.page_color
            && let Some(p) = rect_path(&Rect::new(0.0, 0.0, w, h))
        {
            self.tagged(s, Role::Artifact(ArtifactType::Other), None, |_, s| {
                s.set_stroke(None);
                s.set_fill(Some(fill(c, 1.0)));
                s.draw_path(&p);
                s.set_fill(None);
            });
        }
        if let Some(pic) = self.doc.settings.page_picture.clone() {
            self.tagged(s, Role::Artifact(ArtifactType::Other), None, |me, s| me.page_picture(s, &pic, w, h));
        }
        if let Some(wm) = self.doc.settings.watermark.clone()
            && !wm.text.trim().is_empty()
        {
            let area = page.body;
            self.tagged(s, Role::Artifact(ArtifactType::Watermark), None, |me, s| me.watermark(s, &wm, w, h, area));
        }
        let dopts = DisplayOptions {
            marks: false,
            dim_header: false,
            dim_body: false,
            markup: self.opts.include_markup,
            placeholders: false,
            ..Default::default()
        };
        // What the items share (the authors' colours, the clipped cells) is worked out once per page.
        let shown = ItemDisplay::new(self.doc, page);
        for it in page.header.iter().chain(page.footer.iter()) {
            let draws = shown.draws(it, &dopts);
            self.tagged(s, Role::Artifact(ArtifactType::Other), None, |me, s| {
                for d in &draws {
                    me.draw(s, d);
                }
            });
        }
        for it in &page.items {
            let draws = shown.draws(it, &dopts);
            match it {
                Placed::Lines { story, path, para, .. } => {
                    let idx = self.entry(*story, path, para);
                    for d in &draws {
                        match d {
                            Draw::Glyphs { .. } | Draw::Turned { .. } => self.tagged(s, Role::Para(idx), None, |me, s| me.draw(s, d)),
                            // A picture marked decorative: an artifact screen readers skip.
                            Draw::Image { decorative: true, .. } => self.tagged(s, Role::Artifact(ArtifactType::Other), None, |me, s| me.draw(s, d)),
                            Draw::Image { alt, .. } => self.tagged(s, Role::Figure, Some(alt), |me, s| me.draw(s, d)),
                            // A rotated inline picture, or a rotated group holding pictures: turned as a
                            // whole, each picture tagged on its own (see `rotated_pictures`).
                            Draw::Rotated { items, .. } if items.iter().any(holds_picture) => self.rotated_pictures(s, d),
                            // An inline chart, diagram or described shape: its text is part of the figure, not the paragraph.
                            Draw::Figure { alt, kind, draws: inner } => self.tagged(s, Role::Figure, Some(figure_alt(alt, *kind)), |me, s| {
                                for d in inner {
                                    me.draw(s, d);
                                }
                            }),
                            _ => self.tagged(s, Role::Artifact(ArtifactType::Other), None, |me, s| me.draw(s, d)),
                        }
                    }
                }
                Placed::Image { story, path, off, .. } => {
                    let obj = self.doc.para(*story, path).and_then(|p| p.object_at(*off));
                    let role = if obj.is_some_and(InlineObject::is_decorative) { Role::Artifact(ArtifactType::Other) } else { Role::Figure };
                    let alt = match obj {
                        Some(InlineObject::Image { alt, .. }) => alt.clone(),
                        _ => String::new(),
                    };
                    self.tagged(s, role, Some(&alt), |me, s| {
                        for d in &draws {
                            me.draw(s, d);
                        }
                    });
                }
                // A floating shape with a description: a figure (see `shape_figure_alt`).
                Placed::Shape { alt, .. } if !alt.is_empty() => self.tagged(s, Role::Figure, Some(alt), |me, s| {
                    for d in &draws {
                        me.draw(s, d);
                    }
                }),
                Placed::Graphic { story, path, off, graphic, .. } => {
                    let alt = match self.doc.para(*story, path).and_then(|p| p.object_at(*off)) {
                        Some(InlineObject::Graphic { alt, .. }) => graphic_alt(alt, graphic.kind).to_string(),
                        _ => graphic.kind.noun().to_string(),
                    };
                    self.tagged(s, Role::Figure, Some(&alt), |me, s| {
                        for d in &draws {
                            me.draw(s, d);
                        }
                    });
                }
                _ => self.tagged(s, Role::Artifact(ArtifactType::Other), None, |me, s| {
                    for d in &draws {
                        me.draw(s, d);
                    }
                }),
            }
        }
    }

    /// A heading's text for bookmarks and tags: what the pages show. That leaves out what its
    /// layout `pl` left out (hidden text, resolved as laid out, so table formatting counts too) and,
    /// without markup, tracked deletions.
    fn title_text(&self, p: &wordcraft_doc::Paragraph, pl: &wordcraft_layout::para::ParaLayout) -> String {
        let mut dropped = pl.left_out.clone();
        if !self.opts.include_markup {
            dropped.extend(p.deleted_ranges());
        }
        p.text_without(&dropped)
    }

    /// The structure entry for a paragraph (created on first sight, in reading order).
    fn entry(&mut self, story: StoryRef, path: &DocPath, pl: &wordcraft_layout::para::ParaLayout) -> usize {
        if let Some(i) = self.tag_index.get(&(story, path.clone())) {
            return *i;
        }
        let kind: TagKind = match self.doc.para(story, path).map(|p| self.doc.styles.resolve_para(&p.props).outline_level) {
            Some(Some(l)) if l < 6 => {
                let title = self.doc.para(story, path).map(|p| self.title_text(p, pl).trim().chars().take(200).collect::<String>());
                Tag::Hn(NonZeroU16::new(u16::from(l) + 1).unwrap_or(NonZeroU16::MIN), title).into()
            }
            _ => Tag::P.into(),
        };
        self.tags.push(TagEntry { kind, children: Vec::new() });
        let i = self.tags.len() - 1;
        self.tag_index.insert((story, path.clone()), i);
        i
    }

    pub(crate) fn tag_tree(&mut self) -> TagTree {
        let mut tree = TagTree::new().with_lang(self.doc.styles.default_chr.lang.clone());
        for e in std::mem::take(&mut self.tags) {
            if e.children.is_empty() {
                continue;
            }
            tree.push(TagGroup::with_children(e.kind, e.children));
        }
        tree
    }

    /// Paint one display item on the PDF surface in its current point-coordinate system.
    /// Nested clips and turns are pushed and popped around their child items. Shape and path
    /// outlines set their own dash and cap state, then clear their fill and stroke; ink retains
    /// round caps independently of the optional shape-outline setting.
    fn draw(&mut self, s: &mut Surface, d: &Draw) {
        match d {
            Draw::Figure { draws, .. } => {
                for d in draws {
                    self.draw(s, d);
                }
            }
            Draw::Glyphs { face, size, glyphs, color, alpha, synth_bold, synth_italic, text, link, ranges, x_scale, .. } => {
                match glyphs.first().map(|g| g.1).filter(|_| x_scale.is_finite() && *x_scale > 0.0 && (x_scale - 1.0).abs() > 1e-4) {
                    // Scaled letters (`w:w`): drawn stretched across from the run's start, their
                    // positions brought back so the stretch puts each where the layout has it.
                    Some(x0) => {
                        let k = *x_scale;
                        let moved: Vec<(u32, f32, f32)> = glyphs.iter().map(|&(g, x, y)| (g, x0 + (x - x0) / k, y)).collect();
                        s.push_transform(&Transform::from_row(k, 0.0, 0.0, 1.0, x0 * (1.0 - k), 0.0));
                        self.glyphs(s, face, *size, &moved, *color, *alpha, *synth_bold, *synth_italic, text, ranges);
                        s.pop();
                    }
                    None => self.glyphs(s, face, *size, glyphs, *color, *alpha, *synth_bold, *synth_italic, text, ranges),
                }
                if let Some(l) = link {
                    self.link_rect(face, *size, glyphs, l);
                }
            }
            Draw::Fill { rect, color, alpha, .. } => {
                if let Some(p) = rect_path(rect) {
                    s.set_stroke(None);
                    s.set_fill(Some(fill(*color, *alpha)));
                    s.draw_path(&p);
                    s.set_fill(None);
                }
            }
            Draw::Line { x0, y0, x1, y1, width, color, stroke, alpha, .. } => self.line(s, (*x0, *y0, *x1, *y1), *width, *color, *stroke, *alpha),
            Draw::Image { rect, media, crop, alpha, .. } => self.picture(s, rect, media, crop, *alpha),
            Draw::Path { segs, fill: f, stroke, stroke_width, dash, cap } => {
                let mut pb = PathBuilder::new();
                append_path(&mut pb, &wordcraft_layout::display::seg_path(segs));
                let Some(p) = pb.finish() else { return };
                s.set_fill(f.as_ref().map(paint_fill));
                s.set_stroke(stroke.map(|c| Stroke {
                    paint: rgb::Color::new(c.0, c.1, c.2).into(),
                    // Width 0 (or none) is a hairline.
                    width: if stroke_width.is_finite() && *stroke_width > 0.0 { stroke_width.clamp(0.25, 200.0) } else { 0.75 },
                    dash: (!dash.is_empty()).then(|| StrokeDash { array: dash.clone(), offset: 0.0 }),
                    line_cap: pdf_line_cap(*cap),
                    ..Default::default()
                }));
                if f.is_some() || stroke.is_some() {
                    s.draw_path(&p);
                }
                s.set_fill(None);
                s.set_stroke(None);
            }
            Draw::Ink { pts, color, width, alpha } => {
                let mut pb = PathBuilder::new();
                let mut used = 0usize;
                let mut last = (0.0, 0.0);
                for &(x, y) in pts.iter().filter(|(x, y)| ok(*x) && ok(*y)) {
                    if used > 0 {
                        pb.line_to(x, y);
                    } else {
                        pb.move_to(x, y);
                    }
                    used += 1;
                    last = (x, y);
                }
                // A tap (one usable point): a zero-length line, which round caps draw as a dot.
                if used == 1 {
                    pb.line_to(last.0, last.1);
                }
                let Some(p) = pb.finish() else { return };
                s.set_fill(None);
                s.set_stroke(Some(Stroke {
                    paint: rgb::Color::new(color.0, color.1, color.2).into(),
                    width: if width.is_finite() { width.clamp(0.25, 200.0) } else { 1.0 },
                    opacity: norm(*alpha),
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Default::default()
                }));
                s.draw_path(&p);
                s.set_stroke(None);
            }
            Draw::Shape { rect, kind, fill: f, stroke, stroke_width, effects, corner, dash, cap } => {
                let Some(p) = shape_path(*kind, rect, *corner) else { return };
                let can_fill = *kind != ShapeKind::Line;
                let fx = effects.sanitized();
                if !fx.is_empty() && (f.is_some() && can_fill || stroke.is_some()) {
                    shape_effects(s, *kind, *corner, rect, &fx, f.is_some() && can_fill, stroke.is_some(), *stroke_width);
                }
                // Soft edges: the fill fades out toward the outline (which fades with it).
                if let (Some(rad), Some(c), true) = (fx.soft_edge, f.as_ref(), can_fill) {
                    let rad = rad.min(rect.w.min(rect.h) / 2.0).max(0.0);
                    s.set_stroke(None);
                    for (g, a) in wordcraft_doc::effects::bands(-rad, 0.0, 1.0) {
                        if let Some(p) = shape_path(*kind, &grown(rect, g, 0.0, 0.0), *corner) {
                            s.set_fill(Some(paint_fill(&c.faded(a))));
                            s.draw_path(&p);
                        }
                    }
                    s.set_fill(None);
                    return;
                }
                s.set_fill(f.as_ref().filter(|_| can_fill).map(paint_fill));
                s.set_stroke(stroke.map(|c| Stroke {
                    paint: rgb::Color::new(c.0, c.1, c.2).into(),
                    width: if stroke_width.is_finite() { stroke_width.clamp(0.25, 200.0) } else { 0.75 },
                    dash: (!dash.is_empty()).then(|| StrokeDash { array: dash.clone(), offset: 0.0 }),
                    line_cap: pdf_line_cap(*cap),
                    ..Default::default()
                }));
                if f.is_some() && can_fill || stroke.is_some() {
                    s.draw_path(&p);
                }
                s.set_fill(None);
                s.set_stroke(None);
            }
            Draw::Art { path, fx, color, alpha, .. } => {
                for l in wordcraft_layout::wordart::art_layers(path, fx, *color, *alpha) {
                    let mut pb = PathBuilder::new();
                    append_path(&mut pb, &l.path);
                    let Some(p) = pb.finish() else { continue };
                    let Fill { paint, opacity, .. } = paint_fill(&l.paint);
                    match l.stroke {
                        Some(w) => {
                            s.set_fill(None);
                            s.set_stroke(Some(Stroke {
                                paint,
                                width: w.clamp(0.1, 400.0),
                                opacity,
                                line_join: LineJoin::Round,
                                ..Default::default()
                            }));
                        }
                        None => {
                            s.set_stroke(None);
                            s.set_fill(Some(Fill { paint, opacity, rule: FillRule::NonZero }));
                        }
                    }
                    s.draw_path(&p);
                }
                s.set_fill(None);
                s.set_stroke(None);
            }
            Draw::Mark { .. } | Draw::MarkText { .. } => {}
            Draw::Clip { rect, items } => {
                let Some(clip) = rect_path(rect) else { return };
                s.push_clip_path(&clip, &FillRule::NonZero);
                for it in items {
                    self.draw(s, it);
                }
                s.pop();
            }
            Draw::Turned { x, y, turn, items } => {
                if !ok(*x) || !ok(*y) {
                    return;
                }
                let [a, b, c, d, e, f] = Draw::turn_matrix(*turn, *x, *y);
                s.push_transform(&Transform::from_row(a, b, c, d, e, f));
                for it in items {
                    self.draw(s, it);
                }
                s.pop();
            }
            Draw::Rotated { cx, cy, spin, items } => {
                if !ok(*cx) || !ok(*cy) {
                    return;
                }
                let [a, b, c, d, e, f] = spin.matrix(*cx, *cy);
                s.push_transform(&Transform::from_row(a, b, c, d, e, f));
                for it in items {
                    self.draw(s, it);
                }
                s.pop();
            }
        }
    }

    fn line(&mut self, s: &mut Surface, (x0, y0, x1, y1): (f32, f32, f32, f32), width: f32, c: Rgb, style: LineStyle, alpha: f32) {
        if ![x0, y0, x1, y1].iter().all(|v| ok(*v)) {
            return;
        }
        let w = if width.is_finite() { width.clamp(0.1, 100.0) } else { 0.5 };
        let paint: krilla::paint::Paint = rgb::Color::new(c.0, c.1, c.2).into();
        let stroke = |width: f32, dash: Option<StrokeDash>, cap: LineCap| Stroke {
            paint: paint.clone(),
            width,
            opacity: norm(alpha),
            dash,
            line_cap: cap,
            ..Default::default()
        };
        let seg = |s: &mut Surface, dx: f32, dy: f32| {
            let mut pb = PathBuilder::new();
            pb.move_to(x0 + dx, y0 + dy);
            pb.line_to(x1 + dx, y1 + dy);
            if let Some(p) = pb.finish() {
                s.draw_path(&p);
            }
        };
        s.set_fill(None);
        match style {
            LineStyle::Solid => {
                s.set_stroke(Some(stroke(w, None, LineCap::Butt)));
                seg(s, 0.0, 0.0);
            }
            LineStyle::Dotted | LineStyle::Dashed | LineStyle::BorderDotted | LineStyle::BorderDashed | LineStyle::BorderDotDash => {
                let array = style.dashes().unwrap_or(&[]).iter().map(|k| k * w).collect();
                s.set_stroke(Some(stroke(w, Some(StrokeDash { array, offset: 0.0 }), LineCap::Butt)));
                seg(s, 0.0, 0.0);
            }
            LineStyle::Double => {
                // Two thinner lines either side of the centre line.
                let (dx, dy) = (x1 - x0, y1 - y0);
                let len = (dx * dx + dy * dy).sqrt().max(1e-3);
                let (nx, ny) = (-dy / len * w * 0.9, dx / len * w * 0.9);
                s.set_stroke(Some(stroke(w * 0.6, None, LineCap::Butt)));
                seg(s, nx, ny);
                seg(s, -nx, -ny);
            }
            LineStyle::Triple => {
                // Three thin lines, spaced as a double's, across the line's direction.
                let (dx, dy) = (x1 - x0, y1 - y0);
                let len = (dx * dx + dy * dy).sqrt().max(1e-3);
                let (nx, ny) = (-dy / len * w * 1.8, dx / len * w * 1.8);
                s.set_stroke(Some(stroke(w * 0.6, None, LineCap::Butt)));
                seg(s, nx, ny);
                seg(s, 0.0, 0.0);
                seg(s, -nx, -ny);
            }
            LineStyle::Wave => {
                let amp = (w * 1.2).max(0.8);
                let step = amp * 2.0;
                let (dx, dy) = (x1 - x0, y1 - y0);
                let len = (dx * dx + dy * dy).sqrt();
                if len >= 1e-3 {
                    let (ux, uy) = (dx / len, dy / len);
                    let (nx, ny) = (-uy, ux);
                    let mut pb = PathBuilder::new();
                    pb.move_to(x0, y0);
                    let mut t = 0.0;
                    let mut up = true;
                    while t < len && t < 20_000.0 {
                        let nt = (t + step).min(len);
                        let mid_t = (t + nt) / 2.0;
                        let sign = if up { -1.0 } else { 1.0 };
                        let cx = x0 + ux * mid_t + nx * amp * sign;
                        let cy = y0 + uy * mid_t + ny * amp * sign;
                        let ex = x0 + ux * nt;
                        let ey = y0 + uy * nt;
                        pb.quad_to(cx, cy, ex, ey);
                        t = nt;
                        up = !up;
                    }
                    s.set_stroke(Some(stroke(w * 0.8, None, LineCap::Round)));
                    if let Some(p) = pb.finish() {
                        s.draw_path(&p);
                    }
                }
            }
        }
        s.set_stroke(None);
    }

    /// The page's picture or texture fill: tiled at 96 dpi for a `tile` fill (when that is a sane
    /// number of tiles), else stretched over the page.
    fn page_picture(&mut self, s: &mut Surface, pic: &wordcraft_doc::PagePicture, w: f32, h: f32) {
        const MAX_TILES: f32 = 4096.0;
        if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
            return;
        }
        let dims = self
            .doc
            .media
            .get(&pic.media)
            .and_then(|b| image::ImageReader::new(std::io::Cursor::new(b.as_slice())).with_guessed_format().ok()?.into_dimensions().ok());
        if let Some((pw, ph)) = dims.filter(|_| pic.fill_type.eq_ignore_ascii_case("tile")) {
            let (tw, th) = (pw as f32 * 0.75, ph as f32 * 0.75);
            if tw >= 1.0 && th >= 1.0 && (w / tw).ceil() * (h / th).ceil() <= MAX_TILES {
                let mut y = 0.0;
                while y < h {
                    let mut x = 0.0;
                    while x < w {
                        self.picture(s, &Rect::new(x, y, tw, th), &pic.media, &[0.0; 4], 1.0);
                        x += tw;
                    }
                    y += th;
                }
                return;
            }
        }
        self.picture(s, &Rect::new(0.0, 0.0, w, h), &pic.media, &[0.0; 4], 1.0);
    }

    fn picture(&mut self, s: &mut Surface, rect: &Rect, media: &str, crop: &[f32; 4], alpha: f32) {
        let Some(clip) = rect_path(rect) else { return };
        if let Some(v) = self.vector(media) {
            vector_picture(self, s, &v, rect, crop, &clip, alpha);
            return;
        }
        let Some(img) = self.image(media) else {
            s.set_stroke(None);
            s.set_fill(Some(fill(Rgb(0xD0, 0xD0, 0xD0), 1.0)));
            s.draw_path(&clip);
            s.set_fill(None);
            return;
        };
        let [cl, ct, vis_w, vis_h] = wordcraft_metafile::crop_window(*crop).map(|v| v as f32);
        let (fw, fh) = (rect.w / vis_w, rect.h / vis_h);
        let Some(size) = Size::from_wh(fw.max(0.01), fh.max(0.01)) else { return };
        let fade = alpha < 0.999;
        if fade {
            s.push_opacity(norm(alpha));
        }
        s.push_clip_path(&clip, &FillRule::NonZero);
        s.push_transform(&Transform::from_translate(rect.x - cl * fw, rect.y - ct * fh));
        s.draw_image(img, size);
        s.pop();
        s.pop();
        if fade {
            s.pop();
        }
    }

    /// Unicode text for each glyph: (text, per-glyph byte range). Glyphs and the run's text are
    /// aligned through the font's reverse character map (ligatures take several characters,
    /// extra glyphs share their cluster's text).
    fn map_text(&mut self, face: &FaceRef, glyphs: &[(u32, f32, f32)], text: &str) -> (String, Vec<std::ops::Range<usize>>) {
        let cmap = self.cmap(face);
        let rev = |g: u32| cmap.get(&g).copied();
        if text.is_empty() {
            let mut t = String::new();
            let ranges = glyphs
                .iter()
                .map(|(g, _, _)| {
                    let a = t.len();
                    t.push(rev(*g).unwrap_or(' '));
                    a..t.len()
                })
                .collect();
            return (t, ranges);
        }
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let n = chars.len();
        let byte = |i: usize| chars.get(i).map(|c| c.0).unwrap_or(text.len());
        let mut ranges = Vec::with_capacity(glyphs.len());
        let mut i = 0usize;
        for (k, (g, _, _)) in glyphs.iter().enumerate() {
            let left_g = glyphs.len() - k;
            let left_c = n.saturating_sub(i);
            if left_c == 0 {
                let r = ranges.last().cloned().unwrap_or(0..text.len());
                ranges.push(r);
                continue;
            }
            if k + 1 == glyphs.len() {
                ranges.push(byte(i)..text.len());
                i = n;
                continue;
            }
            let this = chars.get(i).map(|c| c.1);
            let take = if left_c <= left_g || rev(*g) == this {
                if left_g > left_c && rev(*g) != this && k > 0 {
                    // More glyphs than characters: this one shares the previous cluster.
                    let r = ranges.last().cloned().unwrap_or(byte(i)..byte(i + 1));
                    ranges.push(r);
                    continue;
                }
                1
            } else {
                // A ligature (or substituted glyph): consume until the next glyph's character.
                let next = glyphs.get(k + 1).and_then(|(ng, _, _)| rev(*ng));
                let max = left_c - (left_g - 1);
                let mut t = 1;
                while t < max && chars.get(i + t).map(|c| c.1) != next {
                    t += 1;
                }
                if t == max && next.is_some() && chars.get(i + t).map(|c| c.1) != next {
                    // The next glyph doesn't match anything: assume one character each.
                    t = 1;
                }
                t
            };
            ranges.push(byte(i)..byte(i + take));
            i += take;
        }
        (text.to_string(), ranges)
    }

    #[allow(clippy::too_many_arguments)]
    fn glyphs(
        &mut self,
        s: &mut Surface,
        face: &FaceRef,
        size: f32,
        glyphs: &[(u32, f32, f32)],
        color: Rgb,
        alpha: f32,
        bold: bool,
        italic: bool,
        text: &str,
        known: &[std::ops::Range<usize>],
    ) {
        if !size.is_finite() || size <= 0.0 || size > 5000.0 {
            return;
        }
        if glyphs.is_empty() {
            return;
        }
        if !glyphs.iter().all(|(_, x, y)| ok(*x) && ok(*y)) {
            return;
        }
        let font = self.font(face);
        if font.is_none() && !self.outlined.contains(&face.id()) {
            log::warn!("PDF: font {} {} could not be embedded", face.family, face.style);
            return;
        }
        // Layout's glyph → text mapping when it has one (right-to-left text, contextual forms and
        // ligatures can't be guessed back from the font's cmap), else a guess.
        let valid = known.len() == glyphs.len() && known.iter().all(|r| r.start < r.end && text.get(r.clone()).is_some());
        let (txt, ranges) = if valid { (text.to_string(), known.to_vec()) } else { self.map_text(face, glyphs, text) };
        let upem = face.upem.max(1.0) as f32;
        // Right-to-left text is written left to right as it appears (see `left_to_right`); glyphs
        // already in that order are written as they are.
        let (order, ranges) = if left_to_right(glyphs) {
            ((0..glyphs.len()).collect(), ranges)
        } else {
            let parts = split_clusters(&txt, glyphs, &ranges, &self.cmap(face));
            (visual_order(&txt, glyphs, &ranges, &parts), parts)
        };
        let placed: Vec<(u32, f32, f32, std::ops::Range<usize>)> =
            order.iter().filter_map(|&i| Some((glyphs.get(i).copied()?, ranges.get(i).cloned()?))).map(|((g, x, y), r)| (g, x, y, r)).collect();
        let Some(&(_, x0, y0, _)) = placed.first() else { return };
        // The run's baseline: the y most of its glyphs share (marks sit higher or lower).
        let base = baseline(glyphs).unwrap_or(y0);
        let kg: Vec<KrillaGlyph> = placed
            .iter()
            .enumerate()
            .map(|(k, (g, x, y, r))| {
                let adv = match placed.get(k + 1) {
                    Some((_, nx, _, _)) => (nx - x) / size,
                    None => face.advance(*g) as f32 / upem,
                };
                KrillaGlyph::new(GlyphId::new(*g), adv, 0.0, (y0 - y) / size, 0.0, r.clone(), None)
            })
            .collect();
        let pushed = italic;
        if italic {
            s.push_transform(&Transform::from_row(1.0, 0.0, -0.21, 1.0, 0.21 * base, 0.0));
        }
        s.set_fill(Some(fill(color, alpha)));
        s.set_stroke(if bold {
            Some(Stroke { paint: rgb::Color::new(color.0, color.1, color.2).into(), width: size * 0.03, opacity: norm(alpha), ..Default::default() })
        } else {
            None
        });
        match font {
            Some(font) => {
                s.draw_glyphs(Point::from_xy(x0, y0), &kg, font, &txt, size, false);
            }
            None => {
                if let Some(path) = glyph_outlines(face, size, glyphs) {
                    s.draw_path(&path);
                }
            }
        }
        s.set_fill(None);
        s.set_stroke(None);
        if pushed {
            s.pop();
        }
    }

    fn link_rect(&mut self, face: &FaceRef, size: f32, glyphs: &[(u32, f32, f32)], link: &str) {
        let (Some(first), Some(last)) = (glyphs.first(), glyphs.last()) else { return };
        let adv = face.advance(last.0) as f32 * size / face.upem.max(1.0) as f32;
        let (x0, x1) = (first.1.min(last.1), first.1.max(last.1) + adv);
        let (y0, y1) = (first.2 - size * 0.85, first.2 + size * 0.25);
        if ![x0, x1, y0, y1].iter().all(|v| ok(*v)) {
            return;
        }
        if let Some(prev) = self.links.last_mut()
            && prev.4 == link
            && (prev.1 - y0).abs() < 0.5
            && x0 - prev.2 < size
        {
            prev.0 = prev.0.min(x0);
            prev.2 = prev.2.max(x1);
            return;
        }
        if self.links.len() < 10_000 {
            self.links.push((x0, y0, x1, y1, link.to_string()));
        }
    }

    /// Where a bookmark is: (output page, x, y) — the page and line its start lands on, which
    /// in a paragraph split across pages need not be the paragraph's first page.
    pub(crate) fn bookmark(&self, name: &str) -> Option<(usize, f32, f32)> {
        let (exact, folded) = self.bookmarks.get_or_init(|| {
            let (mut exact, mut folded) = (HashMap::new(), HashMap::new());
            for (n, pos) in self.doc.bookmarks() {
                folded.entry(n.to_ascii_lowercase()).or_insert_with(|| pos.clone());
                exact.entry(n).or_insert(pos);
            }
            (exact, folded)
        });
        let pos = exact.get(name).or_else(|| folded.get(&name.to_ascii_lowercase()))?.clone();
        if let Some(c) = self.lay.caret(&pos)
            && let Some(out) = self.out_index.get(&c.page)
        {
            // The left edge of the paragraph's piece on that page, at the top of the line.
            let x = self
                .lay
                .index
                .get(&(pos.story, pos.path.clone()))
                .into_iter()
                .flatten()
                .filter(|(pi, _)| *pi == c.page)
                .find_map(|(pi, ii)| match self.lay.pages.get(*pi).and_then(|p| p.items.get(*ii)) {
                    Some(Placed::Lines { x, .. }) => Some(*x),
                    _ => None,
                })
                .unwrap_or(c.x);
            let (x, y) = self.lay.pages.get(c.page).map(|p| Fit::of(p, self.opts.paper).point(x, c.top)).unwrap_or((x, c.top));
            return Some((*out, x, y));
        }
        self.locate(pos.story, &pos.path)
    }

    /// The first output page showing a paragraph, with the top of its first line.
    fn locate(&self, story: StoryRef, path: &DocPath) -> Option<(usize, f32, f32)> {
        let places = self.lay.index.get(&(story, path.clone()))?;
        for (pi, ii) in places {
            let Some(out) = self.out_index.get(pi) else { continue };
            let Some(page) = self.lay.pages.get(*pi) else { continue };
            let (x, y) = match page.items.get(*ii) {
                Some(Placed::Lines { x, y, .. }) => Fit::of(page, self.opts.paper).point(*x, *y),
                _ => (0.0, 0.0),
            };
            return Some((*out, x, y));
        }
        None
    }

    pub(crate) fn annotations(&self, fit: Fit) -> Vec<Annotation> {
        let mut v = Vec::new();
        for (x0, y0, x1, y1, target) in &self.links {
            let ((x0, y0), (x1, y1)) = (fit.point(*x0, *y0), fit.point(*x1, *y1));
            let Some(rect) = krilla::geom::Rect::from_ltrb(x0, y0, x1, y1) else { continue };
            let t = if let Some(name) = target.strip_prefix('#') {
                match self.bookmark(name) {
                    Some((page, x, y)) => Target::Destination(XyzDestination::new(page, Point::from_xy(x, y)).into()),
                    None => continue,
                }
            } else {
                // Unsafe targets (javascript:, file:, shares, other schemes) get no link; the text stays.
                match write::uri_target(target) {
                    Some(t) => t,
                    None => continue,
                }
            };
            v.push(Annotation::new_link(LinkAnnotation::new(rect, t), Some(target.clone())));
        }
        v
    }

    /// The outline from body headings (outline levels 1–9) on the exported pages.
    pub(crate) fn outline(&self, pages: &[usize]) -> Option<Outline> {
        let mut flat: Vec<(u8, String, usize, f32, f32)> = Vec::new();
        for (out, pi) in pages.iter().enumerate() {
            let Some(page) = self.lay.pages.get(*pi) else { continue };
            for it in &page.items {
                let Placed::Lines { story: StoryRef::Body, path, para, l0, l1, x, y, .. } = it else { continue };
                let Some(p) = self.doc.para(StoryRef::Body, path) else { continue };
                let Some(level) = self.doc.styles.resolve_para(&p.props).outline_level else { continue };
                // A leading page break can be placed on the previous page without the heading.
                let first = para
                    .lines
                    .iter()
                    .take_while(|line| {
                        line.end == LineEnd::PageBreak
                            && para
                                .clusters
                                .get(line.c0..line.c1)
                                .is_some_and(|cs| cs.iter().all(|c| matches!(c.kind, ClKind::PageBreak | ClKind::Marker)))
                    })
                    .count();
                if !(*l0..*l1).contains(&first) {
                    continue;
                }
                let Some(line) = para.lines.get(first) else { continue };
                let Some(piece) = para.lines.get(*l0) else { continue };
                let title: String = self.title_text(p, para).split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect();
                if title.is_empty() {
                    continue;
                }
                let (x, y) = Fit::of(page, self.opts.paper).point(*x, *y + line.top - piece.top);
                flat.push((level.min(8), title, out, x, y));
            }
        }
        if flat.is_empty() {
            return None;
        }
        fn build(flat: &[(u8, String, usize, f32, f32)], i: &mut usize, level: u8) -> Vec<OutlineNode> {
            let mut out = Vec::new();
            while let Some((l, title, page, x, y)) = flat.get(*i) {
                if *l < level {
                    break;
                }
                *i += 1;
                let mut node = OutlineNode::new(title.clone(), XyzDestination::new(*page, Point::from_xy(*x, *y)));
                // Children: following entries with a deeper level.
                if flat.get(*i).is_some_and(|n| n.0 > *l) {
                    for c in build(flat, i, l + 1) {
                        node.push_child(c);
                    }
                }
                out.push(node);
            }
            out
        }
        let mut o = Outline::new();
        let mut i = 0;
        while i < flat.len() {
            let before = i;
            for n in build(&flat, &mut i, 0) {
                o.push_child(n);
            }
            if i == before {
                i += 1;
            }
        }
        Some(o)
    }

    /// Draws a metafile text run as PDF text (outlines when the font cannot be embedded). `tf` maps
    /// the run's own space (see [`PlacedItem::Text`]) to the page.
    pub(crate) fn vector_text(&mut self, s: &mut Surface, t: &wordcraft_metafile::Text, tf: kurbo::Affine) {
        let face = wordcraft_fonts::word::resolve(&t.font, t.bold, t.italic).face;
        let shaped = wordcraft_fonts::shape(&face, &t.text, &[], |c| c);
        let glyphs: Vec<_> = shaped.iter().map(|g| (g.cluster, f64::from(g.x_advance), f64::from(g.x_offset), f64::from(g.y_offset))).collect();
        let Some(lay) = t.layout(face.upem, face.ascent, face.descent, &glyphs) else { return };
        let font = self.font(&face);
        if font.is_none() && !self.outlined.contains(&face.id()) {
            return;
        }
        let em = lay.em as f32;
        let c = tf.as_coeffs().map(|v| v as f32);
        if !c.iter().all(|v| v.is_finite()) || !em.is_finite() {
            return;
        }
        let (paint, opacity) = paint_of(t.color);
        s.push_transform(&Transform::from_row(c[0], c[1], c[2], c[3], c[4], c[5]));
        s.set_stroke(None);
        s.set_fill(Some(Fill { paint: paint.into(), opacity, rule: FillRule::NonZero }));
        match font {
            Some(font) => {
                let upem = face.upem.max(1.0) as f32;
                let ends = cluster_ends(&shaped.iter().map(|g| g.cluster).collect::<Vec<_>>(), t.text.len());
                let kg: Vec<KrillaGlyph> = shaped
                    .iter()
                    .enumerate()
                    .map(|(i, g)| {
                        let end = ends.get(i).copied().unwrap_or(t.text.len());
                        let start = g.cluster.min(end);
                        let x = lay.glyphs.get(i).map_or(0.0, |p| p.0);
                        let adv = match lay.glyphs.get(i + 1) {
                            Some(n) => ((n.0 - x) / lay.em) as f32,
                            None => g.x_advance as f32 / upem,
                        };
                        let dy = lay.glyphs.get(i).map_or(0.0, |p| ((lay.baseline - p.1) / lay.em) as f32);
                        KrillaGlyph::new(GlyphId::new(g.gid), adv, 0.0, dy, 0.0, start..end, None)
                    })
                    .collect();
                let x0 = lay.glyphs.first().map_or(0.0, |p| p.0) as f32;
                s.draw_glyphs(Point::from_xy(x0, lay.baseline as f32), &kg, font, &t.text, em, false);
            }
            None => {
                let pen: Vec<(u32, f32, f32)> = shaped.iter().zip(&lay.glyphs).map(|(g, (x, y))| (g.gid, *x as f32, *y as f32)).collect();
                if let Some(path) = glyph_outlines(&face, em, &pen) {
                    s.draw_path(&path);
                }
            }
        }
        s.set_fill(None);
        s.pop();
    }

    /// The watermark text stretched over its box (Word's `v:textpath fitshape`): its advance across
    /// the box's width, its em down the box's height with the em's foot (the font's descent below
    /// the baseline) on the box's bottom; the box centred on the margins (`area`, else the page)
    /// and, when diagonal, turned 45° counterclockwise.
    fn watermark(&mut self, s: &mut Surface, wm: &wordcraft_doc::Watermark, w: f32, h: f32, area: Rect) {
        let r = wordcraft_fonts::word::resolve(&wm.font, false, false);
        let face = r.face;
        let text: String = wm.text.chars().take(200).collect();
        let shaped = wordcraft_fonts::shape(&face, &text, &[], |c| c);
        let upem = face.upem.max(1.0) as f32;
        let raw_w: f32 = shaped.iter().map(|g| g.x_advance as f32).sum::<f32>() / upem;
        if raw_w <= 0.0 || !raw_w.is_finite() {
            return;
        }
        let [bw, bh] = wm.frame(w, h);
        let size = bh.min(2000.0);
        let stretch = bw / (raw_w * size);
        let descent = (wordcraft_fonts::word::line_metrics(&face).1 / face.upem.max(1.0)) as f32;
        let angle = if wm.diagonal { -std::f32::consts::FRAC_PI_4 } else { 0.0 };
        let (c, sn) = (angle.cos(), angle.sin());
        let (dx, dy) = (-bw / 2.0, bh / 2.0 - descent.clamp(0.0, 1.0) * bh);
        let (cx, cy) = if area.w > 0.0 && area.h > 0.0 { (area.x + area.w / 2.0, area.y + area.h / 2.0) } else { (w / 2.0, h.min(2000.0) / 2.0) };
        let font = self.font(&face);
        if font.is_none() && !self.outlined.contains(&face.id()) {
            return;
        }
        let ends = cluster_ends(&shaped.iter().map(|g| g.cluster).collect::<Vec<_>>(), text.len());
        let glyphs: Vec<KrillaGlyph> = shaped
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let end = ends.get(i).copied().unwrap_or(text.len());
                let start = g.cluster.min(end);
                KrillaGlyph::new(
                    GlyphId::new(g.gid),
                    g.x_advance as f32 / upem,
                    g.x_offset as f32 / upem,
                    g.y_offset as f32 / upem,
                    0.0,
                    start..end,
                    None,
                )
            })
            .collect();
        s.push_transform(&Transform::from_row(c * stretch, sn * stretch, -sn, c, c * dx - sn * dy + cx, sn * dx + c * dy + cy));
        s.set_stroke(None);
        s.set_fill(Some(fill(wm.color, if wm.semitransparent { 0.5 } else { 1.0 })));
        match font {
            Some(font) => {
                s.draw_glyphs(Point::from_xy(0.0, 0.0), &glyphs, font, &text, size, false);
            }
            None => {
                let k = size / upem;
                let mut x = 0.0;
                let mut pen = Vec::with_capacity(shaped.len());
                for g in &shaped {
                    pen.push((g.gid, x + g.x_offset as f32 * k, -(g.y_offset as f32) * k));
                    x += g.x_advance as f32 * k;
                }
                if let Some(path) = glyph_outlines(&face, size, &pen) {
                    s.draw_path(&path);
                }
            }
        }
        s.set_fill(None);
        s.pop();
    }
}
