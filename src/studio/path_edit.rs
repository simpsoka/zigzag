//! Point-by-point editing, Illustrator-style. The Direct Selection tool (A)
//! drags anchors, handles and segments of the selected object; with the Pen
//! tool, clicking a selected object's outline adds a point, clicking a point
//! deletes it, and Option-clicking a point turns it between corner and smooth.
//! The Pen's group also has Add Anchor Point (+), Delete Anchor Point (-) and
//! the Anchor Point tool (Shift+C).
//! Every finished edit is one undo step, written into the SVG as path data.

use crate::app::{Studio, Tool};
use crate::layout::DragMods;
use eframe::egui::{self, Color32, Key, Pos2, Rect, Stroke, Vec2};
use zigzag::bezier::{self, AnchorRef, BezPath};
use zigzag::geom::P;

#[derive(Default, Clone)]
pub struct PathEdit {
    /// Outlines read from the document, by object uid: (the document
    /// revision read at, the path, or None when it can't be edited point by
    /// point). Reading one re-parses the SVG, so it's done once per change.
    outlines: std::collections::HashMap<usize, (u64, Option<BezPath>)>,
    /// The object whose points `selected` refers to.
    points_of: Option<usize>,
    /// Selected anchors of that outline.
    pub selected: Vec<AnchorRef>,
    pub drag: Option<PointDrag>,
    /// Show every handle of the outline (the Pen with Option held lends the
    /// Anchor Point tool, which grabs any of them).
    pub show_all: bool,
}

/// A point, handle or segment drag in progress.
#[derive(Clone)]
pub struct PointDrag {
    base_path: BezPath,
    index: usize,
    start: P,
    grab: Grab,
    /// The anchor grabbed (it snaps; the other selected ones follow it).
    lead: Option<AnchorRef>,
    /// The corner widget grabbed, the corners it rounds, and the radius so far.
    corner: Option<bezier::Corner>,
    corners: Vec<AnchorRef>,
    pub radius: Option<f64>,
    pub path: BezPath,
}

#[derive(Clone, Copy, Debug)]
enum Grab {
    /// The selected anchors (with their handles).
    Anchors,
    /// One handle; `smooth`: the opposite one turns with it.
    Handle {
        at: AnchorRef,
        out: bool,
        smooth: bool,
    },
    /// A curved segment, grabbed at `t`: dragging bends it so the point
    /// grabbed follows the pointer (both handles move; smooth anchors stay
    /// smooth).
    Segment { s: usize, a: usize, t: f64 },
    /// A corner widget: dragging it into the corner rounds the corners in
    /// `PointDrag::corners` (Illustrator's Live Corners).
    Corner,
    /// The Anchor Point tool dragging from a point: a pair of smooth
    /// handles pulled out of it, the leading one under the pointer.
    Pull { at: AnchorRef },
}

/// What's under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    /// A corner's widget.
    Corner(AnchorRef),
    Handle(AnchorRef, bool),
    Anchor(AnchorRef),
    Segment(usize, usize, f64),
    Object(usize),
    Nothing,
}

const BLUE: Color32 = Color32::from_rgb(40, 140, 255);

/// Handles worth showing: those of the selected anchors, and the ones facing
/// them on neighbouring anchors. (anchor, out-handle?)
fn visible_handles(path: &BezPath, selected: &[AnchorRef]) -> Vec<(AnchorRef, bool)> {
    let mut v = Vec::new();
    for &(s, k) in selected {
        let Some(sp) = path.get(s) else { continue };
        let n = sp.anchors.len();
        if k >= n {
            continue;
        }
        v.push(((s, k), false));
        v.push(((s, k), true));
        if k > 0 || sp.closed {
            v.push(((s, (k + n - 1) % n), true));
        }
        if k + 1 < n || sp.closed {
            v.push(((s, (k + 1) % n), false));
        }
    }
    v.retain(|&((s, k), out)| {
        let a = path[s].anchors[k];
        (if out { a.h_out } else { a.h_in }) != a.p
    });
    v.dedup();
    v
}

/// Every anchor of a path (the Anchor Point tool shows all their handles).
fn all_anchors(path: &BezPath) -> Vec<AnchorRef> {
    path.iter()
        .enumerate()
        .flat_map(|(s, sp)| (0..sp.anchors.len()).map(move |k| (s, k)))
        .collect()
}

fn handle_pos(path: &BezPath, (s, k): AnchorRef, out: bool) -> P {
    let a = path[s].anchors[k];
    if out { a.h_out } else { a.h_in }
}

impl Studio {
    /// The selected object's outline, if it can be edited point by point.
    pub fn editable_path(&mut self) -> Option<BezPath> {
        if let Some(d) = &self.path_edit.drag {
            return Some(d.path.clone());
        }
        let i = self.sel_index()?;
        let uid = self.doc.shapes[i].uid;
        if self.path_edit.points_of != Some(uid) {
            self.path_edit.selected.clear();
            self.path_edit.points_of = Some(uid);
        }
        let path = self.outline_of(i);
        if let Some(p) = &path {
            self.path_edit
                .selected
                .retain(|&(s, k)| p.get(s).is_some_and(|sp| k < sp.anchors.len()));
        }
        path
    }

    /// Object `i`'s outline as editable points (cached until the document
    /// changes).
    pub fn outline_of(&mut self, i: usize) -> Option<BezPath> {
        let uid = self.doc.shapes.get(i)?.uid;
        let rev = self.doc.rev();
        if let Some((r, p)) = self.path_edit.outlines.get(&uid)
            && *r == rev
        {
            return p.clone();
        }
        let path = self.doc.edit_path(i).ok();
        // Outlines of an older document go.
        self.path_edit.outlines.retain(|_, (r, _)| *r == rev);
        self.path_edit.outlines.insert(uid, (rev, path.clone()));
        path
    }

    /// The corner widgets of `path`: each corner worth one (sides long
    /// enough on screen; the selected points' corners when there are any)
    /// and where its circle sits, a little way into the corner.
    pub fn corner_widgets(&self, path: &BezPath) -> Vec<(bezier::Corner, P)> {
        if self.tool != Tool::Direct {
            return Vec::new();
        }
        let px = 10.0 / self.view.scale as f64;
        // A traced outline's thousands of points get no corner widgets
        // (nor the work of finding its corners every frame).
        if path.iter().map(|sp| sp.anchors.len()).sum::<usize>() > 200 {
            return Vec::new();
        }
        let corners = bezier::path_corners(path);
        if corners.len() > 64 {
            return Vec::new();
        }
        let sel = &self.path_edit.selected;
        corners
            .into_iter()
            .filter(|c| sel.is_empty() || sel.contains(&c.at_ref))
            .filter(|c| c.lu.min(c.lv) > 36.0 * px)
            .filter_map(|c| {
                let b = c.u + c.v;
                let l = b.len();
                (l > 1e-9).then(|| (c, c.at + b * (14.0 * px / l)))
            })
            .collect()
    }

