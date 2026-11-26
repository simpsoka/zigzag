//! Design canvas: view transform, stitch rendering, selection and hit testing.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use zigzag::design::Design;
use zigzag::geom::{FillRule, P, point_inside};
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
    /// Scale by `factor` about the canvas center (the interface changing
    /// size under a design that isn't to).
    pub fn rescale(&mut self, factor: f32) {
        self.scale = (self.scale * factor).clamp(1.0, 120.0);
    }
    /// Fit a box of `w_mm` × `h_mm` centered at `c` into `rect`.
    pub fn fit(&mut self, rect: Rect, c: P, w_mm: f64, h_mm: f64) {
        self.center = c;
        let s = (rect.width() as f64 / (w_mm * 1.15)).min(rect.height() as f64 / (h_mm * 1.15));
        self.scale = (s as f32).clamp(1.0, 120.0);
    }
}

/// How much detail fits in a length in points: what it's multiplied by
/// before it's judged. It's the interface size, so that a design is drawn
/// the same at every interface size, where a point is more or fewer pixels.
pub fn detail(ctx: &egui::Context) -> f32 {
    ctx.zoom_factor()
}

/// Whether thread `w` points wide is drawn shaded, as a round thread (any
/// thinner and it's a flat line: there's nowhere to put the shading).
pub fn shaded(detail: f32, w: f32) -> bool {
    w * detail >= 2.2
}

/// Whether the Lines view shows needle points at `view`: once there's room
/// to see them.
pub fn needle_points(detail: f32, view: &View) -> bool {
    view.scale * detail > 10.0
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
    /// What's moving is a copy being dragged off (Option held): those
    /// blocks are drawn where they were as well, as they'll stay.
    pub copy: bool,
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
    /// With a stitch limit (the player, a sew in progress): the block being
    /// sewn, as far as it's drawn. It grows by the new stitches only.
    partial: Option<Partial>,
}

