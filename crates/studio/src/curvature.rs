//! The Curvature tool (Shift+`): click points and the curve runs smoothly
//! through them. There are no handles to pull, and nothing to hold down:
//! each stretch of three points follows the circle through them
//! (`bezier::curvature_spline`). After VectorCraft's (and Illustrator's):
//!
//! - A click adds a smooth point; Alt-click adds a corner. Double-click a
//!   point, or Alt-click it, to turn it between the two.
//! - Drag a point and the whole curve follows.
//! - Click the first point (of three or more) to close the shape: that's
//!   a fill. Enter or Escape ends the line where it is: that's a run.
//!   Taking another tool ends it too.
//! - Backspace takes out the point last touched.
//!
//! Like the Pen's, the path is the tool's until it's ended, and then one
//! object and one undo step. (VectorCraft's also edits paths drawn
//! earlier by these rules; here Direct Selection and the Anchor Point
//! tools do that.)

use crate::app::{Studio, Tool};
use crate::cursors::Glyph;
use eframe::egui::{self, Color32, Key, Pos2, Rect, Stroke};
use zigzag::bezier::{self, SubPath};
use zigzag::geom::P;
use zigzag::params::Kind;

const BLUE: Color32 = Color32::from_rgb(40, 140, 255);
/// How near a press has to be to a point to be on it, and how far a press
/// on one has to move to drag it (screen points).
const REACH: f32 = 5.0;
const DRAG: f32 = 3.0;

/// The path being drawn.
#[derive(Default)]
pub struct CurveDraw {
    /// Its points (design units), and whether each is a corner.
    pub pts: Vec<(P, bool)>,
    /// The point last touched: Backspace takes it out.
    current: Option<usize>,
    /// A point pressed on: which, where the press was, and whether it has
    /// moved far enough to be dragging it.
    grab: Option<(usize, Pos2, bool)>,
    /// Where a shape was closed, and until when a press there is the
    /// rest of the double-click that closed it: not the first point of
    /// another.
    closed_at: Option<(Pos2, f64)>,
}

/// A subpath as a line on screen.
fn trace(sp: &SubPath, to: impl Fn(P) -> Pos2) -> Vec<Pos2> {
    let mut pts: Vec<Pos2> = Vec::new();
    for (a, b) in sp.segments() {
        let c = sp.cubic(a, b);
        let n = if c[1] == c[0] && c[2] == c[3] { 1 } else { 20 };
        let skip = usize::from(!pts.is_empty());
        pts.extend(bezier::sample(&c, n).into_iter().skip(skip).map(&to));
    }
    pts
}

