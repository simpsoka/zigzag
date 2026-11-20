//! The Rotate tool (Shift+R), after VectorCraft's
//! `tools/src/xform/transform.rs` (`TransformKind::Rotate`; MIT OR
//! Apache-2.0). The selection turns about a reference point, drawn as a
//! small target: its center, until a click places it somewhere else,
//! snapped to the nearest anchor or center (labelled while hovering). A
//! placed point belongs to the selection it was placed for, and goes when
//! the selection changes. A drag from anywhere turns the selection about
//! the point, the stitches following and the angle beside the pointer
//! (Shift: 45° steps; Option held at release leaves the original and adds
//! a turned copy). Option-click places the point and opens the Rotate
//! dialog for an exact angle; so does a double-click, Return opens it
//! about the point as it is, and a double-click on the tool's button or
//! Object › Rotate… opens it too.
//!
//! The dialog is VectorCraft's (`ui-egui/src/dialogs/transform.rs`, the
//! `"rotate"` case), drawn with the studio's own dialog parts: the Angle
//! field (which has the keyboard as it opens: type, Return), then Copy
//! and Preview as checkboxes, then Cancel and OK. Its Transform Objects
//! and Transform Patterns checkboxes are left out: there are no pattern
//! fills here for them to choose between. The tool's icon is
//! VectorCraft's too, and so are the target (a circle and a crosshair)
//! and the dashed line out to the pointer while turning.
//!
//! Where this differs from VectorCraft's: the document is the SVG, so a
//! turn is written into it through `transform_selected` (one undo step,
//! and nothing about the tool is kept in the file); the gesture is the
//! one the selection box's rotate corner makes ([`Studio::begin_rotate`]),
//! so it turns in whole degrees, clockwise for a positive angle as the
//! Transform panel's does, and ⌘D repeats it, about the same point, copy
//! and all, which is how a ring of copies is made; and with nothing
//! selected a click selects what's under the pointer, as the Fill Angle
//! tool does.

use crate::app::Studio;
use crate::cursors::Glyph;
use crate::layout::{DragMode, DragMods};
use eframe::egui::{self, Color32, Key, Pos2, Rect, Shape, Stroke, Vec2};
use zigzag::document::Document;
use zigzag::geom::{P, rotate_about};

/// The reference point's target (cyan, as VectorCraft's and the Measure
/// tool's line), and its half-width on screen (points).
const CYAN: Color32 = Color32::from_rgb(40, 200, 255);
const TARGET: f32 = 8.0;
/// The dialog's number field (VectorCraft's `FIELD_W`).
const FIELD_W: f32 = 132.0;

#[derive(Default, Clone)]
pub struct RotateState {
    /// A reference point placed by hand (design units), and the selection
    /// it was placed for (its objects' uids, in order).
    pub origin: Option<(P, Vec<usize>)>,
    /// The window for an exact angle, when open.
    pub dialog: Option<RotateDialog>,
}

/// The Rotate dialog: the angle (degrees, clockwise as in the Transform
/// panel), the point to turn about, and its two checkboxes.
#[derive(Clone)]
pub struct RotateDialog {
    pub angle: f64,
    pub origin: P,
    /// Leave the selection as it is and turn a copy of it.
    pub copy: bool,
    /// Show the turn on the canvas as the values change.
    pub preview: bool,
    /// Just opened: the angle field takes the keyboard.
    fresh: bool,
    /// While a preview is showing: the document before it (Cancel puts
    /// it back; OK makes the turn one undo step from it), and the angle
    /// and Copy it shows.
    base: Option<Document>,
    shown: Option<(f64, bool)>,
}

/// Whether a turn of `deg` leaves things where they were (none, or whole
/// turns).
fn no_turn(deg: f64) -> bool {
    let d = deg.rem_euclid(360.0);
    d.min(360.0 - d) < 1e-6
}

impl Studio {
    /// The selection as the Rotate tool knows it: its objects' uids.
    fn rotate_subject(&self) -> Vec<usize> {
        let mut uids: Vec<usize> = self
            .sel_indices()
            .iter()
            .map(|&i| self.doc.shapes[i].uid)
            .collect();
        uids.sort_unstable();
        uids
    }

    /// The point the selection turns about: the one placed for it by hand,
    /// else its center. None with nothing selected.
    pub fn rotate_reference(&self) -> Option<P> {
        let uids = self.rotate_subject();
        if uids.is_empty() {
            return None;
        }
        if let Some((p, of)) = &self.view_tools.rotate.origin
            && *of == uids
        {
            return Some(*p);
        }
        let b = self.selection_bounds();
        (!b.is_empty()).then(|| b.center())
    }

