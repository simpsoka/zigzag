//! Small pictures of the objects in the Layers list: each one's outline in
//! its thread color, on a tile of the cloth's.
//!
//! A picture is drawn once (anti-aliased, at the screen's pixels) and kept
//! as a texture; a row then costs one textured rectangle a frame. It's
//! looked at again only when the document or its plan has changed (the
//! `Stamp`), drawn again only if its outline or color did, and not at all
//! while something is being dragged. Drawing is kept to a few milliseconds
//! a frame: a long list fills in over the next frames rather than holding
//! one up.
//!
//! (The way VectorCraft's Layers panel keeps its thumbnails: a store of
//! textures, each with the revision of what it shows.)

use eframe::egui::{self, Color32, Rect};
use resvg::tiny_skia;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use zigzag::geom::{FillRule, P};
use zigzag::svg::{Geometry, Shape};

/// Whose picture: an object's (its uid), or a group's (its id, and the uid
/// of the first object the row shows: a group listed in runs has a row,
/// and a picture, for each).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Shape(usize),
    Group(String, usize),
}

/// What a picture was last checked against: the document's revision and
/// how many times it has been planned. While neither changes, no outline
/// has, and a picture isn't looked at again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    pub rev: u64,
    pub plans: u64,
}

struct Entry {
    stamp: Stamp,
    /// Of everything the picture shows (`fingerprint`).
    hash: u64,
    /// None: nothing to show (no points).
    texture: Option<egui::TextureHandle>,
    /// The frame it was last shown in.
    used: u64,
}

/// Drawing new pictures stops for the frame once it has taken this long
/// (the one in hand is finished; the rest come next frame).
const BUDGET: Duration = Duration::from_millis(3);
/// A picture not shown for this many frames is forgotten.
const KEEP_FRAMES: u64 = 600;
/// The largest picture, in pixels a side.
const MAX_PX: u32 = 96;

#[derive(Default)]
pub struct Thumbs {
    entries: HashMap<Key, Entry>,
    frame: u64,
    /// Spent drawing pictures this frame.
    spent: Duration,
    /// Pictures drawn, ever (for tests, and for timing).
    pub(crate) drawn: usize,
}

