//! The Lasso (Q): draw a loop, and what it goes round is selected. After
//! VectorCraft's (and Illustrator's), where it selects anchor points:
//!
//! - A point is caught when it's inside the loop (which closes straight
//!   back to where it began); an object when any of its points is.
//! - A plain loop selects what it catches and lets go of the rest; Shift
//!   adds to the selection; Alt takes away from it. A click that isn't a
//!   loop deselects everything (with Shift or Alt, nothing).
//! - One object caught in part is selected with just those points
//!   selected, for Direct Selection (A) to move: the points stay selected
//!   going between the two tools. Points are selected in one object at a
//!   time here, so several objects caught are selected whole.
//!
//! Locked and hidden objects aren't caught, nor anything outside the
//! group that's been entered.

use crate::app::{Studio, Tool};
use crate::cursors::Glyph;
use crate::icons::Icon;
use eframe::egui::{self, Color32, Pos2, Rect, Stroke};
use zigzag::bezier::AnchorRef;
use zigzag::geom::{FillRule, P, point_inside};
use zigzag::svg::Geometry;

/// How a loop changes the selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Set,
    Add,
    Subtract,
}

/// How much of an object is selected.
#[derive(Clone, PartialEq, Debug)]
enum Part {
    Whole,
    Points(Vec<AnchorRef>),
}

/// An object the loop caught: its index, the points inside the loop, and
/// how many points it has (none: it has no points to select one by one,
/// and is caught whole).
struct Caught {
    index: usize,
    points: Vec<AnchorRef>,
    of: usize,
}

impl Caught {
    fn part(&self) -> Part {
        if self.points.len() >= self.of {
            Part::Whole
        } else {
            Part::Points(self.points.clone())
        }
    }
}

