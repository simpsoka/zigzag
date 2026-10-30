//! Design canvas: view transform, stitch rendering, selection and hit testing.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use zigzag::design::Design;
use zigzag::geom::{P, point_inside};
use zigzag::svg::{Geometry, Shape};

/// Maps design units (0.1 mm, y down) to screen points.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Design point (units) shown at the canvas center.
    pub center: P,
    /// Screen points per millimetre.
    pub scale: f32,
}

impl Default for View {
    fn default() -> Self {
        View {
            center: P::default(),
            scale: 8.0,
        }
    }
}

impl View {
    pub fn to_screen(self, rect: Rect, p: P) -> Pos2 {
        let d = (p - self.center) * (self.scale as f64 / 10.0);
        rect.center() + Vec2::new(d.x as f32, d.y as f32)
    }
    pub fn to_design(self, rect: Rect, s: Pos2) -> P {
        let d = s - rect.center();
        self.center + P::new(d.x as f64, d.y as f64) * (10.0 / self.scale as f64)
    }
    /// Zoom by `factor`, keeping the design point under `anchor` fixed.
    pub fn zoom_at(&mut self, rect: Rect, anchor: Pos2, factor: f32) {
        let before = self.to_design(rect, anchor);
        self.scale = (self.scale * factor).clamp(1.0, 120.0);
        let after = self.to_design(rect, anchor);
        self.center = self.center + (before - after);
    }
    /// Fit a box of `w_mm` × `h_mm` centered at `c` into `rect`.
    pub fn fit(&mut self, rect: Rect, c: P, w_mm: f64, h_mm: f64) {
        self.center = c;
        let s = (rect.width() as f64 / (w_mm * 1.15)).min(rect.height() as f64 / (h_mm * 1.15));
        self.scale = (s as f32).clamp(1.0, 120.0);
    }
}

pub fn color32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// How stitches are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StitchView {
    /// Shaded thread, close to how it will look sewn.
    #[default]
    Thread,
    /// Thin lines and needle points in a color that stands out from the
    /// fabric: shows stitch direction and type at a glance.
    Lines,
}

#[derive(Clone, Debug)]
pub struct DrawOptions {
    pub show_jumps: bool,
    /// Draw only the first N stitches (stitch player).
    pub limit: usize,
    /// Rendered thread thickness, mm.
    pub thread_mm: f32,
    pub view: StitchView,
    /// The fabric color behind the stitches.
    pub background: [u8; 3],
    /// Blocks being moved, scaled or rotated (one flag per block) and the
    /// transform so far: their stitches are drawn through it, so the
    /// gesture shows live without re-planning anything until it ends.
    pub moving: Option<(Vec<bool>, zigzag::geom::Affine)>,
    /// An object whose stitches aren't drawn (text being typed with every
    /// letter deleted: its frame and caret show instead).
    pub hidden: Option<usize>,
}

impl DrawOptions {
    /// Block `k` of `d` is only the hidden object's.
    pub fn hides_block(&self, d: &Design, k: usize) -> bool {
        match self.hidden {
            Some(i) => d.block_shapes.get(k).is_some_and(|v| v.as_slice() == [i]),
            None => false,
        }
    }
}

fn luminance(c: [u8; 3]) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2]))
}

/// The thread color, pushed toward black or white when it would disappear
/// into the fabric behind it.
pub fn contrasting(thread: [u8; 3], background: [u8; 3]) -> Color32 {
    let (lt, lb) = (luminance(thread), luminance(background));
    let ratio = (lt.max(lb) + 0.05) / (lt.min(lb) + 0.05);
    if ratio >= 2.2 {
        return color32(thread);
    }
    let toward = if lb > 0.4 { [0, 0, 0] } else { [255, 255, 255] };
    mix(thread, toward, 0.55)
}

