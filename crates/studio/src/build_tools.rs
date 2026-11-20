//! Shapes out of shapes, drawn on the canvas: the Shape Builder's group
//! (the Shape Builder and the Knife) and the Blob Brush. Each is one drag,
//! and what it does to the document is in `zigzag::document` (`knife`,
//! `blob`, `shape_builder`); here are the drags, what they show while
//! they're made, and their options. After VectorCraft's (and
//! Illustrator's):
//!
//! - **Knife (K)**: draw a cut across filled shapes: the selected ones,
//!   or with nothing selected, any it crosses. Alt at the press cuts along
//!   a straight line (Shift: in 45° steps). A cut has to go right across a
//!   shape to part it; its pieces are left selected.
//! - **Blob Brush (Shift+B)**: paint a filled shape in the selection's
//!   thread. Where the paint overlaps fills in that thread, they become
//!   one shape with it. `[` and `]` change the brush's size.
//! - **Shape Builder (Shift+M)**: with overlapping shapes selected, drag
//!   across their parts to make one shape of them, with the look of the
//!   shape in front where the drag began; click a part to make it a shape
//!   of its own. Alt takes parts away instead. Each shape keeps what's
//!   left of itself.

use crate::app::{Studio, Tool};
use crate::cursors::Glyph;
use crate::icons::Icon;
use crate::theme::{pal_ui, space};
use eframe::egui::{self, Color32, Key, Pos2, Rect, Stroke, Vec2};
use zigzag::bezier;
use zigzag::boolean;
use zigzag::document::Face;
use zigzag::geom::P;

const BLUE: Color32 = Color32::from_rgb(40, 140, 255);
const RED: Color32 = Color32::from_rgb(255, 42, 42);

/// The drags in hand, and the Blob Brush's options.
pub struct BuildTools {
    /// The drag so far (design units): the Knife's cut, the Blob Brush's
    /// line, the Shape Builder's way across the parts.
    samples: Vec<P>,
    /// Alt was down at the press: the Knife cuts straight, the Shape
    /// Builder takes away.
    alt: bool,
    /// The Blob Brush's size (mm across), and how much of the hand's
    /// wobble it takes out (0–100).
    pub blob_mm: f64,
    pub blob_smooth: f64,
    /// The Shape Builder's faces, as last found.
    faces: Option<FaceMap>,
}

/// The Shape Builder's faces: the document revision and the selection
/// (each shape's uid and where it is in the sewing order, which changes
/// without a new revision) they were found for, the shapes they're of
/// (indices, back to front), and the faces.
type FaceMap = ((u64, Vec<(usize, usize)>), Vec<usize>, Vec<Face>);

impl Default for BuildTools {
    fn default() -> Self {
        BuildTools {
            samples: Vec::new(),
            alt: false,
            blob_mm: 4.0,
            blob_smooth: 40.0,
            faces: None,
        }
    }
}

impl BuildTools {
    pub fn dragging(&self) -> bool {
        !self.samples.is_empty()
    }

    /// Let go of a drag in hand, with nothing done.
    pub fn cancel(&mut self) {
        self.samples.clear();
    }
}

/// The Blob Brush's size, smallest and largest (mm), and what `[` and `]`
/// change it by.
const BLOB_MM: std::ops::RangeInclusive<f64> = 1.0..=40.0;
const BLOB_STEP: f64 = 0.5;

/// A dashed line that shows on any fabric and any thread.
fn dashed(painter: &egui::Painter, pts: &[Pos2], color: Color32) {
    if pts.len() < 2 {
        return;
    }
    painter.add(egui::Shape::line(
        pts.to_vec(),
        Stroke::new(3.0, Color32::from_white_alpha(170)),
    ));
    painter.extend(egui::Shape::dashed_line(
        pts,
        Stroke::new(1.5, color),
        5.0,
        3.0,
    ));
}

