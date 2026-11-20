//! The Eyedropper (E): click an object and the selection takes its thread
//! and its stitch settings, in one step. After VectorCraft's (and
//! Illustrator's):
//!
//! - A click reads the object under the pointer (locked ones too: reading
//!   changes nothing) and gives what it read to the selection. With
//!   nothing selected it's only kept.
//! - Alt-click goes the other way: the object clicked takes the look of
//!   the selection (its first object), or with nothing selected, the look
//!   last read.
//! - Shift-click reads the thread alone.
//!
//! "Stitch settings" are what Edit › Paste Stitch Settings pastes: the
//! stitch type where the object can be sewn that way, and its settings.
//! Each object keeps its place in the sewing order and where it starts and
//! ends.

use crate::app::{Studio, give_settings};
use crate::cursors::Glyph;
use crate::icons::Icon;
use eframe::egui::{self, Rect};
use zigzag::params::Params;

/// A look read by the Eyedropper: the stitch settings and the thread.
pub type Look = (Params, [u8; 3]);

impl Studio {
    /// Canvas input for the Eyedropper.
    pub fn eyedropper_canvas(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        self.want_cursor(Glyph::Cross(Some(Icon::Eyedropper)));
        self.hud(
            painter,
            rect,
            "Click an object to give the selection its thread and stitch settings · Alt-click to give it the selection's · Shift-click for the thread alone",
        );
        // On the press, as in VectorCraft: nothing to drag.
        let pressed = ui.input(|i| i.pointer.primary_pressed()) && resp.is_pointer_button_down_on();
        let Some(pos) = resp.interact_pointer_pos().filter(|_| pressed) else {
            return;
        };
        let q = self.view.to_design(rect, pos);
        let tol = 6.0 * 10.0 / self.view.scale as f64;
        let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
        if alt {
            // The other way: onto the object clicked, which has to be one
            // that can be changed.
            let Some(target) = self.pick(q, tol) else {
                return;
            };
            let from = self.subjects().first().copied();
            if from == Some(target) {
                return;
            }
            let look = match from {
                Some(i) => Some(self.look_of(i)),
                None => self.view_tools.sample.clone(),
            };
            let Some(look) = look else {
                self.status = "Click an object first, to read its thread and settings.".into();
                return;
            };
            self.give_look(&[target], &look, shift);
            return;
        }
        // Anything that shows can be read, locked or not.
        let hit = crate::canvas::hit_test_where(&self.doc.shapes, q, tol, |i| {
            self.in_scope(i) && !self.doc.shapes[i].params.hidden()
        })
        .or_else(|| {
            crate::canvas::hit_test_where(&self.doc.shapes, q, tol, |i| {
                !self.doc.shapes[i].params.hidden()
            })
        });
        let Some(src) = hit else {
            return;
        };
        let look = self.look_of(src);
        self.view_tools.sample = Some(look.clone());
        let targets: Vec<usize> = self.subjects().into_iter().filter(|&i| i != src).collect();
        if targets.is_empty() {
            self.status = format!(
                "Read {}: Alt-click an object to give it this thread and these settings.",
                self.doc.shapes[src].label
            );
            return;
        }
        self.give_look(&targets, &look, shift);
    }

    /// Object `i`'s look.
    fn look_of(&self, i: usize) -> Look {
        let s = &self.doc.shapes[i];
        (s.params.clone(), s.color)
    }