/// One stitch as shaded thread: shadow, body and a highlight, with the ends
/// pulled in a little where the needle goes through.
fn thread_segment(out: &mut Vec<egui::Shape>, a: Pos2, b: Pos2, w: f32, shade: &[Color32; 3]) {
    let d = b - a;
    let len = d.length();
    if len < 0.5 {
        return;
    }
    let u = d / len;
    let inset = (w * 0.3).min(len * 0.2);
    let (a, b) = (a + u * inset, b - u * inset);
    out.push(egui::Shape::line_segment([a, b], Stroke::new(w, shade[0])));
    out.push(egui::Shape::line_segment(
        [a, b],
        Stroke::new(w * 0.72, shade[1]),
    ));
    // Light from the top left: the highlight sits on that side of the thread.
    let n = Vec2::new(-u.y, u.x);
    let n = if n.x + n.y > 0.0 { -n } else { n };
    let off = n * (w * 0.14);
    let (ha, hb) = (a.lerp(b, 0.15) + off, a.lerp(b, 0.85) + off);
    out.push(egui::Shape::line_segment(
        [ha, hb],
        Stroke::new((w * 0.22).max(0.8), shade[2]),
    ));
}

/// Draw the stitch plan. Returns the position of the last drawn stitch.
pub fn draw_design(
    painter: &egui::Painter,
    rect: Rect,
    view: &View,
    d: &Design,
    opt: &DrawOptions,
) -> Option<Pos2> {
    let mut shapes = Vec::new();
    let last = design_shapes(rect, view, d, opt, Some(rect), None, &mut shapes);
    painter.extend(shapes);
    last
}

/// A tessellated stitch plan, kept between frames (see [`draw_design_cached`]).
#[derive(Default)]
pub struct StitchMesh {
    /// One mesh per color block, keyed by that block's stitches and how
    /// they're drawn: a change to one object re-tessellates only its
    /// block(s), not the whole design.
    blocks: Vec<MeshEntry>,
    /// Each block's stitches hashed ([`block_key`]), for the plan
    /// `generation` they were hashed from: a scan of every stitch once per
    /// plan, not every frame.
    keys: Option<(u64, Vec<u64>)>,
}

struct MeshEntry {
    key: u64,
    /// Where the design's origin was on screen when the mesh was last
    /// placed: panning shifts the mesh in place.
    origin: Pos2,
    mesh: std::sync::Arc<egui::Mesh>,
}

impl MeshEntry {
    /// Put the mesh where the design's origin is now (`origin`), moving it
    /// in place if the view was panned (the painter dropped its copy of
    /// the last frame's mesh, so nothing is cloned).
    fn place(&mut self, origin: Pos2) {
        let shift = origin - self.origin;
        if shift.length() >= 1e-3 {
            std::sync::Arc::make_mut(&mut self.mesh).translate(shift);
            self.origin = origin;
        }
    }

    /// Draw the mesh with the design's origin at `origin`.
    fn paint(&mut self, painter: &egui::Painter, origin: Pos2) {
        self.place(origin);
        painter.add(egui::Shape::Mesh(self.mesh.clone()));
    }

    /// Draw the mesh through a design-space transform `m` (a gesture in
    /// progress): a copy with every vertex moved, which is far cheaper
    /// than tessellating the block's stitches again.
    fn paint_through(
        &mut self,
        painter: &egui::Painter,
        origin: Pos2,
        rect: Rect,
        view: &View,
        m: zigzag::geom::Affine,
    ) {
        self.place(origin);
        // Screen q = C + (p - c)·s for design p. With p' = A·p + t:
        // q' = A·(q - C) + C + s·(A·c + t - c).
        let c = view.center;
        let s = view.scale as f64 / 10.0;
        let cx = rect.center().x as f64;
        let cy = rect.center().y as f64;
        let kx = cx + s * (m[0] * c.x + m[2] * c.y + m[4] - c.x);
        let ky = cy + s * (m[1] * c.x + m[3] * c.y + m[5] - c.y);
        let (a, b, cc, d) = (m[0] as f32, m[1] as f32, m[2] as f32, m[3] as f32);
        let (kx, ky, cx, cy) = (kx as f32, ky as f32, cx as f32, cy as f32);
        let mut mesh = (*self.mesh).clone();
        for v in &mut mesh.vertices {
            let (dx, dy) = (v.pos.x - cx, v.pos.y - cy);
            v.pos = Pos2::new(a * dx + cc * dy + kx, b * dx + d * dy + ky);
        }
        painter.add(egui::Shape::Mesh(std::sync::Arc::new(mesh)));
    }
}