impl Thumbs {
    /// Show the picture of `shapes` (one object, or a group's, bottom
    /// first) in `rect`, on a tile of `cloth`. `hold`: something is being
    /// dragged, so a picture there already stays as it is.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        key: Key,
        shapes: &[&Shape],
        cloth: Color32,
        stamp: Stamp,
        hold: bool,
    ) {
        if !painter.clip_rect().intersects(rect) {
            return;
        }
        let ctx = painter.ctx();
        self.tick(ctx.cumulative_frame_nr());
        let side = rect.width().min(rect.height());
        let px = ((side * ctx.pixels_per_point()).round() as u32).clamp(1, MAX_PX);
        let frame = self.frame;
        let fresh = self
            .entries
            .get(&key)
            .is_some_and(|e| hold || e.stamp == stamp);
        if !fresh {
            let hash = fingerprint(shapes, px);
            if let Some(e) = self.entries.get_mut(&key).filter(|e| e.hash == hash) {
                e.stamp = stamp;
            } else if self.spent < BUDGET {
                // (The first of a frame is always drawn.)
                let t = Instant::now();
                let texture = render(shapes, px)
                    .map(|img| ctx.load_texture("layer-thumb", img, egui::TextureOptions::LINEAR));
                self.spent += t.elapsed();
                self.drawn += 1;
                self.entries.insert(
                    key.clone(),
                    Entry {
                        stamp,
                        hash,
                        texture,
                        used: frame,
                    },
                );
            } else {
                // Out of time: the old picture (if any) for a frame more.
                ctx.request_repaint();
            }
        }
        if let Some(e) = self.entries.get_mut(&key) {
            e.used = frame;
        }
        painter.rect_filled(rect, 3.0, cloth);
        painter.rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0, Color32::from_black_alpha(40)),
            egui::StrokeKind::Inside,
        );
        if let Some(t) = self.entries.get(&key).and_then(|e| e.texture.as_ref()) {
            painter.image(
                t.id(),
                rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }

    /// A new frame: time to draw again, and (now and then) pictures of
    /// what's no longer listed are let go.
    fn tick(&mut self, frame: u64) {
        if frame == self.frame {
            return;
        }
        self.frame = frame;
        self.spent = Duration::ZERO;
        if frame % KEEP_FRAMES == 0 {
            self.entries.retain(|_, e| e.used + KEEP_FRAMES >= frame);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Everything a picture depends on: each object's color and every
/// coordinate of its outline, and the size it's drawn at.
fn fingerprint(shapes: &[&Shape], px: u32) -> u64 {
    use std::hash::{Hash, Hasher};
    fn eat(h: &mut impl Hasher, pts: &[P]) {
        pts.len().hash(h);
        for p in pts {
            p.x.to_bits().hash(h);
            p.y.to_bits().hash(h);
        }
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    px.hash(&mut h);
    for s in shapes {
        s.color.hash(&mut h);
        match &s.geometry {
            Geometry::Fill { rings, rule } => {
                (0u8, *rule == FillRule::EvenOdd, rings.len()).hash(&mut h);
                for r in rings {
                    eat(&mut h, r);
                }
            }
            Geometry::Stroke { lines, width } => {
                (1u8, width.to_bits(), lines.len()).hash(&mut h);
                for (l, closed) in lines {
                    closed.hash(&mut h);
                    eat(&mut h, l);
                }
            }
        }
    }
    h.finish()
}

fn finite(p: &P) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

/// The picture of `shapes`, `px` pixels square, clear around them: fitted
/// to the square whole, in proportion. None when there's nothing to draw.
fn render(shapes: &[&Shape], px: u32) -> Option<egui::ColorImage> {
    let points = || {
        shapes.iter().flat_map(|s| {
            let (rings, lines) = match &s.geometry {
                Geometry::Fill { rings, .. } => (Some(rings), None),
                Geometry::Stroke { lines, .. } => (None, Some(lines)),
            };
            rings
                .into_iter()
                .flatten()
                .flatten()
                .chain(lines.into_iter().flatten().flat_map(|(l, _)| l))
                .filter(|p| finite(p))
        })
    };
    let (mut lo, mut hi) = (
        P::new(f64::INFINITY, f64::INFINITY),
        P::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
    );
    for p in points() {
        lo = P::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = P::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    if !lo.x.is_finite() {
        return None;
    }
    let side = px as f64;
    // The thinnest line drawn (a run, a sliver of a fill): about a point.
    let hair = (side / 16.0).max(1.0);
    // (Halved before subtracting: far-apart coordinates don't overflow.)
    let span = ((hi.x / 2.0 - lo.x / 2.0).max(hi.y / 2.0 - lo.y / 2.0)) * 2.0;
    // (Clear of the tile's edge, which a line along it would be lost in.)
    let room = (side - side / 6.0 - hair).max(1.0);
    let scale = if span > 0.0 && span.is_finite() {
        room / span
    } else {
        0.0
    };
    let mid = P::new(lo.x / 2.0 + hi.x / 2.0, lo.y / 2.0 + hi.y / 2.0);
    let at = |p: &P| {
        (
            ((p.x - mid.x) * scale + side / 2.0) as f32,
            ((p.y - mid.y) * scale + side / 2.0) as f32,
        )
    };
    let trace = |pb: &mut tiny_skia::PathBuilder, pts: &[P], close: bool| {
        let mut pts = pts.iter().filter(|p| finite(p));
        let Some(first) = pts.next() else { return };
        let (x, y) = at(first);
        pb.move_to(x, y);
        // Points less than a third of a pixel on from the last one drawn
        // are passed over (the last of a run of them is drawn): a traced
        // outline has thousands that a picture this size can't show.
        const NEAR: f32 = 0.33;
        let (mut last, mut skipped) = ((x, y), None);
        let mut more = false;
        for p in pts {
            let q = at(p);
            if (q.0 - last.0).abs() + (q.1 - last.1).abs() < NEAR {
                skipped = Some(q);
                continue;
            }
            pb.line_to(q.0, q.1);
            (last, skipped, more) = (q, None, true);
        }
        if let Some(q) = skipped.filter(|&q| q != last) {
            pb.line_to(q.0, q.1);
            more = true;
        }
        if !more {
            // A point alone: a dot (a line of no length, round-capped).
            pb.line_to(x, y);
        } else if close {
            pb.close();
        }
    };
    let mut pm = tiny_skia::Pixmap::new(px, px)?;
    let line = |width: f64| tiny_skia::Stroke {
        width: width as f32,
        line_cap: tiny_skia::LineCap::Round,
        line_join: tiny_skia::LineJoin::Round,
        ..Default::default()
    };
    let none = tiny_skia::Transform::identity();
    for s in shapes {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(s.color[0], s.color[1], s.color[2], 255);
        paint.anti_alias = true;
        let mut pb = tiny_skia::PathBuilder::new();
        match &s.geometry {
            Geometry::Fill { rings, rule } => {
                for r in rings {
                    trace(&mut pb, r, true);
                }
                let Some(path) = pb.finish() else { continue };
                let rule = match rule {
                    FillRule::NonZero => tiny_skia::FillRule::Winding,
                    FillRule::EvenOdd => tiny_skia::FillRule::EvenOdd,
                };
                pm.fill_path(&path, &paint, rule, none, None);
                // (A hairline round it, so a sliver still shows; plain
                // joins, which are much the quicker.)
                let edge = tiny_skia::Stroke {
                    width: (hair / 2.0) as f32,
                    ..Default::default()
                };
                pm.stroke_path(&path, &paint, &edge, none, None);
            }
            Geometry::Stroke { lines, width } => {
                for (l, closed) in lines {
                    trace(&mut pb, l, *closed);
                }
                let Some(path) = pb.finish() else { continue };
                let w = (width * scale).clamp(hair, side);
                pm.stroke_path(&path, &paint, &line(w), none, None);
            }
        }
    }
    Some(egui::ColorImage::from_rgba_premultiplied(
        [px as usize, px as usize],
        pm.data(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shapes(svg: &str) -> Vec<Shape> {
        zigzag::document::Document::from_svg(svg, "t", None)
            .unwrap()
            .shapes
    }

    fn svg(body: &str) -> String {
        format!(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40">{body}</svg>"##)
    }

    fn pixel(img: &egui::ColorImage, x: usize, y: usize) -> Color32 {
        img.pixels[y * img.size[0] + x]
    }

    #[test]
    fn a_fill_is_solid_in_its_color_with_its_holes_open() {
        let s = shapes(&svg(
            r##"<path fill="#e01020" fill-rule="evenodd" d="M0 0H30V30H0Z M10 10H20V20H10Z"/>"##,
        ));
        let img = render(&[&s[0]], 40).unwrap();
        assert_eq!(pixel(&img, 6, 20), Color32::from_rgb(224, 16, 32));
        assert_eq!(pixel(&img, 20, 20), Color32::TRANSPARENT, "the hole");
        assert_eq!(pixel(&img, 0, 0), Color32::TRANSPARENT, "the margin");
    }

    #[test]
    fn a_line_is_drawn_thick_enough_to_see_and_a_wide_shape_keeps_its_proportion() {
        let s = shapes(&svg(
            r##"<path fill="none" stroke="#1020e0" stroke-width="0.01" d="M0 20H40"/>"##,
        ));
        let img = render(&[&s[0]], 40).unwrap();
        let inked = |y: usize| (0..40).filter(|&x| pixel(&img, x, y).a() > 128).count();
        assert!(inked(20) > 30, "across the middle");
        assert_eq!(inked(8) + inked(32), 0, "and only there");
        let rows = (0..40).filter(|&y| inked(y) > 0).count();
        assert!((2..=5).contains(&rows), "{rows} rows of ink");
    }

    #[test]
    fn nothing_and_odd_coordinates_are_safe() {
        let mut s = shapes(&svg(r##"<path fill="#000" d="M0 0H10V10H0Z"/>"##)).remove(0);
        for rings in [
            vec![],
            vec![vec![]],
            vec![vec![P::new(f64::NAN, 0.0)]],
            vec![vec![P::new(1.0, 1.0); 3]],
            vec![vec![
                P::new(-f64::MAX, -f64::MAX),
                P::new(f64::MAX, -f64::MAX),
                P::new(f64::MAX, f64::MAX),
            ]],
        ] {
            let empty = rings.iter().flatten().filter(|p| finite(p)).count() == 0;
            s.geometry = Geometry::Fill {
                rings,
                rule: FillRule::NonZero,
            };
            assert_eq!(render(&[&s], 40).is_none(), empty);
        }
    }

    /// A list of pictures painted for a few frames.
    fn frames(
        ctx: &egui::Context,
        thumbs: &mut Thumbs,
        shapes: &[Shape],
        stamp: Stamp,
        hold: bool,
        n: usize,
    ) {

        for _ in 0..n {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                // (Side by side: all in view, however many.)
                let painter = ui.painter().with_clip_rect(Rect::EVERYTHING);
                for (i, s) in shapes.iter().enumerate() {
                    let rect = Rect::from_min_size(
                        egui::pos2(i as f32 * 20.0, 0.0),
                        egui::Vec2::splat(18.0),
                    );
                    thumbs.paint(
                        &painter,
                        rect,
                        Key::Shape(s.uid),
                        &[s],
                        Color32::WHITE,
                        stamp,
                        hold,
                    );
                }
            });
            out.textures_delta.clear();
        }
    }

    #[test]
    fn a_picture_is_drawn_once_and_again_only_when_its_object_changes() {
        let mut s = shapes(&svg(
            r##"<path id="a" fill="#e01020" d="M0 0H10V10H0Z"/><path id="b" fill="#1020e0" d="M20 0H30V10H20Z"/>"##,
        ));
        let mut t = Thumbs::default();
        let ctx = egui::Context::default();
        let one = Stamp { rev: 1, plans: 1 };
        frames(&ctx, &mut t, &s, one, false, 5);
        assert_eq!((t.drawn, t.len()), (2, 2), "once each, however many frames");
        // The document changed, but not these outlines: looked at, not drawn.
        let two = Stamp { rev: 2, plans: 1 };
        frames(&ctx, &mut t, &s, two, false, 3);
        assert_eq!(t.drawn, 2);
        // One's color changed: that one is drawn again.
        s[0].color = [0, 200, 0];
        frames(&ctx, &mut t, &s, Stamp { rev: 3, plans: 1 }, false, 3);
        assert_eq!(t.drawn, 3);
        // Dragged: the picture waits (even as the plan follows the drag)…
        if let Geometry::Fill { rings, .. } = &mut s[1].geometry {
            rings[0][0].x += 5.0;
        }
        frames(&ctx, &mut t, &s, Stamp { rev: 3, plans: 2 }, true, 3);
        assert_eq!(t.drawn, 3);
        // …and is drawn when it's let go.
        frames(&ctx, &mut t, &s, Stamp { rev: 3, plans: 2 }, false, 3);
        assert_eq!(t.drawn, 4);
    }

    #[test]
    fn a_long_list_fills_in_over_frames_within_the_budget() {
        // Heavy outlines (20,000 points each), more than a frame's worth.
        let mut one = shapes(&svg(r##"<path fill="#000" d="M0 0H10V10H0Z"/>"##)).remove(0);
        let ring: Vec<P> = (0..20_000)
            .map(|i| {
                let a = i as f64 / 20_000.0 * std::f64::consts::TAU;
                let r = 100.0 + 20.0 * (a * 40.0).sin();
                P::new(r * a.cos(), r * a.sin())
            })
            .collect();
        one.geometry = Geometry::Fill {
            rings: vec![ring],
            rule: FillRule::NonZero,
        };
        let many: Vec<Shape> = (0..60)
            .map(|i| {
                let mut s = one.clone();
                s.uid = 1000 + i;
                s
            })
            .collect();
        let mut t = Thumbs::default();
        let ctx = egui::Context::default();
        let stamp = Stamp { rev: 1, plans: 1 };
        frames(&ctx, &mut t, &many, stamp, false, 1);
        let first = t.drawn;
        assert!(first >= 1, "the first frame makes a start");
        for _ in 0..600 {
            if t.drawn == many.len() {
                break;
            }
            frames(&ctx, &mut t, &many, stamp, false, 1);
        }
        assert_eq!(t.drawn, many.len(), "and the rest follow");
    }

    /// How long pictures take (`cargo test --release --features gui thumb_timing -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn thumb_timing() {
        let mut one = shapes(&svg(r##"<path fill="#000" d="M0 0H10V10H0Z"/>"##)).remove(0);
        for n in [100, 2_000, 20_000, 200_000] {
            let ring: Vec<P> = (0..n)
                .map(|i| {
                    let a = i as f64 / n as f64 * std::f64::consts::TAU;
                    let r = 100.0 + 20.0 * (a * 40.0).sin();
                    P::new(r * a.cos(), r * a.sin())
                })
                .collect();
            one.geometry = Geometry::Fill {
                rings: vec![ring],
                rule: FillRule::NonZero,
            };
            let t = Instant::now();
            for _ in 0..20 {
                std::hint::black_box(render(&[&one], 40));
            }
            let draw = t.elapsed() / 20;
            let t = Instant::now();
            for _ in 0..20 {
                std::hint::black_box(fingerprint(&[&one], 40));
            }
            eprintln!("{n:>7} points: draw {draw:?}, check {:?}", t.elapsed() / 20);
        }
    }
}