/// Lines 45° across the inside of a shape (its rings on screen, even-odd),
/// `gap` apart: the hatching of a part the Shape Builder is over.
fn hatch(rings: &[Vec<Pos2>], gap: f32) -> Vec<[Pos2; 2]> {
    // In a frame turned 45°: u along the lines, v across them.
    let k = std::f32::consts::FRAC_1_SQRT_2;
    let turn = |p: Pos2| (k * (p.x + p.y), k * (p.y - p.x));
    let back = |u: f32, v: f32| Pos2::new(k * (u - v), k * (u + v));
    let edges: Vec<((f32, f32), (f32, f32))> = rings
        .iter()
        .flat_map(|r| (0..r.len()).map(move |i| (turn(r[i]), turn(r[(i + 1) % r.len()]))))
        .collect();
    let (lo, hi) = edges.iter().fold((f32::MAX, f32::MIN), |(lo, hi), (a, b)| {
        (lo.min(a.1).min(b.1), hi.max(a.1).max(b.1))
    });
    if edges.is_empty() || !(hi - lo).is_finite() {
        return Vec::new();
    }
    // (No more lines than can be told apart: a part filling the window.)
    let gap = gap.max((hi - lo) / 400.0);
    let mut out = Vec::new();
    let mut v = (lo / gap).ceil() * gap + gap * 0.5;
    while v < hi {
        let mut xs: Vec<f32> = edges
            .iter()
            .filter(|(a, b)| (a.1 > v) != (b.1 > v))
            .map(|(a, b)| a.0 + (b.0 - a.0) * (v - a.1) / (b.1 - a.1))
            .collect();
        xs.sort_by(f32::total_cmp);
        for pair in xs.chunks_exact(2) {
            out.push([back(pair[0], v), back(pair[1], v)]);
        }
        v += gap;
    }
    out
}

impl Studio {
    /// The objects one of these tools may change: free, and inside the
    /// group that's been entered.
    fn workable(&self, i: usize) -> bool {
        self.in_scope(i) && self.free(i)
    }