fn tessellate(ctx: &egui::Context, ppp: f32, shapes: Vec<egui::Shape>) -> egui::Mesh {
    // (No prepared discs: the needle points' small circles tessellate as paths.)
    let (font_size, discs) = (ctx.fonts(|f| f.font_image_size()), Vec::new());
    let options = ctx.tessellation_options(|o| *o);
    let mut t = egui::epaint::Tessellator::new(ppp, options, font_size, discs);
    let mut mesh = egui::Mesh::default();
    for s in shapes {
        t.tessellate_shape(s, &mut mesh);
    }
    mesh
}

/// A block's stitches, hashed (so a mesh can be reused while they're the same).
fn block_key(blk: &zigzag::design::Block) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    blk.color.hash(&mut h);
    blk.runs.len().hash(&mut h);
    for r in &blk.runs {
        r.pts.len().hash(&mut h);
        r.trim_before.hash(&mut h);
        // Every point: cheap next to tessellating them.
        for p in &r.pts {
            p.x.to_bits().hash(&mut h);
            p.y.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

/// [`draw_design`] for a design that isn't changing: each color block's
/// thousands of thread segments are tessellated once into a mesh, reused
/// frame after frame (hovering, selecting, editing points, typing) until
/// that block, the zoom or how stitches are shown changes; panning moves
/// the meshes. Blocks being moved, scaled or rotated keep their mesh too,
/// drawn through the gesture's transform (`opt.moving`). The stitch player
/// draws directly.
///
/// `generation` changes whenever `d` does (a new plan): the per-block
/// hashes are computed once per generation rather than every frame.
pub fn draw_design_cached(
    painter: &egui::Painter,
    rect: Rect,
    view: &View,
    d: &Design,
    opt: &DrawOptions,
    generation: u64,
    cache: &mut Option<StitchMesh>,
) -> Option<Pos2> {
    if opt.limit != usize::MAX {
        return draw_design(painter, rect, view, d, opt);
    }
    use std::hash::{Hash, Hasher};
    let ctx = painter.ctx();
    let ppp = ctx.pixels_per_point();
    let style_key = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (view.scale.to_bits(), ppp.to_bits(), opt.thread_mm.to_bits()).hash(&mut h);
        (opt.background, opt.view == StitchView::Lines).hash(&mut h);
        h.finish()
    };
    let origin = view.to_screen(rect, P::default());
    let cache = cache.get_or_insert_with(StitchMesh::default);
    // The blocks' hashes, from the last frame when the plan hasn't changed.
    let fresh = match &cache.keys {
        Some((g, keys)) => *g == generation && keys.len() == d.blocks.len(),
        None => false,
    };
    if !fresh {
        cache.keys = Some((generation, d.blocks.iter().map(block_key).collect()));
    }
    let block_keys = cache.keys.take().map(|(_, k)| k).unwrap_or_default();
    let moving: Option<&[bool]> = opt.moving.as_ref().map(|(on, _)| on.as_slice());
    // Entries from the last frame, matched to this frame's blocks by key
    // (in order; a block that changed gets a fresh mesh). Taken out as
    // they're matched, so a pan can move each mesh in place.
    let mut old: Vec<Option<MeshEntry>> = std::mem::take(&mut cache.blocks)
        .into_iter()
        .map(Some)
        .collect();
    let mut last: Option<Pos2> = None;
    let jump_stroke = Stroke::new(1.0, Color32::from_gray(140));
    let one = |k: usize| -> Vec<bool> {
        let mut v = vec![false; d.blocks.len()];
        v[k] = true;
        v
    };
    for (k, blk) in d.blocks.iter().enumerate() {
        if opt.hides_block(d, k) {
            last = None;
            continue;
        }
        let through = match (&opt.moving, moving) {
            (Some((_, m)), Some(on)) if on.get(k).copied().unwrap_or(false) => Some(*m),
            _ => None,
        };
        let at = |p: P| match through {
            Some(m) => view.to_screen(rect, zigzag::geom::affine_apply(m, p)),
            None => view.to_screen(rect, p),
        };
        // The jump into this block, drawn directly (it depends on the
        // block before, so it's not part of either mesh).
        let first = blk.runs.first().and_then(|r| r.pts.first()).map(|&p| at(p));
        if let (Some(a), Some(b)) = (last, first)
            && opt.show_jumps
            && a.distance(b) > 1.0
        {
            painter.extend(egui::Shape::dashed_line(&[a, b], jump_stroke, 4.0, 3.0));
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (
            style_key,
            opt.show_jumps,
            block_keys.get(k).copied().unwrap_or(0),
        )
            .hash(&mut h);
        let key = h.finish();
        let entry = match old
            .iter()
            .position(|e| e.as_ref().is_some_and(|e| e.key == key))
        {
            Some(i) => old[i].take().unwrap(),
            None => {
                let mut shapes = Vec::new();
                let flags = one(k);
                design_shapes(
                    rect,
                    view,
                    d,
                    &DrawOptions {
                        moving: None,
                        ..opt.clone()
                    },
                    None,
                    Some((&flags, true)),
                    &mut shapes,
                );
                MeshEntry {
                    key,
                    origin,
                    mesh: std::sync::Arc::new(tessellate(ctx, ppp, shapes)),
                }
            }
        };
        cache.blocks.push(entry);
        let entry = cache.blocks.last_mut().unwrap();
        match through {
            Some(m) => entry.paint_through(painter, origin, rect, view, m),
            None => entry.paint(painter, origin),
        }
        last = blk
            .runs
            .last()
            .and_then(|r| r.pts.last())
            .map(|&p| at(p))
            .or(last);
    }
    cache.keys = Some((generation, block_keys));
    last
}

/// The shapes that draw the stitch plan (only those inside `clip`, when
/// given). Returns the position of the last stitch drawn.
/// `blocks`: draw only the blocks whose flag is (or isn't) set.
fn design_shapes(
    rect: Rect,
    view: &View,
    d: &Design,
    opt: &DrawOptions,
    clip: Option<Rect>,
    blocks: Option<(&[bool], bool)>,
    out: &mut Vec<egui::Shape>,
) -> Option<Pos2> {
    let w = (opt.thread_mm * view.scale).max(1.0);
    let mut remaining = opt.limit;
    let mut last: Option<Pos2> = None;
    let jump_stroke = Stroke::new(1.0, Color32::from_gray(140));
    let clip = clip.map(|c| c.expand(w * 2.0));
    let seen = |p: Pos2| clip.is_none_or(|c| c.contains(p));
    for (k, blk) in d.blocks.iter().enumerate() {
        if let Some((on, want)) = blocks
            && on.get(k).copied().unwrap_or(false) != want
        {
            // (A jump line isn't drawn to or from a block left out.)
            last = None;
            continue;
        }
        if opt.hides_block(d, k) {
            last = None;
            continue;
        }
        let moved = opt
            .moving
            .as_ref()
            .and_then(|(on, m)| on.get(k).copied().unwrap_or(false).then_some(*m));
        let base = color32(blk.color);
        let shade = [
            mix(blk.color, [0, 0, 0], 0.5),
            base,
            mix(blk.color, [255, 255, 255], 0.45),
        ];
        let line = contrasting(blk.color, opt.background);
        for run in &blk.runs {
            if remaining == 0 {
                return last;
            }
            let n = run.pts.len().min(remaining);
            remaining -= n;
            let pts: Vec<Pos2> = match moved {
                Some(m) => run.pts[..n]
                    .iter()
                    .map(|&p| view.to_screen(rect, zigzag::geom::affine_apply(m, p)))
                    .collect(),
                None => run.pts[..n]
                    .iter()
                    .map(|&p| view.to_screen(rect, p))
                    .collect(),
            };
            if let (Some(a), Some(&b)) = (last, pts.first())
                && opt.show_jumps
                && a.distance(b) > 1.0
            {
                out.extend(egui::Shape::dashed_line(&[a, b], jump_stroke, 4.0, 3.0));
            }
            match opt.view {
                StitchView::Lines => {
                    if pts.len() >= 2 {
                        out.push(egui::Shape::line(pts.clone(), Stroke::new(1.0, line)));
                    }
                    // Needle points once there's room to see them.
                    if view.scale > 10.0 {
                        let r = (view.scale * 0.06).clamp(1.2, 2.2);
                        for &q in pts.iter().filter(|q| seen(**q)) {
                            out.push(egui::Shape::circle_filled(q, r, line));
                        }
                    }
                }
                StitchView::Thread if w >= 2.2 => {
                    for s in pts.windows(2) {
                        if seen(s[0]) || seen(s[1]) {
                            thread_segment(out, s[0], s[1], w, &shade);
                        }
                    }
                }
                StitchView::Thread => {
                    if pts.len() >= 2 {
                        out.push(egui::Shape::line(pts.clone(), Stroke::new(w, base)));
                    }
                }
            }
            last = pts.last().copied().or(last);
        }
    }
    last
}

/// Outline of a shape's source geometry, for selection highlighting.
pub fn draw_outline(
    painter: &egui::Painter,
    rect: Rect,
    view: &View,
    shape: &Shape,
    stroke: Stroke,
) {
    let lines: Vec<(Vec<Pos2>, bool)> = match &shape.geometry {
        Geometry::Fill { rings, .. } => rings
            .iter()
            .map(|r| (r.iter().map(|&p| view.to_screen(rect, p)).collect(), true))
            .collect(),
        Geometry::Stroke { lines, .. } => lines
            .iter()
            .map(|(l, c)| (l.iter().map(|&p| view.to_screen(rect, p)).collect(), *c))
            .collect(),
    };
    for (pts, closed) in lines {
        // A traced outline packs many points into each pixel on screen:
        // those add nothing but work (and dashes) here.
        let mut kept: Vec<Pos2> = Vec::with_capacity(pts.len() / 2 + 2);
        for p in pts {
            if kept.last().is_none_or(|l: &Pos2| l.distance(p) >= 0.75) {
                kept.push(p);
            }
        }
        if closed && let Some(&f) = kept.first() {
            kept.push(f);
        }
        painter.extend(egui::Shape::dashed_line(&kept, stroke, 6.0, 4.0));
    }
}

/// Topmost shape under design point `p`. `tol` is the pick tolerance in units.
pub fn hit_test(shapes: &[Shape], p: P, tol: f64) -> Option<usize> {
    hit_test_where(shapes, p, tol, |_| true)
}

/// The topmost object under `p` among those `keep` allows.
pub fn hit_test_where(
    shapes: &[Shape],
    p: P,
    tol: f64,
    keep: impl Fn(usize) -> bool,
) -> Option<usize> {
    // A shape whose box `p` isn't within `tol` of can't be hit: one cheap
    // pass over its points saves the inside and distance tests (most of a
    // traced design's hundreds of objects, every frame the pointer moves).
    let near_box = |pts: &[P], tol: f64| {
        let mut b = zigzag::geom::Bounds::empty();
        pts.iter().for_each(|&q| b.add(q));
        !b.is_empty()
            && p.x >= b.min.x - tol
            && p.x <= b.max.x + tol
            && p.y >= b.min.y - tol
            && p.y <= b.max.y + tol
    };
    shapes
        .iter()
        .enumerate()
        .rev()
        .filter(|(i, _)| keep(*i))
        .find_map(|(i, s)| {
            let near = match &s.geometry {
                Geometry::Fill { rings, .. } => rings.iter().any(|r| near_box(r, tol)),
                Geometry::Stroke { lines, width } => lines
                    .iter()
                    .any(|(l, _)| near_box(l, (width / 2.0).max(tol))),
            };
            if !near {
                return None;
            }
            let hit = match &s.geometry {
                Geometry::Fill { rings, rule } => {
                    point_inside(rings, *rule, p)
                        || rings.iter().any(|r| dist_to_polyline(r, true, p) <= tol)
                }
                Geometry::Stroke { lines, width } => lines
                    .iter()
                    .any(|(l, closed)| dist_to_polyline(l, *closed, p) <= (width / 2.0).max(tol)),
            };
            hit.then_some(i)
        })
}

fn dist_to_polyline(pts: &[P], closed: bool, p: P) -> f64 {
    let n = pts.len();
    if n == 0 {
        return f64::INFINITY;
    }
    let segs = if closed { n } else { n - 1 };
    (0..segs)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let ab = b - a;
            let t = if ab.dot(ab) > 1e-12 {
                ((p - a).dot(ab) / ab.dot(ab)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            p.dist(a + ab * t)
        })
        .fold(f64::INFINITY, f64::min)
}

/// The hoop: a sheet of fabric in the hoop's shape, lifted off the desk with
/// a soft shadow, a faint center mark and a small size label.
#[allow(clippy::too_many_arguments)]
pub fn draw_hoop(
    painter: &egui::Painter,
    rect: Rect,
    view: &View,
    w_mm: f64,
    h_mm: f64,
    label: &str,
    over: bool,
    cloth: Color32,
) {
    let p = crate::theme::pal(painter.ctx());
    let a = view.to_screen(rect, P::new(-w_mm * 5.0, -h_mm * 5.0));
    let b = view.to_screen(rect, P::new(w_mm * 5.0, h_mm * 5.0));
    let r = Rect::from_two_pos(a, b);
    let radius = (8.0 * view.scale / 8.0).clamp(4.0, 24.0);
    let shadow = egui::Shadow {
        offset: [0, 4],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 120 } else { 40 }),
    };
    painter.add(shadow.as_shape(r, radius));
    painter.rect_filled(r, radius, cloth);
    let edge = if over {
        p.error
    } else {
        Color32::from_black_alpha(if p.dark { 90 } else { 45 })
    };
    painter.rect_stroke(
        r,
        radius,
        Stroke::new(if over { 2.0 } else { 1.0 }, edge),
        egui::StrokeKind::Outside,
    );
    // Center mark, like the hoop's template sheet.
    let c = view.to_screen(rect, P::default());
    let s = Stroke::new(1.0, Color32::from_black_alpha(40));
    painter.line_segment([c - Vec2::new(8.0, 0.0), c + Vec2::new(8.0, 0.0)], s);
    painter.line_segment([c - Vec2::new(0.0, 8.0), c + Vec2::new(0.0, 8.0)], s);
    painter.text(
        r.left_top() + Vec2::new(2.0, -6.0),
        egui::Align2::LEFT_BOTTOM,
        label,
        egui::FontId::proportional(11.0),
        if over { p.error } else { p.secondary },
    );
}