impl Studio {
    /// Canvas input and drawing for the Lasso.
    pub fn lasso_canvas(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        self.want_cursor(Glyph::Cross(Some(Icon::Lasso)));
        let px = self.view.scale as f64 / 10.0; // screen points per design unit
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(at) = ui.input(|i| i.pointer.press_origin())
        {
            self.view_tools.lasso = vec![self.view.to_design(rect, at)];
        }
        let held = resp.dragged_by(egui::PointerButton::Primary);
        if held
            && !self.view_tools.lasso.is_empty()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let q = self.view.to_design(rect, pos);
            if self
                .view_tools
                .lasso
                .last()
                .is_none_or(|l| l.dist(q) * px >= 1.0)
            {
                self.view_tools.lasso.push(q);
            }
            ui.ctx().request_repaint();
        }
        // The loop so far: open, as it will close straight back.
        if self.view_tools.lasso.len() > 1 {
            let pts: Vec<Pos2> = self
                .view_tools
                .lasso
                .iter()
                .map(|&p| self.view.to_screen(rect, p))
                .collect();
            painter.add(egui::Shape::line(
                pts.clone(),
                Stroke::new(2.5, Color32::from_white_alpha(150)),
            ));
            painter.extend(egui::Shape::dashed_line(
                &pts,
                Stroke::new(1.0, Color32::from_gray(0x40)),
                4.0,
                3.0,
            ));
        }
        let mode = ui.input(|i| {
            if i.modifiers.alt {
                Mode::Subtract
            } else if i.modifiers.shift {
                Mode::Add
            } else {
                Mode::Set
            }
        });
        if resp.drag_stopped() {
            let drawn = std::mem::take(&mut self.view_tools.lasso);
            if drawn.len() >= 3 {
                self.finish_lasso(&drawn, mode);
            } else if !drawn.is_empty() {
                self.lasso_click(mode);
            }
        } else if resp.clicked() {
            self.lasso_click(mode);
        } else if !held {
            // (Let go off the canvas, or the tool left mid-loop.)
            self.view_tools.lasso.clear();
        }
        self.hud(
            painter,
            rect,
            "Draw a loop round points or objects to select them · Shift adds · Alt takes away",
        );
    }

    /// Escape while a loop is being drawn calls it off.
    pub fn lasso_keys(&mut self, ctx: &egui::Context) -> bool {
        if self.tool != Tool::Lasso || self.view_tools.lasso.is_empty() {
            return false;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.view_tools.lasso.clear();
            return true;
        }
        false
    }

    /// A click that isn't a loop: everything deselected, unless adding or
    /// taking away (then nothing).
    fn lasso_click(&mut self, mode: Mode) {
        if mode == Mode::Set {
            self.set_selection(&[]);
            self.path_edit.selected.clear();
        }
    }

    /// What the loop catches: each object that can be selected with a
    /// point inside it.
    fn lasso_catch(&self, drawn: &[P]) -> Vec<Caught> {
        let ring = [drawn.to_vec()];
        let inside = |p: P| point_inside(&ring, FillRule::EvenOdd, p);
        let paths = self.doc.all_edit_paths();
        (0..self.doc.shapes.len())
            .filter(|&i| self.in_scope(i) && self.free(i))
            // (A shape and its outline are one element with one set of
            // points: caught as the shape, as a click on either is.)
            .filter(|&i| self.shape_of(i) == i)
            .filter_map(|i| match paths.get(i).and_then(Option::as_ref) {
                Some(path) if self.doc.lettering(i).is_none() => {
                    let of = path.iter().map(|sp| sp.anchors.len()).sum();
                    let points: Vec<AnchorRef> = path
                        .iter()
                        .enumerate()
                        .flat_map(|(s, sp)| {
                            sp.anchors
                                .iter()
                                .enumerate()
                                .filter(|(_, a)| inside(a.p))
                                .map(move |(k, _)| (s, k))
                        })
                        .collect();
                    (!points.is_empty()).then_some(Caught {
                        index: i,
                        points,
                        of,
                    })
                }
                // Text, and anything else not edited point by point: its
                // outline as it's drawn, and it's caught whole.
                _ => {
                    let any = match &self.doc.shapes[i].geometry {
                        Geometry::Fill { rings, .. } => rings.iter().flatten().any(|&p| inside(p)),
                        Geometry::Stroke { lines, .. } => {
                            lines.iter().flat_map(|(l, _)| l).any(|&p| inside(p))
                        }
                    };
                    any.then_some(Caught {
                        index: i,
                        points: Vec::new(),
                        of: 0,
                    })
                }
            })
            .collect()
    }

    /// The loop drawn: select by it.
    fn finish_lasso(&mut self, drawn: &[P], mode: Mode) {
        let caught = self.lasso_catch(drawn);
        // The selection as it is: each object whole, but the one whose
        // points are selected.
        let before = self.sel_indices();
        let pointed = (before.len() == 1 && !self.path_edit.selected.is_empty())
            .then(|| before[0])
            .filter(|&i| self.path_edit.points_of == Some(self.doc.shapes[i].uid));
        let mut now: Vec<(usize, Part)> = match mode {
            Mode::Set => Vec::new(),
            _ => before
                .iter()
                .map(|&i| {
                    let part = if pointed == Some(i) {
                        Part::Points(self.path_edit.selected.clone())
                    } else {
                        Part::Whole
                    };
                    (i, part)
                })
                .collect(),
        };
        for c in &caught {
            let at = now.iter().position(|(i, _)| *i == c.index);
            match (mode, at) {
                (Mode::Set | Mode::Add, None) => now.push((c.index, c.part())),
                (Mode::Set | Mode::Add, Some(k)) => {
                    if let Part::Points(have) = &mut now[k].1 {
                        for a in &c.points {
                            if !have.contains(a) {
                                have.push(*a);
                            }
                        }
                        if have.len() >= c.of {
                            now[k].1 = Part::Whole;
                        }
                    }
                }
                (Mode::Subtract, None) => {}
                (Mode::Subtract, Some(k)) => {
                    let left: Vec<AnchorRef> = match &now[k].1 {
                        // (An object with no points of its own goes whole.)
                        Part::Whole if c.of == 0 => Vec::new(),
                        Part::Whole => {
                            let path = self.doc.edit_path(c.index).unwrap_or_default();
                            path.iter()
                                .enumerate()
                                .flat_map(|(s, sp)| (0..sp.anchors.len()).map(move |k| (s, k)))
                                .filter(|a| !c.points.contains(a))
                                .collect()
                        }
                        Part::Points(have) => have
                            .iter()
                            .copied()
                            .filter(|a| !c.points.contains(a))
                            .collect(),
                    };
                    if left.is_empty() {
                        now.remove(k);
                    } else {
                        now[k].1 = Part::Points(left);
                    }
                }
            }
        }
        let objects: Vec<usize> = now.iter().map(|(i, _)| *i).collect();
        self.set_selection(&objects);
        self.path_edit.selected.clear();
        match now.as_slice() {
            // One object, in part: its points.
            [(i, Part::Points(points))] => {
                self.path_edit.points_of = Some(self.doc.shapes[*i].uid);
                self.path_edit.selected = points.clone();
                self.status = format!(
                    "{} point{} of {} selected.",
                    points.len(),
                    if points.len() == 1 { "" } else { "s" },
                    self.doc.shapes[*i].label
                );
            }
            [] => self.status = "Nothing selected.".into(),
            all => {
                self.status = format!(
                    "{} object{} selected.",
                    all.len(),
                    if all.len() == 1 { "" } else { "s" }
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::shape_bounds;
    use egui::{Key, Vec2};
    use egui_kittest::Harness;

    fn studio() -> Harness<'static, Studio> {
        let path =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/badge.svg"));
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(path), None),
            );
        h.run_steps(3);
        h.key_press(Key::Q);
        h.run_steps(1);
        assert_eq!(h.state().tool, Tool::Lasso);
        assert_eq!(h.state().view_tools.direct_slot, Tool::Lasso);
        h
    }

    fn named(h: &Harness<'_, Studio>, name: &str) -> usize {
        h.state()
            .doc
            .shapes
            .iter()
            .position(|s| s.label.contains(name))
            .unwrap_or_else(|| panic!("no {name}"))
    }

    /// Draw a loop round the box from `lo` to `hi` (design units).
    fn lasso(h: &mut Harness<'_, Studio>, lo: P, hi: P) {
        let to = |h: &Harness<'_, Studio>, p: P| {
            let s = h.state();
            s.view.to_screen(s.canvas_rect, p)
        };
        let corners = [
            lo,
            P::new(hi.x, lo.y),
            hi,
            P::new(lo.x, hi.y),
            P::new(lo.x, lo.y + (hi.y - lo.y) * 0.1),
        ];
        let start = to(h, corners[0]);
        h.hover_at(start);
        h.run_steps(1);
        h.drag_at(start);
        h.run_steps(1);
        for w in corners.windows(2) {
            for k in 1..=4 {
                let p = w[0].lerp(w[1], k as f64 / 4.0);
                let at = to(h, p);
                h.hover_at(at);
                h.run_steps(1);
            }
        }
        let end = to(h, corners[4]);
        h.drop_at(end);
        h.run_steps(2);
    }

    fn with_mods(
        h: &mut Harness<'_, Studio>,
        m: egui::Modifiers,
        f: impl FnOnce(&mut Harness<'_, Studio>),
    ) {
        h.event(egui::Event::ModifiersChanged(m));
        h.run_steps(1);
        f(h);
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        h.run_steps(1);
    }

    /// The box of object `i`, grown by `by` units all round.
    fn around(h: &Harness<'_, Studio>, i: usize, by: f64) -> (P, P) {
        let b = shape_bounds(&h.state().doc.shapes[i]);
        (
            P::new(b.min.x - by, b.min.y - by),
            P::new(b.max.x + by, b.max.y + by),
        )
    }

    #[test]
    fn a_loop_selects_what_it_goes_round() {
        let mut h = studio();
        let (sun, swoosh) = (named(&h, "sun"), named(&h, "swoosh"));
        // Round the sun: the sun, whole (every point of it), and nothing
        // of the disc it sits on, whose points are out at its edge.
        let (lo, hi) = around(&h, sun, 8.0);
        lasso(&mut h, lo, hi);
        assert_eq!(h.state().sel_indices(), vec![sun]);
        assert!(h.state().path_edit.selected.is_empty(), "whole");
        assert!(h.state().view_tools.lasso.is_empty());
        assert_eq!(h.state().tool, Tool::Lasso, "the tool stays");
        // Shift, round the swoosh's first point: the swoosh as well (and
        // with two objects selected, each of them whole).
        let end = h.state().doc.edit_path(swoosh).unwrap()[0].anchors[0].p;
        let (lo, hi) = (
            P::new(end.x - 5.0, end.y - 5.0),
            P::new(end.x + 5.0, end.y + 5.0),
        );
        with_mods(&mut h, egui::Modifiers::SHIFT, |h| lasso(h, lo, hi));
        let sel = h.state().sel_indices();
        assert_eq!(sel.len(), 2, "{sel:?}");
        assert!(sel.contains(&sun) && sel.contains(&swoosh));
        assert_eq!(sel[0], sun, "what was selected first still is");
        assert!(h.state().path_edit.selected.is_empty());
        // Alt, round the sun: the swoosh is left.
        let (lo, hi) = around(&h, sun, 8.0);
        with_mods(&mut h, egui::Modifiers::ALT, |h| lasso(h, lo, hi));
        assert_eq!(h.state().sel_indices(), vec![swoosh]);
        // A plain loop round nothing lets go of everything.
        let b = shape_bounds(&h.state().doc.shapes[named(&h, "disc")]);
        let off = P::new(b.max.x + 30.0, b.max.y + 30.0);
        lasso(&mut h, off, P::new(off.x + 40.0, off.y + 40.0));
        assert!(h.state().sel_indices().is_empty());
    }

    #[test]
    fn part_of_one_object_is_its_points_and_they_carry_to_direct_selection() {
        let mut h = studio();
        let sun = named(&h, "sun");
        let path = h.state().doc.edit_path(sun).unwrap();
        let total: usize = path.iter().map(|sp| sp.anchors.len()).sum();
        assert!(total >= 3, "a round shape has points all round: {total}");
        // The topmost point of the sun, in a small loop of its own.
        let top = path
            .iter()
            .flat_map(|sp| sp.anchors.iter().map(|a| a.p))
            .min_by(|a, b| a.y.total_cmp(&b.y))
            .unwrap();
        let caught: Vec<AnchorRef> = path
            .iter()
            .enumerate()
            .flat_map(|(s, sp)| {
                sp.anchors
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| a.p.dist(top) < 6.0)
                    .map(move |(k, _)| (s, k))
            })
            .collect();
        assert!(!caught.is_empty() && caught.len() < total);
        let (lo, hi) = (
            P::new(top.x - 6.0, top.y - 6.0),
            P::new(top.x + 6.0, top.y + 6.0),
        );
        lasso(&mut h, lo, hi);
        assert_eq!(h.state().sel_indices(), vec![sun]);
        let mut got = h.state().path_edit.selected.clone();
        got.sort_unstable();
        assert_eq!(got, caught);
        // To Direct Selection and back: still selected.
        h.key_press(Key::A);
        h.run_steps(2);
        assert_eq!(h.state().tool, Tool::Direct);
        assert_eq!(h.state().path_edit.selected.len(), caught.len());
        h.key_press(Key::Q);
        h.run_steps(2);
        assert_eq!(h.state().path_edit.selected.len(), caught.len());
        // Shift, round the whole sun: all of it, so it's whole again.
        let (lo, hi) = around(&h, sun, 8.0);
        with_mods(&mut h, egui::Modifiers::SHIFT, |h| lasso(h, lo, hi));
        assert_eq!(h.state().sel_indices(), vec![sun]);
        assert!(h.state().path_edit.selected.is_empty());
        // Alt, round that top point: every point but it.
        let (lo, hi) = (
            P::new(top.x - 6.0, top.y - 6.0),
            P::new(top.x + 6.0, top.y + 6.0),
        );
        with_mods(&mut h, egui::Modifiers::ALT, |h| lasso(h, lo, hi));
        assert_eq!(h.state().sel_indices(), vec![sun]);
        assert_eq!(h.state().path_edit.selected.len(), total - caught.len());
        // No selecting is an edit.
        assert!(h.state().undo.is_empty());
    }

    #[test]
    fn a_click_lets_go_and_locked_objects_are_not_caught() {
        let mut h = studio();
        let (sun, disc) = (named(&h, "sun"), named(&h, "disc"));
        h.state_mut().set_selection(&[sun]);
        h.run_steps(1);
        // Shift-click: nothing changes. A plain click: nothing selected.
        let at = h.state().canvas_rect.left_top() + Vec2::new(8.0, 80.0);
        let click = |h: &mut Harness<'_, Studio>| {
            h.hover_at(at);
            h.run_steps(1);
            h.drag_at(at);
            h.run_steps(1);
            h.drop_at(at);
            h.run_steps(2);
        };
        with_mods(&mut h, egui::Modifiers::SHIFT, |h| click(h));
        assert_eq!(h.state().sel_indices(), vec![sun]);
        click(&mut h);
        assert!(h.state().sel_indices().is_empty());
        // Everything, in one loop: but not the sun once it's locked.
        h.state_mut().doc.shapes[sun].params.lock = Some(true);
        h.run_steps(1);
        let (lo, hi) = around(&h, disc, 12.0);
        lasso(&mut h, lo, hi);
        let sel = h.state().sel_indices();
        assert!(sel.contains(&disc) && !sel.contains(&sun), "{sel:?}");
        // Escape part way round calls the loop off.
        let before = h.state().sel_indices();
        let c = h.state().canvas_rect.center();
        h.hover_at(c);
        h.run_steps(1);
        h.drag_at(c);
        h.run_steps(1);
        h.hover_at(c + Vec2::new(40.0, 0.0));
        h.run_steps(1);
        h.hover_at(c + Vec2::new(40.0, 40.0));
        h.run_steps(1);
        assert!(h.state().view_tools.lasso.len() >= 2);
        h.key_press(Key::Escape);
        h.run_steps(1);
        assert!(h.state().view_tools.lasso.is_empty());
        assert_eq!(
            h.state().tool,
            Tool::Lasso,
            "Escape took the loop, not the tool"
        );
        h.drop_at(c + Vec2::new(40.0, 40.0));
        h.run_steps(2);
        assert_eq!(h.state().sel_indices(), before);
    }

    #[test]
    fn a_shape_with_an_outline_is_caught_as_one_object() {
        // A filled square with a stroke: two objects of one element.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="60mm" viewBox="0 0 60 60"><rect id="box" x="15" y="15" width="30" height="30" fill="#c00" stroke="#000" stroke-width="1"/></svg>"##;
        let file = std::env::temp_dir().join(format!("zigzag-lasso-{}.svg", std::process::id()));
        std::fs::write(&file, svg).unwrap();
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(file.clone()), None),
            );
        h.run_steps(3);
        let _ = std::fs::remove_file(&file);
        assert_eq!(h.state().doc.shapes.len(), 2, "the fill and its outline");
        let fill = h
            .state()
            .doc
            .shapes
            .iter()
            .position(|s| s.part == zigzag::svg::Part::Fill)
            .unwrap();
        h.state_mut().set_tool(Tool::Lasso);
        h.run_steps(1);
        // Round one corner: that corner of the shape, selected as a point.
        let corner = h.state().doc.edit_path(fill).unwrap()[0].anchors[0].p;
        lasso(
            &mut h,
            P::new(corner.x - 20.0, corner.y - 20.0),
            P::new(corner.x + 20.0, corner.y + 20.0),
        );
        assert_eq!(h.state().sel_indices(), vec![fill]);
        assert_eq!(h.state().path_edit.selected.len(), 1);
    }
}
