//! What shows under an object while it's moved.
//!
//! With overlaps removed, a fill isn't sewn where an object sewn later
//! covers it: its stitches have a hole there. The stitches aren't planned
//! again until a gesture ends (see `Studio::update_drag`), so an object
//! dragged away would leave that hole behind it, the fabric showing
//! through in its shape, for as long as the drag lasts (and, where
//! planning is the worker's, a moment longer).
//!
//! Until the new plan is in, the hole is backed with the thread colors of
//! the fills it's cut in: flat color, not stitches, but the right color,
//! in the right places, under the stitches that are there.

use crate::app::{Studio, shape_bounds};
use crate::canvas::StitchView;
use eframe::egui::{self, Color32, Rect};
use resvg::tiny_skia;
use zigzag::document::Document;
use zigzag::geom::{Bounds, FillRule, P, Polyline};
use zigzag::params::Kind;
use zigzag::svg::Geometry;

/// The longest side of the picture, pixels (it's stretched past that:
/// its edges are under stitches, and painting it is a few milliseconds
/// in the frame the gesture starts moving).
const MAX_SIDE: f64 = 1024.0;

/// The backing of one gesture: what was under the objects it carries,
/// where they were when it began.
pub struct Backing {
    /// The fills' colors inside the carried objects' outlines (none:
    /// nothing under them had a hole to back).
    picture: Option<(egui::TextureHandle, Bounds)>,
}

/// The fills under `carried` (objects of `base`, the document before the
/// gesture) painted in their thread colors, inside the outlines of what's
/// carried: the picture and the box of the design it covers. `sew_order`:
/// the plan's (a fill is cut by what's sewn after it). `scale`: pixels a
/// design unit. None when nothing there has a hole.
pub(crate) fn picture(
    base: &Document,
    carried: &[usize],
    sew_order: &[usize],
    scale: f64,
) -> Option<(egui::ColorImage, Bounds)> {
    if !base.settings.remove_overlaps {
        return None;
    }
    let shapes = &base.shapes;
    // When each object is sewn (the document's order, for a plan that
    // isn't of these objects).
    let mut at: Vec<usize> = (0..shapes.len()).collect();
    let mut seen = vec![false; shapes.len()];
    if sew_order.len() == shapes.len()
        && sew_order
            .iter()
            .all(|&i| i < shapes.len() && !std::mem::replace(&mut seen[i], true))
    {
        for (k, &i) in sew_order.iter().enumerate() {
            at[i] = k;
        }
    }
    // What's carried covers (a hidden object is planned as one that's
    // off: nothing is cut for it).
    let covers: Vec<(usize, Vec<Polyline>, FillRule)> = carried
        .iter()
        .filter_map(|&j| shapes.get(j).map(|s| (j, s)))
        .filter(|(_, s)| !s.params.hidden())
        .filter_map(|(j, s)| zigzag::design::coverage(s).map(|(rings, rule)| (j, rings, rule)))
        .collect();
    let mut b = Bounds::empty();
    for (_, rings, _) in &covers {
        rings.iter().flatten().for_each(|&p| b.add(p));
    }
    if b.is_empty() || b.width() <= 0.0 || b.height() <= 0.0 {
        return None;
    }
    let last = covers.iter().map(|c| at[c.0]).max()?;
    // The fills sewn before it that reach under it, first sewn first.
    let mut under: Vec<usize> = (0..shapes.len())
        .filter(|i| !carried.contains(i) && at[*i] < last)
        .filter(|&i| {
            let s = &shapes[i];
            matches!(s.geometry, Geometry::Fill { .. })
                && matches!(
                    s.params.kind.unwrap_or(Kind::Fill),
                    Kind::Fill | Kind::Contour
                )
                && !s.params.hidden()
        })
        .filter(|&i| {
            let sb = shape_bounds(&shapes[i]);
            !sb.is_empty()
                && sb.min.x <= b.max.x
                && b.min.x <= sb.max.x
                && sb.min.y <= b.max.y
                && b.min.y <= sb.max.y
        })
        .collect();
    if under.is_empty() {
        return None;
    }
    under.sort_by_key(|&i| at[i]);

    // A pixel of margin all round, for the soft edge.
    let scale = scale
        .min((MAX_SIDE - 2.0) / b.width())
        .min((MAX_SIDE - 2.0) / b.height())
        .max(1e-3);
    let (w, h) = (
        (b.width() * scale).ceil() as u32 + 2,
        (b.height() * scale).ceil() as u32 + 2,
    );
    let to_px = tiny_skia::Transform::from_row(
        scale as f32,
        0.0,
        0.0,
        scale as f32,
        (1.0 - b.min.x * scale) as f32,
        (1.0 - b.min.y * scale) as f32,
    );
    let path = |rings: &[Polyline]| {
        let mut pb = tiny_skia::PathBuilder::new();
        for r in rings.iter().filter(|r| r.len() >= 3) {
            pb.move_to(r[0].x as f32, r[0].y as f32);
            for p in &r[1..] {
                pb.line_to(p.x as f32, p.y as f32);
            }
            pb.close();
        }
        pb.finish()
    };
    let rule = |r: FillRule| match r {
        FillRule::NonZero => tiny_skia::FillRule::Winding,
        FillRule::EvenOdd => tiny_skia::FillRule::EvenOdd,
    };
    // Only inside what's carried: elsewhere the stitches are whole, and
    // the fabric shows between them as it does everywhere.
    let mut inside = tiny_skia::Mask::new(w, h)?;
    for (_, rings, r) in &covers {
        if let Some(p) = path(rings) {
            inside.fill_path(&p, rule(*r), true, to_px);
        }
    }
    let mut pm = tiny_skia::Pixmap::new(w, h)?;
    for i in under {
        let s = &shapes[i];
        let Geometry::Fill { rings, rule: r } = &s.geometry else {
            continue;
        };
        let Some(p) = path(rings) else { continue };
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(s.color[0], s.color[1], s.color[2], 255);
        paint.anti_alias = true;
        pm.fill_path(&p, &paint, rule(*r), to_px, Some(&inside));
    }
    let pad = 1.0 / scale;
    let covered = Bounds {
        min: P::new(b.min.x - pad, b.min.y - pad),
        max: P::new(
            b.min.x - pad + w as f64 / scale,
            b.min.y - pad + h as f64 / scale,
        ),
    };
    Some((
        egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], pm.data()),
        covered,
    ))
}