/// For a selected satin: its two rails and a few stitch directions, so you can
/// see how the column was laid out (especially for automatic satins).
pub fn draw_rails(painter: &egui::Painter, rect: Rect, view: &View, info: &crate::app::SatinInfo) {
    let accent = Color32::from_rgb(200, 120, 255);
    for (a, b) in &info.rails {
        for rail in [a, b] {
            let pts: Vec<Pos2> = rail.iter().map(|&p| view.to_screen(rect, p)).collect();
            painter.add(egui::Shape::line(pts, Stroke::new(1.5, accent)));
        }
    }
    let rung = Stroke::new(1.0, accent.gamma_multiply(0.6));
    for &(a, b) in &info.rungs {
        painter.line_segment([view.to_screen(rect, a), view.to_screen(rect, b)], rung);
    }
}

#[cfg(test)]
mod perf_bench {
    use crate::app::Studio;
    use eframe::egui::Vec2;
    use egui_kittest::Harness;

    fn time(h: &mut Harness<'static, Studio>, what: &str) {
        h.run_steps(3);
        let t = std::time::Instant::now();
        for _ in 0..20 {
            h.step();
        }
        let shapes = h.output().shapes.clone();
        let n = shapes.len();
        let tt = std::time::Instant::now();
        let _ = h.ctx.tessellate(shapes, 1.0);
        println!(
            "{what}: frame {:.2} ms, tessellate {:.2} ms ({n} shapes)",
            t.elapsed().as_secs_f64() * 1000.0 / 20.0,
            tt.elapsed().as_secs_f64() * 1000.0
        );
    }

    #[test]
    #[ignore]
    fn idle_frame_cost() {
        let png = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/agent-badge-preview.png"
        ));
        let mut h = Harness::builder()
            .with_size(Vec2::new(1600.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        let doc = zigzag::document::Document::open(&png).unwrap();
        h.state_mut().set_document(doc);
        h.run_steps(5);
        println!("stitches {}", h.state().design.stitch_count());
        time(&mut h, "idle, nothing selected");
        let big = (0..h.state().doc.shapes.len())
            .max_by_key(|&i| {
                h.state()
                    .doc
                    .edit_path(i)
                    .map(|p| p[0].anchors.len())
                    .unwrap_or(0)
            })
            .unwrap();
        h.state_mut().set_selection(&[big]);
        time(&mut h, "idle, big shape selected");
        h.state_mut().doc.shapes[big].params.kind = Some(zigzag::params::Kind::Satin);
        h.state_mut().dirty = true;
        h.run_steps(30);
        time(&mut h, "idle, big satin selected");
        h.state_mut().doc.shapes[big].params.kind = None;
        h.state_mut().dirty = true;
        h.run_steps(30);
        h.state_mut().set_tool(crate::app::Tool::Direct);
        time(&mut h, "direct tool on big shape");
    }
}
