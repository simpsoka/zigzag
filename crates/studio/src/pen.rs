//! The Pen tool, the way Illustrator's works (after VectorCraft's `pen.rs`).
//!
//! - Click for a corner point; click-drag for a smooth point (the handle you
//!   pull leads the curve on; the other mirrors it). Option while dragging
//!   moves only the leading handle. Shift keeps new points and handles at 45°.
//!   Hold Space while dragging to move the point you're placing.
//! - Click the last point to make it a corner (drag from it for a new handle).
//! - Click the first point to close the shape (drag there to shape the
//!   closing curve).
//! - While drawing, Option lends the Anchor Point tool to the path being
//!   drawn: drag a segment to bend it, a handle to move it alone (click its
//!   end to take it in), drag from a point for new smooth handles (click a
//!   smooth point for a corner). Cmd lends Direct Selection: drag a point,
//!   handles and all, or a handle (a smooth point stays smooth); Cmd-click
//!   elsewhere ends the path.
//! - Enter, Esc or another tool ends an open path.
//! - With a path selected: click an open end to continue it, click the
//!   outline to add a point, a point to delete it; Option-click a point to
//!   turn it corner ↔ smooth, Option-drag from it to pull new handles,
//!   Option-drag a segment to bend it or a handle to move it alone.
//!
//! A hint by the cursor says what a click will do: * new path, + add point,
//! − delete point, o close, / continue, ^ convert, & join.

use crate::app::{Studio, Tool};
use crate::view_tools::PenMode;
use eframe::egui::{self, Color32, Key, Pos2, Rect, Stroke, Vec2};
use zigzag::bezier::{self, Anchor, AnchorRef, SubPath};
use zigzag::geom::P;
use zigzag::params::Kind;

/// What the pen's mouse press is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PenPress {
    /// A new point: dragging pulls out its handles.
    Place,
    /// On the first point: closing the path.
    Close,
    /// On the last point: click makes it a corner, drag pulls a new handle.
    Last,
    /// Option on a selected path's point: convert it.
    Convert(AnchorRef),
    /// On another open path's end: joining onto it.
    Join(crate::snap::End),
    /// The path being drawn: a handle end (`smooth`: the other turns with
    /// it, as Direct Selection; else it moves alone, as Anchor Point).
    DrawnHandle { k: usize, out: bool, smooth: bool },
    /// Cmd on a point of the path being drawn: moving it.
    DrawnAnchor(usize),
    /// Option on a point of the path being drawn: pulling smooth handles.
    DrawnPull(usize),
    /// Option on a segment of the path being drawn: bending it.
    DrawnBend { a: usize, t: f64 },
    /// Option on the selected path's handle or segment (Anchor Point tool).
    Reshape,
}

/// What's under the pointer on the path being drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drawn {
    Handle(usize, bool),
    Anchor(usize),
    Segment(usize, f64),
}

/// What a click at a spot would do.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    New,
    Next,
    Close,
    Last,
    /// Continue the selected path from an open end: (subpath, at its start).
    Continue(usize, bool),
    /// Join the path being drawn onto an open path's end.
    Join(crate::snap::End),
    Add,
    Delete,
    Convert(AnchorRef),
}

impl Spot {
    fn hint(self) -> &'static str {
        match self {
            Spot::New => "*",
            Spot::Next => "",
            Spot::Close => "o",
            Spot::Last => "^",
            Spot::Continue(..) => "/",
            Spot::Join(_) => "&",
            Spot::Add => "+",
            Spot::Delete => "-",
            Spot::Convert(_) => "^",
        }
    }
}

const ACCENT: Color32 = Color32::from_rgb(40, 140, 255);

impl Studio {
    fn pen_spot(&mut self, q: P, tol: f64, alt: bool) -> Spot {
        let pen = &self.view_tools.pen;
        let n = pen.len();
        let near = |p: P| (p - q).len() <= tol;
        if n >= 2 && near(pen[0].p) {
            return Spot::Close;
        }
        if n >= 1 && near(pen[n - 1].p) {
            return Spot::Last;
        }
        if n > 0 {
            // Another open path's end: join onto it (not the one being continued).
            let cont = self.view_tools.pen_continue;
            let end = self.snap.targets.as_ref().and_then(|t| {
                t.ends
                    .iter()
                    .filter(|e| near(e.p) && cont != Some((e.uid, e.s)))
                    .min_by(|a, b| (a.p - q).len().total_cmp(&(b.p - q).len()))
                    .copied()
            });
            return match end {
                Some(e) => Spot::Join(e),
                None => Spot::Next,
            };
        }
        if let Some(path) = self.editable_path() {
            for (s, sp) in path.iter().enumerate() {
                if sp.closed || sp.anchors.len() < 2 {
                    continue;
                }
                if near(sp.anchors[0].p) && !alt {
                    return Spot::Continue(s, true);
                }
                if near(sp.anchors[sp.anchors.len() - 1].p) && !alt {
                    return Spot::Continue(s, false);
                }
            }
            if let Some(a) = bezier::hit_anchor(&path, q, tol) {
                return if alt { Spot::Convert(a) } else { Spot::Delete };
            }
            if !alt && bezier::hit_segment(&path, q, tol).is_some() {
                return Spot::Add;
            }
        }
        Spot::New
    }