    /// The handles drawn and grabbable: the selected points' (and their
    /// neighbours'); with the Anchor Point tool, every one.
    fn shown_handles(&self, path: &BezPath) -> Vec<(AnchorRef, bool)> {
        if self.tool == Tool::Anchor || self.path_edit.show_all {
            visible_handles(path, &all_anchors(path))
        } else {
            visible_handles(path, &self.path_edit.selected)
        }
    }

    fn direct_hit(&self, path: Option<&BezPath>, q: P, tol: f64) -> Hit {
        if let Some(path) = path {
            let px = 10.0 / self.view.scale as f64;
            if let Some((c, _)) = self
                .corner_widgets(path)
                .into_iter()
                .filter(|(_, w)| w.dist(q) <= 5.0 * px)
                .min_by(|a, b| a.1.dist(q).total_cmp(&b.1.dist(q)))
            {
                return Hit::Corner(c.at_ref);
            }
            for (a, out) in self.shown_handles(path) {
                if (handle_pos(path, a, out) - q).len() <= tol {
                    return Hit::Handle(a, out);
                }
            }
            if let Some(a) = bezier::hit_anchor(path, q, tol) {
                return Hit::Anchor(a);
            }
            if let Some((s, a, t)) = bezier::hit_segment(path, q, tol) {
                return Hit::Segment(s, a, t);
            }
        }
        match crate::canvas::hit_test(&self.doc.shapes, q, tol) {
            Some(i) => Hit::Object(i),
            None => Hit::Nothing,
        }
    }

    /// Commit a new outline for the selected object as one undo step.
    fn commit_path(&mut self, path: BezPath, what: &str) {
        let Some(i) = self.sel_index() else { return };
        if path.is_empty() {
            self.delete_selection();
            self.path_edit.selected.clear();
            return;
        }
        match self.structural(|d| d.set_path(i, &path)) {
            Ok(()) => self.status = what.into(),
            Err(e) => self.status = format!("Couldn't edit the path: {e:#}"),
        }
    }

    fn begin_point_drag(&mut self, path: BezPath, start: P, grab: Grab, lead: Option<AnchorRef>) {
        let Some(index) = self.sel_index() else {
            return;
        };
        self.snap_targets(Some(self.doc.shapes[index].uid), false);
        self.path_edit.drag = Some(PointDrag {
            base_path: path.clone(),
            index,
            start,
            grab,
            lead,
            corner: None,
            corners: Vec::new(),
            radius: None,
            path,
        });
    }

    fn update_point_drag(&mut self, q: P, mods: DragMods) {
        let selected = self.path_edit.selected.clone();
        let Some(d) = &self.path_edit.drag else {
            return;
        };
        let uid = self.doc.shapes.get(d.index).map(|s| s.uid);
        let (start, base_path, grab, lead) = (d.start, d.base_path.clone(), d.grab, d.lead);
        self.snap_targets(uid, true);
        // The rest of this path's points are targets too.
        // Not the points being dragged, nor a handle's own anchor (it
        // would snap onto it and vanish).
        let skip: Vec<AnchorRef> = match grab {
            Grab::Anchors => selected.clone(),
            Grab::Handle { at, .. } | Grab::Pull { at } => vec![at],
            _ => vec![],
        };
        let others = Studio::anchors_except(&base_path, &skip);
        let mut delta = q - start;
        // The grabbed point snaps (Shift: keeps to 45° from where it was,
        // sliding into line with a target); the others move with it.
        match grab {
            Grab::Anchors => {
                if let Some((s, k)) = lead
                    && let Some(a) = base_path.get(s).and_then(|sp| sp.anchors.get(k))
                {
                    let from = mods.shift.then_some(a.p);
                    delta = self.snap_point(a.p + delta, from, mods.shift, &others) - a.p;
                } else if mods.shift && delta.len() > 0.0 {
                    delta = bezier::constrain_45(P::default(), delta);
                }
            }
            Grab::Handle {
                at: (s, k), out, ..
            } => {
                let a = base_path[s].anchors[k];
                let h = if out { a.h_out } else { a.h_in };
                // Level and upright from its anchor, other points.
                delta = self.snap_point(h + delta, Some(a.p), mods.shift, &others) - h;
            }
            Grab::Pull { at: (s, k) } => {
                // The leading handle under the pointer, level and upright
                // from its point too.
                let a = base_path[s].anchors[k];
                delta = self.snap_point(a.p + delta, Some(a.p), mods.shift, &others) - a.p;
            }
            Grab::Segment { .. } | Grab::Corner => {}
        }
        let Some(d) = &mut self.path_edit.drag else {
            return;
        };
        let mut path = base_path;
        match d.grab {
            Grab::Pull { at: (s, k) } => {
                let a = &mut path[s].anchors[k];
                if delta.len() > 1e-9 {
                    a.h_out = a.p + delta;
                    a.h_in = a.p - delta;
                }
            }
            Grab::Corner => {
                let Some(c) = d.corner else { return };
                let r = c.radius_change(delta).max(0.0);
                let set = &d.corners;
                path = bezier::cut_corners(
                    &path,
                    |x| if set.contains(&x) { r } else { 0.0 },
                    bezier::CornerKind::Round,
                );
                d.radius = Some(r);
            }
            Grab::Segment { s, a, t } => {
                bezier::bend_segment(&mut path[s], a, t, delta, !mods.alt);
            }
            Grab::Anchors => {
                for &(s, k) in &selected {
                    if let Some(a) = path.get_mut(s).and_then(|sp| sp.anchors.get_mut(k)) {
                        a.p = a.p + delta;
                        a.h_in = a.h_in + delta;
                        a.h_out = a.h_out + delta;
                    }
                }
            }
            Grab::Handle {
                at: (s, k),
                out,
                smooth,
            } => {
                let a = &mut path[s].anchors[k];
                let target = (if out { a.h_out } else { a.h_in }) + delta;
                let other = if out { a.h_in } else { a.h_out };
                // Smooth points stay smooth unless Option breaks them.
                let other = if smooth && !mods.alt {
                    let v = target - a.p;
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
                    (a.h_out, a.h_in) = (target, other);
                } else {
                    (a.h_in, a.h_out) = (target, other);
                }
            }
        }
        // Only the outline follows the pointer: rewriting the SVG and
        // re-planning every stitch each frame made dragging crawl on big
        // designs. The document changes once, on release.
        d.path = path;
    }

    fn end_point_drag(&mut self) {
        let Some(d) = self.path_edit.drag.take() else {
            return;
        };
        if d.path != d.base_path {
            let what = match d.grab {
                Grab::Anchors => "Moved points.",
                Grab::Handle { .. } | Grab::Segment { .. } => "Reshaped the curve.",
                Grab::Corner => "Rounded the corners.",
                Grab::Pull { .. } => "Made a smooth point.",
            };
            if matches!(d.grab, Grab::Corner) {
                // The cut corners are new points now.
                self.path_edit.selected.clear();
            }
            self.commit_path(d.path, what);
        }
    }