    /// Start, grow and end the drag these tools share. Returns the drag
    /// when it's let go (a click: its one point), having taken it.
    fn build_drag(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: Rect,
        straight: bool,
    ) -> Option<Vec<P>> {
        let px = self.view.scale as f64 / 10.0;
        let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(at) = ui.input(|i| i.pointer.press_origin())
        {
            self.view_tools.build.samples = vec![self.view.to_design(rect, at)];
            self.view_tools.build.alt = alt;
        }
        let held = resp.dragged_by(egui::PointerButton::Primary);
        let b = &mut self.view_tools.build;
        if held
            && !b.samples.is_empty()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let q = self.view.to_design(rect, pos);
            if straight && b.alt {
                // From the press straight to the pointer.
                let from = b.samples[0];
                b.samples.truncate(1);
                b.samples.push(if shift {
                    bezier::constrain_45(from, q)
                } else {
                    q
                });
            } else if b.samples.last().is_none_or(|l| l.dist(q) * px >= 1.0) {
                b.samples.push(q);
            }
            ui.ctx().request_repaint();
        }
        if resp.drag_stopped() {
            let drawn = std::mem::take(&mut b.samples);
            return (!drawn.is_empty()).then_some(drawn);
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            b.samples.clear();
            b.alt = alt;
            return Some(vec![self.view.to_design(rect, pos)]);
        }
        if !held {
            // (Let go off the canvas, or the tool left mid-drag.)
            b.samples.clear();
        }
        None
    }

    /// Escape calls off a drag of one of these tools; `[` and `]` size
    /// the Blob Brush.
    pub fn build_keys(&mut self, ctx: &egui::Context) -> bool {
        if !matches!(self.tool, Tool::Knife | Tool::Blob | Tool::ShapeBuilder)
            || ctx.egui_wants_keyboard_input()
        {
            return false;
        }
        if self.view_tools.build.dragging()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
        {
            self.view_tools.build.samples.clear();
            return true;
        }
        if self.tool == Tool::Blob {
            let step = ctx.input_mut(|i| {
                let down = i.consume_key(egui::Modifiers::NONE, Key::OpenBracket);
                let up = i.consume_key(egui::Modifiers::NONE, Key::CloseBracket);
                f64::from(i8::from(up) - i8::from(down))
            });
            if step != 0.0 {
                let b = &mut self.view_tools.build;
                b.blob_mm = (b.blob_mm + step * BLOB_STEP).clamp(*BLOB_MM.start(), *BLOB_MM.end());
                return true;
            }
        }
        false
    }

    // ---- The Knife.

    /// Canvas input and drawing for the Knife.
    pub fn knife_canvas(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        self.want_cursor(Glyph::Cross(Some(Icon::Knife)));
        let px = self.view.scale as f64 / 10.0;
        let done = self.build_drag(ui, resp, rect, true);
        let line: Vec<Pos2> = self
            .view_tools
            .build
            .samples
            .iter()
            .map(|&p| self.view.to_screen(rect, p))
            .collect();
        dashed(painter, &line, BLUE);
        if let Some(cut) = done {
            let length: f64 = cut.windows(2).map(|w| w[0].dist(w[1])).sum();
            if cut.len() >= 2 && length * px >= 2.0 {
                self.finish_knife(&cut);
            }
        }
        self.hud(
            painter,
            rect,
            if self.sel_indices().is_empty() {
                "Draw a cut right across a filled shape · Alt for a straight cut (Shift: 45° steps)"
            } else {
                "Draw a cut right across the selected shapes · Alt for a straight cut (Shift: 45° steps)"
            },
        );
    }

    /// Cut along the line drawn: the selected shapes, or with nothing
    /// selected, any it crosses.
    fn finish_knife(&mut self, cut: &[P]) {
        let sel = self.sel_indices();
        let targets: Vec<usize> = if sel.is_empty() {
            (0..self.doc.shapes.len())
                .filter(|&i| self.workable(i))
                .collect()
        } else {
            sel.into_iter().filter(|&i| self.free(i)).collect()
        };
        let mut pieces = Vec::new();
        match self.structural(|d| {
            pieces = d.knife(&targets, cut)?;
            Ok(())
        }) {
            Ok(()) => {
                self.set_selection(&pieces);
                self.status = format!("Cut into {} pieces.", pieces.len());
            }
            Err(e) => self.status = format!("Nothing cut: {e:#}."),
        }
    }

    // ---- The Blob Brush.

    /// Canvas input and drawing for the Blob Brush.
    pub fn blob_canvas(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        let px = self.view.scale as f64 / 10.0;
        let size = self.view_tools.build.blob_mm * 10.0; // design units
        let radius = (size * px / 2.0) as f32;
        self.want_cursor(Glyph::Brush(radius.max(1.5)));
        let color = self.new_shape_color();
        let ink = Color32::from_rgb(color[0], color[1], color[2]);
        let done = self.build_drag(ui, resp, rect, false);
        // The paint so far, as wide as the brush.
        let line: Vec<Pos2> = self
            .view_tools
            .build
            .samples
            .iter()
            .map(|&p| self.view.to_screen(rect, p))
            .collect();
        match line.as_slice() {
            [] => {}
            [dab] => {
                painter.circle_filled(*dab, radius, ink.gamma_multiply(0.55));
            }
            _ => {
                painter.add(egui::Shape::line(
                    line.clone(),
                    Stroke::new((radius * 2.0).max(1.5), ink.gamma_multiply(0.55)),
                ));
                for end in [line[0], line[line.len() - 1]] {
                    painter.circle_filled(end, radius, ink.gamma_multiply(0.55));
                }
            }
        }
        if let Some(drawn) = done {
            self.finish_blob(&drawn, px);
        }
        self.hud(
            painter,
            rect,
            "Paint a filled shape: it joins the fills in the same thread that it touches · [ and ] size the brush",
        );
        self.blob_options(ui.ctx(), ui.make_persistent_id("blob-options"), rect);
    }

    /// The line painted, smoothed as the Brush's is, as a line of points.
    fn blob_line(&self, samples: &[P], px: f64) -> Vec<P> {
        if samples.len() < 3 {
            return samples.to_vec();
        }
        let s = self.view_tools.build.blob_smooth.clamp(0.0, 100.0) / 100.0;
        let (tol, passes) = ((0.6 + 7.4 * s) / px, (s * 4.0).round() as usize);
        let Some(sp) = bezier::fit_stroke(samples, tol, passes, false) else {
            return samples.to_vec();
        };
        let mut line: Vec<P> = Vec::new();
        for (a, b) in sp.segments() {
            let c = sp.cubic(a, b);
            let n = if c[1] == c[0] && c[2] == c[3] { 1 } else { 24 };
            let skip = usize::from(!line.is_empty());
            line.extend(bezier::sample(&c, n).into_iter().skip(skip));
        }
        if line.len() < 2 {
            samples.to_vec()
        } else {
            line
        }
    }

    /// Add the paint: a new fill, or more of the fills it touches.
    fn finish_blob(&mut self, samples: &[P], px: f64) {
        let size = self.view_tools.build.blob_mm * 10.0;
        let region = boolean::brush_area(&self.blob_line(samples, px), size);
        let among: Vec<usize> = (0..self.doc.shapes.len())
            .filter(|&i| self.workable(i))
            .collect();
        let color = self.new_shape_color();
        let (mut idx, mut joined) = (0, false);
        match self.structural(|d| {
            (idx, joined) = d.blob(&region, color, &among)?;
            Ok(())
        }) {
            Ok(()) => {
                self.set_selection(&[idx]);
                self.status = if joined {
                    "Painted onto the fill it touched.".into()
                } else {
                    "Painted a new fill.".into()
                };
            }
            Err(e) => self.status = format!("Couldn't paint that: {e:#}"),
        }
    }

    /// The bar at the top of the canvas: the brush's size, and how smooth.
    fn blob_options(&mut self, ctx: &egui::Context, id: egui::Id, rect: Rect) {
        egui::Area::new(id)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(rect.center_top() + Vec2::new(0.0, space::M))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .corner_radius(9)
                    .inner_margin(egui::Margin::symmetric(space::S as i8, space::XS as i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = space::S;
                            let p = pal_ui(ui);
                            let label = |ui: &mut egui::Ui, t: &str| {
                                ui.label(egui::RichText::new(t).size(12.0).color(p.secondary));
                            };
                            let b = &mut self.view_tools.build;
                            label(ui, "Size");
                            ui.add(
                                egui::DragValue::new(&mut b.blob_mm)
                                    .range(BLOB_MM)
                                    .speed(0.05)
                                    .fixed_decimals(1)
                                    .suffix(" mm"),
                            )
                            .on_hover_text("How wide the brush paints. [ and ] change it too.");
                            label(ui, "Smoothness");
                            ui.add(
                                egui::Slider::new(&mut b.blob_smooth, 0.0..=100.0)
                                    .show_value(false),
                            )
                            .on_hover_text("How much of the hand's wobble is taken out");
                        });
                    });
            });
    }

    // ---- The Shape Builder.

    /// The shapes the Shape Builder works on (the selected filled shapes
    /// that can be changed), and the faces they divide each other into:
    /// found once for a document and a selection.
    fn builder_faces(&mut self) -> (Vec<usize>, Vec<Face>) {
        let sel: Vec<usize> = self
            .sel_indices()
            .into_iter()
            .filter(|&i| self.free(i) && self.doc.builds(i))
            .collect();
        let key = (
            self.doc.rev(),
            sel.iter()
                .map(|&i| (self.doc.shapes[i].uid, i))
                .collect::<Vec<_>>(),
        );
        if let Some((k, inputs, faces)) = &self.view_tools.build.faces
            && *k == key
        {
            return (inputs.clone(), faces.clone());
        }
        let (inputs, faces) = self.doc.builder_faces(&sel);
        self.view_tools.build.faces = Some((key, inputs.clone(), faces.clone()));
        (inputs, faces)
    }

    /// Canvas input and drawing for the Shape Builder.
    pub fn builder_canvas(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        self.want_cursor(Glyph::Cross(Some(Icon::ShapeBuilder)));
        let (inputs, faces) = self.builder_faces();
        if inputs.is_empty() {
            self.view_tools.build.samples.clear();
            self.hud(
                painter,
                rect,
                "Select overlapping shapes first (V), then drag across their parts here",
            );
            return;
        }
        let done = self.build_drag(ui, resp, rect, false);
        let b = &self.view_tools.build;
        let dragging = b.dragging();
        // Taking away: Alt as it was at the press, or as it is now.
        let erase = if dragging {
            b.alt
        } else {
            ui.input(|i| i.modifiers.alt)
        };
        let color = if erase { RED } else { BLUE };
        // The faces the drag has reached; or with nothing held, the one
        // under the pointer.
        let mut over: Vec<usize> = Vec::new();
        if dragging {
            for &p in &b.samples {
                if let Some(f) = faces.iter().position(|f| f.contains(p))
                    && !over.contains(&f)
                {
                    over.push(f);
                }
            }
        } else if let Some(at) = resp.hover_pos() {
            let p = self.view.to_design(rect, at);
            over.extend(faces.iter().position(|f| f.contains(p)));
        }
        for &f in &over {
            let rings: Vec<Vec<Pos2>> = faces[f]
                .rings
                .iter()
                .map(|r| r.iter().map(|&p| self.view.to_screen(rect, p)).collect())
                .collect();
            for seg in hatch(&rings, 5.0) {
                painter.line_segment(seg, Stroke::new(1.0, color));
            }
            for r in rings {
                painter.add(egui::Shape::closed_line(r, Stroke::new(1.5, color)));
            }
        }
        if b.samples.len() > 1 {
            let trail: Vec<Pos2> = b
                .samples
                .iter()
                .map(|&p| self.view.to_screen(rect, p))
                .collect();
            painter.add(egui::Shape::line(trail, Stroke::new(1.0, color)));
        }
        if let Some(along) = done {
            let erase = self.view_tools.build.alt;
            self.finish_builder(&inputs, &along, erase);
        }
        self.hud(
            painter,
            rect,
            "Drag across parts to make one shape of them · click a part to make it a shape of its own · Alt takes parts away",
        );
    }

    /// Join (or take away) the parts the drag went across.
    fn finish_builder(&mut self, inputs: &[usize], along: &[P], erase: bool) {
        // What else was selected (a line, say) stays selected.
        let others: Vec<usize> = self
            .sel_indices()
            .into_iter()
            .filter(|i| !inputs.contains(i))
            .map(|i| self.doc.shapes[i].uid)
            .collect();
        let mut out = Vec::new();
        // (A drag that crossed no part of them is no mistake: nothing.)
        if self
            .structural(|d| {
                out = d.shape_builder(inputs, along, erase)?;
                Ok(())
            })
            .is_ok()
        {
            out.extend(self.uid_indices(&others));
            self.set_selection(&out);
            self.status = if erase {
                "Took those parts away.".into()
            } else {
                "Made one shape of those parts.".into()
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::shape_bounds;
    use egui_kittest::Harness;
    use zigzag::params::Kind;
    use zigzag::svg::Geometry;

    /// A studio with filled squares, each `(x, y, side)` in design units
    /// (the first in red, the rest in blue), all showing.
    fn studio(squares: &[(f64, f64, f64)]) -> Harness<'static, Studio> {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        for (k, &(x, y, w)) in squares.iter().enumerate() {
            let pts = [
                P::new(x, y),
                P::new(x + w, y),
                P::new(x + w, y + w),
                P::new(x, y + w),
            ];
            let color = if k == 0 { [200, 0, 0] } else { [0, 0, 200] };
            h.state_mut()
                .structural(|d| d.add_drawing(&pts, true, color, Kind::Fill).map(|_| ()))
                .unwrap();
        }
        h.state_mut().set_selection(&[]);
        h.state_mut().request_fit();
        h.run_steps(4);
        // (Room round them on the canvas, to start a drag from.)
        h.state_mut().view.scale *= 0.5;
        h.run_steps(1);
        h
    }

    fn to(h: &Harness<'_, Studio>, p: P) -> Pos2 {
        let s = h.state();
        s.view.to_screen(s.canvas_rect, p)
    }

    fn drag(h: &mut Harness<'_, Studio>, path: &[P]) {
        let at = to(h, path[0]);
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        for w in path.windows(2) {
            for k in 1..=6 {
                let at = to(h, w[0].lerp(w[1], k as f64 / 6.0));
                h.hover_at(at);
                h.run_steps(1);
            }
        }
        let end = to(h, path[path.len() - 1]);
        h.drop_at(end);
        h.run_steps(3);
    }

    fn click(h: &mut Harness<'_, Studio>, p: P) {
        let at = to(h, p);
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(3);
    }

    fn with_alt(h: &mut Harness<'_, Studio>, f: impl FnOnce(&mut Harness<'_, Studio>)) {
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::ALT));
        h.run_steps(1);
        f(h);
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        h.run_steps(1);
    }

    /// Object `i`'s area, mm².
    fn mm2(h: &Harness<'_, Studio>, i: usize) -> f64 {
        let b = shape_bounds(&h.state().doc.shapes[i]);
        // (These are boxes, or near enough to tell them apart by.)
        b.width() * b.height() / 100.0
    }

    #[test]
    fn hatching_stays_inside_the_part_and_its_holes_stay_clear() {
        let sq = |x: f32, y: f32, w: f32| {
            vec![
                Pos2::new(x, y),
                Pos2::new(x + w, y),
                Pos2::new(x + w, y + w),
                Pos2::new(x, y + w),
            ]
        };
        let rings = vec![sq(0.0, 0.0, 100.0), sq(40.0, 40.0, 20.0)];
        let lines = hatch(&rings, 5.0);
        assert!(lines.len() > 20, "{}", lines.len());
        for [a, b] in &lines {
            for p in [a, b] {
                assert!(p.x > -0.01 && p.x < 100.01 && p.y > -0.01 && p.y < 100.01);
            }
            // Its middle is in the square and out of the hole.
            let m = a.lerp(*b, 0.5);
            assert!(
                !(m.x > 41.0 && m.x < 59.0 && m.y > 41.0 && m.y < 59.0),
                "{m:?}"
            );
        }
        assert!(hatch(&[], 5.0).is_empty());
        // However big the part: no more lines than can be told apart.
        assert!(hatch(&[sq(0.0, 0.0, 100_000.0)], 5.0).len() <= 800);
    }

    #[test]
    fn k_and_a_drag_across_cut_a_shape_in_two() {
        let mut h = studio(&[(-200.0, -200.0, 400.0)]);
        h.key_press(Key::K);
        h.run_steps(1);
        assert_eq!(h.state().tool, Tool::Knife);
        assert_eq!(h.state().view_tools.build_slot, Tool::Knife);
        let undo = h.state().undo.len();
        // Into the shape and out again on the same side: nothing parts.
        drag(&mut h, &[P::new(-260.0, -80.0), P::new(0.0, -80.0)]);
        assert_eq!(h.state().doc.shapes.len(), 1);
        assert_eq!(h.state().undo.len(), undo, "no step for no cut");
        assert!(
            h.state().status.starts_with("Nothing cut"),
            "{}",
            h.state().status
        );
        // Right across, a quarter of the way down: two pieces, selected.
        drag(&mut h, &[P::new(-260.0, -100.0), P::new(260.0, -100.0)]);
        let s = h.state();
        assert_eq!(s.doc.shapes.len(), 2);
        assert_eq!(s.undo.len(), undo + 1, "one step");
        assert_eq!(s.sel_indices().len(), 2);
        assert!(s.view_tools.build.samples.is_empty());
        let mut areas = [mm2(&h, 0), mm2(&h, 1)];
        areas.sort_by(f64::total_cmp);
        assert!(
            (areas[0] - 400.0).abs() < 12.0 && (areas[1] - 1200.0).abs() < 12.0,
            "{areas:?}"
        );
        for sh in &h.state().doc.shapes {
            assert_eq!(sh.color, [200, 0, 0]);
            assert!(matches!(sh.geometry, Geometry::Fill { .. }));
        }
        assert!(h.state().design.stitch_count() > 100, "they sew");
        // With one piece selected, a cut across both parts only it.
        let small = if mm2(&h, 0) < mm2(&h, 1) { 0 } else { 1 };
        h.state_mut().set_selection(&[small]);
        h.run_steps(1);
        // Alt: straight from the press to where it's let go, however the
        // hand wanders on the way.
        with_alt(&mut h, |h| {
            drag(
                h,
                &[P::new(0.0, -260.0), P::new(120.0, 0.0), P::new(0.0, 260.0)],
            )
        });
        assert_eq!(
            h.state().doc.shapes.len(),
            3,
            "the small piece in two, the other whole"
        );
        // Escape part way calls a cut off.
        h.state_mut().set_selection(&[]);
        h.run_steps(1);
        let (a, b) = (to(&h, P::new(-260.0, 100.0)), to(&h, P::new(0.0, 100.0)));
        h.hover_at(a);
        h.run_steps(1);
        h.drag_at(a);
        h.run_steps(1);
        h.hover_at(b);
        h.run_steps(1);
        assert!(h.state().view_tools.build.dragging());
        h.key_press(Key::Escape);
        h.run_steps(1);
        assert!(!h.state().view_tools.build.dragging());
        assert_eq!(
            h.state().tool,
            Tool::Knife,
            "Escape took the cut, not the tool"
        );
        h.drop_at(to(&h, P::new(260.0, 100.0)));
        h.run_steps(2);
        assert_eq!(h.state().doc.shapes.len(), 3);
    }

    #[test]
    fn shift_b_paints_a_fill_that_joins_what_it_touches() {
        let mut h = studio(&[(-300.0, -100.0, 200.0)]);
        // The square's thread for the paint.
        h.state_mut().set_selection(&[0]);
        h.key_press_modifiers(egui::Modifiers::SHIFT, Key::B);
        h.run_steps(1);
        assert_eq!(h.state().tool, Tool::Blob);
        assert_eq!(h.state().view_tools.brush_slot, Tool::Blob);
        // ] and [ size the brush.
        let size = h.state().view_tools.build.blob_mm;
        h.key_press(Key::CloseBracket);
        h.key_press(Key::CloseBracket);
        h.run_steps(1);
        assert_eq!(h.state().view_tools.build.blob_mm, size + 2.0 * BLOB_STEP);
        h.key_press(Key::OpenBracket);
        h.key_press(Key::OpenBracket);
        h.run_steps(1);
        assert_eq!(h.state().view_tools.build.blob_mm, size);
        // From inside the square out to the right: the square grows.
        let uid = h.state().doc.shapes[0].uid;
        let wide = shape_bounds(&h.state().doc.shapes[0]).width();
        drag(&mut h, &[P::new(-200.0, 0.0), P::new(150.0, 0.0)]);
        let s = h.state();
        assert_eq!(s.doc.shapes.len(), 1, "joined: nothing new");
        assert_eq!(s.doc.shapes[0].uid, uid);
        assert!(shape_bounds(&s.doc.shapes[0]).width() > wide + 200.0);
        assert_eq!(s.sel_indices(), vec![0]);
        assert_eq!(s.undo.len(), 2, "the square, and the paint");
        // Clear of it: a fill of its own, in the same thread (the
        // selection's). A click paints a dab.
        click(&mut h, P::new(-200.0, 220.0));
        let s = h.state();
        assert_eq!(s.doc.shapes.len(), 2);
        let dab = s.sel_indices()[0];
        assert_eq!(s.doc.shapes[dab].color, [200, 0, 0]);
        assert_eq!(s.doc.shapes[dab].params.kind, Some(Kind::Fill));
        let b = shape_bounds(&s.doc.shapes[dab]);
        let d = s.view_tools.build.blob_mm * 10.0;
        assert!(
            (b.width() - d).abs() < 3.0 && (b.height() - d).abs() < 3.0,
            "{b:?}"
        );
    }

    #[test]
    fn shift_m_joins_the_parts_dragged_across() {
        // Two squares, the second half across the first and in front.
        let two = || {
            let mut h = studio(&[(-300.0, -200.0, 400.0), (-100.0, -200.0, 400.0)]);
            h.key_press_modifiers(egui::Modifiers::SHIFT, Key::M);
            h.run_steps(1);
            assert_eq!(h.state().tool, Tool::ShapeBuilder);
            h
        };
        // Nothing selected: nothing to build from.
        let mut h = two();
        drag(&mut h, &[P::new(-200.0, 0.0), P::new(0.0, 0.0)]);
        assert_eq!(h.state().doc.shapes.len(), 2);
        assert_eq!(h.state().undo.len(), 2);
        // Both selected, a drag from the first's own part into the
        // overlap: that's one shape, in the first's thread; the second
        // keeps the rest of itself.
        h.state_mut().set_selection(&[0, 1]);
        h.run_steps(2);
        drag(&mut h, &[P::new(-200.0, 0.0), P::new(0.0, 0.0)]);
        let s = h.state();
        assert_eq!(s.doc.shapes.len(), 2);
        assert_eq!(s.undo.len(), 3, "one step");
        assert_eq!(s.sel_indices().len(), 2, "both still selected, to go on");
        let red = s
            .doc
            .shapes
            .iter()
            .position(|x| x.color == [200, 0, 0])
            .unwrap();
        let blue = 1 - red;
        assert!((mm2(&h, red) - 1600.0).abs() < 30.0, "{}", mm2(&h, red));
        assert!((mm2(&h, blue) - 800.0).abs() < 30.0, "{}", mm2(&h, blue));
        // Alt-click what's left of the second: it's taken away.
        with_alt(&mut h, |h| click(h, P::new(200.0, 0.0)));
        assert_eq!(h.state().doc.shapes.len(), 1);
        assert_eq!(h.state().doc.shapes[0].color, [200, 0, 0]);
        // A click on the overlap makes it a shape of its own, in the
        // thread of the one in front.
        let mut h = two();
        h.state_mut().set_selection(&[0, 1]);
        h.run_steps(2);
        click(&mut h, P::new(0.0, 0.0));
        let s = h.state();
        assert_eq!(s.doc.shapes.len(), 3);
        assert_eq!(
            s.doc
                .shapes
                .iter()
                .filter(|x| x.color == [0, 0, 200])
                .count(),
            2
        );
        assert_eq!(s.sel_indices().len(), 3);
        // A drag across nothing of theirs: nothing, and no step.
        let undo = h.state().undo.len();
        drag(&mut h, &[P::new(-200.0, 400.0), P::new(0.0, 400.0)]);
        assert_eq!(h.state().undo.len(), undo);
        assert_eq!(h.state().doc.shapes.len(), 3);
    }

    #[test]
    fn the_shape_builder_works_on_the_selection_however_the_order_changes() {
        // Two overlapping squares, selected, and a third over both that
        // isn't; then the third is moved to be sewn first.
        let mut h = studio(&[
            (-300.0, -200.0, 400.0),
            (-100.0, -200.0, 400.0),
            (-350.0, -250.0, 700.0),
        ]);
        h.state_mut().doc.shapes[2].color = [0, 200, 0];
        h.state_mut().set_selection(&[0, 1]);
        h.state_mut().set_tool(Tool::ShapeBuilder);
        h.run_steps(2);
        assert!(h.state_mut().move_layers(&[2], 0));
        h.run_steps(2);
        let green = |h: &Harness<'_, Studio>| -> (usize, f64) {
            let s = h.state();
            let i = s
                .doc
                .shapes
                .iter()
                .position(|x| x.color == [0, 200, 0])
                .unwrap();
            (i, mm2(h, i))
        };
        let (at, was) = green(&h);
        assert_eq!(at, 0);
        click(&mut h, P::new(0.0, 0.0));
        // The overlap of the two is a shape of its own; the third is as
        // it was, and not selected.
        let s = h.state();
        assert_eq!(s.doc.shapes.len(), 4);
        let (at, now) = green(&h);
        assert_eq!(at, 0);
        assert!((now - was).abs() < 1.0, "{now} for {was}");
        assert!(!s.sel_indices().contains(&at));
        assert_eq!(s.sel_indices().len(), 3);
    }

    #[test]
    fn space_and_a_drag_pans_and_nothing_is_cut_painted_or_built() {
        let mut h = studio(&[(-300.0, -200.0, 400.0), (-100.0, -200.0, 400.0)]);
        h.state_mut().set_selection(&[0, 1]);
        h.run_steps(2);
        for tool in [Tool::Knife, Tool::Blob, Tool::ShapeBuilder] {
            h.state_mut().set_tool(tool);
            h.run_steps(1);
            let undo = h.state().undo.len();
            h.key_down(Key::Space);
            h.run_steps(1);
            drag(&mut h, &[P::new(-200.0, 0.0), P::new(0.0, 0.0)]);
            h.key_up(Key::Space);
            h.run_steps(2);
            let s = h.state();
            assert_eq!(s.undo.len(), undo, "{tool:?}");
            assert_eq!(s.doc.shapes.len(), 2, "{tool:?}");
            assert_eq!(s.sel_indices().len(), 2, "{tool:?}");
        }
        // A click on a shape that stands clear of the other is no step.
        let mut h = studio(&[(-300.0, -200.0, 100.0), (100.0, -200.0, 100.0)]);
        h.state_mut().set_selection(&[0, 1]);
        h.state_mut().set_tool(Tool::ShapeBuilder);
        h.run_steps(2);
        let undo = h.state().undo.len();
        click(&mut h, P::new(-250.0, -150.0));
        assert_eq!(h.state().undo.len(), undo);
        assert_eq!(h.state().doc.shapes.len(), 2);
    }
}