    /// The handles of the path being drawn that show (and can be grabbed):
    /// the last point's, and the one leading out of the point before it;
    /// every one while Option or Cmd is held.
    fn drawn_handles(&self, all: bool) -> Vec<(usize, bool)> {
        let pen = &self.view_tools.pen;
        let n = pen.len();
        let mut v = Vec::new();
        for (k, a) in pen.iter().enumerate() {
            let near_end = k + 1 == n;
            if all || near_end {
                v.push((k, false));
            }
            if all || near_end || k + 2 == n {
                v.push((k, true));
            }
            let _ = a;
        }
        v.retain(|&(k, out)| {
            let a = pen[k];
            (if out { a.h_out } else { a.h_in }) != a.p
        });
        v
    }

    /// The handle, point or segment of the path being drawn under `q`.
    fn drawn_hit(&self, q: P, tol: f64, all: bool) -> Option<Drawn> {
        let pen = &self.view_tools.pen;
        for (k, out) in self.drawn_handles(all) {
            let h = if out { pen[k].h_out } else { pen[k].h_in };
            if (h - q).len() <= tol {
                return Some(Drawn::Handle(k, out));
            }
        }
        let path = vec![SubPath {
            anchors: pen.clone(),
            closed: false,
        }];
        if let Some((_, k)) = bezier::hit_anchor(&path, q, tol) {
            return Some(Drawn::Anchor(k));
        }
        bezier::hit_segment(&path, q, tol).map(|(_, a, t)| Drawn::Segment(a, t))
    }