    /// Give `targets` a look (its thread alone, if `thread_only`), as one
    /// undo step; none if they have it already.
    fn give_look(&mut self, targets: &[usize], look: &Look, thread_only: bool) {
        let (params, color) = look;
        let mut after = self.doc.shapes.clone();
        for &i in targets {
            after[i].color = *color;
            if !thread_only {
                give_settings(&mut after[i], params);
            }
        }
        let changed = targets
            .iter()
            .filter(|&&i| {
                after[i].color != self.doc.shapes[i].color
                    || after[i].params != self.doc.shapes[i].params
            })
            .count();
        if changed == 0 {
            self.status = "Nothing to change: it's sewn that way already.".into();
            return;
        }
        let _ = self.structural(|d| {
            d.shapes = after;
            Ok(())
        });
        self.status = format!(
            "Eyedropper: {changed} object{} given the {}.",
            if changed == 1 { "" } else { "s" },
            if thread_only {
                "thread"
            } else {
                "thread and stitch settings"
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Tool, shape_bounds};
    use egui::{Key, Pos2, Vec2};
    use egui_kittest::Harness;
    use zigzag::params::Kind;

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

    /// A point on screen inside object `i` (the middle of its box, which
    /// for these is inside it).
    fn on(h: &Harness<'_, Studio>, i: usize) -> Pos2 {
        let s = h.state();
        let b = shape_bounds(&s.doc.shapes[i]);
        s.view.to_screen(s.canvas_rect, b.center())
    }

    fn click(h: &mut Harness<'_, Studio>, at: Pos2) {
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
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

    #[test]
    fn e_and_a_click_give_the_selection_the_clicked_objects_look() {
        let mut h = studio();
        let (sun, swoosh) = (named(&h, "sun"), named(&h, "swoosh"));
        let (sun_color, swoosh_color) = {
            let d = &h.state().doc;
            (d.shapes[sun].color, d.shapes[swoosh].color)
        };
        assert_ne!(sun_color, swoosh_color);
        // The sun's fill, made its own: an angle and a spacing to carry.
        {
            let s = h.state_mut();
            s.doc.shapes[sun].params.kind = Some(Kind::Fill);
            s.doc.shapes[sun].params.angle = Some(72.0);
            s.doc.shapes[sun].params.spacing = Some(0.6);
            s.set_selection(&[swoosh]);
        }
        h.run_steps(2);
        h.key_press(Key::E);
        h.run_steps(1);
        assert_eq!(h.state().tool, Tool::Eyedropper);
        assert_eq!(h.state().view_tools.sample_slot, Tool::Eyedropper);
        let undo = h.state().undo.len();
        let at = on(&h, sun);
        click(&mut h, at);
        let s = h.state();
        // The swoosh is a line: it takes the thread and the settings, not
        // a stitch type a line can't be sewn as.
        assert_eq!(s.doc.shapes[swoosh].color, sun_color);
        assert_eq!(s.doc.shapes[swoosh].params.angle, Some(72.0));
        assert_eq!(s.doc.shapes[swoosh].params.spacing, Some(0.6));
        let line = matches!(
            s.doc.shapes[swoosh].geometry,
            zigzag::svg::Geometry::Stroke { .. }
        );
        assert_eq!(
            s.doc.shapes[swoosh].params.kind == Some(Kind::Fill),
            !line,
            "a line isn't made a fill"
        );
        assert_eq!(s.undo.len(), undo + 1, "one undo step");
        // The sun itself is as it was, and still not selected.
        assert_eq!(s.doc.shapes[sun].color, sun_color);
        assert_eq!(s.sel_indices(), vec![swoosh]);
        // Again changes nothing, and adds no step.
        click(&mut h, at);
        assert_eq!(h.state().undo.len(), undo + 1);
        // Empty canvas: nothing.
        let corner = h.state().canvas_rect.left_top() + Vec2::new(6.0, 60.0);
        click(&mut h, corner);
        assert_eq!(h.state().undo.len(), undo + 1);
        assert_eq!(h.state().sel_indices(), vec![swoosh]);
    }

    #[test]
    fn shift_reads_the_thread_alone_and_alt_gives_the_other_way() {
        let mut h = studio();
        let (sun, disc) = (named(&h, "sun"), named(&h, "disc"));
        let (sun_color, disc_color) = {
            let d = &h.state().doc;
            (d.shapes[sun].color, d.shapes[disc].color)
        };
        {
            let s = h.state_mut();
            s.doc.shapes[sun].params.angle = Some(72.0);
            s.doc.shapes[disc].params.angle = Some(10.0);
            s.set_selection(&[disc]);
            s.set_tool(Tool::Eyedropper);
        }
        h.run_steps(2);
        // Shift: the sun's thread onto the disc, whose angle stays.
        let at = on(&h, sun);
        with_mods(&mut h, egui::Modifiers::SHIFT, |h| click(h, at));
        assert_eq!(h.state().doc.shapes[disc].color, sun_color);
        assert_eq!(h.state().doc.shapes[disc].params.angle, Some(10.0));
        h.state_mut().doc.shapes[disc].color = disc_color;
        // Nothing selected: a click only reads; Alt-click then gives what
        // was read to the object clicked. (The disc, clear of the sun: its
        // left edge's middle.)
        h.state_mut().set_selection(&[]);
        h.run_steps(2);
        let undo = h.state().undo.len();
        click(&mut h, at);
        assert_eq!(h.state().undo.len(), undo, "reading is no edit");
        assert!(h.state().sel_indices().is_empty());
        let left = {
            let s = h.state();
            let b = shape_bounds(&s.doc.shapes[disc]);
            let p = zigzag::geom::P::new(b.min.x + b.width() * 0.06, b.center().y);
            s.view.to_screen(s.canvas_rect, p)
        };
        with_mods(&mut h, egui::Modifiers::ALT, |h| click(h, left));
        let s = h.state();
        assert_eq!(s.doc.shapes[disc].color, sun_color);
        assert_eq!(s.doc.shapes[disc].params.angle, Some(72.0));
        assert_eq!(s.undo.len(), undo + 1);
        // With a selection, Alt gives the selection's look to what's
        // clicked: the disc's (the sun's, by now) back onto the sun is no
        // change; a new angle on the disc goes to the sun.
        h.state_mut().doc.shapes[disc].params.angle = Some(33.0);
        h.state_mut().set_selection(&[disc]);
        h.run_steps(2);
        with_mods(&mut h, egui::Modifiers::ALT, |h| click(h, at));
        assert_eq!(h.state().doc.shapes[sun].params.angle, Some(33.0));
        assert_eq!(h.state().sel_indices(), vec![disc], "the selection stays");
    }

    #[test]
    fn a_locked_objects_look_is_read_without_its_lock() {
        let mut h = studio();
        let (sun, swoosh) = (named(&h, "sun"), named(&h, "swoosh"));
        {
            let s = h.state_mut();
            s.doc.shapes[sun].params.lock = Some(true);
            s.doc.shapes[sun].params.angle = Some(72.0);
            s.set_selection(&[swoosh]);
            s.set_tool(Tool::Eyedropper);
        }
        h.run_steps(2);
        let at = on(&h, sun);
        click(&mut h, at);
        let s = h.state();
        assert_eq!(s.doc.shapes[swoosh].params.angle, Some(72.0));
        assert!(
            !s.doc.shapes[swoosh].params.locked(),
            "its look, not its lock"
        );
        assert!(s.doc.shapes[sun].params.locked());
        // Nor is a hidden object shown by being given a look.
        let mut h = studio();
        let (sun, disc) = (named(&h, "sun"), named(&h, "disc"));
        {
            let s = h.state_mut();
            s.doc.shapes[disc].params.hide = Some(true);
            s.set_selection(&[disc]);
            s.set_tool(Tool::Eyedropper);
        }
        h.run_steps(2);
        let at = on(&h, sun);
        click(&mut h, at);
        assert!(h.state().doc.shapes[disc].params.hidden());
    }
}
