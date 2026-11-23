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
use zigzag::bezier::{self, AnchorRef, BezPath, LiveCorners};
use zigzag::geom::P;
use zigzag::svg::Geometry;

#[derive(Default)]
pub struct PathEdit {
    /// Outlines read from the document, by object uid: (the document
    /// revision read at, the path, or None when it can't be edited point by
    /// point). Reading one re-parses the SVG, so it's done once per change.
    outlines: std::collections::HashMap<usize, (u64, Option<BezPath>)>,
    /// The object whose points `selected` refers to.
    pub(crate) points_of: Option<usize>,
    /// Selected anchors of that outline. With the Selection tool, the
    /// corners picked by clicking their widgets (a drag rounds just those).
    pub selected: Vec<AnchorRef>,
    pub drag: Option<PointDrag>,
    /// Show every handle of the outline (the Pen with Option held lends the
    /// Anchor Point tool, which grabs any of them).
    pub show_all: bool,
    /// The selected object's corners, for their widgets (read once per
    /// change to the document).
    corners: Option<Corners>,
    /// The document revision the Selection tool's picked corners are of:
    /// they're let go of when it changes other than by rounding them.
    picks_at: u64,
}

/// An object's corners as their widgets show them.
struct Corners {
    uid: usize,
    rev: u64,
    /// None: no widgets for this one (text, or more points than is useful).
    live: Option<LiveCorners>,
    /// The corners of the uncut outline, each with the radius it's drawn at.
    corners: Vec<(bezier::Corner, f64)>,
    /// For each point of the path as drawn (per subpath), the anchor of
    /// the uncut outline it comes from: a rounded corner's two points come
    /// from that corner.
    from: Vec<Vec<usize>>,
}

/// How far a corner's widget keeps from the corner's two sides, on screen
/// (beyond the stitches of an outline sewn along them).
const WIDGET_CLEAR_PX: f64 = 15.0;
/// Corners with a side shorter than this on screen show no widget.
const WIDGET_SIDE_PX: f64 = 36.0;