    /// Canvas input and drawing for the Pen.
    pub fn pen_canvas(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        self.want_cursor(crate::cursors::Glyph::Pen(""));
        self.snap_targets(None, false);
        // The path being drawn is a target too (line up with its points).
        let own: Vec<P> = self.view_tools.pen.iter().map(|a| a.p).collect();
        let tol = 6.0 * 10.0 / self.view.scale as f64;
        let manual = self.view_tools.pen_mode == PenMode::Manual;
        let (pressed, down, released, alt, cmd, shift, space, press_pos, pos) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.modifiers.alt,
                i.modifiers.command,
                i.modifiers.shift,
                i.key_down(Key::Space),
                i.pointer.press_origin(),
                i.pointer.interact_pos(),
            )
        });
        let drawing = !self.view_tools.pen.is_empty();
        // Option shows the selected path's every handle (to grab one).
        self.path_edit.show_all = alt && !drawing;

        // ---- Press. (A quick tap's press and release can come in one
        // frame, its press position already gone: use where it is.)
        if pressed
            && resp.hovered()
            && let Some(at) = press_pos.or(pos)
        {
            let raw = self.view.to_design(rect, at);
            let last = self.view_tools.pen.last().map(|a| a.p);
            let q = self.snap_point(raw, last, shift, &own);
            self.view_tools.pen_moved = false;
            self.view_tools.pen_pointer = Some(raw);
            self.view_tools.pen_press_at = raw;
            self.view_tools.pen_base = self.view_tools.pen.clone();
            let n = self.view_tools.pen.len();
            let first_near = n >= 2 && (self.view_tools.pen[0].p - raw).len() <= tol;
            let hit = if drawing {
                self.drawn_hit(raw, tol, alt || cmd)
            } else {
                None
            };
            let press = match (hit, cmd, alt) {
                // Cmd: Direct Selection on the path being drawn.
                (Some(Drawn::Handle(k, out)), true, _) => {
                    let smooth = self.view_tools.pen[k].is_smooth();
                    Some(PenPress::DrawnHandle { k, out, smooth })
                }
                (Some(Drawn::Anchor(k)), true, _) => Some(PenPress::DrawnAnchor(k)),
                // Option: the Anchor Point tool (closing comes first).
                _ if first_near && !cmd => None,
                (Some(Drawn::Handle(k, out)), false, true) => Some(PenPress::DrawnHandle {
                    k,
                    out,
                    smooth: false,
                }),
                (Some(Drawn::Anchor(k)), false, true) => Some(PenPress::DrawnPull(k)),
                (Some(Drawn::Segment(a, t)), false, true) => Some(PenPress::DrawnBend { a, t }),
                _ => None,
            };
            if let Some(p) = press {
                self.view_tools.pen_press = Some(p);
            } else if !drawing && alt && self.alt_reshape_begin(raw, tol) {
                self.view_tools.pen_press = Some(PenPress::Reshape);
            } else {
                let spot = self.pen_spot(raw, tol, alt);
                match spot {
                    Spot::Close => self.view_tools.pen_press = Some(PenPress::Close),
                    Spot::Last => self.view_tools.pen_press = Some(PenPress::Last),
                    Spot::Next if cmd => {
                        // Cmd-click away from the path ends it.
                        if n >= 2 {
                            self.finish_pen(false);
                        }
                    }
                    Spot::Continue(s, at_start) => {
                        let path = self.editable_path().unwrap_or_default();
                        let sp = if at_start {
                            bezier::reversed(&path[s])
                        } else {
                            path[s].clone()
                        };
                        let uid = self.sel_index().map(|i| self.doc.shapes[i].uid);
                        self.view_tools.pen = sp.anchors;
                        self.view_tools.pen_continue = uid.map(|u| (u, s));
                        self.view_tools.pen_press = Some(PenPress::Last);
                    }
                    Spot::Convert(a) => self.view_tools.pen_press = Some(PenPress::Convert(a)),
                    Spot::Join(e) => {
                        self.view_tools.pen.push(Anchor::corner(e.p));
                        self.view_tools.pen_press = Some(PenPress::Join(e));
                    }
                    Spot::Add | Spot::Delete => {
                        self.pen_edits_selected(raw, tol, false);
                    }
                    Spot::New | Spot::Next => {
                        self.view_tools.pen.push(Anchor::corner(q));
                        self.view_tools.pen_press = Some(PenPress::Place);
                    }
                }
            }
        }

        // ---- Drag.
        if down && let (Some(press), Some(pos)) = (self.view_tools.pen_press, pos) {
            let raw = self.view.to_design(rect, pos);
            let start = press_pos.unwrap_or(pos);
            if (pos - start).length() > 3.0 {
                self.view_tools.pen_moved = true;
            }
            // A handle snaps too (to other points, or level and upright from
            // its anchor), but not to the anchor it pulls out of.
            let anchor = match press {
                PenPress::Close => self.view_tools.pen.first(),
                PenPress::DrawnHandle { k, .. } | PenPress::DrawnPull(k) => {
                    self.view_tools.pen.get(k)
                }
                _ => self.view_tools.pen.last(),
            }
            .map(|a| a.p);
            let n = own.len();
            let others: Vec<P> = own
                .iter()
                .enumerate()
                .filter(|&(k, _)| k != 0 && k + 1 != n)
                .map(|(_, p)| *p)
                .collect();
            let d = if space || manual || !self.view_tools.pen_moved {
                raw
            } else {
                self.snap_point(raw, anchor, false, &others)
            };
            let prev = self.view_tools.pen_pointer.unwrap_or(raw);
            self.view_tools.pen_pointer = Some(raw);
            let moved = self.view_tools.pen_moved;
            let press_at = self.view_tools.pen_press_at;
            if press == PenPress::Reshape {
                if moved {
                    self.alt_reshape_drag(raw, shift);
                }
            } else if let PenPress::DrawnBend { a, t } = press {
                if moved {
                    let mut sp = SubPath {
                        anchors: self.view_tools.pen_base.clone(),
                        closed: false,
                    };
                    bezier::bend_segment(&mut sp, a, t, raw - press_at, true);
                    self.view_tools.pen = sp.anchors;
                }
            } else {
                let pen = &mut self.view_tools.pen;
                let aim = |a: &Anchor| {
                    if shift {
                        bezier::constrain_45(a.p, d)
                    } else {
                        d
                    }
                };
                match press {
                    PenPress::DrawnAnchor(k) => {
                        if let Some(a) = pen.get_mut(k) {
                            let delta = raw - prev;
                            a.p = a.p + delta;
                            a.h_in = a.h_in + delta;
                            a.h_out = a.h_out + delta;
                        }
                    }
                    PenPress::DrawnHandle { k, out, smooth } if moved => {
                        if let Some(a) = pen.get_mut(k) {
                            let h = aim(a);
                            let other = if out { a.h_in } else { a.h_out };
                            let other = if smooth {
                                let v = h - a.p;
                                let len = (other - a.p).len();
                                if v.len() > 1e-9 {
                                    a.p - v * (len / v.len())
                                } else {
                                    other
                                }
                            } else {
                                other
                            };
                            if out {
                                (a.h_out, a.h_in) = (h, other);
                            } else {
                                (a.h_in, a.h_out) = (h, other);
                            }
                        }
                    }
                    PenPress::DrawnPull(k) if moved => {
                        if let Some(a) = pen.get_mut(k) {
                            let h = aim(a);
                            a.h_out = h;
                            a.h_in = a.p * 2.0 - h;
                        }
                    }
                    PenPress::Place if space => {
                        // Space: move the point being placed.
                        if let Some(a) = pen.last_mut() {
                            let delta = raw - prev;
                            a.p = a.p + delta;
                            a.h_in = a.h_in + delta;
                            a.h_out = a.h_out + delta;
                        }
                    }
                    PenPress::Place if moved && !manual => {
                        if let Some(a) = pen.last_mut() {
                            let h = aim(a);
                            a.h_out = h;
                            if !alt {
                                a.h_in = a.p * 2.0 - h;
                            }
                        }
                    }
                    PenPress::Close if moved && !manual => {
                        if let Some(a) = pen.first_mut() {
                            let h = aim(a);
                            a.h_in = a.p * 2.0 - h;
                            if !alt {
                                a.h_out = h;
                            }
                        }
                    }
                    PenPress::Last if moved && !manual => {
                        if let Some(a) = pen.last_mut() {
                            a.h_out = aim(a);
                        }
                    }
                    PenPress::Join(_) if moved && !manual => {
                        // Shape the curve arriving at the joint.
                        if let Some(a) = pen.last_mut() {
                            let h = aim(a);
                            a.h_in = a.p * 2.0 - h;
                        }
                    }
                    _ => {}
                }
            }
        }

        // ---- Release.
        if released && let Some(press) = self.view_tools.pen_press.take() {
            let moved = self.view_tools.pen_moved;
            match press {
                PenPress::Close => self.finish_pen(true),
                PenPress::Join(e) => self.finish_join(e),
                PenPress::Last if !moved => {
                    // A click on the last point: a corner from here on.
                    if let Some(a) = self.view_tools.pen.last_mut() {
                        a.h_out = a.p;
                    }
                }
                PenPress::DrawnPull(k) if !moved => {
                    // Option-click a smooth point: a corner.
                    if let Some(a) = self.view_tools.pen.get_mut(k) {
                        a.h_in = a.p;
                        a.h_out = a.p;
                    }
                }
                PenPress::DrawnHandle {
                    k,
                    out,
                    smooth: false,
                } if !moved => {
                    // Option-click a handle's end: it goes back in.
                    if let Some(a) = self.view_tools.pen.get_mut(k) {
                        if out {
                            a.h_out = a.p;
                        } else {
                            a.h_in = a.p;
                        }
                    }
                }
                PenPress::Reshape => self.alt_reshape_end(moved),
                PenPress::Convert((s, k)) => {
                    if let Some(mut path) = self.editable_path() {
                        let at = path[s].anchors[k].p;
                        let end = pos.map(|p| {
                            self.snap_point(self.view.to_design(rect, p), Some(at), false, &[])
                        });
                        let a = &mut path[s].anchors[k];
                        match end {
                            // Option-drag: new smooth handles, pulled out.
                            Some(h) if moved => {
                                let h = if shift {
                                    bezier::constrain_45(a.p, h)
                                } else {
                                    h
                                };
                                a.h_out = h;
                                a.h_in = a.p * 2.0 - h;
                            }
                            // Option-click: smooth → corner, corner → smooth.
                            _ if a.has_handles() => {
                                a.h_in = a.p;
                                a.h_out = a.p;
                            }
                            _ => bezier::auto_smooth(&mut path[s], k),
                        }
                        self.commit_pen_edit(path, "Converted the point.");
                    }
                }
                _ => {}
            }
            self.view_tools.pen_moved = false;
        }

        // ---- Drawing.
        let to = |p: P| self.view.to_screen(rect, p);
        let pen = &self.view_tools.pen;
        let curve =
            |c: [P; 4]| -> Vec<Pos2> { bezier::sample(&c, 24).into_iter().map(to).collect() };
        for w in pen.windows(2) {
            painter.add(egui::Shape::line(
                curve([w[0].p, w[0].h_out, w[1].h_in, w[1].p]),
                Stroke::new(1.5, ACCENT),
            ));
        }
        let editing = matches!(
            self.view_tools.pen_press,
            Some(
                PenPress::DrawnHandle { .. }
                    | PenPress::DrawnAnchor(_)
                    | PenPress::DrawnPull(_)
                    | PenPress::DrawnBend { .. }
                    | PenPress::Reshape
            )
        );
        let raw_hover = resp.hover_pos().map(|h| self.view.to_design(rect, h));
        // What Option or Cmd would grab, while drawing.
        let over = match raw_hover {
            Some(q) if drawing && (alt || cmd) && self.view_tools.pen_press.is_none() => {
                self.drawn_hit(q, tol, true)
            }
            _ => None,
        };
        // Where a click would land, guides showing why (not while the
        // pointer is on the path to reshape it).
        let pen = &self.view_tools.pen;
        let hover = resp
            .hover_pos()
            .filter(|_| self.view_tools.pen_press.is_none() && over.is_none())
            .map(|h| {
                let last = pen.last().map(|a| a.p);
                self.snap_point(self.view.to_design(rect, h), last, shift, &own)
            });
        // The next segment, following the pointer (Illustrator's rubber band),
        // unless the pointer is on the path to reshape it.
        if let (Some(last), Some(q)) = (pen.last(), hover)
            && self.view_tools.pen_press.is_none()
            && over.is_none()
            && !(cmd && drawing)
        {
            painter.extend(egui::Shape::dashed_line(
                &curve([last.p, last.h_out, q, q]),
                Stroke::new(1.0, ACCENT),
                5.0,
                4.0,
            ));
        }
        // Closing preview while dragging on the first point.
        if self.view_tools.pen_press == Some(PenPress::Close)
            && let (Some(first), Some(last)) = (pen.first(), pen.last())
        {
            painter.add(egui::Shape::line(
                curve([last.p, last.h_out, first.h_in, first.p]),
                Stroke::new(1.5, ACCENT),
            ));
        }
        // Handles: the last point's and the one before it (every one while
        // Option or Cmd is held or one is being moved), the first while closing.
        let mut shown = self.drawn_handles(alt || cmd || editing);
        if self.view_tools.pen_press == Some(PenPress::Close) {
            shown.push((0, false));
            shown.push((0, true));
        }
        let pen = &self.view_tools.pen;
        for (k, out) in shown {
            let a = pen[k];
            let h = if out { a.h_out } else { a.h_in };
            if h != a.p {
                let hot = over == Some(Drawn::Handle(k, out));
                painter.line_segment([to(a.p), to(h)], Stroke::new(1.0, ACCENT));
                painter.circle_filled(to(h), if hot { 4.5 } else { 3.0 }, ACCENT);
            }
        }
        // The segment Option would bend, highlighted.
        if let Some(Drawn::Segment(a, _)) = over
            && a + 1 < pen.len()
        {
            painter.add(egui::Shape::line(
                curve([pen[a].p, pen[a].h_out, pen[a + 1].h_in, pen[a + 1].p]),
                Stroke::new(3.0, ACCENT),
            ));
        }
        // Points: hollow squares; the last one filled.
        let n = pen.len();
        for (k, a) in pen.iter().enumerate() {
            let hot = over == Some(Drawn::Anchor(k));
            let r = Rect::from_center_size(to(a.p), Vec2::splat(if hot { 8.0 } else { 6.0 }));
            if k + 1 == n || hot {
                painter.rect_filled(r, 0.0, ACCENT);
            } else {
                painter.rect_filled(r, 0.0, Color32::WHITE);
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, ACCENT), egui::StrokeKind::Inside);
            }
        }
        // The cursor: what a click would do (or what Option / Cmd would grab).
        if let (Some(q), None) = (raw_hover, self.view_tools.pen_press) {
            use crate::cursors::Glyph;
            let glyph = match (over, cmd, alt) {
                (Some(Drawn::Handle(..) | Drawn::Anchor(_)), true, _) => Glyph::Hollow(true),
                (Some(Drawn::Handle(..) | Drawn::Anchor(_)), false, true) => Glyph::Convert,
                (Some(Drawn::Segment(..)), false, true) => Glyph::Reshape,
                _ if drawing && cmd => Glyph::Hollow(false),
                _ if !drawing && alt => match self.alt_reshape_hit(q, tol) {
                    Some(true) => Glyph::Convert,
                    Some(false) => Glyph::Reshape,
                    None => Glyph::Pen(self.pen_spot(hover.unwrap_or(q), tol, alt).hint()),
                },
                _ => Glyph::Pen(self.pen_spot(hover.unwrap_or(q), tol, alt).hint()),
            };
            self.want_cursor(glyph);
        } else if editing {
            self.want_cursor(match self.view_tools.pen_press {
                Some(PenPress::DrawnAnchor(_)) => crate::cursors::Glyph::Hollow(true),
                Some(PenPress::DrawnHandle { smooth: true, .. }) => {
                    crate::cursors::Glyph::Hollow(true)
                }
                Some(PenPress::DrawnBend { .. }) | Some(PenPress::Reshape) => {
                    crate::cursors::Glyph::Reshape
                }
                _ => crate::cursors::Glyph::Convert,
            });
        }
    }

    /// Snap a design point to the grid, when the grid is on.
    pub fn snap_to_grid(&self, p: P) -> P {
        if !self.view_tools.grid {
            return p;
        }
        let g = crate::view_tools::grid_steps(&self.view).0;
        P::new((p.x / g).round() * g, (p.y / g).round() * g)
    }

    fn commit_pen_edit(&mut self, path: zigzag::bezier::BezPath, what: &str) {
        let Some(i) = self.sel_index() else { return };
        match self.structural(|d| d.set_path(i, &path)) {
            Ok(()) => self.status = what.into(),
            Err(e) => self.status = format!("Couldn't edit the path: {e:#}"),
        }
    }

    /// Keys for the pen. Returns true when it used the key.
    pub fn pen_keys(&mut self, ctx: &egui::Context) -> bool {
        if self.tool != Tool::Pen || ctx.egui_wants_keyboard_input() {
            return false;
        }
        if self.view_tools.pen.is_empty() {
            return false;
        }
        let (enter, back, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
                i.consume_key(egui::Modifiers::NONE, Key::Backspace)
                    || i.consume_key(egui::Modifiers::NONE, Key::Delete),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if enter || esc {
            // Illustrator: Enter or Esc ends the path, keeping it.
            self.finish_pen(false);
        }
        if back {
            self.view_tools.pen.pop();
            if self.view_tools.pen.is_empty() {
                self.view_tools.pen_continue = None;
            }
        }
        enter || back || esc
    }

    /// End the path being drawn: a new object, or (when continuing) the
    /// selected path, extended.
    pub fn finish_pen(&mut self, closed: bool) {
        let pts = std::mem::take(&mut self.view_tools.pen);
        self.view_tools.pen_press = None;
        let cont = self.view_tools.pen_continue.take();
        let curved = pts.iter().any(|a| a.has_handles());
        let need = if closed && !curved { 3 } else { 2 };
        if pts.len() < need {
            if !closed {
                // Nothing worth keeping: a single click.
                return;
            }
            self.view_tools.pen = pts;
            self.view_tools.pen_continue = cont;
            return;
        }
        let n = pts.len();
        let sub = SubPath {
            anchors: pts,
            closed,
        };
        // Continuing a selected path: put the longer subpath back in place.
        if let Some((uid, s)) = cont
            && let Some(i) = self.doc.shapes.iter().position(|x| x.uid == uid)
            && let Ok(mut path) = self.doc.edit_path(i)
            && s < path.len()
        {
            path[s] = sub;
            match self.structural(|d| d.set_path(i, &path)) {
                Ok(()) => {
                    if let Some(i) = self.doc.shapes.iter().position(|x| x.uid == uid) {
                        self.set_selection(&[i]);
                    }
                    self.status = if closed {
                        "Closed the path.".into()
                    } else {
                        format!("Extended the path ({n} points).")
                    };
                }
                Err(e) => self.status = format!("Couldn't extend the path: {e:#}"),
            }
            return;
        }
        let kind = match (self.view_tools.pen_mode, closed) {
            (PenMode::Manual, _) => Kind::Manual,
            (PenMode::Shape, true) => Kind::Fill,
            (PenMode::Shape, false) => Kind::Run,
        };
        let color = self.new_shape_color();
        let mut idx = 0;
        let path = vec![sub];
        match self.structural(|d| {
            idx = d.add_drawing_path(&path, color, kind)?;
            Ok(())
        }) {
            Ok(()) => {
                self.set_selection(&[idx]);
                self.status = format!(
                    "Drew a {} ({} points).",
                    if closed { "shape" } else { "line" },
                    n
                );
            }
            Err(e) => self.status = format!("Couldn't add the drawing: {e:#}"),
        }
    }

    /// End the path being drawn by joining it onto an open path's end `e`:
    /// one path through both (one undo step). Continuing a path and joining
    /// another merges the two into the continued one.
    pub fn finish_join(&mut self, e: crate::snap::End) {
        let drawn = std::mem::take(&mut self.view_tools.pen);
        self.view_tools.pen_press = None;
        let cont = self.view_tools.pen_continue.take();
        let Some(ti) = self.doc.shapes.iter().position(|x| x.uid == e.uid) else {
            return;
        };
        let Ok(tpath) = self.doc.edit_path(ti) else {
            return;
        };
        let Some(target) = tpath.get(e.s) else {
            return;
        };
        let joined = bezier::join(&drawn, target, e.at_start);
        let n = joined.anchors.len();
        let mut keep_uid = e.uid;
        let result = self.structural(|d| {
            let ti = d.shapes.iter().position(|x| x.uid == e.uid).unwrap();
            match cont {
                // Continuing a path: the joined line replaces it, and the
                // path joined onto leaves its object.
                Some((cu, cs)) => {
                    keep_uid = cu;
                    let ci = d.shapes.iter().position(|x| x.uid == cu).unwrap();
                    let mut cpath = d.edit_path(ci)?;
                    cpath[cs] = joined.clone();
                    if cu == e.uid {
                        cpath.remove(e.s);
                        d.set_path(ci, &cpath)?;
                    } else {
                        d.set_path(ci, &cpath)?;
                        let ti = d.shapes.iter().position(|x| x.uid == e.uid).unwrap();
                        let mut tpath = d.edit_path(ti)?;
                        tpath.remove(e.s);
                        if tpath.is_empty() {
                            d.delete(ti)?;
                        } else {
                            d.set_path(ti, &tpath)?;
                        }
                    }
                }
                // A new line joins onto the path it ends on.
                None => {
                    let mut tpath = d.edit_path(ti)?;
                    tpath[e.s] = joined.clone();
                    d.set_path(ti, &tpath)?;
                }
            }
            Ok(())
        });
        match result {
            Ok(()) => {
                if let Some(i) = self.doc.shapes.iter().position(|x| x.uid == keep_uid) {
                    self.set_selection(&[i]);
                }
                self.status = format!("Joined the paths ({n} points).");
            }
            Err(e) => self.status = format!("Couldn't join the paths: {e:#}"),
        }
    }

    /// Thread for new shapes and drawings: the selection's, else dark grey.
    pub fn new_shape_color(&self) -> [u8; 3] {
        self.sel_index()
            .map(|i| self.doc.shapes[i].color)
            .unwrap_or([40, 40, 40])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    fn studio() -> Harness<'static, Studio> {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        h.state_mut().set_tool(Tool::Pen);
        h.run_steps(1);
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

    fn drag(h: &mut Harness<'_, Studio>, from: Pos2, to: Pos2) {
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        for k in 1..=4 {
            h.hover_at(from + (to - from) * (k as f32 / 4.0));
            h.run_steps(1);
        }
        h.drop_at(to);
        h.run_steps(2);
    }

    #[test]
    fn option_drag_breaks_the_handle_and_clicking_the_last_point_makes_a_corner() {
        let mut h = studio();
        let c = h.state().canvas_rect.center();
        click(&mut h, c);
        // Option-drag: only the leading handle moves.
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::ALT));
        drag(
            &mut h,
            c + Vec2::new(100.0, 0.0),
            c + Vec2::new(140.0, 40.0),
        );
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        h.run_steps(1);
        let a = h.state().view_tools.pen[1];
        assert_eq!(a.h_in, a.p, "the trailing handle stayed put");
        assert!(a.h_out != a.p);
        // Clicking the last point drops its leading handle.
        click(&mut h, c + Vec2::new(100.0, 0.0));
        let a = h.state().view_tools.pen[1];
        assert_eq!(a.h_out, a.p);
        assert_eq!(h.state().view_tools.pen.len(), 2, "no new point");
        // Esc ends the path and keeps it.
        h.key_press(Key::Escape);
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), 1);
        assert!(h.state().view_tools.pen.is_empty());
    }

    #[test]
    fn clicking_an_open_end_continues_the_path() {
        let mut h = studio();
        let c = h.state().canvas_rect.center();
        click(&mut h, c);
        click(&mut h, c + Vec2::new(120.0, 0.0));
        h.key_press(Key::Enter);
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), 1);
        // The new line is selected: click its end, add a point, finish.
        click(&mut h, c + Vec2::new(120.0, 0.0));
        assert_eq!(h.state().view_tools.pen.len(), 2, "picked up the path");
        click(&mut h, c + Vec2::new(120.0, 100.0));
        h.key_press(Key::Enter);
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), 1, "the same object, longer");
        let path = h.state_mut().editable_path().unwrap();
        assert_eq!(path[0].anchors.len(), 3);
        // Continue from the other end and close it into a triangle.
        click(&mut h, c);
        assert_eq!(h.state().view_tools.pen.len(), 3);
        click(&mut h, c + Vec2::new(120.0, 100.0));
        h.run_steps(3);
        let path = h.state_mut().editable_path().unwrap();
        assert!(path[0].closed, "{path:?}");
        assert_eq!(h.state().doc.shapes.len(), 1);
    }

    #[test]
    fn shift_keeps_points_at_45_degrees() {
        let mut h = studio();
        let c = h.state().canvas_rect.center();
        click(&mut h, c);
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
        click(&mut h, c + Vec2::new(100.0, 12.0));
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        h.run_steps(1);
        let pen = &h.state().view_tools.pen;
        assert!((pen[1].p.y - pen[0].p.y).abs() < 1e-6, "level");
    }

    #[test]
    fn points_snap_onto_other_paths_anchors() {
        let mut h = studio();
        let c = h.state().canvas_rect.center();
        // A line to snap to.
        click(&mut h, c + Vec2::new(-150.0, -80.0));
        click(&mut h, c + Vec2::new(150.0, -80.0));
        h.key_press(Key::Enter);
        h.run_steps(2);
        assert_eq!(h.state().doc.shapes.len(), 1);
        let line = h.state().doc.edit_path(0).unwrap();
        let end = line[0].anchors[1].p;
        // A new path whose first click is 3 px off that end: it lands on it
        // (no join: it's a new path's first point).
        h.state_mut().set_selection(&[]);
        h.run_steps(1);
        let near = h.state().view.to_screen(h.state().canvas_rect, end) + Vec2::new(3.0, -2.0);
        // Far first, so this is the second point.
        click(&mut h, c + Vec2::new(0.0, 200.0));
        click(&mut h, near);
        h.run_steps(1);
        // Clicking an open end while drawing joins the paths into one.
        assert_eq!(h.state().doc.shapes.len(), 1, "joined, not a second object");
        let joined = h.state().doc.edit_path(0).unwrap();
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].anchors.len(), 3);
        assert!(joined[0].anchors.iter().any(|a| a.p.dist(end) < 1e-6));
        assert!(h.state().view_tools.pen.is_empty(), "the path ended");
    }

    #[test]
    fn a_new_point_lands_exactly_on_an_anchor_nearby() {
        let mut h = studio();
        let c = h.state().canvas_rect.center();
        // A closed triangle.
        for p in [
            Vec2::new(-100.0, 60.0),
            Vec2::new(100.0, 60.0),
            Vec2::new(0.0, -100.0),
        ] {
            click(&mut h, c + p);
        }
        click(&mut h, c + Vec2::new(-100.0, 60.0));
        assert_eq!(h.state().doc.shapes.len(), 1);
        let tri = h.state().doc.edit_path(0).unwrap();
        let apex = tri[0]
            .anchors
            .iter()
            .map(|a| a.p)
            .min_by(|a, b| a.y.total_cmp(&b.y))
            .unwrap();
        h.state_mut().set_selection(&[]);
        h.run_steps(1);
        let near = h.state().view.to_screen(h.state().canvas_rect, apex) + Vec2::new(4.0, 3.0);
        click(&mut h, near);
        assert_eq!(
            h.state().view_tools.pen[0].p,
            apex,
            "snapped onto the anchor"
        );
    }

    fn scr(h: &Harness<'_, Studio>, p: P) -> Pos2 {
        h.state().view.to_screen(h.state().canvas_rect, p)
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
    fn a_quick_tap_still_places_a_point() {
        let mut h = studio();
        let c = h.state().canvas_rect.center();
        h.hover_at(c);
        h.run_steps(1);
        // Press and release in one frame, as a trackpad tap can arrive.
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: c,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        h.run_steps(2);
        assert_eq!(h.state().view_tools.pen.len(), 1);
    }

    #[test]
    fn option_reshapes_the_path_being_drawn() {
        let mut h = studio();
        let c = h.state().canvas_rect.center();
        click(&mut h, c);
        click(&mut h, c + Vec2::new(200.0, 0.0));
        // Option-drag the straight segment's middle: it bends under the pointer.
        let (a, b) = (h.state().view_tools.pen[0], h.state().view_tools.pen[1]);
        let mid = bezier::cubic_at(&[a.p, a.h_out, b.h_in, b.p], 0.5);
        let from = scr(&h, mid);
        with_mods(&mut h, egui::Modifiers::ALT, |h| {
            drag(h, from, from + Vec2::new(0.0, 60.0))
        });
        let pen = h.state().view_tools.pen.clone();
        assert_eq!(pen.len(), 2, "no new point");
        let bent = bezier::cubic_at(&[pen[0].p, pen[0].h_out, pen[1].h_in, pen[1].p], 0.5);
        let want = h
            .state()
            .view
            .to_design(h.state().canvas_rect, from + Vec2::new(0.0, 60.0));
        assert!(bent.dist(want) < 3.0, "{bent:?} {want:?}");
        // A smooth point next: Option-drag one of its handles moves it alone.
        drag(
            &mut h,
            c + Vec2::new(300.0, 100.0),
            c + Vec2::new(360.0, 100.0),
        );
        let p2 = h.state().view_tools.pen[2];
        assert!(p2.is_smooth());
        let from = scr(&h, p2.h_in);
        with_mods(&mut h, egui::Modifiers::ALT, |h| {
            drag(h, from, from + Vec2::new(0.0, -50.0))
        });
        let q2 = h.state().view_tools.pen[2];
        assert_eq!(q2.h_out, p2.h_out, "the other handle stayed");
        assert!(q2.h_in != p2.h_in);
        // Option-click a smooth point: a corner.
        let mut h2 = studio();
        click(&mut h2, c);
        drag(
            &mut h2,
            c + Vec2::new(150.0, 0.0),
            c + Vec2::new(200.0, 40.0),
        );
        click(&mut h2, c + Vec2::new(300.0, 0.0));
        assert!(h2.state().view_tools.pen[1].has_handles());
        let at = scr(&h2, h2.state().view_tools.pen[1].p);
        with_mods(&mut h2, egui::Modifiers::ALT, |h| click(h, at));
        let p = h2.state().view_tools.pen[1];
        assert!(!p.has_handles(), "{p:?}");
        assert_eq!(h2.state().view_tools.pen.len(), 3);
    }

    #[test]
    fn cmd_moves_points_and_handles_while_drawing() {
        let mut h = studio();
        let c = h.state().canvas_rect.center();
        click(&mut h, c);
        drag(
            &mut h,
            c + Vec2::new(150.0, 0.0),
            c + Vec2::new(200.0, 40.0),
        );
        let before = h.state().view_tools.pen[1];
        // Cmd-drag the point: it moves, handles and all.
        let at = scr(&h, before.p);
        with_mods(&mut h, egui::Modifiers::COMMAND, |h| {
            drag(h, at, at + Vec2::new(0.0, 50.0))
        });
        let moved = h.state().view_tools.pen[1];
        let d = moved.p - before.p;
        assert!(d.len() > 1.0 && d.x.abs() < 1e-6, "{d:?}");
        assert!(((moved.h_out - moved.p) - (before.h_out - before.p)).len() < 1e-6);
        // Cmd-drag its handle: it stays smooth.
        let at = scr(&h, moved.h_out);
        with_mods(&mut h, egui::Modifiers::COMMAND, |h| {
            drag(h, at, at + Vec2::new(-20.0, 60.0))
        });
        let a = h.state().view_tools.pen[1];
        assert!(a.is_smooth(), "{a:?}");
        assert!(a.h_out != moved.h_out);
        // And drawing goes on.
        click(&mut h, c + Vec2::new(300.0, -80.0));
        assert_eq!(h.state().view_tools.pen.len(), 3);
    }

    #[test]
    fn option_bends_a_selected_paths_segment() {
        let mut h = studio();
        let c = h.state().canvas_rect.center();
        click(&mut h, c);
        click(&mut h, c + Vec2::new(200.0, 0.0));
        h.key_press(Key::Enter);
        h.run_steps(2);
        assert_eq!(h.state().doc.shapes.len(), 1);
        let undo = h.state().undo.len();
        let from = c + Vec2::new(100.0, 0.0);
        with_mods(&mut h, egui::Modifiers::ALT, |h| {
            drag(h, from, from + Vec2::new(0.0, 60.0))
        });
        let path = h.state_mut().editable_path().unwrap();
        assert_eq!(path[0].anchors.len(), 2, "bent, no point added");
        let bent = bezier::cubic_at(&path[0].cubic(0, 1), 0.5);
        let want = h
            .state()
            .view
            .to_design(h.state().canvas_rect, from + Vec2::new(0.0, 60.0));
        assert!(bent.dist(want) < 3.0, "{bent:?} {want:?}");
        assert_eq!(h.state().undo.len(), undo + 1);
        assert!(h.state().view_tools.pen.is_empty(), "no new path started");
    }
}