impl Studio {
    /// Canvas input and drawing for the Curvature tool.
    pub fn curvature_canvas(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        self.snap_targets(None, false);
        let (pressed, down, alt, press_pos, pos, now) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.modifiers.alt,
                i.pointer.press_origin(),
                i.pointer.interact_pos(),
                i.time,
            )
        });
        let view = self.view;
        let to = |p: P| view.to_screen(rect, p);
        // The drawn point under a place on screen (the first, in the order
        // they were drawn).
        let on = |pts: &[(P, bool)], at: Pos2| {
            pts.iter()
                .position(|&(p, _)| (to(p) - at).length() <= REACH)
        };
        let hover = pos.filter(|_| resp.hovered());
        let over = hover.and_then(|at| on(&self.view_tools.curve.pts, at));
        let closable = |pts: &[(P, bool)]| pts.len() >= 3;

        // ---- Press. (Not while Space pans the view.)
        let panning = !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_down(Key::Space));
        if pressed
            && resp.hovered()
            && !panning
            && let Some(at) = press_pos.or(pos)
            && !self
                .view_tools
                .curve
                .closed_at
                .is_some_and(|(p, until)| now < until && (p - at).length() <= 2.0 * REACH)
        {
            match on(&self.view_tools.curve.pts, at) {
                Some(i) => {
                    let d = &mut self.view_tools.curve;
                    d.current = Some(i);
                    let close = !alt && i == 0 && closable(&d.pts);
                    if alt {
                        d.pts[i].1 = !d.pts[i].1;
                    } else if close {
                        d.closed_at = Some((at, now + 0.4));
                    } else {
                        d.grab = Some((i, at, false));
                    }
                    if close {
                        self.finish_curvature(true);
                    }
                }
                None => {
                    let raw = self.view.to_design(rect, at);
                    let own: Vec<P> = self.view_tools.curve.pts.iter().map(|p| p.0).collect();
                    let q = self.snap_point(raw, own.last().copied(), false, &own);
                    let d = &mut self.view_tools.curve;
                    d.pts.push((q, alt));
                    d.current = Some(d.pts.len() - 1);
                }
            }
        }
        // ---- A point being dragged: the whole curve follows.
        if down
            && let Some((i, from, moved)) = self.view_tools.curve.grab
            && let Some(at) = pos
            && (moved || (at - from).length() >= DRAG)
        {
            let others: Vec<P> = self
                .view_tools
                .curve
                .pts
                .iter()
                .enumerate()
                .filter(|(k, _)| *k != i)
                .map(|(_, p)| p.0)
                .collect();
            let q = self.snap_point(self.view.to_design(rect, at), None, false, &others);
            let d = &mut self.view_tools.curve;
            d.grab = Some((i, from, true));
            if let Some(p) = d.pts.get_mut(i) {
                p.0 = q;
            }
            ui.ctx().request_repaint();
        }
        if !down {
            self.view_tools.curve.grab = None;
        }
        // ---- Double-click a point: a corner, or smooth again.
        if resp.double_clicked()
            && !alt
            && let Some(i) = pos.and_then(|at| on(&self.view_tools.curve.pts, at))
        {
            let d = &mut self.view_tools.curve;
            d.pts[i].1 = !d.pts[i].1;
            d.current = Some(i);
        }

        // ---- The curve so far; while nothing's held, on to the pointer
        // (as the next click would make it).
        let d = &self.view_tools.curve;
        let dragging = d.grab.is_some_and(|g| g.2);
        let mut shown = d.pts.clone();
        if !shown.is_empty()
            && !down
            && over.is_none()
            && let Some(at) = hover
        {
            shown.push((self.view.to_design(rect, at), false));
        }
        if shown.len() >= 2 {
            let line = trace(&bezier::curvature_spline(&shown, false), to);
            painter.add(egui::Shape::line(line, Stroke::new(1.0, BLUE)));
        }
        for (k, &(p, corner)) in d.pts.iter().enumerate() {
            let c = to(p);
            let fill = if d.current == Some(k) {
                BLUE
            } else {
                Color32::WHITE
            };
            // Round for a smooth point, square for a corner.
            if corner {
                let r = Rect::from_center_size(c, egui::Vec2::splat(6.0));
                painter.rect_filled(r, 0.0, fill);
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, BLUE), egui::StrokeKind::Inside);
            } else {
                painter.circle_filled(c, 3.0, fill);
                painter.circle_stroke(c, 3.0, Stroke::new(1.0, BLUE));
            }
        }
        self.want_cursor(Glyph::Pen(match over {
            Some(_) if alt => "^",
            Some(0) if closable(&d.pts) && !dragging => "o",
            _ => "~",
        }));
        self.hud(
            painter,
            rect,
            if d.pts.is_empty() {
                "Click points: the curve runs through them · Alt-click for a corner"
            } else {
                "Click for the next point (Alt-click: a corner) · drag a point to move it · double-click one to turn it between smooth and corner · click the first to close · Enter ends the line"
            },
        );
    }

    /// Enter and Escape end the line being drawn, keeping it; Backspace
    /// takes out the point last touched.
    pub fn curvature_keys(&mut self, ctx: &egui::Context) -> bool {
        if self.tool != Tool::Curvature
            || ctx.egui_wants_keyboard_input()
            || self.view_tools.curve.pts.is_empty()
        {
            return false;
        }
        let (end, back) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::Enter)
                    || i.consume_key(egui::Modifiers::NONE, Key::Escape),
                i.consume_key(egui::Modifiers::NONE, Key::Backspace)
                    || i.consume_key(egui::Modifiers::NONE, Key::Delete),
            )
        });
        if end {
            self.finish_curvature(false);
        }
        if back {
            let d = &mut self.view_tools.curve;
            let k = d
                .current
                .filter(|&k| k < d.pts.len())
                .unwrap_or(d.pts.len() - 1);
            d.pts.remove(k);
            d.current = None;
            d.grab = None;
        }
        end || back
    }

    /// End the path being drawn: a new object (a fill if it's closed, a
    /// run if not), or nothing if there's too little of it.
    pub fn finish_curvature(&mut self, closed: bool) {
        let d = &mut self.view_tools.curve;
        let pts = std::mem::take(&mut d.pts);
        d.current = None;
        d.grab = None;
        if pts.len() < if closed { 3 } else { 2 } {
            return;
        }
        let n = pts.len();
        let path = vec![bezier::curvature_spline(&pts, closed)];
        let kind = if closed { Kind::Fill } else { Kind::Run };
        let color = self.new_shape_color();
        let mut idx = 0;
        match self.structural(|d| {
            idx = d.add_drawing_path(&path, color, kind)?;
            Ok(())
        }) {
            Ok(()) => {
                self.set_selection(&[idx]);
                self.status = format!(
                    "Drew a {} through {n} points.",
                    if closed { "shape" } else { "line" }
                );
            }
            Err(e) => self.status = format!("Couldn't add the drawing: {e:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Vec2;
    use egui_kittest::Harness;
    use zigzag::svg::Geometry;

    fn studio() -> Harness<'static, Studio> {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            // (Frames as they come, so two clicks can be a double-click.)
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::SHIFT, Key::Backtick);
        h.run_steps(1);
        assert_eq!(h.state().tool, Tool::Curvature);
        assert_eq!(h.state().view_tools.pen_slot, Tool::Curvature);
        h
    }

    fn click(h: &mut Harness<'_, Studio>, at: Pos2) {
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(2);
    }

    fn with_alt(h: &mut Harness<'_, Studio>, f: impl FnOnce(&mut Harness<'_, Studio>)) {
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::ALT));
        h.run_steps(1);
        f(h);
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        h.run_steps(1);
    }

    /// Three places on the canvas: left, up in the middle, right.
    fn three(h: &Harness<'_, Studio>) -> [Pos2; 3] {
        let c = h.state().canvas_rect.center();
        [
            c + Vec2::new(-150.0, 60.0),
            c + Vec2::new(0.0, -80.0),
            c + Vec2::new(150.0, 60.0),
        ]
    }

    #[test]
    fn clicks_draw_a_curve_through_them_and_enter_ends_the_line() {
        let mut h = studio();
        let at = three(&h);
        for p in at {
            click(&mut h, p);
        }
        assert_eq!(h.state().view_tools.curve.pts.len(), 3);
        assert!(
            h.state().doc.shapes.is_empty(),
            "the tool's, until it's ended"
        );
        h.key_press(Key::Enter);
        h.run_steps(3);
        let s = h.state();
        assert!(s.view_tools.curve.pts.is_empty());
        assert_eq!(s.doc.shapes.len(), 1);
        assert_eq!(s.undo.len(), 1, "one step");
        assert_eq!(s.doc.shapes[0].params.kind, Some(Kind::Run));
        assert!(matches!(s.doc.shapes[0].geometry, Geometry::Stroke { .. }));
        assert_eq!(s.sel_indices(), vec![0]);
        // Three points, as clicked: the middle one smooth, with the
        // handles the circle through the three gives it; each end with
        // its one handle into the curve.
        let path = s.doc.edit_path(0).unwrap();
        let a = &path[0].anchors;
        assert!(!path[0].closed);
        assert_eq!(a.len(), 3);
        for (k, p) in at.iter().enumerate() {
            let on = s.view.to_screen(s.canvas_rect, a[k].p);
            assert!((on - *p).length() < 1.0, "point {k} where it was clicked");
        }
        assert!(a[1].is_smooth());
        let want = bezier::smooth_handles(a[0].p, a[1].p, a[2].p).unwrap();
        assert!(a[1].h_in.dist(want.0) < 0.5 && a[1].h_out.dist(want.1) < 0.5);
        assert!(a[0].h_out != a[0].p && a[0].h_in == a[0].p);
        assert!(a[2].h_in != a[2].p && a[2].h_out == a[2].p);
        assert!(s.design.stitch_count() > 10, "it sews");
    }

    #[test]
    fn clicking_the_first_point_closes_a_shape() {
        let mut h = studio();
        let at = three(&h);
        for p in at {
            click(&mut h, p);
        }
        // (Twice over: the rest of a double-click starts nothing new.)
        click(&mut h, at[0]);
        h.hover_at(at[0]);
        h.drag_at(at[0]);
        h.run_steps(1);
        h.drop_at(at[0]);
        h.run_steps(2);
        let s = h.state();
        assert!(s.view_tools.curve.pts.is_empty());
        assert_eq!(s.doc.shapes.len(), 1);
        assert_eq!(s.doc.shapes[0].params.kind, Some(Kind::Fill));
        assert!(matches!(s.doc.shapes[0].geometry, Geometry::Fill { .. }));
        let path = s.doc.edit_path(0).unwrap();
        assert!(path[0].closed);
        assert_eq!(path[0].anchors.len(), 3);
        assert!(
            path[0].anchors.iter().all(|a| a.is_smooth()),
            "smooth all round"
        );
        // Two points can't close: a click on the first is on a point, and
        // adds nothing.
        let mut h = studio();
        click(&mut h, at[0]);
        click(&mut h, at[1]);
        click(&mut h, at[0]);
        assert_eq!(h.state().view_tools.curve.pts.len(), 2);
        assert!(h.state().doc.shapes.is_empty());
    }

    #[test]
    fn corners_drags_and_backspace_shape_the_path_being_drawn() {
        let mut h = studio();
        let at = three(&h);
        click(&mut h, at[0]);
        // Alt-click: a corner from the start.
        with_alt(&mut h, |h| click(h, at[1]));
        click(&mut h, at[2]);
        let flags = |h: &Harness<'_, Studio>| -> Vec<bool> {
            h.state().view_tools.curve.pts.iter().map(|p| p.1).collect()
        };
        assert_eq!(flags(&h), [false, true, false]);
        // Alt-click it again: smooth. Double-click it: a corner again.
        with_alt(&mut h, |h| click(h, at[1]));
        assert_eq!(flags(&h), [false, false, false]);
        // (A moment later: these two clicks are the double-click.)
        h.run_steps(30);
        click(&mut h, at[1]);
        h.hover_at(at[1]);
        h.drag_at(at[1]);
        h.run_steps(1);
        h.drop_at(at[1]);
        h.run_steps(2);
        assert_eq!(flags(&h), [false, true, false]);
        assert_eq!(h.state().view_tools.curve.pts.len(), 3, "no point added");
        // Drag the middle point up: it moves, and nothing else is added.
        let to = at[1] + Vec2::new(20.0, -60.0);
        h.hover_at(at[1]);
        h.run_steps(1);
        h.drag_at(at[1]);
        h.run_steps(1);
        for k in 1..=4 {
            h.hover_at(at[1] + (to - at[1]) * (k as f32 / 4.0));
            h.run_steps(1);
        }
        h.drop_at(to);
        h.run_steps(2);
        let s = h.state();
        assert_eq!(s.view_tools.curve.pts.len(), 3);
        let moved = s.view.to_screen(s.canvas_rect, s.view_tools.curve.pts[1].0);
        assert!((moved - to).length() < 1.5, "{moved:?}");
        // Backspace: the point last touched goes (the middle one); again,
        // the last.
        h.key_press(Key::Backspace);
        h.run_steps(1);
        assert_eq!(h.state().view_tools.curve.pts.len(), 2);
        let left = h.state().view_tools.curve.pts[1].0;
        let s = h.state();
        assert!((s.view.to_screen(s.canvas_rect, left) - at[2]).length() < 1.0);
        h.key_press(Key::Backspace);
        h.run_steps(1);
        assert_eq!(h.state().view_tools.curve.pts.len(), 1);
        // One point is nothing to keep: Escape ends it, with no object.
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert!(h.state().view_tools.curve.pts.is_empty());
        assert!(h.state().doc.shapes.is_empty() && h.state().undo.is_empty());
        assert_eq!(
            h.state().tool,
            Tool::Curvature,
            "Escape ended the line, not the tool"
        );
    }

    #[test]
    fn another_tool_ends_the_line_keeping_it() {
        let mut h = studio();
        let at = three(&h);
        for p in at {
            click(&mut h, p);
        }
        h.key_press(Key::V);
        h.run_steps(3);
        assert_eq!(h.state().tool, Tool::Select);
        assert_eq!(h.state().doc.shapes.len(), 1);
        assert!(h.state().view_tools.curve.pts.is_empty());
    }

    #[test]
    fn a_click_elsewhere_right_after_closing_starts_the_next_shape() {
        let mut h = studio();
        let at = three(&h);
        for p in at {
            click(&mut h, p);
        }
        click(&mut h, at[0]);
        assert_eq!(h.state().doc.shapes.len(), 1);
        // At once, well away from where it closed: a first point.
        let away = at[0] + Vec2::new(-80.0, 120.0);
        h.hover_at(away);
        h.drag_at(away);
        h.run_steps(1);
        h.drop_at(away);
        h.run_steps(2);
        assert_eq!(h.state().view_tools.curve.pts.len(), 1);
        // Space held (to pan): a press places nothing.
        h.key_down(Key::Space);
        h.run_steps(1);
        click(&mut h, at[1]);
        h.key_up(Key::Space);
        h.run_steps(1);
        assert_eq!(h.state().view_tools.curve.pts.len(), 1);
    }
}