    /// Canvas input and drawing for the Direct Selection tool.
    pub fn direct_canvas(
        &mut self,
        ctx: &egui::Context,
        resp: &egui::Response,
        rect: Rect,
        tol: f64,
        mods: DragMods,
    ) {
        let path = self.editable_path();
        if resp.drag_started_by(egui::PointerButton::Primary) {
            let start = ctx
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos())
                .unwrap_or(rect.center());
            let q = self.view.to_design(rect, start);
            match self.direct_hit(path.as_ref(), q, tol) {
                Hit::Corner(at) => {
                    let path = path.clone().unwrap();
                    let widgets = self.corner_widgets(&path);
                    if let Some((c, _)) = widgets.iter().find(|(c, _)| c.at_ref == at) {
                        // The corners with widgets round together (the
                        // selected points', or all of them).
                        let set: Vec<AnchorRef> = widgets.iter().map(|(c, _)| c.at_ref).collect();
                        let c = *c;
                        self.begin_point_drag(path, q, Grab::Corner, None);
                        if let Some(d) = &mut self.path_edit.drag {
                            d.corner = Some(c);
                            d.corners = set;
                        }
                    }
                }
                Hit::Handle(at, out) => {
                    let path = path.clone().unwrap();
                    let smooth = path[at.0].anchors[at.1].is_smooth();
                    self.begin_point_drag(path, q, Grab::Handle { at, out, smooth }, None);
                }
                Hit::Anchor(a) => {
                    if !self.path_edit.selected.contains(&a) {
                        if !mods.shift {
                            self.path_edit.selected.clear();
                        }
                        self.path_edit.selected.push(a);
                    }
                    self.begin_point_drag(path.clone().unwrap(), q, Grab::Anchors, Some(a));
                }
                Hit::Segment(s, a, t) => {
                    self.press_segment(path.clone().unwrap(), (s, a, t), q, mods);
                }
                Hit::Object(i) => match self.outline_hit(i, q, tol) {
                    // On another path's outline: that path, its points
                    // showing, and the point or segment pressed, as if it
                    // had been selected already (VectorCraft, Illustrator).
                    Some((path, Hit::Anchor(a))) => {
                        self.set_selection(&[i]);
                        self.path_edit.points_of = Some(self.doc.shapes[i].uid);
                        self.path_edit.selected = vec![a];
                        self.begin_point_drag(path, q, Grab::Anchors, Some(a));
                    }
                    Some((path, Hit::Segment(s, a, t))) => {
                        self.set_selection(&[i]);
                        self.path_edit.points_of = Some(self.doc.shapes[i].uid);
                        self.path_edit.selected.clear();
                        self.press_segment(path, (s, a, t), q, mods);
                    }
                    // Its fill: the whole object moves.
                    _ => {
                        if self.sel_index() != Some(i) {
                            self.set_selection(&[i]);
                            self.path_edit.selected.clear();
                        }
                        self.begin_drag(None, q);
                    }
                },
                Hit::Nothing => self.panning = true,
            }
        }
        if resp.dragged_by(egui::PointerButton::Primary)
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let q = self.view.to_design(rect, pos);
            if self.path_edit.drag.is_some() {
                self.update_point_drag(q, mods);
                ctx.request_repaint();
            } else if self.drag.is_some() {
                self.update_drag(q, mods);
                ctx.request_repaint();
            }
        }
        if resp.drag_stopped() {
            self.end_point_drag();
            self.end_drag();
            self.panning = false;
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let q = self.view.to_design(rect, pos);
            match self.direct_hit(path.as_ref(), q, tol) {
                Hit::Anchor(a) => {
                    if mods.shift {
                        if let Some(k) = self.path_edit.selected.iter().position(|x| *x == a) {
                            self.path_edit.selected.remove(k);
                        } else {
                            self.path_edit.selected.push(a);
                        }
                    } else {
                        self.path_edit.selected = vec![a];
                    }
                }
                Hit::Handle(..) | Hit::Corner(_) => {}
                Hit::Segment(..) => self.path_edit.selected.clear(),
                Hit::Object(i) => {
                    // On its outline: the point clicked, or the segment's two.
                    let picked = match self.outline_hit(i, q, tol) {
                        Some((_, Hit::Anchor(a))) => vec![a],
                        Some((p, Hit::Segment(s, a, _))) => {
                            vec![(s, a), (s, (a + 1) % p[s].anchors.len())]
                        }
                        _ => Vec::new(),
                    };
                    self.set_selection(&[i]);
                    self.path_edit.points_of = Some(self.doc.shapes[i].uid);
                    self.path_edit.selected = picked;
                }
                Hit::Nothing => {
                    self.set_selection(&[]);
                    self.path_edit.selected.clear();
                }
            }
        }
        // The hollow arrow; a square beside it over a point or handle.
        let on_point = resp.hover_pos().is_some_and(|h| {
            self.path_edit.drag.is_none()
                && self.drag.is_none()
                && matches!(
                    self.direct_hit(path.as_ref(), self.view.to_design(rect, h), tol),
                    Hit::Anchor(_) | Hit::Handle(..) | Hit::Corner(_)
                )
        });
        let _ = ctx;
        self.want_cursor(crate::cursors::Glyph::Hollow(on_point));
    }

    /// A press on a segment of the selected path: a curved one bends (Option
    /// bends a straight one too); a straight one moves, both ends with it.
    fn press_segment(
        &mut self,
        path: BezPath,
        (s, a, t): (usize, usize, f64),
        q: P,
        mods: DragMods,
    ) {
        let b = (a + 1) % path[s].anchors.len();
        let c = path[s].cubic(a, b);
        let curved = c[1] != c[0] || c[2] != c[3];
        if curved || mods.alt {
            self.path_edit.selected.clear();
            self.begin_point_drag(path, q, Grab::Segment { s, a, t }, None);
        } else {
            if !mods.shift {
                self.path_edit.selected.clear();
            }
            for r in [(s, a), (s, b)] {
                if !self.path_edit.selected.contains(&r) {
                    self.path_edit.selected.push(r);
                }
            }
            self.begin_point_drag(path, q, Grab::Anchors, None);
        }
    }

    /// Whether `q` is on object `i`'s outline (not just inside it): the
    /// point or segment there, with the outline.
    fn outline_hit(&mut self, i: usize, q: P, tol: f64) -> Option<(BezPath, Hit)> {
        let path = self.outline_of(i)?;
        if let Some(a) = bezier::hit_anchor(&path, q, tol) {
            return Some((path, Hit::Anchor(a)));
        }
        let (s, a, t) = bezier::hit_segment(&path, q, tol)?;
        Some((path, Hit::Segment(s, a, t)))
    }

    /// Canvas input for the Anchor Point tool (Shift+C), as Illustrator's:
    /// click a smooth point to make it a corner; drag from a point to pull
    /// out smooth handles; drag a handle to move it on its own (click its
    /// end to take it back in); drag a segment, straight or curved, to bend
    /// it. Shift-drag a point to move it.
    pub fn anchor_canvas(
        &mut self,
        ctx: &egui::Context,
        resp: &egui::Response,
        rect: Rect,
        tol: f64,
        mods: DragMods,
    ) {
        let path = self.editable_path();
        if resp.drag_started_by(egui::PointerButton::Primary) {
            let start = ctx
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos())
                .unwrap_or(rect.center());
            let q = self.view.to_design(rect, start);
            match self.direct_hit(path.as_ref(), q, tol) {
                Hit::Handle(at, out) => {
                    let grab = Grab::Handle {
                        at,
                        out,
                        smooth: false,
                    };
                    self.begin_point_drag(path.clone().unwrap(), q, grab, None);
                }
                Hit::Anchor(a) if mods.shift => {
                    self.path_edit.selected = vec![a];
                    self.begin_point_drag(path.clone().unwrap(), q, Grab::Anchors, Some(a));
                }
                Hit::Anchor(a) => {
                    let path = path.clone().unwrap();
                    // From the point itself, whatever spot on it was pressed.
                    let at = path[a.0].anchors[a.1].p;
                    self.begin_point_drag(path, at, Grab::Pull { at: a }, None);
                }
                Hit::Segment(s, a, t) => {
                    self.begin_point_drag(
                        path.clone().unwrap(),
                        q,
                        Grab::Segment { s, a, t },
                        None,
                    );
                }
                Hit::Object(i) => {
                    if self.sel_index() != Some(i) {
                        self.set_selection(&[i]);
                        self.path_edit.selected.clear();
                    }
                }
                Hit::Corner(_) | Hit::Nothing => self.panning = true,
            }
        }
        if resp.dragged_by(egui::PointerButton::Primary)
            && let Some(pos) = resp.interact_pointer_pos()
            && self.path_edit.drag.is_some()
        {
            self.update_point_drag(self.view.to_design(rect, pos), mods);
            ctx.request_repaint();
        }
        if resp.drag_stopped() {
            self.end_point_drag();
            self.panning = false;
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let q = self.view.to_design(rect, pos);
            match self.direct_hit(path.as_ref(), q, tol) {
                Hit::Anchor((s, k)) => {
                    let mut path = path.clone().unwrap();
                    let a = &mut path[s].anchors[k];
                    if a.has_handles() {
                        a.h_in = a.p;
                        a.h_out = a.p;
                        self.commit_path(path, "Made a corner point.");
                    } else {
                        self.status = "Already a corner: drag from it to pull out handles.".into();
                    }
                }
                Hit::Handle((s, k), out) => {
                    // A click on a handle's end takes just that handle in.
                    let mut path = path.clone().unwrap();
                    let a = &mut path[s].anchors[k];
                    if out {
                        a.h_out = a.p;
                    } else {
                        a.h_in = a.p;
                    }
                    self.commit_path(path, "Took the handle in.");
                }
                Hit::Object(i) => {
                    self.set_selection(&[i]);
                    self.path_edit.selected.clear();
                }
                Hit::Nothing => {
                    self.set_selection(&[]);
                    self.path_edit.selected.clear();
                }
                Hit::Segment(..) | Hit::Corner(_) => {}
            }
        }
        self.want_cursor(crate::cursors::Glyph::Convert);
    }

    /// Add Anchor Point (+) and Delete Anchor Point (-): click a path's
    /// outline to add a point there (its shape unchanged), or a point to
    /// take it out. Option swaps them while held. A path that isn't
    /// selected is picked up by the click. Dragging pans.
    pub fn point_tool_canvas(
        &mut self,
        ctx: &egui::Context,
        resp: &egui::Response,
        rect: Rect,
        tol: f64,
        alt: bool,
    ) {
        let add = (self.tool == Tool::AddAnchor) != alt;
        self.want_cursor(crate::cursors::Glyph::Pen(if add { "+" } else { "-" }));
        if resp.drag_started_by(egui::PointerButton::Primary) {
            self.panning = true;
        }
        if resp.drag_stopped() {
            self.panning = false;
        }
        if !resp.clicked() {
            return;
        }
        let Some(pos) = resp.interact_pointer_pos() else {
            return;
        };
        let _ = ctx;
        let q = self.view.to_design(rect, pos);
        // On the selected path, else the one under the pointer.
        let mut path = self.editable_path();
        let on_selected = path.as_ref().is_some_and(|p| {
            if add {
                bezier::hit_segment(p, q, tol).is_some()
            } else {
                bezier::hit_anchor(p, q, tol).is_some()
            }
        });
        if !on_selected {
            match crate::canvas::hit_test(&self.doc.shapes, q, tol) {
                Some(i) if self.sel_index() != Some(i) => {
                    self.set_selection(&[i]);
                    self.path_edit.selected.clear();
                    path = self.editable_path();
                }
                _ => {}
            }
        }
        let Some(mut path) = path else {
            self.status = if self.sel_index().is_some() {
                "This object has no points to edit.".into()
            } else if add {
                "Click a path's outline to add a point.".into()
            } else {
                "Click a path's point to delete it.".into()
            };
            return;
        };
        if add {
            if let Some((s, a, t)) = bezier::hit_segment(&path, q, tol) {
                let r = bezier::split(&mut path, s, a, t);
                self.path_edit.selected = vec![r];
                self.commit_path(path, "Added a point.");
            } else {
                self.status = "Click on the outline to add a point there.".into();
            }
        } else if let Some(a) = bezier::hit_anchor(&path, q, tol) {
            bezier::delete(&mut path, &[a]);
            self.path_edit.selected.clear();
            self.commit_path(path, "Deleted a point.");
        } else {
            self.status = "Click on a point to delete it.".into();
        }
    }

    /// What Option lends the Pen over the selected path: the Anchor Point
    /// tool's handle (moves alone) or segment (bends) under `q`.
    pub(crate) fn alt_reshape_hit(&mut self, q: P, tol: f64) -> Option<bool> {
        let path = self.editable_path()?;
        self.path_edit.show_all = true;
        match self.direct_hit(Some(&path), q, tol) {
            Hit::Handle(..) => Some(true),
            Hit::Segment(..) => Some(false),
            _ => None,
        }
    }

    /// Start that gesture: true when it began.
    pub(crate) fn alt_reshape_begin(&mut self, q: P, tol: f64) -> bool {
        let Some(path) = self.editable_path() else {
            return false;
        };
        self.path_edit.show_all = true;
        match self.direct_hit(Some(&path), q, tol) {
            Hit::Handle(at, out) => {
                let grab = Grab::Handle {
                    at,
                    out,
                    smooth: false,
                };
                self.begin_point_drag(path, q, grab, None);
                true
            }
            Hit::Segment(s, a, t) => {
                self.begin_point_drag(path, q, Grab::Segment { s, a, t }, None);
                true
            }
            _ => false,
        }
    }

    /// Follow the pointer (smooth points stay smooth while bending).
    pub(crate) fn alt_reshape_drag(&mut self, q: P, shift: bool) {
        let mods = DragMods {
            shift,
            alt: false,
            snap: self.view_tools.smart_guides,
            tol: 0.0,
        };
        self.update_point_drag(q, mods);
    }

    /// Finish it. A click without moving on a handle's end takes it in.
    pub(crate) fn alt_reshape_end(&mut self, moved: bool) {
        if !moved && let Some(d) = self.path_edit.drag.take() {
            if let Grab::Handle {
                at: (s, k), out, ..
            } = d.grab
            {
                let mut path = d.base_path;
                let a = &mut path[s].anchors[k];
                if out {
                    a.h_out = a.p;
                } else {
                    a.h_in = a.p;
                }
                self.commit_path(path, "Took the handle in.");
            }
            return;
        }
        self.end_point_drag();
    }

    /// The selected objects' points as small filled squares (Selection
    /// tool). Skipped past a few thousand points, where they'd be a smear.
    pub fn draw_selected_points(&mut self, painter: &egui::Painter, rect: Rect, color: Color32) {
        const MOST: usize = 4000;
        let mut shown = 0;
        for i in self.sel_indices() {
            // Text is text, not its letters' outlines: a box, no points
            // (Direct Selection, or Create Outlines, is the way to its paths).
            if self.doc.lettering(i).is_some() {
                continue;
            }
            let Some(path) = self.outline_of(i) else {
                continue;
            };
            let n: usize = path.iter().map(|sp| sp.anchors.len()).sum();
            shown += n;
            if shown > MOST {
                return;
            }
            for sp in &path {
                for a in &sp.anchors {
                    let c = self.view.to_screen(rect, a.p);
                    if rect.contains(c) {
                        painter.rect_filled(
                            Rect::from_center_size(c, Vec2::splat(5.0)),
                            0.0,
                            color,
                        );
                    }
                }
            }
        }
    }

    /// Outline, anchors and handles of the object being edited.
    pub fn draw_path_edit(&mut self, painter: &egui::Painter, rect: Rect) {
        if self.drag.is_some() {
            return;
        }
        let Some(path) = self.editable_path() else {
            return;
        };
        let to = |p: P| self.view.to_screen(rect, p);
        // Each subpath as one line, a curve sampled by its size on screen.
        for sp in &path {
            let mut pts: Vec<Pos2> = Vec::new();
            for (a, b) in sp.segments() {
                let c = sp.cubic(a, b);
                let reach = (to(c[0]) - to(c[1])).length()
                    + (to(c[1]) - to(c[2])).length()
                    + (to(c[2]) - to(c[3])).length();
                let straight = c[1] == c[0] && c[2] == c[3];
                let n = if straight {
                    1
                } else {
                    ((reach / 6.0).ceil() as usize).clamp(2, 24)
                };
                let s = bezier::sample(&c, n);
                let skip = usize::from(!pts.is_empty());
                pts.extend(s.into_iter().skip(skip).map(to));
            }
            if pts.len() >= 2 {
                painter.add(egui::Shape::line(pts, Stroke::new(1.0, BLUE)));
            }
        }
        for ((s, k), out) in self.shown_handles(&path) {
            let a = to(path[s].anchors[k].p);
            let h = to(handle_pos(&path, (s, k), out));
            painter.line_segment([a, h], Stroke::new(1.0, BLUE));
            painter.circle_filled(h, 3.0, BLUE);
        }
        let selected: std::collections::HashSet<AnchorRef> =
            self.path_edit.selected.iter().copied().collect();
        let seen = rect.expand(4.0);
        for (s, sp) in path.iter().enumerate() {
            for (k, a) in sp.anchors.iter().enumerate() {
                let c = to(a.p);
                if !seen.contains(c) {
                    continue;
                }
                let r = Rect::from_center_size(c, Vec2::splat(6.0));
                if selected.contains(&(s, k)) {
                    painter.rect_filled(r, 0.0, BLUE);
                } else {
                    painter.rect_filled(r, 0.0, Color32::WHITE);
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0, BLUE), egui::StrokeKind::Inside);
                }
            }
        }
        // Corner widgets: a ringed dot inside each corner to drag.
        if self.path_edit.drag.is_none() {
            for (_, w) in self.corner_widgets(&path) {
                let w = to(w);
                painter.circle_filled(w, 4.5, Color32::WHITE);
                painter.circle_stroke(w, 4.5, Stroke::new(1.0, BLUE));
                painter.circle_filled(w, 1.5, BLUE);
            }
        }
        // The radius while rounding.
        if let Some(r) = self.path_edit.drag.as_ref().and_then(|d| d.radius)
            && let Some(pos) = painter.ctx().pointer_latest_pos()
        {
            crate::view_tools::label(
                painter,
                pos + Vec2::new(14.0, 14.0),
                format!("Radius {}", crate::units::fmt(r)),
            );
        }
    }

    /// Keys for the Direct Selection tool: Delete removes the selected
    /// points, arrows nudge them, Esc deselects them. True when used.
    pub fn direct_keys(&mut self, ctx: &egui::Context) -> bool {
        if self.tool != Tool::Direct
            || ctx.egui_wants_keyboard_input()
            || self.path_edit.selected.is_empty()
        {
            return false;
        }
        let (del, esc, nudge) = ctx.input_mut(|i| {
            let del = i.consume_key(egui::Modifiers::NONE, Key::Backspace)
                || i.consume_key(egui::Modifiers::NONE, Key::Delete);
            let esc = i.consume_key(egui::Modifiers::NONE, Key::Escape);
            let step = if i.modifiers.shift { 50.0 } else { 5.0 };
            let mut d = P::default();
            for (k, v) in [
                (Key::ArrowLeft, P::new(-step, 0.0)),
                (Key::ArrowRight, P::new(step, 0.0)),
                (Key::ArrowUp, P::new(0.0, -step)),
                (Key::ArrowDown, P::new(0.0, step)),
            ] {
                if i.consume_key(egui::Modifiers::NONE, k)
                    || i.consume_key(egui::Modifiers::SHIFT, k)
                {
                    d = d + v;
                }
            }
            (del, esc, d)
        });
        if esc {
            self.path_edit.selected.clear();
            return true;
        }
        let Some(mut path) = self.editable_path() else {
            return false;
        };
        if del {
            let n = self.path_edit.selected.len();
            bezier::delete(&mut path, &self.path_edit.selected);
            self.path_edit.selected.clear();
            self.commit_path(path, &format!("Deleted {n} point(s)."));
            return true;
        }
        if nudge.len() > 0.0 {
            for &(s, k) in &self.path_edit.selected {
                let a = &mut path[s].anchors[k];
                a.p = a.p + nudge;
                a.h_in = a.h_in + nudge;
                a.h_out = a.h_out + nudge;
            }
            self.commit_path(path, "Nudged points.");
            return true;
        }
        false
    }

    /// The Pen over a selected object: on its outline, add a point; on a
    /// point, delete it, or with Option turn it corner ↔ smooth. True when
    /// the click was used that way.
    pub fn pen_edits_selected(&mut self, q: P, tol: f64, alt: bool) -> bool {
        let Some(mut path) = self.editable_path() else {
            return false;
        };
        if let Some((s, k)) = bezier::hit_anchor(&path, q, tol) {
            if alt {
                let a = &mut path[s].anchors[k];
                if a.has_handles() {
                    a.h_in = a.p;
                    a.h_out = a.p;
                } else {
                    bezier::auto_smooth(&mut path[s], k);
                }
                self.commit_path(path, "Converted the point.");
            } else {
                bezier::delete(&mut path, &[(s, k)]);
                self.path_edit.selected.clear();
                self.commit_path(path, "Deleted a point.");
            }
            return true;
        }
        if !alt && let Some((s, a, t)) = bezier::hit_segment(&path, q, tol) {
            let r = bezier::split(&mut path, s, a, t);
            self.path_edit.selected = vec![r];
            self.commit_path(path, "Added a point.");
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::shape_bounds;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use std::path::PathBuf;
    use zigzag::document::Document;

    fn studio() -> Harness<'static, Studio> {
        let path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg"));
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(path), None),
            );
        h.run_steps(4);
        h
    }

    fn screen(h: &Harness<'_, Studio>, p: P) -> Pos2 {
        h.state().view.to_screen(h.state().canvas_rect, p)
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
        h.run_steps(3);
    }

    fn click(h: &mut Harness<'_, Studio>, at: Pos2) {
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(3);
    }

    fn sun(h: &Harness<'_, Studio>) -> usize {
        h.state().names.iter().position(|n| n == "sun").unwrap()
    }

    #[test]
    fn direct_selection_drags_a_point_and_its_handles() {
        let mut h = studio();
        h.key_press(Key::A);
        h.run_steps(1);
        assert_eq!(h.state().tool, Tool::Direct);
        let i = sun(&h);
        h.state_mut().set_selection(&[i]);
        h.run_steps(2);
        let path = h.state_mut().editable_path().expect("a circle is editable");
        assert_eq!(path[0].anchors.len(), 4);
        let b0 = shape_bounds(&h.state().doc.shapes[i]);
        // The rightmost point, dragged 3 mm further right.
        let right = path[0]
            .anchors
            .iter()
            .max_by(|a, b| a.p.x.total_cmp(&b.p.x))
            .unwrap()
            .p;
        let from = screen(&h, right);
        let to = screen(&h, right + P::new(30.0, 0.0));
        drag(&mut h, from, to);
        let i = sun(&h);
        let b1 = shape_bounds(&h.state().doc.shapes[i]);
        assert!(b1.max.x > b0.max.x + 25.0, "{} → {}", b0.max.x, b1.max.x);
        assert!((b1.min.x - b0.min.x).abs() < 0.5, "the other side stays");
        assert_eq!(h.state().undo.len(), 1);
        assert_eq!(h.state().path_edit.selected.len(), 1);

        // Its handle: drag it up; the point stays smooth.
        let path = h.state_mut().editable_path().unwrap();
        let (s, k) = h.state().path_edit.selected[0];
        let a = path[s].anchors[k];
        let (from, to) = (
            screen(&h, a.h_out),
            screen(&h, a.h_out + P::new(0.0, -20.0)),
        );
        drag(&mut h, from, to);
        let path = h.state_mut().editable_path().unwrap();
        let a2 = path[s].anchors[k];
        assert!(a2.h_out != a.h_out && a2.is_smooth(), "{a2:?}");
        assert_eq!(h.state().undo.len(), 2);

        // Delete removes the point.
        h.key_press(Key::Backspace);
        h.run_steps(3);
        assert_eq!(h.state_mut().editable_path().unwrap()[0].anchors.len(), 3);
        assert!(
            h.state().doc.shapes.iter().any(|s| s.label.contains("sun")),
            "still there"
        );
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
        h.run_steps(3);
        assert_eq!(h.state_mut().editable_path().unwrap()[0].anchors.len(), 4);
    }

    #[test]
    fn the_pen_adds_and_deletes_points_on_the_selection() {
        let mut h = studio();
        let i = sun(&h);
        h.state_mut().set_selection(&[i]);
        h.key_press(Key::P);
        h.run_steps(2);
        let path = h.state_mut().editable_path().unwrap();
        let c = path[0].cubic(0, 1);
        let mid = bezier::cubic_at(&c, 0.5);
        let at = screen(&h, mid);
        click(&mut h, at);
        assert_eq!(
            h.state_mut().editable_path().unwrap()[0].anchors.len(),
            5,
            "added"
        );
        assert!(h.state().view_tools.pen.is_empty(), "not a new drawing");
        click(&mut h, at);
        assert_eq!(
            h.state_mut().editable_path().unwrap()[0].anchors.len(),
            4,
            "deleted"
        );
    }

    #[test]
    fn the_pen_draws_curves() {
        let mut h = studio();
        h.state_mut().set_selection(&[]);
        h.key_press(Key::P);
        h.run_steps(1);
        let n = h.state().doc.shapes.len();
        let r = h.state().canvas_rect;
        let a = r.center() + Vec2::new(-200.0, 300.0);
        let b = r.center() + Vec2::new(200.0, 300.0);
        // Click-drag at the first point pulls out a handle: a curve.
        drag(&mut h, a, a + Vec2::new(0.0, 80.0));
        click(&mut h, b);
        h.key_press(Key::Enter);
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), n + 1);
        let path = h.state_mut().editable_path().unwrap();
        assert!(path[0].anchors[0].has_handles(), "{path:?}");
        let new = &h.state().doc.shapes[h.state().sel_index().unwrap()];
        let bb = shape_bounds(new);
        let low = h.state().view.to_design(r, a).y;
        assert!(bb.max.y > low + 10.0, "it bulges below the straight line");
        assert!(h.state().doc.to_svg().unwrap().contains(" C"));
    }

    #[test]
    fn dragging_a_curved_segment_bends_it_under_the_pointer() {
        let mut h = studio();
        h.state_mut().view_tools.smart_guides = false;
        h.key_press(Key::A);
        h.run_steps(1);
        let i = sun(&h);
        h.state_mut().set_selection(&[i]);
        h.run_steps(2);
        let path = h.state_mut().editable_path().unwrap();
        let c = path[0].cubic(0, 1);
        let mid = bezier::cubic_at(&c, 0.5);
        let out = (mid - (c[0] + c[3]) * 0.5).norm() * 20.0;
        let (from, to) = (screen(&h, mid), screen(&h, mid + out));
        drag(&mut h, from, to);
        let after = h.state_mut().editable_path().unwrap();
        // The ends stay; the middle of the curve follows the pointer.
        assert_eq!(after[0].anchors[0].p, path[0].anchors[0].p);
        assert_eq!(after[0].anchors[1].p, path[0].anchors[1].p);
        let moved = bezier::cubic_at(&after[0].cubic(0, 1), 0.5);
        assert!(moved.dist(mid + out) < 2.0, "{moved:?} vs {:?}", mid + out);
        // Smooth points stay smooth.
        assert!(after[0].anchors[0].is_smooth() && after[0].anchors[1].is_smooth());
        assert_eq!(h.state().undo.len(), 1, "one undo step");
    }

    fn square() -> Harness<'static, Studio> {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100"><rect id="sq" x="20" y="20" width="60" height="60" fill="#c00"/></svg>"##;
        let doc = Document::from_svg(svg, "sq", None).unwrap();
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.state_mut().set_document(doc);
        h.run_steps(3);
        h
    }

    #[test]
    fn dragging_a_corner_widget_rounds_the_corners() {
        let mut h = square();
        h.state_mut().set_tool(Tool::Direct);
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        let path = h.state_mut().editable_path().unwrap();
        let widgets = h.state().corner_widgets(&path);
        assert_eq!(widgets.len(), 4, "a widget in every corner");
        let (c, w) = widgets[0];
        // Drag it 5 mm further into the corner, along the bisector.
        let inward = (c.u + c.v).norm() * 50.0;
        let (from, to) = (screen(&h, w), screen(&h, w + inward));
        drag(&mut h, from, to);
        let after = h.state_mut().editable_path().unwrap();
        assert_eq!(after[0].anchors.len(), 8, "all four corners cut");
        assert!(
            bezier::path_corners(&after).is_empty(),
            "no sharp corners left"
        );
        assert_eq!(h.state().undo.len(), 1);
    }

    #[test]
    fn round_corners_previews_and_ok_keeps_it_as_one_step() {
        let mut h = square();
        h.state_mut().set_selection(&[0]);
        h.state_mut()
            .run_action(crate::commands::Action::RoundCorners);
        h.run_steps(3);
        // Previewed already.
        let p = h.state().doc.edit_path(0).unwrap();
        assert_eq!(p[0].anchors.len(), 8);
        assert!(h.state().undo.is_empty(), "nothing committed yet");
        // Modal: other edits wait (here Delete would remove the square).
        h.key_press(Key::Delete);
        h.run_steps(2);
        assert_eq!(h.state().doc.shapes.len(), 1, "Delete did nothing");
        // Chamfer instead, then OK.
        h.get_by_label("Chamfer").click();
        h.run_steps(3);
        let p = h.state().doc.edit_path(0).unwrap();
        assert!(
            p[0].anchors.iter().all(|a| !a.has_handles()),
            "straight cuts"
        );
        h.get_by_label("OK").click();
        h.run_steps(3);
        assert!(h.state().layout.corner_dialog.is_none());
        assert_eq!(h.state().undo.len(), 1);
        assert_eq!(h.state().doc.edit_path(0).unwrap()[0].anchors.len(), 8);
        // Cancel puts it back.
        h.state_mut()
            .run_action(crate::commands::Action::RoundCorners);
        h.run_steps(3);
        h.get_by_label("Cancel").click();
        h.run_steps(3);
        assert_eq!(h.state().doc.edit_path(0).unwrap()[0].anchors.len(), 8);
        assert_eq!(h.state().undo.len(), 1);
    }

    #[test]
    fn the_anchor_point_tool_converts_points_and_moves_handles_alone() {
        let mut h = square();
        h.state_mut().view_tools.smart_guides = false;
        h.state_mut().set_selection(&[0]);
        h.key_press_modifiers(egui::Modifiers::SHIFT, Key::C);
        h.run_steps(2);
        assert_eq!(h.state().tool, Tool::Anchor);
        assert!(h.state().doc.shapes[0].params.kind.is_some());
        let path = h.state_mut().editable_path().unwrap();
        let corner = path[0].anchors[0].p;
        // Drag from a corner: smooth handles, the leading one under the pointer.
        let pull = P::new(24.0, -12.0);
        let (from, to) = (screen(&h, corner), screen(&h, corner + pull));
        drag(&mut h, from, to);
        let p = h.state_mut().editable_path().unwrap();
        let a = p[0].anchors[0];
        assert!(a.is_smooth(), "{a:?}");
        assert!((a.h_out - (corner + pull)).len() < 1.0, "{a:?}");
        assert!((a.h_in - (corner - pull)).len() < 1.0);
        // Drag the out handle: it moves alone, the point stops being smooth.
        let (from, to) = (screen(&h, a.h_out), screen(&h, a.h_out + P::new(0.0, 60.0)));
        drag(&mut h, from, to);
        let b = h.state_mut().editable_path().unwrap()[0].anchors[0];
        assert!((b.h_in - a.h_in).len() < 1e-6, "the other handle stayed");
        assert!(!b.is_smooth());
        // Click the in handle's end: just that handle goes back in.
        let at = screen(&h, b.h_in);
        click(&mut h, at);
        let c = h.state_mut().editable_path().unwrap()[0].anchors[0];
        assert_eq!(c.h_in, c.p);
        assert_ne!(c.h_out, c.p);
        // Click the point: a corner.
        let at = screen(&h, c.p);
        click(&mut h, at);
        let d = h.state_mut().editable_path().unwrap()[0].anchors[0];
        assert!(!d.has_handles());
        // Drag a straight side: it bends.
        let p = h.state_mut().editable_path().unwrap();
        let mid = (p[0].anchors[1].p + p[0].anchors[2].p) * 0.5;
        let (from, to) = (screen(&h, mid), screen(&h, mid + P::new(40.0, 0.0)));
        drag(&mut h, from, to);
        let p2 = h.state_mut().editable_path().unwrap();
        let bent = bezier::cubic_at(&p2[0].cubic(1, 2), 0.5);
        assert!(bent.dist(mid + P::new(40.0, 0.0)) < 2.0, "{bent:?}");
        assert_eq!(h.state().undo.len(), 5, "each edit one undo step");
    }

    #[test]
    fn add_and_delete_anchor_point_tools() {
        let mut h = square();
        h.state_mut().view_tools.smart_guides = false;
        h.state_mut().request_fit();
        h.run_steps(3);
        let before = shape_bounds(&h.state().doc.shapes[0]);
        // + (=) takes Add Anchor Point; a click on a side adds a point there.
        h.key_press(Key::Equals);
        h.run_steps(2);
        assert_eq!(h.state().tool, Tool::AddAnchor);
        assert_eq!(
            h.state().view_tools.pen_slot,
            Tool::AddAnchor,
            "the Pen slot shows it"
        );
        let p = h.state_mut().editable_path();
        assert!(p.is_none(), "nothing selected yet");
        let mid = P::new(before.center().x, before.min.y); // the middle of the top side
        let at = screen(&h, mid);
        click(&mut h, at);
        let p = h.state_mut().editable_path().unwrap();
        assert_eq!(
            p[0].anchors.len(),
            5,
            "the click picked the square up and added a point"
        );
        assert!(p[0].anchors.iter().any(|a| a.p.dist(mid) < 3.0));
        let after = shape_bounds(&h.state().doc.shapes[0]);
        assert!(
            (after.width() - before.width()).abs() < 0.5,
            "its shape unchanged"
        );
        // Option with Add Anchor Point deletes instead.
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::ALT));
        let at = screen(
            &h,
            p[0].anchors.iter().find(|a| a.p.dist(mid) < 3.0).unwrap().p,
        );
        click(&mut h, at);
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        h.run_steps(1);
        assert_eq!(h.state_mut().editable_path().unwrap()[0].anchors.len(), 4);
        // - takes Delete Anchor Point; a click on a corner takes it out.
        h.key_press(Key::Minus);
        h.run_steps(2);
        assert_eq!(h.state().tool, Tool::DeleteAnchor);
        let corner = h.state_mut().editable_path().unwrap()[0].anchors[0].p;
        let at = screen(&h, corner);
        click(&mut h, at);
        assert_eq!(h.state_mut().editable_path().unwrap()[0].anchors.len(), 3);
        assert_eq!(h.state().undo.len(), 3, "each one undo step");
    }

    #[test]
    fn the_pen_slot_has_illustrators_flyout() {
        let mut h = square();
        // Hold the Pen down: the flyout opens; let go on a tool to take it.
        let pen = h.get_by_label("Pen").rect().center();
        h.hover_at(pen);
        h.run_steps(1);
        h.drag_at(pen);
        h.run_steps(40);
        let item = h.get_by_label("Delete Anchor Point Tool").rect().center();
        h.hover_at(item);
        h.run_steps(1);
        h.drop_at(item);
        h.run_steps(3);
        assert_eq!(h.state().tool, Tool::DeleteAnchor);
        assert!(h.query_by_label("Pen Tool").is_none(), "closed");
        // The slot shows it now; Option-click steps to the next in the group.
        let slot = h.get_by_label("Delete Anchor Point").rect().center();
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::ALT));
        h.hover_at(slot);
        h.run_steps(1);
        h.drag_at(slot);
        h.run_steps(1);
        h.drop_at(slot);
        h.run_steps(2);
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        h.run_steps(1);
        assert_eq!(h.state().tool, Tool::Anchor);
        // Right-click for the flyout; tear the group off into a toolbar.
        h.get_by_label("Anchor Point").click_secondary();
        h.run_steps(2);
        assert!(h.query_by_label("Add Anchor Point Tool").is_some());
        h.get_by_label("Tear Off").click();
        h.run_steps(3);
        assert!(
            h.query_by_label("Add Anchor Point Tool").is_none(),
            "the flyout closed"
        );
        assert!(
            h.query_by_label("Pen Tools").is_some(),
            "a toolbar of its own"
        );
        h.get_all_by_label("Add Anchor Point")
            .last()
            .expect("the toolbar's button")
            .click();
        h.run_steps(2);
        assert_eq!(h.state().tool, Tool::AddAnchor);
        h.get_by_label("Close Pen Tools").click();
        h.run_steps(2);
        assert!(h.query_by_label("Pen Tools").is_none());
    }

    #[test]
    fn dragging_plans_the_stitches_once_when_let_go() {
        let mut h = studio();
        let i = sun(&h);
        h.state_mut().set_selection(&[i]);
        h.state_mut().set_tool(Tool::Direct);
        h.run_steps(2);
        let p = h.state_mut().editable_path().unwrap();
        let a = p[0].anchors[0].p;
        let from = screen(&h, a);
        let n = h.state().replans;
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        for k in 1..=6 {
            h.hover_at(from + Vec2::new(4.0 * k as f32, 0.0));
            h.run_steps(1);
        }
        assert_eq!(
            h.state().replans,
            n,
            "nothing re-planned while dragging a point"
        );
        let preview = h.state_mut().editable_path().unwrap();
        assert!(preview[0].anchors[0].p.dist(a) > 1.0, "the outline follows");
        h.drop_at(from + Vec2::new(24.0, 0.0));
        h.run_steps(3);
        assert_eq!(h.state().replans, n + 1, "once on release");
        // Moving the object with the Selection tool: the same.
        h.state_mut().set_tool(Tool::Select);
        h.run_steps(2);
        let c = screen(&h, shape_bounds(&h.state().doc.shapes[i]).center());
        let n = h.state().replans;
        h.hover_at(c);
        h.run_steps(1);
        h.drag_at(c);
        h.run_steps(1);
        for k in 1..=6 {
            h.hover_at(c + Vec2::new(0.0, 5.0 * k as f32));
            h.run_steps(1);
        }
        assert_eq!(h.state().replans, n, "nothing re-planned while moving");
        h.drop_at(c + Vec2::new(0.0, 30.0));
        h.run_steps(3);
        assert_eq!(h.state().replans, n + 1);
    }

    #[test]
    fn direct_selection_on_an_unselected_paths_outline_edits_it_right_away() {
        let mut h = square();
        h.state_mut().view_tools.smart_guides = false;
        h.state_mut().request_fit();
        h.run_steps(3);
        h.state_mut().set_tool(Tool::Direct);
        h.state_mut().set_selection(&[]);
        h.run_steps(2);
        let b = shape_bounds(&h.state().doc.shapes[0]);
        // Click the top side: the square, its two top points selected.
        let top = screen(&h, P::new(b.center().x, b.min.y));
        click(&mut h, top);
        assert_eq!(h.state().sel_index(), Some(0));
        let sel = h.state().path_edit.selected.clone();
        assert_eq!(sel.len(), 2, "{sel:?}");
        let p = h.state_mut().editable_path().unwrap();
        assert!(
            sel.iter()
                .all(|&(s, k)| (p[s].anchors[k].p.y - b.min.y).abs() < 1e-6)
        );
        // Unselected again: a drag on the side moves that side right away.
        h.state_mut().set_selection(&[]);
        h.run_steps(2);
        drag(&mut h, top, top + Vec2::new(0.0, -40.0));
        assert_eq!(h.state().sel_index(), Some(0));
        let after = shape_bounds(&h.state().doc.shapes[0]);
        assert!(after.min.y < b.min.y - 1.0, "the top side moved up");
        assert!((after.max.y - b.max.y).abs() < 1e-6, "the bottom stayed");
        // Inside (the fill): the whole object moves instead.
        h.state_mut().set_selection(&[]);
        h.run_steps(2);
        let before = shape_bounds(&h.state().doc.shapes[0]);
        let mid = screen(&h, before.center());
        drag(&mut h, mid, mid + Vec2::new(30.0, 0.0));
        let moved = shape_bounds(&h.state().doc.shapes[0]);
        assert!((moved.width() - before.width()).abs() < 1e-6);
        assert!(moved.min.x > before.min.x + 1.0);
    }

    #[test]
    fn the_selection_tool_shows_the_selected_paths_points() {
        let mut h = square();
        h.state_mut().set_tool(Tool::Select);
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        let uid = h.state().doc.shapes[0].uid;
        let rev = h.state().doc.rev();
        let cached = h.state().path_edit.outlines.get(&uid).cloned();
        assert!(
            cached.is_some_and(|(r, p)| r == rev && p.is_some_and(|p| p[0].anchors.len() == 4)),
            "its outline read once and kept for drawing its points"
        );
    }
}