    /// Turn the selection `deg` about `c`, or with `copy` leave it and add
    /// a turned copy: one undo step, which Transform Again repeats about
    /// the same point.
    pub fn rotate_selected_about(&mut self, c: P, deg: f64, copy: bool) -> bool {
        // (A whole turn, or none: nothing to do, and no copy on top of
        // the original to be sewn twice.)
        if no_turn(deg) {
            return false;
        }
        let m = rotate_about(c, deg);
        let done = if copy {
            self.copy_selection_with(m, "Copied")
        } else {
            self.transform_selected(m, "Rotated")
        };
        if done {
            self.keep_last_fixed();
        }
        done
    }

    /// Open the Rotate dialog, to turn the selection about `origin` (the
    /// reference point as it is, if none is given).
    pub fn open_rotate_dialog(&mut self, origin: Option<P>) {
        let Some(origin) = origin.or_else(|| self.rotate_reference()) else {
            return;
        };
        self.close_rotate_dialog();
        self.view_tools.rotate.dialog = Some(RotateDialog {
            angle: 0.0,
            origin,
            copy: false,
            preview: false,
            fresh: true,
            base: None,
            shown: None,
        });
    }

    /// Close the Rotate dialog as Cancel does: a preview on show goes, the
    /// document back as it was.
    pub fn close_rotate_dialog(&mut self) {
        if let Some(d) = self.view_tools.rotate.dialog.take()
            && let Some(base) = d.base
        {
            self.doc = base;
            self.dirty = true;
        }
    }