struct Partial {
    entry: MeshEntry,
    /// The block's stitches in the mesh.
    upto: usize,
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

/// Draw the stitch plan; returns where the last stitch drawn is. Each color block's
/// thousands of thread segments are tessellated once into a mesh, reused
/// frame after frame (hovering, selecting, editing points, typing) until
/// that block, the zoom or how stitches are shown changes; panning moves
/// the meshes. Blocks being moved, scaled or rotated keep their mesh too,
/// drawn through the gesture's transform (`opt.moving`). With a stitch
/// limit (the player, a sew in progress), finished blocks are drawn from
/// their meshes and the one in progress grows by its new stitches.
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
    use std::hash::{Hash, Hasher};
    let limited = opt.limit != usize::MAX;
    let mut remaining = opt.limit;
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
        if limited && remaining == 0 {
            break;
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
        let count: usize = blk.runs.iter().map(|r| r.pts.len()).sum();
        if limited && count > remaining {
            // The block being sewn: drawn as far as the limit.
            if through.is_some() {
                let one = one(k);
                let mut shapes = Vec::new();
                let end = design_shapes(
                    detail(ctx),
                    rect,
                    view,
                    d,
                    &DrawOptions {
                        limit: remaining,
                        ..opt.clone()
                    },
                    Some(rect),
                    Some((&one, true)),
                    &mut shapes,
                );
                painter.extend(shapes);
                last = end.or(last);
            } else {
                let p = partial_block(
                    ctx,
                    ppp,
                    rect,
                    view,
                    blk,
                    opt,
                    key,
                    origin,
                    remaining,
                    cache.partial.take(),
                );
                last = stitch_at(blk, remaining - 1)
                    .map(|p| view.to_screen(rect, p))
                    .or(last);
                cache.partial = Some(p);
                let p = cache.partial.as_mut().unwrap();
                p.entry.paint(painter, origin);
            }
            break;
        }
        remaining = remaining.saturating_sub(count);
        let entry = match old
            .iter()
            .position(|e| e.as_ref().is_some_and(|e| e.key == key))
        {
            Some(i) => old[i].take().unwrap(),
            None => {
                let mut shapes = Vec::new();
                let flags = one(k);
                design_shapes(
                    detail(ctx),
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
            Some(m) => {
                if opt.copy {
                    entry.paint(painter, origin);
                }
                entry.paint_through(painter, origin, rect, view, m)
            }
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

/// Stitch `i` of a block (counting through its runs).
fn stitch_at(blk: &zigzag::design::Block, mut i: usize) -> Option<P> {
    for r in &blk.runs {
        if i < r.pts.len() {
            return Some(r.pts[i]);
        }
        i -= r.pts.len();
    }
    None
}

/// The mesh of a block's first `upto` stitches, grown from `prev` (the
/// last frame's) when it's the same block drawn the same way, so only the
/// stitches sewn since are tessellated.
#[allow(clippy::too_many_arguments)]
fn partial_block(
    ctx: &egui::Context,
    ppp: f32,
    rect: Rect,
    view: &View,
    blk: &zigzag::design::Block,
    opt: &DrawOptions,
    key: u64,
    origin: Pos2,
    upto: usize,
    prev: Option<Partial>,
) -> Partial {
    let (mut entry, from) = match prev {
        Some(mut p) if p.entry.key == key && p.upto <= upto => {
            p.entry.place(origin);
            (p.entry, p.upto)
        }
        _ => (
            MeshEntry {
                key,
                origin,
                mesh: std::sync::Arc::new(egui::Mesh::default()),
            },
            0,
        ),
    };
    if from < upto {
        // Stitches from..upto, with the one before so the line joins up
        // (on its own when it ends the run before: a jump, not a stitch).
        let start = from.saturating_sub(1);
        let mut runs = Vec::new();
        let mut s = 0;
        for r in &blk.runs {
            let e = s + r.pts.len();
            let (lo, hi) = (s.max(start), e.min(upto));
            if lo < hi {
                runs.push(zigzag::design::Run {
                    pts: r.pts[lo - s..hi - s].to_vec(),
                    trim_before: r.trim_before,
                });
            }
            s = e;
        }
        let piece = Design {
            blocks: vec![zigzag::design::Block {
                color: blk.color,
                runs,
            }],
            ..Design::default()
        };
        let mut shapes = Vec::new();
        design_shapes(
            detail(ctx),
            rect,
            view,
            &piece,
            &DrawOptions {
                limit: usize::MAX,
                moving: None,
                hidden: None,
                ..opt.clone()
            },
            None,
            None,
            &mut shapes,
        );
        let more = tessellate(ctx, ppp, shapes);
        std::sync::Arc::make_mut(&mut entry.mesh).append(more);
    }
    Partial { entry, upto }
}

/// The shapes that draw the stitch plan (only those inside `clip`, when
/// given). Returns the position of the last stitch drawn.
/// `blocks`: draw only the blocks whose flag is (or isn't) set.
fn design_shapes(
    detail: f32,
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
                    if needle_points(detail, view) {
                        let r = (view.scale * 0.06).clamp(1.2, 2.2);
                        for &q in pts.iter().filter(|q| seen(**q)) {
                            out.push(egui::Shape::circle_filled(q, r, line));
                        }
                    }
                }
                StitchView::Thread if shaded(detail, w) => {
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
    let near_box = |pts: &mut dyn Iterator<Item = &P>, tol: f64| {
        let mut b = zigzag::geom::Bounds::empty();
        pts.for_each(|&q| b.add(q));
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
            // (A stroke's box is all its lines': a satin drawn as rails and
            // rungs is hit between them, where no one line's box is.)
            let near = match &s.geometry {
                Geometry::Fill { rings, .. } => rings.iter().any(|r| near_box(&mut r.iter(), tol)),
                Geometry::Stroke { lines, width } => near_box(
                    &mut lines.iter().flat_map(|(l, _)| l),
                    (width / 2.0).max(tol),
                ),
            };
            if !near {
                return None;
            }
            let hit = match &s.geometry {
                Geometry::Fill { rings, rule } => {
                    point_inside(rings, *rule, p)
                        || rings.iter().any(|r| dist_to_polyline(r, true, p) <= tol)
                }
                Geometry::Stroke { lines, width } => {
                    lines.iter().any(|(l, closed)| {
                        dist_to_polyline(l, *closed, p) <= (width / 2.0).max(tol)
                    }) || column_of(s).is_some_and(|ring| point_inside(&ring, FillRule::EvenOdd, p))
                }
            };
            hit.then_some(i)
        })
}

/// What a satin drawn as its two rails covers: the column between them
/// (the rings of a closed one), where its stitches are, and so where a
/// click picks it. Its lines alone are a hairline each.
fn column_of(s: &Shape) -> Option<Vec<Vec<P>>> {
    let Geometry::Stroke { lines, .. } = &s.geometry else {
        return None;
    };
    if s.params.kind != Some(zigzag::params::Kind::Satin)
        || s.outline
        || s.is_single_line()
        || lines.len() < 2
    {
        return None;
    }
    let r = zigzag::stitch::satin::rails_from_subpaths(lines, s.params.satin_rails())?;
    Some(r.outline())
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

#[cfg(test)]
mod tests {
    use super::*;
    use zigzag::design::{Block, Run};

    fn zigzag_design() -> Design {
        let run = |y: f64, n: usize, trim: bool| Run {
            pts: (0..n)
                .map(|i| P::new(i as f64 * 20.0, y + if i % 2 == 0 { 0.0 } else { 30.0 }))
                .collect(),
            trim_before: trim,
        };
        Design {
            blocks: vec![
                Block {
                    color: [200, 30, 30],
                    runs: vec![run(0.0, 40, true), run(100.0, 25, true)],
                },
                Block {
                    color: [30, 30, 200],
                    runs: vec![run(200.0, 50, true)],
                },
            ],
            ..Design::default()
        }
    }

    /// Vertices drawn, and where the needle is, with the stitches up to `limit`.
    fn draw(
        ctx: &egui::Context,
        d: &Design,
        limit: usize,
        cache: &mut Option<StitchMesh>,
    ) -> (usize, Option<Pos2>) {
        let mut needle = None;
        let mut out = ctx.run_pass(egui::RawInput::default(), |ctx| {
            let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
            let painter = egui::Painter::new(ctx.clone(), egui::LayerId::background(), rect);
            let opt = DrawOptions {
                show_jumps: false,
                limit,
                thread_mm: 0.4,
                view: StitchView::Thread,
                background: [255, 255, 255],
                moving: None,
                copy: false,
                hidden: None,
            };
            let view = View {
                center: P::new(400.0, 100.0),
                scale: 10.0,
            };
            needle = draw_design_cached(&painter, rect, &view, d, &opt, 1, cache);
        });
        let vertices = out
            .shapes
            .iter()
            .map(|c| match &c.shape {
                egui::Shape::Mesh(m) => m.vertices.len(),
                _ => 0,
            })
            .sum();
        // This headless test inspects geometry without uploading textures.
        out.textures_delta.clear();
        (vertices, needle)
    }

    #[test]
    fn a_stitch_limit_grows_the_mesh_to_the_same_picture() {
        let ctx = egui::Context::default();
        let d = zigzag_design();
        let mut stepped = None;
        for limit in [1, 7, 39, 40, 41, 52, 65, 66, 90] {
            let grown = draw(&ctx, &d, limit, &mut stepped);
            let whole = draw(&ctx, &d, limit, &mut None);
            assert_eq!(grown, whole, "at {limit}");
        }
        // Back to an earlier stitch: drawn again from the start of its block.
        assert_eq!(
            draw(&ctx, &d, 20, &mut stepped),
            draw(&ctx, &d, 20, &mut None)
        );
        // No limit: everything.
        let all = draw(&ctx, &d, usize::MAX, &mut None);
        assert_eq!(draw(&ctx, &d, 115, &mut stepped).0, all.0);
        // The last stitch, (980, 230), with the view centered on (400, 100).
        assert_eq!(all.1, Some(egui::pos2(980.0, 430.0)));
    }

    #[test]
    fn a_copy_being_dragged_is_drawn_where_it_was_as_well() {
        // The second block carried 30 mm to the right: moved, its
        // stitches are drawn once, there; as a copy, there and where they
        // were (the same again).
        let d = zigzag_design();
        let drawn = |moving: Option<(Vec<bool>, zigzag::geom::Affine)>, copy: bool| {
            let ctx = egui::Context::default();
            let mut xs: Vec<f32> = Vec::new();
            let mut out = ctx.run_pass(egui::RawInput::default(), |ctx| {
                let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
                let painter = egui::Painter::new(ctx.clone(), egui::LayerId::background(), rect);
                let opt = DrawOptions {
                    show_jumps: false,
                    limit: usize::MAX,
                    thread_mm: 0.4,
                    view: StitchView::Thread,
                    background: [255, 255, 255],
                    moving: moving.clone(),
                    copy,
                    hidden: None,
                };
                let view = View {
                    center: P::new(400.0, 100.0),
                    scale: 10.0,
                };
                draw_design_cached(&painter, rect, &view, &d, &opt, 1, &mut None);
            });
            let mut vertices = 0;
            for c in &out.shapes {
                if let egui::Shape::Mesh(m) = &c.shape {
                    vertices += m.vertices.len();
                    xs.extend(m.vertices.iter().map(|v| v.pos.x));
                }
            }
            out.textures_delta.clear();
            let far = xs.iter().copied().fold(f32::MIN, f32::max);
            (vertices, far)
        };
        let carry = Some((vec![false, true], [1.0, 0.0, 0.0, 1.0, 300.0, 0.0]));
        let (still, far) = drawn(None, false);
        let (moved, far_moved) = drawn(carry.clone(), false);
        let (copied, far_copied) = drawn(carry, true);
        assert_eq!(moved, still, "moved: the same stitches, elsewhere");
        assert!(
            (far_moved - far - 300.0).abs() < 1.0,
            "{far_moved} from {far}"
        );
        // The second block's stitches, twice.
        let second = {
            let one = Design {
                blocks: vec![d.blocks[1].clone()],
                ..Design::default()
            };
            let ctx = egui::Context::default();
            draw(&ctx, &one, usize::MAX, &mut None).0
        };
        assert_eq!(copied, still + second, "the copy, and what it's a copy of");
        assert!((far_copied - far_moved).abs() < 1.0);
    }
}

#[cfg(test)]
mod stays_on_the_canvas {
    use crate::app::{Studio, Tool};
    use eframe::egui::{self, Rect, Vec2};
    use egui_kittest::Harness;

    /// What's drawn over the canvas and on past its left or right edge, over
    /// a dock, and isn't cut off there. (Not what lies under the whole
    /// canvas: the window's background.)
    fn over_the_docks(h: &Harness<'_, Studio>) -> Vec<Rect> {
        let c = h.state().canvas_rect;
        h.output()
            .shapes
            .iter()
            .filter(|s| {
                let r = s.shape.visual_bounding_rect();
                let clip = s.clip_rect;
                r.is_finite()
                    && r.intersects(c.shrink(1.0))
                    && !r.contains_rect(c)
                    && ((r.left() < c.left() - 1.0 && clip.left() < c.left() - 0.5)
                        || (r.right() > c.right() + 1.0 && clip.right() > c.right() + 0.5))
            })
            .map(|s| s.shape.visual_bounding_rect())
            .collect()
    }

    #[test]
    fn nothing_on_the_canvas_is_drawn_over_the_docks() {
        // An iPad-sized window with both docks open: a narrow canvas, the
        // tools' option bars wider than it.
        let badge = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/badge.svg");
        let mut s = Studio::with_file(Some(badge), Some(2));
        s.dock.toggle(crate::dock::PanelId::Agent);
        let mut h = Harness::builder()
            .with_size(Vec2::new(1000.0, 760.0))
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), s);
        h.run_steps(3);
        let c = h.state().canvas_rect;
        assert!(c.width() < 320.0, "a narrow canvas: {c:?}");
        // The selection's task bar.
        let over = over_the_docks(&h);
        assert!(over.is_empty(), "with a selection: {over:?}");
        for tool in [
            Tool::Brush,
            Tool::Blob,
            Tool::Smooth,
            Tool::Shape,
            Tool::Pen,
            Tool::Text,
            Tool::Rotate,
        ] {
            h.state_mut().set_tool(tool);
            h.run_steps(3);
            let over = over_the_docks(&h);
            assert!(over.is_empty(), "{tool:?}: {over:?}");
        }
    }

    /// The bars floating over the canvas, and any part of one that's past
    /// its edges (cut off).
    fn bars_cut_off(h: &Harness<'_, Studio>) -> Vec<Rect> {
        let c = h.state().canvas_rect;
        h.ctx.memory(|m| {
            m.areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|l| matches!(l.order, egui::Order::Middle | egui::Order::Foreground))
                .filter_map(|l| m.area_rect(l.id))
                .filter(|r| r.intersects(c) && !c.expand(0.5).contains_rect(*r))
                .collect()
        })
    }

    #[test]
    fn the_bars_on_a_narrow_canvas_wrap_to_fit_it() {
        let badge = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/badge.svg");
        let mut s = Studio::with_file(Some(badge), Some(2));
        s.dock.toggle(crate::dock::PanelId::Agent);
        let mut h = Harness::builder()
            .with_size(Vec2::new(1000.0, 760.0))
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), s);
        h.run_steps(4);
        let cut = bars_cut_off(&h);
        assert!(
            cut.is_empty(),
            "the task bar: {cut:?} on {:?}",
            h.state().canvas_rect
        );
        for tool in [
            Tool::Brush,
            Tool::Blob,
            Tool::Smooth,
            Tool::Shape,
            Tool::Pen,
        ] {
            h.state_mut().set_tool(tool);
            h.run_steps(4);
            let cut = bars_cut_off(&h);
            assert!(
                cut.is_empty(),
                "{tool:?}: {cut:?} on {:?}",
                h.state().canvas_rect
            );
        }
    }
}