impl Studio {
    /// Draw the backing of the gesture under way (or just ended, its
    /// stitches not planned yet), under the stitches. Made when the
    /// gesture first moves something; gone once the plan is the
    /// document's again.
    pub(crate) fn draw_backing(&mut self, painter: &egui::Painter, rect: Rect) {
        let moving = self.drag.as_ref().is_some_and(|d| d.split);
        if !moving && !self.dirty && !self.planner.pending {
            self.backing = None;
            return;
        }
        if self.backing.is_none()
            && let Some(d) = self.drag.as_ref().filter(|d| d.split)
        {
            let ctx = painter.ctx();
            let scale = (self.view.scale / 10.0 * ctx.pixels_per_point()) as f64;
            let picture = picture(&d.base, &d.carried, &self.design.sew_order, scale).map(
                |(image, covered)| {
                    (
                        ctx.load_texture("drag-backing", image, egui::TextureOptions::LINEAR),
                        covered,
                    )
                },
            );
            self.backing = Some(Backing { picture });
        }
        // (Not under stitch lines, which are drawn on bare fabric; nor
        // under a stitch-out part way through, where what's missing
        // hasn't been sewn.)
        if self.view_tools.stitch_view != StitchView::Thread || self.player.is_some() {
            return;
        }
        if let Some(Backing {
            picture: Some((texture, b)),
        }) = &self.backing
        {
            painter.image(
                texture.id(),
                Rect::from_two_pos(
                    self.view.to_screen(rect, b.min),
                    self.view.to_screen(rect, b.max),
                ),
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }

    /// Where the backing on show is (design units), if there is one.
    #[cfg(test)]
    pub(crate) fn backing_bounds(&self) -> Option<Bounds> {
        self.backing
            .as_ref()
            .and_then(|b| b.picture.as_ref())
            .map(|p| p.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;
    use egui_kittest::Harness;

    /// A red square with a blue disc on it, a green square beside them
    /// (not under the disc), and a yellow square sewn over the disc.
    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="60mm" viewBox="0 0 100 60">
        <rect id="under" x="10" y="10" width="40" height="40" fill="#c00000"/>
        <rect id="beside" x="60" y="10" width="20" height="20" fill="#00a000"/>
        <circle id="disc" cx="30" cy="30" r="10" fill="#0000c0"/>
        <rect id="over" x="28" y="28" width="4" height="4" fill="#c0c000"/>
    </svg>"##;

    fn doc() -> Document {
        let mut d = Document::from_svg(SVG, "t", None).unwrap();
        d.settings.remove_overlaps = true;
        d
    }

    fn pixel(image: &egui::ColorImage, b: &Bounds, p: P) -> Color32 {
        let x = ((p.x - b.min.x) / b.width() * image.size[0] as f64) as usize;
        let y = ((p.y - b.min.y) / b.height() * image.size[1] as f64) as usize;
        image.pixels[y * image.size[0] + x]
    }

    #[test]
    fn the_backing_is_the_color_under_what_is_carried_and_only_there() {
        let d = doc();
        let names = zigzag::svg::display_names(&d.shapes);
        let disc = names.iter().position(|n| n == "disc").unwrap();
        let order: Vec<usize> = (0..d.shapes.len()).collect();
        let (image, b) = picture(&d, &[disc], &order, 1.0).expect("a hole to back");
        // The disc's box (20 mm across; design units are tenths of one).
        let at = shape_bounds(&d.shapes[disc]);
        let c = at.center();
        assert!((at.width() - 200.0).abs() < 1.0);
        assert!((b.min.x - at.min.x).abs() < 3.0 && (b.max.x - at.max.x).abs() < 3.0);
        assert!((b.min.y - at.min.y).abs() < 3.0 && (b.max.y - at.max.y).abs() < 3.0);
        // Inside the disc: the square under it, in its thread. (What's
        // sewn over the disc has no hole from it: it isn't painted.)
        assert_eq!(
            pixel(&image, &b, c - P::new(50.0, 0.0)),
            Color32::from_rgb(0xc0, 0, 0)
        );
        assert_eq!(pixel(&image, &b, c), Color32::from_rgb(0xc0, 0, 0));
        // The corners of its box are outside it: nothing.
        assert_eq!(
            pixel(&image, &b, c - P::new(95.0, 95.0)),
            Color32::TRANSPARENT
        );

        // Nothing under what's carried, or overlaps left in: no backing.
        let beside = names.iter().position(|n| n == "beside").unwrap();
        assert!(picture(&d, &[beside], &order, 1.0).is_none());
        let under = names.iter().position(|n| n == "under").unwrap();
        assert!(picture(&d, &[under], &order, 1.0).is_none());
        let mut whole = doc();
        whole.settings.remove_overlaps = false;
        assert!(picture(&whole, &[disc], &order, 1.0).is_none());
        // Sewn first, the disc is under the square instead: no hole of
        // its making.
        let mut first = vec![disc];
        first.extend(order.iter().filter(|&&i| i != disc));
        assert!(picture(&d, &[disc], &first, 1.0).is_none());
    }

    #[test]
    fn a_fill_under_two_others_shows_the_nearer_one() {
        // The disc over a small square over the big one: with the disc
        // gone, it's the small square that shows where they all overlap.
        let svg = SVG.replace(
            r#"<circle id="disc""#,
            r##"<rect id="middle" x="25" y="25" width="10" height="10" fill="#00c0c0"/><circle id="disc""##,
        );
        let mut d = Document::from_svg(&svg, "t", None).unwrap();
        d.settings.remove_overlaps = true;
        let names = zigzag::svg::display_names(&d.shapes);
        let disc = names.iter().position(|n| n == "disc").unwrap();
        let order: Vec<usize> = (0..d.shapes.len()).collect();
        let (image, b) = picture(&d, &[disc], &order, 1.0).unwrap();
        let c = shape_bounds(&d.shapes[disc]).center();
        assert_eq!(pixel(&image, &b, c), Color32::from_rgb(0, 0xc0, 0xc0));
        assert_eq!(
            pixel(&image, &b, c - P::new(0.0, 80.0)),
            Color32::from_rgb(0xc0, 0, 0)
        );
    }

    #[test]
    fn a_drag_is_backed_until_its_stitches_are_planned() {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 900.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.state_mut().set_document(doc());
        h.run_steps(4);
        let disc = h.state().names.iter().position(|n| n == "disc").unwrap();
        h.state_mut().set_selection(&[disc]);
        h.run_steps(2);
        assert!(h.state().backing.is_none());
        let rect = h.state().canvas_rect;
        // (Off the middle of the disc, where the square over it is.)
        let was = shape_bounds(&h.state().doc.shapes[disc]);
        let from = h
            .state()
            .view
            .to_screen(rect, was.center() - P::new(60.0, 0.0));
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        let replans = h.state().replans;
        for k in 1..=4 {
            h.hover_at(from + Vec2::new(k as f32 * 20.0, 0.0));
            h.run_steps(1);
        }
        // Where the disc was, however far it has gone.
        let b = h.state().backing_bounds().expect("backed while it moves");
        assert!((b.min.x - was.min.x).abs() < 3.0 && (b.max.x - was.max.x).abs() < 3.0);
        assert!(shape_bounds(&h.state().doc.shapes[disc]).min.x > was.min.x + 10.0);
        // Dropped, and planned in the same frame: the stitches are whole
        // again, and the backing is gone with the hole.
        h.drop_at(from + Vec2::new(80.0, 0.0));
        h.run_steps(2);
        assert!(h.state().replans > replans);
        assert!(h.state().backing.is_none());

        // A plan that's the worker's: backed until it arrives.
        h.state_mut().planner.slow = true;
        let from = h.state().view.to_screen(
            rect,
            shape_bounds(&h.state().doc.shapes[disc]).center() - P::new(0.0, 60.0),
        );
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        for k in 1..=4 {
            h.hover_at(from + Vec2::new(0.0, k as f32 * 10.0));
            h.run_steps(1);
        }
        assert!(h.state().backing_bounds().is_some());
        h.drop_at(from + Vec2::new(0.0, 40.0));
        h.run_steps(1);
        assert!(
            h.state().backing_bounds().is_some(),
            "still backed while the plan is out"
        );
        for _ in 0..400 {
            h.run_steps(1);
            if !h.state().planner.pending && !h.state().dirty {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        h.run_steps(1);
        assert!(h.state().backing.is_none());
    }
}