    /// The Rotate tool's keys: Return opens the window for an exact angle,
    /// Escape calls off a turn under way. True if one was taken.
    pub fn rotate_keys(&mut self, ctx: &egui::Context) -> bool {
        if self.tool != crate::app::Tool::Rotate || ctx.egui_wants_keyboard_input() {
            return false;
        }
        if self.drag.is_some() {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                // (The document as it was; a plan split to show the turn
                // is made again.)
                if let Some(d) = self.drag.take() {
                    self.doc = d.base;
                    self.dirty |= d.split;
                }
                return true;
            }
            return false;
        }
        if self.rotate_reference().is_some()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter))
        {
            self.open_rotate_dialog(None);
            return true;
        }
        false
    }

    /// The Rotate tool's pointer on the canvas: a click places the
    /// reference point, a drag turns the selection about it.
    pub fn rotate_canvas(
        &mut self,
        ctx: &egui::Context,
        resp: &egui::Response,
        rect: Rect,
        mods: DragMods,
    ) {
        let primary = egui::PointerButton::Primary;
        let uids = self.rotate_subject();
        if uids.is_empty() {
            // Nothing to turn yet: a click selects what's under the pointer.
            self.want_cursor(Glyph::Cross(None));
            if resp.clicked()
                && let Some(i) = self.hovered
            {
                self.click_select(i, false, false);
            }
            return;
        }
        if resp.drag_started_by(primary)
            && let Some(c) = self.rotate_reference()
        {
            let start = ctx
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos())
                .unwrap_or(rect.center());
            self.begin_rotate(c, self.view.to_design(rect, start));
        }
        if self.drag.is_some()
            && resp.dragged_by(primary)
            && let Some(pos) = resp.interact_pointer_pos()
        {
            // (The pointer sets an angle, in whole degrees: nothing for
            // it to snap to.)
            let mods = DragMods {
                snap: false,
                ..mods
            };
            self.update_drag(self.view.to_design(rect, pos), mods);
            ctx.request_repaint();
        }
        if resp.drag_stopped() {
            // Option as the button comes up decides whether it's a copy.
            if let Some(d) = &mut self.drag {
                d.copy = mods.alt && d.tool;
            }
            self.end_drag();
        }
        // The cursor, as VectorCraft's: a plain crosshair to place the
        // point with, the curved arrow while turning.
        match (&self.drag, resp.interact_pointer_pos()) {
            (Some(d), Some(hp)) => {
                if let DragMode::Rotate { center } = d.mode {
                    let v = hp - self.view.to_screen(rect, center);
                    self.want_cursor(Glyph::Rotate(v.y.atan2(v.x)));
                }
            }
            _ => self.want_cursor(Glyph::Cross(None)),
        }
        if self.drag.is_some() {
            return;
        }
        // Where a click would put the point (the snap is labelled), and
        // the click itself.
        self.snap_targets(None, false);
        let center = self.selection_bounds().center();
        let at = if resp.clicked() {
            resp.interact_pointer_pos()
        } else {
            resp.hover_pos().filter(|_| !resp.dragged())
        };
        let Some(at) = at else { return };
        let pick = self.snap_pick(self.view.to_design(rect, at), &[center]);
        if resp.clicked() {
            self.view_tools.rotate.origin = Some((pick, uids));
            // Option-click, or a double-click: the exact angle.
            if mods.alt || resp.double_clicked() {
                self.open_rotate_dialog(Some(pick));
            }
        }
    }

    /// The reference point's target, the line out to the pointer while
    /// turning, and the tool's hint.
    pub fn draw_rotate(&self, painter: &egui::Painter, rect: Rect, pointer: Option<Pos2>) {
        let turning = match &self.drag {
            Some(d) => match d.mode {
                DragMode::Rotate { center } => Some(center),
                _ => None,
            },
            None => None,
        };
        let Some(c) = turning.or_else(|| self.rotate_reference()) else {
            self.hud(painter, rect, "Click an object to rotate it");
            return;
        };
        if self.playing {
            return;
        }
        let c = self.view.to_screen(rect, c);
        if let (Some(_), Some(p)) = (turning, pointer) {
            painter.extend(Shape::dashed_line(
                &[c, p],
                Stroke::new(1.0, CYAN),
                4.0,
                3.0,
            ));
        }
        // VectorCraft's target: a circle and a crosshair, a point wide
        // (over a halo, as the Measure tool's line is, to read on any
        // thread).
        for (w, color) in [(3.0, Color32::from_white_alpha(200)), (1.0, CYAN)] {
            let s = Stroke::new(w, color);
            painter.circle_stroke(c, TARGET * 0.6, s);
            painter.line_segment([c - Vec2::new(TARGET, 0.0), c + Vec2::new(TARGET, 0.0)], s);
            painter.line_segment([c - Vec2::new(0.0, TARGET), c + Vec2::new(0.0, TARGET)], s);
        }
        self.hud(
            painter,
            rect,
            "Click to set the reference point · drag to rotate · Shift: 45° steps · Option: a copy · Option-click for an exact angle",
        );
    }

    /// The Rotate dialog, laid out as VectorCraft's: the angle, Copy and
    /// Preview, then Cancel and OK.
    pub fn rotate_window(&mut self, ctx: &egui::Context) {
        // A point placed by hand goes when the selection it was placed
        // for does.
        if let Some((_, of)) = &self.view_tools.rotate.origin
            && *of != self.rotate_subject()
        {
            self.view_tools.rotate.origin = None;
        }
        if self.view_tools.rotate.dialog.is_none() {
            return;
        }
        if self.sel_indices().is_empty() {
            self.close_rotate_dialog();
            return;
        }
        let Some(mut d) = self.view_tools.rotate.dialog.take() else {
            return;
        };
        let (mut ok, mut cancel) = (false, false);
        // Modal, like the other dialogs that preview into the document:
        // nothing else may edit it until OK or Cancel. No dimming, and
        // near the top: the preview is the point.
        use crate::dialog;
        let id = egui::Id::unique("rotate-dialog");
        dialog::show(ctx, id, dialog::Place::Top, |ui| {
            dialog::heading(ui, "Rotate");
            // (Its label at the dialog's left edge, over the checkboxes,
            // as VectorCraft's field grid has it.)
            ui.horizontal(|ui| {
                let secondary = crate::theme::pal_ui(ui).secondary;
                let l = ui.label(egui::RichText::new("Angle").color(secondary));
                let r = ui
                    .add_sized(
                        [FIELD_W, 22.0],
                        egui::DragValue::new(&mut d.angle)
                            .speed(1.0)
                            .range(-360.0..=360.0)
                            .max_decimals(3)
                            .suffix("°"),
                    )
                    .labelled_by(l.id)
                    .on_hover_text("A positive angle turns clockwise, as in the Transform panel");
                // Its first field has the keyboard as it opens, as
                // VectorCraft's does: type the angle, Return.
                if d.fresh {
                    r.request_focus();
                    d.fresh = false;
                }
            });
            ui.add_space(6.0);
            ui.checkbox(&mut d.copy, "Copy (make a transformed copy)");
            ui.add_space(8.0 - ui.spacing().item_spacing.y);
            ui.checkbox(&mut d.preview, "Preview");
            dialog::buttons(ui, |ui| {
                ok = dialog::primary(ui, "OK", true).clicked()
                    || ui.input(|i| i.key_pressed(Key::Enter));
                cancel = dialog::secondary(ui, "Cancel").clicked()
                    || ui.input(|i| i.key_pressed(Key::Escape));
            });
        });
        if ok || cancel {
            // The document as it was; OK then makes the turn, one undo step.
            if let Some(base) = d.base.take() {
                self.doc = base;
                self.dirty = true;
            }
            if ok {
                self.rotate_selected_about(d.origin, d.angle, d.copy);
            }
            return;
        }
        // Preview: the turn made on the document as it was.
        let want = (d.preview && !no_turn(d.angle)).then_some((d.angle, d.copy));
        if want != d.shown {
            if let Some(base) = &d.base {
                self.doc = base.clone();
            }
            match want {
                Some((angle, copy)) => {
                    let base = d.base.get_or_insert_with(|| self.doc.clone());
                    let idx = self.sel_indices();
                    let m = rotate_about(d.origin, angle);
                    let mut doc = base.clone();
                    let turned = if copy {
                        doc.duplicate(&idx, P::default())
                            .and_then(|made| doc.transform(&made, m))
                    } else {
                        doc.transform(&idx, m)
                    };
                    // (One that can't be made isn't shown; OK says why.)
                    if turned.is_ok() {
                        self.doc = doc;
                    }
                }
                None => d.base = None,
            }
            self.dirty = true;
            d.shown = want;
        }
        self.view_tools.rotate.dialog = Some(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Tool, shape_bounds};
    use eframe::egui::Modifiers;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use zigzag::document::Document;
    use zigzag::geom::{Bounds, affine_apply};

    /// A 20 × 10 mm block up and left of the hoop's center, and a small
    /// square down and right of it.
    const ART: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
      <rect id="a" x="10" y="10" width="20" height="10" fill="#c00"/>
      <rect id="b" x="70" y="70" width="10" height="10" fill="#080"/>
    </svg>"##;

    fn studio() -> Harness<'static, Studio> {
        studio_stepping(0.25)
    }

    /// (A frame every `dt` seconds on egui's clock, which is what it
    /// counts a double-click by.)
    fn studio_stepping(dt: f32) -> Harness<'static, Studio> {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .with_step_dt(dt)
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        h.state_mut()
            .set_document(Document::from_svg(ART, "t", None).unwrap());
        h.run_steps(4);
        h
    }

    /// Where the object called `name` is now. (Not by its element's id,
    /// which is rewritten as it's edited.)
    fn index_of(h: &Harness<'_, Studio>, name: &str) -> usize {
        h.state()
            .names
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("{name} among {:?}", h.state().names))
    }

    fn bounds_of(h: &Harness<'_, Studio>, i: usize) -> Bounds {
        shape_bounds(&h.state().doc.shapes[i])
    }

    /// Select the object with this id and take up the Rotate tool.
    fn rotating(h: &mut Harness<'_, Studio>, id: &str) -> Bounds {
        let i = index_of(h, id);
        h.state_mut().set_selection(&[i]);
        h.state_mut().set_tool(Tool::Rotate);
        h.run_steps(2);
        h.state().selection_bounds()
    }

    fn screen(h: &Harness<'_, Studio>, p: P) -> Pos2 {
        h.state().view.to_screen(h.state().canvas_rect, p)
    }

    fn click(h: &mut Harness<'_, Studio>, at: Pos2) {
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(2);
    }

    fn held(h: &mut Harness<'_, Studio>, m: Modifiers) {
        h.event(egui::Event::ModifiersChanged(m));
    }

    /// Press at `from` and move to `to`, the button still down.
    fn press_and_move(h: &mut Harness<'_, Studio>, from: Pos2, to: Pos2) {
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        for k in 1..=4 {
            h.hover_at(from + (to - from) * (k as f32 / 4.0));
            h.run_steps(1);
        }
    }

    fn near(a: P, b: P) -> bool {
        (a - b).len() < 0.5
    }

    fn close(b: &Bounds, min: P, max: P) -> bool {
        near(b.min, min) && near(b.max, max)
    }

    #[test]
    fn a_click_places_the_point_and_a_drag_turns_the_selection_about_it() {
        let mut h = studio();
        let b = rotating(&mut h, "a");
        // Until a click places it, the point is the selection's center.
        assert_eq!(h.state().rotate_reference(), Some(b.center()));
        // A click a few points off the block's bottom-right corner lands
        // on the corner.
        let c = b.max;
        let at = screen(&h, c) + Vec2::new(3.0, -2.0);
        click(&mut h, at);
        let at = h.state().rotate_reference().unwrap();
        assert!((at - c).len() < 1e-6, "snapped to the corner: {at:?}");
        assert_eq!(h.state().undo.len(), 0, "placing the point is no edit");

        // A drag from beside the corner to above it: a quarter turn
        // clockwise about the corner, shown as it goes.
        let from = screen(&h, c + P::new(-150.0, 0.0));
        let to = screen(&h, c + P::new(0.0, -150.0));
        press_and_move(&mut h, from, to);
        let s = h.state();
        let m = s.drag.as_ref().expect("turning").m;
        assert!(near(affine_apply(m, c), c), "about the corner");
        assert_eq!(s.drag_label().as_deref(), Some("90°"));
        let (on, shown) = s.moving_blocks().unwrap();
        assert_eq!(shown, m, "the stitches are drawn through the turn");
        assert!(on.iter().any(|x| *x));
        assert_eq!(s.undo.len(), 0, "nothing written until it's let go");
        h.drop_at(to);
        h.run_steps(3);

        let a = index_of(&h, "a");
        let turned = bounds_of(&h, a);
        assert!(
            close(&turned, c + P::new(0.0, -200.0), c + P::new(100.0, 0.0)),
            "{turned:?} about {c:?}"
        );
        assert_eq!(h.state().undo.len(), 1, "one undo step");
        assert!(h.state().status.starts_with("Rotated"));
        assert!(h.state().drag.is_none());
        assert!(
            h.state()
                .doc
                .to_svg()
                .unwrap()
                .contains("transform=\"matrix("),
            "written into the SVG"
        );
        // The point stays for the selection it was placed for, and ⌘D
        // turns about it again.
        assert!(near(h.state().rotate_reference().unwrap(), c));
        h.key_press_modifiers(Modifiers::COMMAND, Key::D);
        h.run_steps(3);
        let again = bounds_of(&h, index_of(&h, "a"));
        assert!(
            close(&again, c, c + P::new(200.0, 100.0)),
            "half a turn about the corner: {again:?}"
        );
        assert_eq!(h.state().undo.len(), 2);
        for _ in 0..2 {
            h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
            h.run_steps(3);
        }
        assert!(close(&bounds_of(&h, index_of(&h, "a")), b.min, b.max));
    }

    #[test]
    fn shift_turns_in_45_degree_steps_and_escape_calls_a_turn_off() {
        let mut h = studio();
        let b = rotating(&mut h, "a");
        let c = b.center();
        let from = screen(&h, c + P::new(300.0, 0.0));
        let to = screen(&h, c + P::new(300.0, 240.0)); // about 38.7°
        press_and_move(&mut h, from, to);
        let angle = |h: &Harness<'_, Studio>| {
            let m = h.state().drag.as_ref().unwrap().m;
            m[1].atan2(m[0]).to_degrees()
        };
        assert!(
            (angle(&h) - 39.0).abs() < 1e-6,
            "whole degrees: {}",
            angle(&h)
        );
        held(&mut h, Modifiers::SHIFT);
        h.hover_at(to + Vec2::new(1.0, 0.0));
        h.run_steps(1);
        assert!((angle(&h) - 45.0).abs() < 1e-6, "{}", angle(&h));
        // Escape: back as it was, the tool and the selection kept.
        held(&mut h, Modifiers::NONE);
        h.key_press(Key::Escape);
        h.run_steps(1);
        h.drop_at(to);
        h.run_steps(3);
        assert_eq!(h.state().undo.len(), 0);
        assert_eq!(h.state().tool, Tool::Rotate);
        assert_eq!(h.state().sel_indices(), vec![index_of(&h, "a")]);
        assert!(close(&bounds_of(&h, index_of(&h, "a")), b.min, b.max));
        assert_eq!(h.state().design.per_shape.len(), h.state().doc.shapes.len());
    }

    #[test]
    fn option_at_release_leaves_a_turned_copy_and_transform_again_goes_round() {
        let mut h = studio();
        let n = h.state().doc.shapes.len();
        let b = rotating(&mut h, "a");
        // The point: the hoop's center (a click near it lands on it).
        let o = P::new(0.0, 0.0);
        let at = screen(&h, o) + Vec2::new(3.0, 2.0);
        click(&mut h, at);
        assert!((h.state().rotate_reference().unwrap() - o).len() < 1e-6);
        // A sixth of a turn, Option down as the button comes up.
        let spoke = |deg: f64| {
            let (s, c) = deg.to_radians().sin_cos();
            o + P::new(200.0 * c, 200.0 * s)
        };
        let (from, to) = (screen(&h, spoke(180.0)), screen(&h, spoke(240.0)));
        press_and_move(&mut h, from, to);
        held(&mut h, Modifiers::ALT);
        h.drop_at(to);
        held(&mut h, Modifiers::NONE);
        h.run_steps(3);

        assert_eq!(h.state().doc.shapes.len(), n + 1, "{}", h.state().status);
        assert_eq!(h.state().undo.len(), 1, "one undo step");
        assert!(h.state().status.starts_with("Copied"));
        let orig = index_of(&h, "a");
        assert!(
            close(&bounds_of(&h, orig), b.min, b.max),
            "the original stays where it was"
        );
        let copy = h.state().sel_index().unwrap();
        assert_ne!(copy, orig, "the copy is selected");
        let step = |k: f64| affine_apply(rotate_about(o, 60.0 * k), b.center());
        let at = bounds_of(&h, copy).center();
        assert!(near(at, step(1.0)), "{at:?} vs {:?}", step(1.0));
        assert_eq!(h.state().design.per_shape.len(), n + 1, "and it's sewn");

        // ⌘D: another copy, a sixth of a turn further round the same
        // point, until the ring is full.
        for k in 2..=5 {
            h.key_press_modifiers(Modifiers::COMMAND, Key::D);
            h.run_steps(3);
            assert_eq!(h.state().doc.shapes.len(), n + k, "{}", h.state().status);
            let at = bounds_of(&h, h.state().sel_index().unwrap()).center();
            assert!(near(at, step(k as f64)), "copy {k}: {at:?}");
        }
        assert_eq!(h.state().undo.len(), 5);
        assert!(close(&bounds_of(&h, index_of(&h, "a")), b.min, b.max));
    }

    #[test]
    fn option_click_opens_the_window_for_an_exact_angle() {
        let mut h = studio();
        let n = h.state().doc.shapes.len();
        let b = rotating(&mut h, "a");
        // Option-click by the other square's corner: the point lands on
        // it and the window opens.
        let corner = bounds_of(&h, index_of(&h, "b")).min;
        held(&mut h, Modifiers::ALT);
        let at = screen(&h, corner) + Vec2::new(-2.0, 3.0);
        click(&mut h, at);
        held(&mut h, Modifiers::NONE);
        h.run_steps(2);
        let d = h.state().view_tools.rotate.dialog.clone().expect("open");
        assert!((d.origin - corner).len() < 1e-6, "{:?}", d.origin);
        assert_eq!((d.angle, d.copy, d.preview), (0.0, false, false));
        assert_eq!(h.state().doc.shapes.len(), n, "a click, not a copy");
        // The angle field has the keyboard: type it, Return.
        h.event(egui::Event::Text("90".into()));
        h.run_steps(1);
        h.key_press(Key::Enter);
        h.run_steps(3);
        assert!(h.state().view_tools.rotate.dialog.is_none());
        assert_eq!(h.state().undo.len(), 1, "{}", h.state().status);
        let want = |p: P| affine_apply(rotate_about(corner, 90.0), p);
        let turned = bounds_of(&h, index_of(&h, "a"));
        assert!(
            near(turned.center(), want(b.center())),
            "{turned:?} about {corner:?}"
        );
        assert!((turned.width() - b.height()).abs() < 0.5, "on its side");

        // Return opens it again, about the same point; with Copy ticked
        // OK leaves the block and adds a turned one, and ⌘D carries on
        // round.
        let b = h.state().selection_bounds();
        h.key_press(Key::Enter);
        h.run_steps(3);
        let d = h.state().view_tools.rotate.dialog.clone().expect("open");
        assert!((d.origin - corner).len() < 1e-6);
        h.state_mut()
            .view_tools
            .rotate
            .dialog
            .as_mut()
            .unwrap()
            .angle = 45.0;
        h.run_steps(1);
        h.get_by_label("Copy (make a transformed copy)").click();
        h.run_steps(2);
        assert_eq!(h.state().doc.shapes.len(), n, "not until OK");
        h.get_by_label("OK").click();
        h.run_steps(3);
        assert!(h.state().view_tools.rotate.dialog.is_none());
        assert_eq!(h.state().doc.shapes.len(), n + 1, "{}", h.state().status);
        assert_eq!(h.state().undo.len(), 2);
        let step = |k: f64| affine_apply(rotate_about(corner, 45.0 * k), b.center());
        assert!(close(&bounds_of(&h, index_of(&h, "a")), b.min, b.max));
        let at = bounds_of(&h, h.state().sel_index().unwrap()).center();
        assert!(near(at, step(1.0)), "{at:?}");
        h.key_press_modifiers(Modifiers::COMMAND, Key::D);
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), n + 2);
        let at = bounds_of(&h, h.state().sel_index().unwrap()).center();
        assert!(near(at, step(2.0)), "{at:?}");
    }

    #[test]
    fn the_menu_opens_the_window_and_escape_or_no_angle_changes_nothing() {
        let mut h = studio();
        let a = index_of(&h, "a");
        h.state_mut().set_selection(&[a]);
        h.run_steps(1);
        let b = h.state().selection_bounds();
        // Object › Rotate…, with any tool: about the selection's center.
        h.state_mut()
            .run_action(crate::commands::Action::RotateDialog);
        h.run_steps(2);
        let d = h.state().view_tools.rotate.dialog.clone().expect("open");
        assert!(near(d.origin, b.center()));
        // As VectorCraft's: the angle, Copy, Preview, then Cancel and OK.
        for label in ["Copy (make a transformed copy)", "Preview", "Cancel", "OK"] {
            h.get_by_label(label);
        }
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert!(h.state().view_tools.rotate.dialog.is_none());
        assert_eq!(h.state().sel_indices(), vec![a], "only the dialog closed");
        // OK with no angle: nothing to undo. Nor a copy on top.
        for copy in [false, true] {
            h.state_mut()
                .run_action(crate::commands::Action::RotateDialog);
            h.run_steps(2);
            if copy {
                h.get_by_label("Copy (make a transformed copy)").click();
                h.run_steps(1);
            }
            h.get_by_label("OK").click();
            h.run_steps(3);
            assert!(h.state().view_tools.rotate.dialog.is_none());
        }
        assert_eq!(h.state().undo.len(), 0);
        assert_eq!(h.state().doc.shapes.len(), 2);
        // A quarter turn in place.
        h.state_mut()
            .run_action(crate::commands::Action::RotateDialog);
        h.run_steps(2);
        h.state_mut()
            .view_tools
            .rotate
            .dialog
            .as_mut()
            .unwrap()
            .angle = -90.0;
        h.get_by_label("OK").click();
        h.run_steps(3);
        let turned = h.state().selection_bounds();
        assert!(near(turned.center(), b.center()));
        assert!((turned.width() - b.height()).abs() < 0.5);
        assert_eq!(h.state().undo.len(), 1);
    }

    #[test]
    fn a_double_click_places_the_point_and_opens_the_window() {
        let mut h = studio_stepping(1.0 / 60.0);
        let b = rotating(&mut h, "a");
        let at = screen(&h, b.min) + Vec2::new(2.0, 2.0);
        h.hover_at(at);
        h.run_steps(30);
        for k in 0..2 {
            assert!(h.state().view_tools.rotate.dialog.is_none(), "click {k}");
            h.drag_at(at);
            h.run_steps(1);
            h.drop_at(at);
            h.run_steps(1);
        }
        h.run_steps(2);
        let d = h.state().view_tools.rotate.dialog.clone().expect("open");
        assert!((d.origin - b.min).len() < 1e-6, "{:?}", d.origin);
        assert_eq!(h.state().undo.len(), 0);
    }

    #[test]
    fn preview_shows_the_turn_and_only_ok_keeps_it() {
        let mut h = studio();
        let n = h.state().doc.shapes.len();
        let b = rotating(&mut h, "a");
        let turn = |deg: f64| affine_apply(rotate_about(b.max, deg), b.center());
        let open = |h: &mut Harness<'_, Studio>, angle: f64| {
            h.state_mut().open_rotate_dialog(Some(b.max));
            h.state_mut()
                .view_tools
                .rotate
                .dialog
                .as_mut()
                .unwrap()
                .angle = angle;
            h.run_steps(2);
        };
        let block = |h: &Harness<'_, Studio>| bounds_of(h, index_of(h, "a")).center();

        // Without Preview nothing shows until OK.
        open(&mut h, 90.0);
        assert!(near(block(&h), b.center()));
        // Preview: the turn shows on the canvas, stitches and all, and
        // follows the angle; it's no edit yet.
        h.get_by_label("Preview").click();
        h.run_steps(3);
        assert!(near(block(&h), turn(90.0)), "{:?}", block(&h));
        assert!(!h.state().dirty, "the stitches are planned for it");
        h.state_mut()
            .view_tools
            .rotate
            .dialog
            .as_mut()
            .unwrap()
            .angle = 30.0;
        h.run_steps(3);
        assert!(near(block(&h), turn(30.0)), "{:?}", block(&h));
        // Copy as well: the block back where it was, a turned copy beside.
        h.get_by_label("Copy (make a transformed copy)").click();
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), n + 1);
        assert!(near(block(&h), b.center()));
        assert_eq!(h.state().undo.len(), 0, "a preview is no edit");
        assert!(!h.state().modified);
        // Cancel: as it was.
        h.get_by_label("Cancel").click();
        h.run_steps(3);
        assert!(h.state().view_tools.rotate.dialog.is_none());
        assert_eq!(h.state().doc.shapes.len(), n);
        assert!(near(block(&h), b.center()));
        assert_eq!(h.state().undo.len(), 0);
        assert_eq!(h.state().design.per_shape.len(), n);

        // Escape does the same; unticking Preview takes the turn off show.
        open(&mut h, 90.0);
        h.get_by_label("Preview").click();
        h.run_steps(3);
        assert!(near(block(&h), turn(90.0)));
        h.get_by_label("Preview").click();
        h.run_steps(3);
        assert!(near(block(&h), b.center()), "preview off");
        h.get_by_label("Preview").click();
        h.run_steps(3);
        h.key_press(Key::Escape);
        h.run_steps(3);
        assert!(h.state().view_tools.rotate.dialog.is_none());
        assert!(near(block(&h), b.center()));
        assert_eq!(h.state().undo.len(), 0);

        // OK with the preview showing: the turn, as one undo step back to
        // the document as it was.
        open(&mut h, 90.0);
        h.get_by_label("Preview").click();
        h.run_steps(3);
        h.get_by_label("OK").click();
        h.run_steps(3);
        assert!(near(block(&h), turn(90.0)));
        assert_eq!(h.state().undo.len(), 1);
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run_steps(3);
        assert!(near(block(&h), b.center()));
        assert_eq!(h.state().doc.shapes.len(), n);
    }

    #[test]
    fn a_double_click_on_the_tool_opens_its_dialog() {
        let mut h = studio_stepping(1.0 / 60.0);
        let a = index_of(&h, "a");
        h.state_mut().set_selection(&[a]);
        h.run_steps(30);
        let b = h.state().selection_bounds();
        let button = h
            .get_by_role_and_label(egui::accesskit::Role::Button, "Rotate")
            .rect()
            .center();
        h.hover_at(button);
        h.run_steps(2);
        for _ in 0..2 {
            h.drag_at(button);
            h.run_steps(1);
            h.drop_at(button);
            h.run_steps(1);
        }
        h.run_steps(2);
        assert_eq!(h.state().tool, Tool::Rotate);
        let d = h.state().view_tools.rotate.dialog.clone().expect("open");
        assert!(near(d.origin, b.center()), "about the selection's center");
    }

    #[test]
    fn a_placed_point_goes_when_the_selection_changes() {
        let mut h = studio();
        let b = rotating(&mut h, "a");
        let at = screen(&h, b.min) + Vec2::new(2.0, 2.0);
        click(&mut h, at);
        assert!((h.state().rotate_reference().unwrap() - b.min).len() < 1e-6);
        let other = index_of(&h, "b");
        h.state_mut().set_selection(&[other]);
        h.run_steps(1);
        let ob = h.state().selection_bounds();
        assert_eq!(h.state().rotate_reference(), Some(ob.center()));
        // And it doesn't come back with the selection.
        let a = index_of(&h, "a");
        h.state_mut().set_selection(&[a]);
        h.run_steps(1);
        assert_eq!(h.state().rotate_reference(), Some(b.center()));
        assert!(h.state().view_tools.rotate.origin.is_none());
    }

    #[test]
    fn the_tool_strip_and_shift_r_pick_the_tool_and_a_click_selects() {
        let mut h = studio();
        // Shift+R: R alone is still the Run stitch.
        let a = index_of(&h, "a");
        h.state_mut().set_selection(&[a]);
        h.run_steps(1);
        let kind = h.state().doc.shapes[a].params.kind;
        h.key_press_modifiers(Modifiers::SHIFT, Key::R);
        h.run_steps(2);
        assert_eq!(h.state().tool, Tool::Rotate);
        assert_eq!(
            h.state().doc.shapes[a].params.kind,
            kind,
            "not a stitch type"
        );
        assert_eq!(h.state().undo.len(), 0);
        // The tool strip's button.
        h.state_mut().set_tool(Tool::Select);
        h.state_mut().set_selection(&[]);
        h.run_steps(1);
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Rotate")
            .click();
        h.run_steps(2);
        assert_eq!(h.state().tool, Tool::Rotate);
        assert_eq!(h.state().rotate_reference(), None, "nothing to turn");
        // With nothing selected, a click selects what's under the pointer
        // (and places no point).
        let at = screen(&h, bounds_of(&h, a).center());
        click(&mut h, at);
        assert_eq!(h.state().sel_indices(), vec![a]);
        assert!(h.state().view_tools.rotate.origin.is_none());
        assert_eq!(h.state().tool, Tool::Rotate);
    }

    #[test]
    fn lettering_and_live_corners_come_through_a_turn_and_a_turned_copy() {
        let mut h = studio();
        // Round the block's corners (live), and add some text.
        let a = index_of(&h, "a");
        h.state_mut()
            .structural(|d| {
                d.round_corners(&[a], 30.0, Default::default(), &[])?;
                Ok(())
            })
            .unwrap();
        let spec = zigzag::document::TextSpec {
            text: "Hi".into(),
            ..Default::default()
        };
        let text = h.state_mut().add_lettering(&spec, (10.0, -20.0)).unwrap();
        h.run_steps(3);
        let a = index_of(&h, "a");
        let radius = h.state().doc.corner_radius(&[a]).expect("live corners");
        let n = h.state().doc.shapes.len();
        let o = P::new(120.0, 80.0);

        // Turned about a point of their own choosing.
        h.state_mut().set_selection(&[a, text]);
        assert!(h.state_mut().rotate_selected_about(o, 30.0, false));
        h.run_steps(3);
        let s = h.state();
        let sel = s.sel_indices();
        assert_eq!(sel.len(), 2);
        assert_eq!(s.doc.shapes.len(), n);
        for &i in &sel {
            match s.doc.lettering(i) {
                Some(t) => assert_eq!(t.text, "Hi", "still text"),
                None => {
                    let r = s.doc.corner_radius(&[i]).expect("corners still live");
                    assert!((r - radius).abs() < 0.5, "{r} vs {radius}");
                }
            }
        }
        assert_eq!(
            sel.iter()
                .filter(|&&i| s.doc.lettering(i).is_some())
                .count(),
            1
        );

        // And a turned copy of the two: text and live corners again.
        assert!(h.state_mut().rotate_selected_about(o, 120.0, true));
        h.run_steps(3);
        let s = h.state();
        assert_eq!(s.doc.shapes.len(), n + 2, "{}", s.status);
        let copies = s.sel_indices();
        assert_eq!(copies.len(), 2);
        assert!(
            copies.iter().all(|i| !sel.contains(i)),
            "the copies are selected"
        );
        let (mut texts, mut rounded) = (0, 0);
        for &i in &copies {
            match s.doc.lettering(i) {
                Some(t) => {
                    assert_eq!(t.text, "Hi");
                    texts += 1;
                }
                None => {
                    let r = s
                        .doc
                        .corner_radius(&[i])
                        .expect("the copy's corners are live");
                    assert!((r - radius).abs() < 0.5, "{r} vs {radius}");
                    rounded += 1;
                }
            }
        }
        assert_eq!((texts, rounded), (1, 1));
        // It all reads back from the file the same.
        let saved = Document::from_svg(&s.doc.to_svg().unwrap(), "t", None).unwrap();
        assert_eq!(saved.shapes.len(), n + 2);
        let lettered = (0..saved.shapes.len())
            .filter(|&i| saved.lettering(i).is_some())
            .count();
        assert_eq!(lettered, 2);
    }
}