/// A point, handle or segment drag in progress.
pub struct PointDrag {
    base_path: BezPath,
    index: usize,
    start: P,
    grab: Grab,
    /// The anchor grabbed (it snaps; the other selected ones follow it).
    lead: Option<AnchorRef>,
    /// The corner widget grabbed, the corners it rounds (anchors of the
    /// uncut outline), and the radius so far.
    corner: Option<bezier::Corner>,
    corners: Vec<AnchorRef>,
    pub radius: Option<f64>,
    /// A corner drag: the corners as it began (with the radius the grabbed
    /// one was drawn at) and as they are now, and whether the pointer has
    /// left the press (a press held still rounds nothing).
    live: Option<(LiveCorners, f64)>,
    now: Option<LiveCorners>,
    moved: bool,
    /// A corner drag shows its result in the document's shapes as it goes
    /// (the stitches follow); this is what to put back when it ends: the
    /// document revision it was shown in, and each shape's geometry by uid.
    shown: Option<(u64, Vec<(usize, Geometry)>)>,
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
        self.refresh_corners(i, path.as_ref());
        path
    }

    /// Read object `i`'s corners for their widgets, if the document has
    /// changed since they were read. `path`: its outline as drawn.
    fn refresh_corners(&mut self, i: usize, path: Option<&BezPath>) {
        let (uid, rev) = (self.doc.shapes[i].uid, self.doc.rev());
        if self
            .path_edit
            .corners
            .as_ref()
            .is_some_and(|c| c.uid == uid && c.rev == rev)
        {
            return;
        }
        // The Selection tool's picked corners are of the outline as it
        // was: a change that wasn't a rounding of them lets go of them.
        if self.tool == Tool::Select && self.path_edit.picks_at != rev {
            self.path_edit.selected.clear();
        }
        // A traced outline's thousands of points get no corner widgets
        // (nor the work of finding its corners).
        let few = path.is_some_and(|p| p.iter().map(|sp| sp.anchors.len()).sum::<usize>() <= 200);
        // (Text's corners are its letters': no live ones, but Direct
        // Selection rounds them as it edits any of its points.)
        let live = match path {
            Some(p) if few => self
                .doc
                .live_corners(i)
                .ok()
                .or_else(|| Some(LiveCorners::plain(p.clone()))),
            _ => None,
        };
        let mut corners = Vec::new();
        let mut from = Vec::new();
        if let (Some(live), Some(path)) = (&live, path) {
            let fitted = bezier::fitted_radii(&live.base, |r| live.radius(r));
            corners = bezier::path_corners(&live.base)
                .into_iter()
                .map(|c| {
                    let r = fitted
                        .iter()
                        .find(|(at, _)| *at == c.at_ref)
                        .map_or(0.0, |(_, r)| *r);
                    (c, r)
                })
                .collect();
            // Which corner each drawn point is of, by where it is (a path
            // read back from the file may have dropped a doubled point).
            let (cut, of) = live.path_from();
            from = path
                .iter()
                .enumerate()
                .map(|(s, sp)| {
                    sp.anchors
                        .iter()
                        .map(|a| {
                            cut.get(s)
                                .and_then(|c| {
                                    (0..c.anchors.len())
                                        .map(|k| (k, c.anchors[k].p.dist(a.p)))
                                        .min_by(|x, y| x.1.total_cmp(&y.1))
                                })
                                .filter(|(_, d)| *d < 0.05)
                                .map_or(usize::MAX, |(k, _)| of[s][k])
                        })
                        .collect()
                })
                .collect();
        }
        self.path_edit.corners = Some(Corners {
            uid,
            rev,
            live,
            corners,
            from,
        });
    }

    /// The selected object's corners, as last read (see `editable_path`),
    /// if they're still those of the document as it is.
    fn corners_now(&self) -> Option<&Corners> {
        let uid = self.doc.shapes.get(self.sel_index()?)?.uid;
        self.path_edit
            .corners
            .as_ref()
            .filter(|c| c.uid == uid && c.rev == self.doc.rev() && c.live.is_some())
    }

    /// The corners (anchors of the uncut outline) that drawn points belong to.
    fn corners_of_points(c: &Corners, points: &[AnchorRef]) -> Vec<AnchorRef> {
        points
            .iter()
            .filter_map(|&(s, a)| Some((s, *c.from.get(s)?.get(a)?)))
            .filter(|(_, k)| *k != usize::MAX)
            .collect()
    }

    /// The drawn points of the corner at `at` (of the uncut outline).
    fn points_of_corner(c: &Corners, (s, k): AnchorRef) -> Vec<AnchorRef> {
        c.from.get(s).map_or(Vec::new(), |from| {
            (0..from.len())
                .filter(|a| from[*a] == k)
                .map(|a| (s, a))
                .collect()
        })
    }

    /// How far the stitches of an outline reach inside the selected
    /// object's path: half the width its stroke is sewn at.
    fn outline_reach(&self) -> f64 {
        let Some(id) = self.sel_index().map(|i| &self.doc.shapes[i].id) else {
            return 0.0;
        };
        self.sel_indices()
            .into_iter()
            .map(|i| &self.doc.shapes[i])
            .chain(
                self.doc
                    .shapes
                    .iter()
                    .filter(|s| !id.is_empty() && s.id == *id),
            )
            .filter(|s| s.params.kind != Some(zigzag::params::Kind::Skip) && !s.params.hidden())
            .filter_map(|s| match &s.geometry {
                Geometry::Stroke { width, .. } => Some(width / 2.0),
                Geometry::Fill { .. } => None,
            })
            .fold(0.0, f64::max)
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

    /// Whether the Selection tool shows corner widgets: on one selected
    /// shape (its outline, selected with it, is the same path) that can be
    /// edited (neither it nor the other half of its element is locked),
    /// and not on text, which is its letters.
    pub fn select_corners(&self) -> bool {
        self.tool == Tool::Select
            && !self.playing
            && self.subjects().len() == 1
            && self.sel_index().is_some_and(|i| {
                let id = &self.doc.shapes[i].id;
                self.free(i)
                    && self.doc.lettering(i).is_none()
                    && self
                        .doc
                        .shapes
                        .iter()
                        .all(|s| id.is_empty() || s.id != *id || !s.params.locked())
            })
    }

    /// The corner widgets of the selected object (as `editable_path` last
    /// read it): each corner worth one (sides long enough on screen) and
    /// where its circle sits: inside the corner, clear of its sides and of
    /// an outline's stitches; at the centre of its round, once that's
    /// further in. A rounded corner keeps its widget (its corner being the
    /// one of the outline before it was rounded). Direct Selection shows
    /// the selected points' corners when there are any (those round
    /// together); the Selection tool shows every corner of the one shape
    /// selected, the picked ones filled in.
    pub fn corner_widgets(&self) -> Vec<(bezier::Corner, P)> {
        let direct = match self.tool {
            Tool::Direct => true,
            Tool::Select if self.select_corners() => false,
            _ => return Vec::new(),
        };
        let Some(c) = self.corners_now() else {
            return Vec::new();
        };
        if c.corners.len() > 64 {
            return Vec::new();
        }
        let shown = (direct && !self.path_edit.selected.is_empty())
            .then(|| Self::corners_of_points(c, &self.path_edit.selected));
        let px = 10.0 / self.view.scale as f64;
        let clear = WIDGET_CLEAR_PX * px + self.outline_reach();
        c.corners
            .iter()
            .filter(|(c, _)| shown.as_ref().is_none_or(|s| s.contains(&c.at_ref)))
            .filter(|(c, _)| c.lu.min(c.lv) > WIDGET_SIDE_PX * px)
            .filter_map(|&(c, r)| {
                // Along the bisector: half the angle between the sides
                // has this cosine and sine.
                let b = c.u + c.v;
                let cos = b.len() / 2.0;
                let sin = (1.0 - cos * cos).max(0.0).sqrt();
                if cos <= 1e-9 || sin <= 1e-9 {
                    return None;
                }
                // Clear of both sides, no further than level with half
                // the shorter one; or the centre of the round.
                let inset = (clear / sin).min(c.lu.min(c.lv) / (2.0 * cos));
                Some((c, c.at + b * ((r / sin).max(inset) / b.len())))
            })
            .collect()
    }

    /// The corner whose widget is under `q` (design units), if one is.
    fn corner_at(&self, q: P) -> Option<AnchorRef> {
        let reach = 6.0 * 10.0 / self.view.scale as f64;
        self.corner_widgets()
            .into_iter()
            .filter(|(_, w)| w.dist(q) <= reach)
            .min_by(|a, b| a.1.dist(q).total_cmp(&b.1.dist(q)))
            .map(|(c, _)| c.at_ref)
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
            if let Some(at) = self.corner_at(q) {
                return Hit::Corner(at);
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
        // (An outline's points are its shape's: over it, the shape.)
        match crate::canvas::hit_test(&self.doc.shapes, q, tol) {
            Some(i) => Hit::Object(self.shape_of(i)),
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
        // (One left hanging, its release missed, isn't carried into this.)
        self.cancel_point_drag();
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
            live: None,
            now: None,
            moved: false,
            shown: None,
            path,
        });
    }

    /// Start rounding from the widget of the corner at `at` (Illustrator's
    /// Live Corners, as VectorCraft has them): the corners it rounds all
    /// take the radius the pointer gives this one, more or less than they
    /// had. Direct Selection rounds the corners whose widgets show: the
    /// selected points', or all of them. The Selection tool rounds every
    /// corner of the shape, unless some were picked by clicking their
    /// widgets: then just those, or just this one when it isn't among them.
    fn begin_corner_drag(&mut self, path: BezPath, at: AnchorRef, q: P) {
        let widgets = self.corner_widgets();
        let Some(c) = widgets
            .iter()
            .find(|(c, _)| c.at_ref == at)
            .map(|(c, _)| *c)
        else {
            return;
        };
        let Some(now) = self.corners_now() else {
            return;
        };
        let Some(live) = now.live.clone() else {
            return;
        };
        let drawn = now
            .corners
            .iter()
            .find(|(c, _)| c.at_ref == at)
            .map_or(0.0, |(_, r)| *r);
        let every: Vec<AnchorRef> = now.corners.iter().map(|(c, _)| c.at_ref).collect();
        let set: Vec<AnchorRef> = if self.tool == Tool::Select {
            let picked = &mut self.path_edit.selected;
            if picked.is_empty() {
                every
            } else {
                if !picked.contains(&at) {
                    *picked = vec![at];
                    self.path_edit.picks_at = self.doc.rev();
                }
                self.path_edit.selected.clone()
            }
        } else {
            widgets.iter().map(|(c, _)| c.at_ref).collect()
        };
        self.begin_point_drag(path, q, Grab::Corner, None);
        if let Some(d) = &mut self.path_edit.drag {
            d.corner = Some(c);
            d.corners = set;
            d.live = Some((live, drawn));
        }
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
        let px = 10.0 / self.view.scale as f64;
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
                let (Some(c), Some((live, drawn))) = (d.corner, &d.live) else {
                    return;
                };
                // Until the pointer has left the press, nothing: a widget
                // pressed and held, or nudged a hair, rounds no corner.
                if !d.moved && delta.len() < 3.0 * px {
                    return;
                }
                d.moved = true;
                // From the radius it's drawn at, by how far the pointer has
                // gone along the corner's bisector; no more than fits.
                let set = |live: &LiveCorners, r: f64| {
                    let mut now = live.clone();
                    for &k in &d.corners {
                        now.set_radius(k, r);
                    }
                    now
                };
                let asked = (drawn + c.radius_change(delta)).max(0.0);
                let fits = set(live, asked).drawn(c.at_ref);
                let now = set(live, asked.min(fits));
                d.radius = Some(fits.min(asked));
                path = if now == *live { path } else { now.path() };
                d.now = Some(now);
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
        // designs. The document changes once, on release. (A corner being
        // rounded shows in the stitches too, without the SVG: see
        // `show_corner_drag`.)
        let same = d.path == path;
        d.path = path;
        if matches!(grab, Grab::Corner) && !same {
            self.show_corner_drag();
        }
    }

    /// A corner being rounded shows as it goes: the shapes of the path take
    /// the geometry the rounded outline will give them (the SVG isn't
    /// touched until the drag ends), and where planning fits in a frame
    /// the stitches are planned from that, here and now. (A bigger
    /// design's outline follows alone, its stitches when the drag ends:
    /// asked of the worker at every move, no plan would be the newest long
    /// enough to be shown.)
    fn show_corner_drag(&mut self) {
        let Some(d) = &mut self.path_edit.drag else {
            return;
        };
        // The document was read afresh under the drag (a save, an edit by
        // a shortcut): its shapes are no longer the ones this began with.
        if d.shown
            .as_ref()
            .is_some_and(|(rev, _)| *rev != self.doc.rev())
        {
            self.path_edit.drag = None;
            return;
        }
        let Some(shape) = self.doc.shapes.get(d.index) else {
            return;
        };
        if d.shown.is_none() {
            let id = shape.id.clone();
            let was = self
                .doc
                .shapes
                .iter()
                .enumerate()
                .filter(|(i, s)| *i == d.index || (!id.is_empty() && s.id == id))
                .map(|(_, s)| (s.uid, s.geometry.clone()))
                .collect();
            d.shown = Some((self.doc.rev(), was));
        }
        self.doc.preview_path(d.index, &d.path);
        if !self.planner.slow {
            self.dirty = true;
            self.replan();
        }
    }

    /// Put back the shapes a corner drag showed its result in. (Not into
    /// a document that has changed since: its shapes were read afresh.)
    fn unshow_corner_drag(&mut self, shown: Option<(u64, Vec<(usize, Geometry)>)>) {
        let Some((rev, was)) = shown else { return };
        if self.doc.rev() != rev {
            return;
        }
        for (uid, geometry) in was {
            if let Some(s) = self.doc.shapes.iter_mut().find(|s| s.uid == uid) {
                s.geometry = geometry;
            }
        }
        self.dirty = true;
    }

    /// Drop the point drag in progress, nothing changed.
    pub(crate) fn cancel_point_drag(&mut self) {
        if let Some(d) = self.path_edit.drag.take() {
            self.unshow_corner_drag(d.shown);
        }
    }

    fn end_point_drag(&mut self) {
        let Some(d) = self.path_edit.drag.take() else {
            return;
        };
        // The undo step keeps the shapes as they were before the drag.
        self.unshow_corner_drag(d.shown);
        if matches!(d.grab, Grab::Corner) {
            if let (Some(now), Some((was, _))) = (d.now, d.live)
                && now != was
            {
                self.commit_corners(now, &d.corners);
            }
            // The stitches of what the drag left, before this frame draws
            // (they followed the drag; a frame of the old ones would flicker).
            self.plan_after_gesture();
            return;
        }
        if d.path != d.base_path {
            let what = match d.grab {
                Grab::Anchors => "Moved points.",
                Grab::Handle { .. } | Grab::Segment { .. } => "Reshaped the curve.",
                Grab::Corner => "Rounded the corners.",
                Grab::Pull { .. } => "Made a smooth point.",
            };
            self.commit_path(d.path, what);
        }
    }

    /// Round the selected object's corners as `live` says, as one undo
    /// step. They stay live: the uncut outline and the radii are kept with
    /// the path, so the same widgets round them more or less afterwards.
    /// `rounded`: the corners this changed, which stay picked (Selection
    /// tool) or selected (Direct Selection, as the points they are now).
    fn commit_corners(&mut self, live: LiveCorners, rounded: &[AnchorRef]) {
        let Some(i) = self.sel_index() else { return };
        let uid = self.doc.shapes[i].uid;
        let sharp = live.is_plain();
        match self.structural(|d| d.set_live_corners(i, &live)) {
            Ok(()) => {
                self.status = if sharp {
                    "Made the corners sharp.".into()
                } else {
                    "Rounded the corners.".into()
                }
            }
            Err(e) => {
                self.status = format!("Couldn't round the corners: {e:#}");
                return;
            }
        }
        // (Writing the path may have moved the object in the list.)
        if self.sel_index().is_none() {
            self.restore_selection(&[uid]);
        }
        match self.tool {
            Tool::Select => self.path_edit.picks_at = self.doc.rev(),
            // The points picked are now the points of the same corners.
            _ if !self.path_edit.selected.is_empty() => {
                self.path_edit.selected.clear();
                self.editable_path();
                if let Some(c) = self.corners_now() {
                    self.path_edit.selected = rounded
                        .iter()
                        .flat_map(|&k| Self::points_of_corner(c, k))
                        .collect();
                }
            }
            _ => {}
        }
    }

    /// Corner widgets with the Selection tool: dragging one rounds every
    /// corner of the selected shape; a click on one picks that corner (it
    /// fills in), and a drag then rounds just the picked ones. Shift-click
    /// picks more; a click on the only one picked, or anywhere else, goes
    /// back to all of them. True when the pointer is a widget's this
    /// frame, and the Selection tool leaves it alone.
    pub fn corners_canvas(
        &mut self,
        ctx: &egui::Context,
        resp: &egui::Response,
        rect: Rect,
        mods: DragMods,
    ) -> bool {
        if self.tool != Tool::Select {
            return false;
        }
        if self.path_edit.drag.is_some() {
            if resp.dragged_by(egui::PointerButton::Primary)
                && let Some(pos) = resp.interact_pointer_pos()
            {
                self.update_point_drag(self.view.to_design(rect, pos), mods);
                ctx.request_repaint();
            }
            if resp.drag_stopped() || !ctx.input(|i| i.pointer.primary_down()) {
                self.end_point_drag();
            }
            return true;
        }
        if self.drag.is_some() || self.marquee.is_some() {
            return false;
        }
        if !self.select_corners() {
            // (Picked corners are one shape's, while it's the one selected.)
            self.path_edit.selected.clear();
            return false;
        }
        let Some(path) = self.editable_path() else {
            return false;
        };
        let at = |s: &Studio, pos: Option<Pos2>| s.corner_at(s.view.to_design(rect, pos?));
        if resp.drag_started_by(egui::PointerButton::Primary) {
            let start = ctx
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos());
            let Some(c) = at(self, start) else {
                return false;
            };
            let q = self.view.to_design(rect, start.unwrap_or_default());
            self.begin_corner_drag(path, c, q);
            return self.path_edit.drag.is_some();
        }
        if resp.clicked() {
            let Some(c) = at(self, resp.interact_pointer_pos()) else {
                // The Selection tool's click: no corner stays picked.
                self.path_edit.selected.clear();
                return false;
            };
            let picked = &mut self.path_edit.selected;
            if mods.shift {
                match picked.iter().position(|x| *x == c) {
                    Some(k) => {
                        picked.remove(k);
                    }
                    None => picked.push(c),
                }
            } else if picked.as_slice() == [c] {
                picked.clear();
            } else {
                *picked = vec![c];
            }
            self.path_edit.picks_at = self.doc.rev();
            let picked = &self.path_edit.selected;
            self.status = match picked.len() {
                0 => "Drag a corner's circle to round every corner.".into(),
                1 => "Drag to round just this corner. Shift-click picks more; click it again for all of them.".into(),
                n => format!("Drag to round these {n} corners."),
            };
            return true;
        }
        if at(self, resp.hover_pos()).is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            return true;
        }
        false
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
                Hit::Corner(at) => self.begin_corner_drag(path.clone().unwrap(), at, q),
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
                // A corner's widget: its point, so that the next drag on
                // the widget rounds that corner alone (VectorCraft's way).
                Hit::Corner(at) => {
                    // (A rounded corner is two points: both.)
                    let points = self
                        .corners_now()
                        .map_or(Vec::new(), |c| Self::points_of_corner(c, at));
                    let sel = &mut self.path_edit.selected;
                    if !mods.shift {
                        *sel = points;
                    } else if points.iter().all(|a| sel.contains(a)) {
                        sel.retain(|a| !points.contains(a));
                    } else {
                        for a in points {
                            if !sel.contains(&a) {
                                sel.push(a);
                            }
                        }
                    }
                }
                Hit::Handle(..) => {}
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
            let hit = crate::canvas::hit_test(&self.doc.shapes, q, tol).map(|i| self.shape_of(i));
            match hit {
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
            snap: self.prefs.smart_guides,
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
        self.draw_corner_widgets(painter, rect);
    }

    /// The Selection tool's corner widgets, on the one shape selected.
    pub fn draw_select_corners(&mut self, painter: &egui::Painter, rect: Rect) {
        if self.path_edit.drag.is_none() && (self.drag.is_some() || !self.select_corners()) {
            return;
        }
        if self.editable_path().is_some() {
            self.draw_corner_widgets(painter, rect);
        }
    }

    /// Corner widgets: a ringed dot inside each corner to drag (filled in
    /// when the Selection tool has picked that corner); while one is
    /// dragged, the radius beside the pointer instead.
    fn draw_corner_widgets(&self, painter: &egui::Painter, rect: Rect) {
        let Some(d) = &self.path_edit.drag else {
            let picked: &[AnchorRef] = if self.tool == Tool::Select {
                &self.path_edit.selected
            } else {
                &[]
            };
            // As heavy as the lines of the icons: a whole number of pixels.
            let line = crate::icons::line_width(9.0, painter.ctx().pixels_per_point());
            for (c, w) in self.corner_widgets() {
                let w = self.view.to_screen(rect, w);
                let (fill, ink) = if picked.contains(&c.at_ref) {
                    (BLUE, Color32::WHITE)
                } else {
                    (Color32::WHITE, BLUE)
                };
                painter.circle_filled(w, 4.5, fill);
                painter.circle_stroke(w, 4.5, Stroke::new(line, BLUE));
                painter.circle_filled(w, 1.5, ink);
            }
            return;
        };
        if let Some(r) = d.radius
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
    use zigzag::geom::{rotate_about, scale_about, translation};

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
    fn a_broken_out_column_is_picked_inside_and_its_rails_reshaped() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
          <path id="H_satin" fill="#123456" d="M10 10 L13 10 L13 19 L27 19 L27 10 L30 10 L30 30 L27 30 L27 21 L13 21 L13 30 L10 30 Z"/>
        </svg>"##;
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.state_mut()
            .set_document(Document::from_svg(svg, "t", None).unwrap());
        h.run_steps(3);
        h.state_mut().set_selection(&[0]);
        h.state_mut().break_into_columns_selected();
        h.run_steps(3);
        let n = h.state().doc.shapes.len();
        assert!(n >= 3, "{n} columns");
        // In the middle of a column, nowhere near its rails' lines: halfway
        // across it, halfway along. (Not the middle of its bounds, which a
        // column that turns a corner needn't cover.)
        let inside = match &h.state().doc.shapes[0].geometry {
            Geometry::Stroke { lines, .. } if lines.len() >= 2 => {
                let mid = |l: &[P]| l[0].lerp(l[l.len() - 1], 0.5);
                mid(&lines[0].0).lerp(mid(&lines[1].0), 0.5)
            }
            _ => shape_bounds(&h.state().doc.shapes[0]).center(),
        };
        let at = screen(&h, inside);
        h.state_mut().set_tool(Tool::Select);
        h.state_mut().set_selection(&[]);
        h.run_steps(2);
        click(&mut h, at);
        assert_eq!(
            h.state().sel_indices(),
            (0..n).collect::<Vec<_>>(),
            "the group, as a group is"
        );
        h.state_mut().set_tool(Tool::Direct);
        h.state_mut().set_selection(&[]);
        h.run_steps(2);
        click(&mut h, at);
        assert_eq!(h.state().sel_indices(), vec![0], "the column itself");
        // Its rails are a few points each, to drag.
        let path = h.state_mut().editable_path().expect("its rails and rungs");
        assert!(
            path[0].anchors.len() <= 4 && path[1].anchors.len() <= 4,
            "{:?}",
            path.iter().map(|sp| sp.anchors.len()).collect::<Vec<_>>()
        );
        let a = path[0].anchors[0].p;
        let was = h.state().doc.shapes.clone();
        let undo = h.state().undo.len();
        let from = screen(&h, a);
        drag(&mut h, from, from + Vec2::new(30.0, 0.0));
        let now = &h.state().doc.shapes;
        assert_ne!(
            format!("{:?}", now[0].geometry),
            format!("{:?}", was[0].geometry),
            "reshaped"
        );
        assert!(
            (1..n).all(|k| format!("{:?}", now[k].geometry) == format!("{:?}", was[k].geometry)),
            "the others as they were"
        );
        assert_eq!(now[0].params.rails, Some((1, 2)), "still two rails");
        assert_eq!(h.state().undo.len(), undo + 1);
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
        h.state_mut().prefs.smart_guides = false;
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
        let widgets = widgets(&mut h);
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

    /// The corner widgets showing on the selected object.
    fn widgets(h: &mut Harness<'_, Studio>) -> Vec<(bezier::Corner, P)> {
        h.state_mut().editable_path().unwrap();
        h.state().corner_widgets()
    }

    /// The radius each corner of object `i` is rounded with, in anchor order.
    fn radii(h: &Harness<'_, Studio>, i: usize) -> Vec<f64> {
        h.state().doc.live_corners(i).unwrap().radii.concat()
    }

    /// How far the nearest stitch is from `p`.
    fn nearest_stitch(h: &Harness<'_, Studio>, p: P) -> f64 {
        h.state()
            .design
            .blocks
            .iter()
            .flat_map(|b| &b.runs)
            .flat_map(|r| &r.pts)
            .map(|q| q.dist(p))
            .fold(f64::INFINITY, f64::min)
    }

    fn ring_len(h: &Harness<'_, Studio>, i: usize) -> usize {
        match &h.state().doc.shapes[i].geometry {
            Geometry::Fill { rings, .. } => rings[0].len(),
            Geometry::Stroke { lines, .. } => lines[0].0.len(),
        }
    }

    const NO_MODS: DragMods = DragMods {
        shift: false,
        alt: false,
        snap: false,
        tol: 0.0,
    };

    /// A drag of the widget at `at` by `by` along its corner's bisector
    /// (towards the middle of the shape when positive), in design units.
    fn pull(h: &mut Harness<'_, Studio>, (c, at): (bezier::Corner, P), by: f64) {
        let (from, to) = (screen(h, at), screen(h, at + (c.u + c.v).norm() * by));
        drag(h, from, to);
    }

    #[test]
    fn the_selection_tool_rounds_every_corner_from_one_widget() {
        let mut h = square();
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        assert_eq!(h.state().tool, Tool::Select);
        let w = widgets(&mut h);
        assert_eq!(w.len(), 4, "a widget in every corner");
        let (c, at) = w[0];
        assert!(nearest_stitch(&h, c.at) < 8.0, "stitches reach the corner");
        // 5 mm further into the corner, along the bisector.
        let inward = (c.u + c.v).norm() * 50.0;
        let (from, to) = (screen(&h, at), screen(&h, at + inward));
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        h.hover_at(from + (to - from) * 0.5);
        h.run_steps(2);
        // Halfway: the shape and its stitches show it, the SVG waits.
        assert!(h.state().path_edit.drag.is_some());
        assert!(ring_len(&h, 0) > 4, "the shape previews the rounds");
        assert!(!h.state().dirty, "and its stitches are planned from it");
        assert!(nearest_stitch(&h, c.at) > 3.0, "they've left the corner");
        assert!(h.state().undo.is_empty(), "nothing committed yet");
        assert_eq!(h.state().doc.edit_path(0).unwrap()[0].anchors.len(), 4);
        assert!(h.state().path_edit.drag.as_ref().unwrap().radius.unwrap() > 10.0);
        // Held still, nothing is planned again.
        let plans = h.state().replans;
        h.run_steps(3);
        assert_eq!(h.state().replans, plans, "no plan per frame held");
        h.hover_at(to);
        h.run_steps(1);
        h.drop_at(to);
        h.run_steps(3);
        let after = h.state().doc.edit_path(0).unwrap();
        assert_eq!(after[0].anchors.len(), 8, "all four corners cut");
        assert!(bezier::path_corners(&after).is_empty());
        // Each cut starts 3.5 mm (50 / √2) along its side.
        let r = 50.0 / 2f64.sqrt();
        assert!(
            after[0]
                .anchors
                .iter()
                .any(|a| a.p.dist(c.at + c.u * r) < 0.5),
            "{after:?}"
        );
        assert_eq!(h.state().undo.len(), 1, "one undo step");
        assert!(h.state().path_edit.drag.is_none());
        assert_eq!(h.state().sel_index(), Some(0), "still selected");
        // Undo: square again, in the SVG and in the shape.
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
        h.run_steps(3);
        assert_eq!(h.state().doc.edit_path(0).unwrap()[0].anchors.len(), 4);
        assert_eq!(ring_len(&h, 0), 4);
        assert!(nearest_stitch(&h, c.at) < 8.0);
    }

    #[test]
    fn rounded_corners_keep_their_widgets_and_round_more_or_less() {
        let mut h = square();
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        let w = widgets(&mut h);
        pull(&mut h, w[0], 50.0);
        let r = 50.0 / 2f64.sqrt();
        for got in radii(&h, 0) {
            assert!((got - r).abs() < 0.5, "{got} for {r}");
        }
        // The widgets are still there, each at the centre of its round.
        let w = widgets(&mut h);
        assert_eq!(w.len(), 4, "a rounded corner keeps its widget");
        let (c, at) = w[0];
        let centre = c.at + (c.u + c.v).norm() * (r * 2f64.sqrt());
        assert!(at.dist(centre) < 1.0, "{at:?} vs {centre:?}");
        // Further in: rounder, from the radius they had.
        pull(&mut h, w[0], 30.0);
        let more = r + 30.0 / 2f64.sqrt();
        for got in radii(&h, 0) {
            assert!((got - more).abs() < 0.5, "{got} for {more}");
        }
        assert_eq!(h.state().doc.edit_path(0).unwrap()[0].anchors.len(), 8);
        assert_eq!(h.state().undo.len(), 2);
        // Back out past the corner: sharp again, a plain square.
        let w = widgets(&mut h);
        pull(&mut h, w[2], -200.0);
        assert_eq!(radii(&h, 0), vec![0.0; 4]);
        assert_eq!(h.state().doc.edit_path(0).unwrap()[0].anchors.len(), 4);
        assert!(!h.state().doc.to_svg().unwrap().contains("data-tl-corners"));
        assert_eq!(widgets(&mut h).len(), 4);
        assert_eq!(h.state().undo.len(), 3);
        // As far in as it goes: each corner takes half its sides, no more.
        let w = widgets(&mut h);
        pull(&mut h, w[0], 2000.0);
        for got in radii(&h, 0) {
            assert!((got - 300.0).abs() < 0.5, "{got}");
        }
        assert_eq!(widgets(&mut h).len(), 4, "a circle now, still four corners");
    }

    #[test]
    fn live_corners_come_back_from_the_file_and_follow_the_shape() {
        let mut h = square();
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        let w = widgets(&mut h);
        pull(&mut h, w[0], 50.0);
        let r = 50.0 / 2f64.sqrt();
        // Saved and opened again.
        let svg = h.state().doc.to_svg().unwrap();
        let again = Document::from_svg(&svg, "sq", None).unwrap();
        let live = again.live_corners(0).unwrap();
        assert_eq!(live.base[0].anchors.len(), 4);
        assert!(
            live.radii[0].iter().all(|x| (x - r).abs() < 0.5),
            "{live:?}"
        );
        // Moved, turned and scaled evenly, they're the same corners.
        let s = h.state_mut();
        s.transform_selected(translation(P::new(40.0, -25.0)), "Moved");
        s.transform_selected(rotate_about(P::new(0.0, 0.0), 30.0), "Rotated");
        s.transform_selected(scale_about(P::new(0.0, 0.0), 0.5, 0.5), "Resized");
        h.run_steps(2);
        for got in radii(&h, 0) {
            assert!((got - r / 2.0).abs() < 0.3, "{got}");
        }
        assert_eq!(widgets(&mut h).len(), 4);
        // A copy has them too.
        h.state_mut().run_action(crate::commands::Action::Duplicate);
        h.run_steps(2);
        let copy = h.state().sel_index().unwrap();
        assert_ne!(copy, 0);
        assert!(radii(&h, copy).iter().all(|x| (x - r / 2.0).abs() < 0.3));
        // Stretched, its corners stay round (not squashed with it), and
        // live: the outline is the stretched one, cut again.
        let was = bezier::bounds(&h.state().doc.live_corners(copy).unwrap().base);
        h.state_mut()
            .transform_selected(scale_about(P::new(0.0, 0.0), 2.0, 1.0), "Resized");
        h.run_steps(2);
        let i = h.state().sel_index().unwrap();
        let live = h.state().doc.live_corners(i).unwrap();
        let b = bezier::bounds(&live.base);
        assert!((b.width() - was.width() * 2.0).abs() < 0.5, "{b:?}");
        assert!((b.height() - was.height()).abs() < 0.5, "{b:?}");
        let want = r / 2.0 * 2f64.sqrt();
        assert!(
            live.radii[0].iter().all(|x| (x - want).abs() < 0.3),
            "{live:?}"
        );
        let drawn = h.state().doc.edit_path(i).unwrap();
        assert_eq!(drawn[0].anchors.len(), 8);
        let corner = live.base[0].anchors[0].p;
        let near: Vec<f64> = drawn[0]
            .anchors
            .iter()
            .map(|a| a.p.dist(corner))
            .filter(|d| *d < want * 1.5)
            .collect();
        assert_eq!(near.len(), 2, "{near:?}");
        assert!(
            (near[0] - near[1]).abs() < 0.05,
            "a round, not an oval: {near:?}"
        );
        assert_eq!(widgets(&mut h).len(), 4);
        // A path whose points were edited some other way is a plain path,
        // its rounds ordinary points.
        h.state_mut().set_selection(&[0]);
        let mut path = h.state().doc.edit_path(0).unwrap();
        path[0].anchors[0].p.x += 5.0;
        h.state_mut().structural(|d| d.set_path(0, &path)).unwrap();
        h.run_steps(2);
        assert_eq!(radii(&h, 0), vec![0.0; 8]);
        let svg = h.state().doc.to_svg().unwrap();
        assert_eq!(svg.matches("data-tl-corners").count(), 1, "the copy's");
    }

    #[test]
    fn a_path_changed_behind_its_corners_is_a_plain_path() {
        let mut h = square();
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        let w = widgets(&mut h);
        pull(&mut h, w[0], 50.0);
        // Another program rewrote the path and kept the attribute.
        let svg = h.state().doc.to_svg().unwrap();
        assert!(svg.contains("data-tl-corners"));
        let d = svg
            .split(" d=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let other = svg.replace(d, "M 20 20 L 80 20 L 80 80 L 30 80 Z");
        let doc = Document::from_svg(&other, "sq", None).unwrap();
        let live = doc.live_corners(0).unwrap();
        assert!(live.is_plain(), "what's kept is for another outline");
        assert_eq!(live.base, doc.edit_path(0).unwrap());
    }

    #[test]
    fn a_clicked_widget_rounds_its_corner_alone() {
        let mut h = square();
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        let w = widgets(&mut h);
        let (c, at) = w[1];
        // A click picks the corner; every widget still shows.
        let on = screen(&h, at);
        click(&mut h, on);
        assert_eq!(h.state().path_edit.selected, vec![c.at_ref]);
        assert_eq!(widgets(&mut h).len(), 4);
        assert!(h.state().undo.is_empty(), "a click changes nothing");
        assert_eq!(h.state().sel_index(), Some(0));
        pull(&mut h, w[1], 50.0);
        let after = h.state().doc.edit_path(0).unwrap();
        assert_eq!(after[0].anchors.len(), 5, "one corner cut");
        let r = 50.0 / 2f64.sqrt();
        let got = radii(&h, 0);
        assert!((got[1] - r).abs() < 0.5 && got[0] + got[2] + got[3] == 0.0);
        assert_eq!(h.state().undo.len(), 1);
        // It stays picked: the next drag is its own again.
        assert_eq!(h.state().path_edit.selected, vec![c.at_ref]);
        let w = widgets(&mut h);
        assert_eq!(w.len(), 4);
        pull(&mut h, w[1], 20.0);
        let got = radii(&h, 0);
        assert!(got[1] > r + 10.0 && got[0] + got[2] + got[3] == 0.0);

        // With one picked, a drag on another's widget rounds that one.
        pull(&mut h, w[3], 30.0);
        let now = radii(&h, 0);
        assert!((now[3] - 30.0 / 2f64.sqrt()).abs() < 0.5, "{now:?}");
        assert_eq!((now[0], now[1], now[2]), (0.0, got[1], 0.0));
        assert_eq!(h.state().path_edit.selected, vec![w[3].0.at_ref]);

        // Shift-click picks another as well; again lets go of it.
        let w = widgets(&mut h);
        let on = screen(&h, w[0].1);
        let shift_click = |h: &mut Harness<'_, Studio>, at: Pos2| {
            h.event(egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
            click(h, at);
            h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
            h.run_steps(1);
        };
        shift_click(&mut h, on);
        assert_eq!(
            h.state().path_edit.selected,
            vec![w[3].0.at_ref, w[0].0.at_ref]
        );
        // Both round together, to the radius of the one dragged.
        pull(&mut h, w[0], 40.0);
        let now = radii(&h, 0);
        assert!((now[0] - 40.0 / 2f64.sqrt()).abs() < 0.5, "{now:?}");
        assert!((now[3] - now[0]).abs() < 1e-6, "{now:?}");
        assert_eq!((now[1], now[2]), (got[1], 0.0));

        // A click on the only one picked lets go of it; so does a click
        // on the shape, and deselecting.
        let w = widgets(&mut h);
        let on = screen(&h, w[2].1);
        click(&mut h, on);
        assert_eq!(h.state().path_edit.selected, vec![w[2].0.at_ref]);
        click(&mut h, on);
        assert!(h.state().path_edit.selected.is_empty());
        click(&mut h, on);
        let middle = screen(&h, P::new(0.0, 0.0));
        click(&mut h, middle);
        assert!(h.state().path_edit.selected.is_empty());
        assert_eq!(h.state().sel_index(), Some(0));
        click(&mut h, on);
        h.state_mut().set_selection(&[]);
        h.run_steps(2);
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        assert!(h.state().path_edit.selected.is_empty());
        // None picked: every corner takes the radius of the one dragged.
        let w = widgets(&mut h);
        pull(&mut h, w[2], 60.0);
        let now = radii(&h, 0);
        assert!(now.iter().all(|x| (x - 60.0 / 2f64.sqrt()).abs() < 0.5));
    }

    #[test]
    fn a_widget_pressed_and_held_rounds_nothing() {
        let mut h = square();
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        let (c, at) = widgets(&mut h)[0];
        let path = h.state_mut().editable_path().unwrap();
        // A hair of drift (under three pixels on screen), then let go.
        let px = 10.0 / h.state().view.scale as f64;
        let s = h.state_mut();
        s.begin_corner_drag(path, c.at_ref, at);
        s.update_point_drag(at + (c.u + c.v).norm() * (2.0 * px), NO_MODS);
        assert!(s.path_edit.drag.as_ref().unwrap().radius.is_none());
        s.end_point_drag();
        assert!(s.undo.is_empty());
        assert_eq!(ring_len(&h, 0), 4);
        assert_eq!(widgets(&mut h).len(), 4);
    }

    #[test]
    fn a_big_designs_corner_drag_leaves_planning_to_the_release() {
        let mut h = square();
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        let (c, at) = widgets(&mut h)[0];
        let path = h.state_mut().editable_path().unwrap();
        let s = h.state_mut();
        // As after a slow plan: the worker's from now on.
        s.planner.slow = true;
        let plans = s.replans;
        s.begin_corner_drag(path, c.at_ref, at);
        s.update_point_drag(at + (c.u + c.v).norm() * 50.0, NO_MODS);
        s.update_point_drag(at + (c.u + c.v).norm() * 60.0, NO_MODS);
        assert_eq!(s.replans, plans, "no plan while it's dragged");
        assert!(!s.dirty, "and none asked of the worker at every move");
        assert!(ring_len(&h, 0) > 4, "the outline follows");
        let s = h.state_mut();
        s.end_point_drag();
        assert!(s.dirty, "planned when it's let go");
        assert_eq!(s.undo.len(), 1);
    }

    #[test]
    fn a_corner_drag_ends_with_its_stitches_planned() {
        // A square with an outline: one path, two objects.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100"><rect id="sq" x="20" y="20" width="60" height="60" fill="#c00" stroke="#024" stroke-width="2"/></svg>"##;
        let mut h = square();
        h.state_mut()
            .set_document(Document::from_svg(svg, "sq", None).unwrap());
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), 2);
        h.state_mut().set_selection(&[0, 1]);
        h.run_steps(2);
        let w = widgets(&mut h);
        assert_eq!(w.len(), 4, "the shape and its outline are one thing");
        let (c, at) = w[0];
        // The widget stands clear of the outline's stitches (1 mm each
        // side of the path) by as much as it does of a bare edge.
        let clear = (at - c.at).dot(P::new(-c.u.y, c.u.x)).abs();
        let px = 10.0 / h.state().view.scale as f64;
        assert!(
            (clear - (10.0 + WIDGET_CLEAR_PX * px)).abs() < 0.01,
            "{clear}"
        );
        let path = h.state_mut().editable_path().unwrap();
        let was = (ring_len(&h, 0), ring_len(&h, 1));
        let s = h.state_mut();
        s.begin_corner_drag(path, c.at_ref, at);
        s.update_point_drag(at + (c.u + c.v).norm() * 50.0, NO_MODS);
        assert!(!s.dirty, "planned as it goes");
        assert!(ring_len(&h, 0) > was.0 && ring_len(&h, 1) > was.1);
        assert!(nearest_stitch(&h, c.at) > 3.0);
        // Back out to nothing: the shapes as they were, nothing to undo.
        let s = h.state_mut();
        s.update_point_drag(at - (c.u + c.v).norm() * 5.0, NO_MODS);
        s.end_point_drag();
        assert!(!s.dirty);
        assert!(s.undo.is_empty());
        assert_eq!((ring_len(&h, 0), ring_len(&h, 1)), was);
        assert!(nearest_stitch(&h, c.at) < 15.0);

        // And for real: when the drag ends, the stitches are those of the
        // document it left, before any frame is drawn.
        let path = h.state_mut().editable_path().unwrap();
        let s = h.state_mut();
        s.begin_corner_drag(path, c.at_ref, at);
        s.update_point_drag(at + (c.u + c.v).norm() * 50.0, NO_MODS);
        s.end_point_drag();
        assert!(!s.dirty, "no frame of the old stitches");
        assert_eq!(s.undo.len(), 1);
        let fresh = s.doc.plan_cached(&mut Default::default());
        assert_eq!(fresh.stitch_count(), s.design.stitch_count());
        assert!(nearest_stitch(&h, c.at) > 3.0);
        // The outline is rounded with its shape.
        assert!(ring_len(&h, 0) > was.0 && ring_len(&h, 1) > was.1);
        assert_eq!(h.state().doc.edit_path(1).unwrap()[0].anchors.len(), 8);
    }

    #[test]
    fn an_interrupted_corner_drag_leaves_the_shapes_as_they_were() {
        let mut h = square();
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        let (c, at) = widgets(&mut h)[0];
        let path = h.state_mut().editable_path().unwrap();
        let begin = |s: &mut Studio| {
            s.editable_path();
            s.begin_corner_drag(path.clone(), c.at_ref, at);
            s.update_point_drag(at + (c.u + c.v).norm() * 50.0, NO_MODS);
            assert!(s.path_edit.drag.is_some());
        };
        let ring = |d: &Document| match &d.shapes[0].geometry {
            Geometry::Fill { rings, .. } => rings[0].len(),
            Geometry::Stroke { .. } => 0,
        };
        // Another tool, by its key.
        begin(h.state_mut());
        assert!(ring(&h.state().doc) > 4);
        h.state_mut().set_tool(Tool::Hand);
        assert!(h.state().path_edit.drag.is_none());
        assert_eq!(ring(&h.state().doc), 4);
        h.state_mut().set_tool(Tool::Select);
        // An edit by a shortcut: its undo step is the square, not the
        // rounds the drag was showing.
        begin(h.state_mut());
        h.state_mut().structural(|_| Ok(())).unwrap();
        assert!(h.state().path_edit.drag.is_none());
        assert_eq!(ring(&h.state().doc), 4);
        assert_eq!(ring(h.state().undo.last().unwrap()), 4);
        // Undo: what's kept for Redo is as the file has it.
        begin(h.state_mut());
        assert!(h.state_mut().apply_history(crate::app::History::Undo));
        assert!(h.state().path_edit.drag.is_none());
        assert_eq!(ring(&h.state().doc), 4);
        assert_eq!(ring(h.state().redo.last().unwrap()), 4);
        // Escape calls it off (and leaves the shape selected).
        begin(h.state_mut());
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert!(h.state().path_edit.drag.is_none());
        assert_eq!(ring(&h.state().doc), 4);
        assert_eq!(h.state().sel_index(), Some(0));
        // The document read afresh under it (a save writes settings into
        // the file): the drag is over, and nothing of it is left behind.
        begin(h.state_mut());
        h.state_mut().doc.shapes[0].params.spacing = Some(0.5);
        h.state_mut().doc.sync().unwrap();
        h.state_mut()
            .update_point_drag(at + (c.u + c.v).norm() * 70.0, NO_MODS);
        assert!(h.state().path_edit.drag.is_none());
        assert_eq!(ring(&h.state().doc), 4);
        // Saving ends it too: what's kept as saved is the square.
        begin(h.state_mut());
        let file = std::env::temp_dir().join(format!("zigzag-corners-{}.svg", std::process::id()));
        h.state_mut().save(Some(&file));
        let _ = std::fs::remove_file(&file);
        assert!(h.state().path_edit.drag.is_none());
        assert_eq!(ring(&h.state().doc), 4);
        h.run_steps(2);
        assert!(
            nearest_stitch(&h, c.at) < 8.0,
            "the stitches are the square's"
        );
    }

    #[test]
    fn no_corner_widgets_on_text_curves_or_several_objects() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100"><rect id="sq" x="10" y="10" width="30" height="30" fill="#c00"/><circle id="dot" cx="70" cy="70" r="15" fill="#06c"/></svg>"##;
        let mut h = square();
        h.state_mut()
            .set_document(Document::from_svg(svg, "two", None).unwrap());
        h.run_steps(3);
        h.state_mut().set_selection(&[0]);
        h.run_steps(1);
        assert_eq!(widgets(&mut h).len(), 4);
        // A circle has no corners.
        h.state_mut().set_selection(&[1]);
        h.run_steps(1);
        assert!(widgets(&mut h).is_empty());
        // Two objects: the box's handles only.
        h.state_mut().set_selection(&[0, 1]);
        h.run_steps(1);
        assert!(widgets(&mut h).is_empty());
        // Text is text, however many corners its letters have.
        let spec = zigzag::document::TextSpec {
            text: "EH".into(),
            ..Default::default()
        };
        let t = h.state_mut().add_lettering(&spec, (0.0, 0.0)).unwrap();
        h.run_steps(3);
        assert_eq!(h.state().sel_index(), Some(t));
        let path = h.state_mut().editable_path().unwrap();
        assert!(!bezier::path_corners(&path).is_empty());
        assert!(h.state().corner_widgets().is_empty());
        // A locked shape isn't edited.
        h.state_mut().set_selection(&[0]);
        h.state_mut().doc.shapes[0].params.lock = Some(true);
        h.run_steps(1);
        assert!(widgets(&mut h).is_empty());
    }

    #[test]
    fn direct_selection_picks_a_corner_by_clicking_its_widget() {
        let mut h = square();
        h.state_mut().set_tool(Tool::Direct);
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        let w = widgets(&mut h);
        let (c, at) = w[2];
        let on = screen(&h, at);
        click(&mut h, on);
        assert_eq!(h.state().path_edit.selected, vec![c.at_ref]);
        assert_eq!(widgets(&mut h).len(), 1, "its widget alone shows");
        pull(&mut h, w[2], 50.0);
        let after = h.state_mut().editable_path().unwrap();
        assert_eq!(after[0].anchors.len(), 5, "that corner alone");
        assert_eq!(h.state().undo.len(), 1);
        // The corner is still the one selected, as its two points now, so
        // its widget is still the one showing: drag it again for more.
        assert_eq!(h.state().path_edit.selected, vec![(0, 2), (0, 3)]);
        let w = widgets(&mut h);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].0.at_ref, c.at_ref);
        pull(&mut h, w[0], 20.0);
        let got = radii(&h, 0);
        assert!((got[2] - 70.0 / 2f64.sqrt()).abs() < 0.5, "{got:?}");
        assert_eq!(h.state().path_edit.selected.len(), 2);
    }

    fn flat(s: &zigzag::svg::Shape) -> Vec<P> {
        match &s.geometry {
            Geometry::Fill { rings, .. } => rings.concat(),
            Geometry::Stroke { lines, .. } => lines.iter().flat_map(|(l, _)| l.clone()).collect(),
        }
    }

    /// How far the points of `a` stray from the outline through `b`.
    fn stray(a: &[P], b: &[P]) -> f64 {
        let to_segment = |p: P, (u, v): (P, P)| {
            let d = v - u;
            let t = if d.dot(d) > 0.0 {
                ((p - u).dot(d) / d.dot(d)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            p.dist(u + d * t)
        };
        a.iter()
            .map(|&p| {
                (0..b.len())
                    .map(|k| to_segment(p, (b[k], b[(k + 1) % b.len()])))
                    .fold(f64::INFINITY, f64::min)
            })
            .fold(0.0, f64::max)
    }

    /// Round the corners of every object of a design that shows widgets,
    /// all together and one alone, then round them again: what the drag
    /// shows is what it leaves, the corners stay live (through a save and
    /// reopening too), the stitches are planned when it ends, it's one
    /// undo step, and undoing it gives the file back as it was. Returns
    /// how many objects had widgets.
    fn corners_hold_on(name: &str, svg: &str) -> usize {
        let mut h = square();
        h.state_mut()
            .set_document(Document::from_svg(svg, name, None).unwrap());
        h.run_steps(3);
        let mut rounded = 0;
        for i in 0..h.state().doc.shapes.len() {
            for alone in [false, true] {
                let s = h.state_mut();
                s.set_selection(&[i]);
                // Zoomed to the object, as someone working on it would be.
                let b = shape_bounds(&s.doc.shapes[i]);
                s.view.scale = (6000.0 / b.width().max(b.height()).max(1.0)) as f32;
                // (One rounded already starts from its sharp outline, as
                // the rest do.)
                if let Ok(live) = s.doc.live_corners(i)
                    && !live.is_plain()
                {
                    let sharp = LiveCorners::plain(live.base);
                    s.structural(|d| d.set_live_corners(i, &sharp)).unwrap();
                    s.undo.clear();
                    s.set_selection(&[i]);
                    s.replan();
                }
                let Some(path) = s.editable_path() else {
                    continue;
                };
                let w = s.corner_widgets();
                let Some(&(c, at)) = w.last() else {
                    continue;
                };
                let what = format!("{name}, object {i} ({}), alone: {alone}", s.names[i]);
                let before = s.doc.to_svg().unwrap();
                // (By uid: writing the path may spell the element's id anew.)
                let id = &s.doc.shapes[i].id;
                let uids: Vec<usize> = s
                    .doc
                    .shapes
                    .iter()
                    .filter(|x| x.id == *id)
                    .map(|x| x.uid)
                    .collect();
                let mine = |s: &Studio| -> Vec<Vec<P>> {
                    s.doc
                        .shapes
                        .iter()
                        .filter(|x| uids.contains(&x.uid))
                        .map(flat)
                        .collect()
                };
                let was = mine(s);
                let steps = s.undo.len();
                let corners = bezier::path_corners(&path).len();
                s.path_edit.selected = if alone { vec![c.at_ref] } else { vec![] };
                s.path_edit.picks_at = s.doc.rev();
                s.begin_corner_drag(path.clone(), c.at_ref, at);
                // A fifth of the way to as round as the corner can be.
                let reach = c.lu.min(c.lv) * 0.1 / c.radius_change((c.u + c.v).norm());
                s.update_point_drag(at + (c.u + c.v).norm() * reach, NO_MODS);
                let shown = mine(s);
                assert_ne!(shown, was, "{what}: the drag shows");
                assert_eq!(s.doc.to_svg().unwrap(), before, "{what}: the file waits");
                s.end_point_drag();
                assert!(!s.dirty, "{what}: stitches planned at the release");
                assert_eq!(s.undo.len(), steps + 1, "{what}: one undo step");
                let fresh = s.doc.plan_cached(&mut Default::default());
                assert_eq!(fresh.stitch_count(), s.design.stitch_count(), "{what}");
                // What was shown is what was left. (To within the 0.03 mm
                // that curves are flattened to: the file keeps three
                // decimals, and a curve read back from it may flatten to a
                // point more or less.)
                let left = mine(s);
                assert_eq!(shown.len(), left.len(), "{what}");
                for (a, b) in shown.iter().zip(&left) {
                    let d = stray(a, b).max(stray(b, a));
                    assert!(d < 0.5, "{what}: shown {d} from what's left");
                }
                // The corners are still live: as many widgets as before,
                // on the same uncut outline, also once saved and reopened.
                let j = s.sel_index().expect("still selected");
                let live = s.doc.live_corners(j).unwrap();
                let anchors = |p: &BezPath| p.iter().map(|sp| sp.anchors.len()).sum::<usize>();
                assert!(!live.is_plain(), "{what}: kept live");
                assert_eq!(anchors(&live.base), anchors(&path), "{what}");
                assert!(live.radius(c.at_ref) > 0.0, "{what}");
                let cut = live.radii.iter().flatten().filter(|r| **r > 0.0).count();
                assert_eq!(cut, if alone { 1 } else { corners }, "{what}");
                let saved = s.doc.to_svg().unwrap();
                let again = Document::from_svg(&saved, name, None).unwrap();
                let k = again
                    .shapes
                    .iter()
                    .position(|x| x.id == s.doc.shapes[j].id && x.part == s.doc.shapes[j].part)
                    .expect("there after reopening");
                let back = again.live_corners(k).unwrap();
                assert!(!back.is_plain(), "{what}: live after reopening");
                assert_eq!(anchors(&back.base), anchors(&path), "{what}");
                s.editable_path();
                assert_eq!(s.corner_widgets().len(), w.len(), "{what}: widgets stay");
                // Round it again, back to sharp: the outline it started as.
                let now = s.editable_path().unwrap();
                let (c2, at2) = *s
                    .corner_widgets()
                    .iter()
                    .find(|(x, _)| x.at_ref == c.at_ref)
                    .expect("its widget");
                s.begin_corner_drag(now, c2.at_ref, at2);
                s.update_point_drag(at2 - (c2.u + c2.v).norm() * (reach * 3.0), NO_MODS);
                s.end_point_drag();
                assert_eq!(s.undo.len(), steps + 2, "{what}: sharp again, a step");
                let j = s.sel_index().expect("still selected");
                let sharp = s.doc.live_corners(j).unwrap();
                assert!(sharp.is_plain(), "{what}: {:?}", sharp.radii);
                for (a, b) in mine(s).iter().zip(&was) {
                    let d = stray(a, b).max(stray(b, a));
                    assert!(d < 0.5, "{what}: sharp again {d} from where it began");
                }
                assert!(s.apply_history(crate::app::History::Undo));
                assert!(s.apply_history(crate::app::History::Undo));
                assert_eq!(s.doc.to_svg().unwrap(), before, "{what}: undone");
                assert_eq!(mine(s), was, "{what}: undone");
                s.undo.clear();
                s.redo.clear();
                rounded += 1;
            }
        }
        rounded / 2
    }

    #[test]
    fn corner_drags_hold_on_the_example_designs() {
        let mut rounded = 0;
        for name in ["badge", "groups", "lettering"] {
            let path = format!("{}/examples/{name}.svg", env!("CARGO_MANIFEST_DIR"));
            if let Ok(svg) = std::fs::read_to_string(&path) {
                rounded += corners_hold_on(name, &svg);
            }
        }
        assert!(rounded > 0, "no example had a corner to round");
    }

    /// The same of your own designs: every `.svg` in the folder
    /// `ZIGZAG_DESIGNS` names
    /// (`cargo test --features gui -- --ignored corner_drags_hold_on_designs_of_your_own`).
    #[test]
    #[ignore]
    fn corner_drags_hold_on_designs_of_your_own() {
        let dir = std::env::var("ZIGZAG_DESIGNS").expect("ZIGZAG_DESIGNS: a folder of .svg files");
        let (mut seen, mut rounded) = (0, 0);
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "svg") {
                let name = path.file_stem().unwrap().to_string_lossy().into_owned();
                let n = corners_hold_on(&name, &std::fs::read_to_string(&path).unwrap());
                eprintln!("{name}: {n} objects rounded");
                rounded += n;
                seen += 1;
            }
        }
        assert!(seen > 0, "no .svg files in {dir}");
        eprintln!("{seen} designs checked, {rounded} objects rounded");
    }

    #[test]
    fn round_corners_says_and_sets_what_the_handles_do() {
        let mut h = square();
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        // Rounded by a handle: 3.5 mm (50 / √2 units).
        let w = widgets(&mut h);
        pull(&mut h, w[0], 50.0);
        let r = 50.0 / 2f64.sqrt();
        // The window opens on that radius, for those four corners.
        h.state_mut()
            .run_action(crate::commands::Action::RoundCorners);
        h.run_steps(3);
        let d = h.state().layout.corner_dialog.as_ref().unwrap();
        assert!((d.radius - r / 10.0).abs() < 0.006, "{}", d.radius);
        assert_eq!(d.cut, 4, "the rounded corners are corners to round");
        h.get_by_label("4 corners. Each rounds as far as its sides allow.");
        // OK with nothing changed: nothing to undo.
        h.get_by_label("OK").click();
        h.run_steps(3);
        assert_eq!(h.state().undo.len(), 1);
        // A radius typed there is the handles' radius from then on.
        h.state_mut()
            .run_action(crate::commands::Action::RoundCorners);
        h.run_steps(2);
        h.state_mut().layout.corner_dialog.as_mut().unwrap().radius = 6.0;
        h.run_steps(3);
        for got in radii(&h, 0) {
            assert!((got - 60.0).abs() < 0.05, "previewed: {got}");
        }
        h.get_by_label("OK").click();
        h.run_steps(3);
        assert_eq!(h.state().undo.len(), 2);
        for got in radii(&h, 0) {
            assert!((got - 60.0).abs() < 0.05, "{got}");
        }
        let w = widgets(&mut h);
        assert_eq!(w.len(), 4, "still live: the handles are there");
        // And a handle dragged from there is what the window says next.
        pull(&mut h, w[1], 20.0);
        h.state_mut()
            .run_action(crate::commands::Action::RoundCorners);
        h.run_steps(3);
        let d = h.state().layout.corner_dialog.as_ref().unwrap();
        let now = (60.0 + 20.0 / 2f64.sqrt()) / 10.0;
        assert!((d.radius - now).abs() < 0.02, "{} for {now}", d.radius);
        // Zero makes them sharp again: a plain square.
        h.state_mut().layout.corner_dialog.as_mut().unwrap().radius = 0.0;
        h.run_steps(3);
        h.get_by_label("OK").click();
        h.run_steps(3);
        assert_eq!(radii(&h, 0), vec![0.0; 4]);
        assert_eq!(h.state().doc.edit_path(0).unwrap()[0].anchors.len(), 4);
        // A chamfer is cut into the path: ordinary points, no rounds.
        let w = widgets(&mut h);
        pull(&mut h, w[0], 50.0);
        h.state_mut()
            .run_action(crate::commands::Action::RoundCorners);
        h.run_steps(2);
        h.get_by_label("Chamfer").click();
        h.run_steps(3);
        h.get_by_label("OK").click();
        h.run_steps(3);
        let p = h.state().doc.edit_path(0).unwrap();
        assert_eq!(p[0].anchors.len(), 8);
        assert!(p[0].anchors.iter().all(|a| !a.has_handles()));
        assert!(h.state().doc.live_corners(0).unwrap().is_plain());
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
        h.state_mut().prefs.smart_guides = false;
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
        h.state_mut().prefs.smart_guides = false;
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
        h.state_mut().prefs.smart_guides = false;
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
