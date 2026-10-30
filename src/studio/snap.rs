//! Smart guides for drawn and dragged points, after VectorCraft's
//! `tools/src/guides.rs` (MIT OR Apache-2.0): a point being placed with the
//! Pen or a Shape tool, or dragged with Direct Selection, lands on (in this
//! order) another object's anchor or center, a crossing of two outlines, an
//! outline, a 45° ray from the point it leaves, or in line with a target;
//! each snap is labelled ("anchor", "center", "intersect", "path", "align",
//! "45°") and draws its guide lines. With Shift, the point keeps its 45°
//! angle and slides along it into line with the nearest target.

use crate::app::{Studio, shape_bounds};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use std::cell::RefCell;
use zigzag::bezier::{self, BezPath};
use zigzag::geom::P;

pub const MAGENTA: Color32 = Color32::from_rgb(236, 0, 140);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Anchor,
    Center,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Anchor => "anchor",
            Kind::Center => "center",
        }
    }
}

/// One outline segment with its control-point bounds.
struct Seg {
    c: [P; 4],
    lo: P,
    hi: P,
}

impl Seg {
    fn new(c: [P; 4]) -> Seg {
        let lo = P::new(
            c.iter().map(|p| p.x).fold(f64::MAX, f64::min),
            c.iter().map(|p| p.y).fold(f64::MAX, f64::min),
        );
        let hi = P::new(
            c.iter().map(|p| p.x).fold(f64::MIN, f64::max),
            c.iter().map(|p| p.y).fold(f64::MIN, f64::max),
        );
        Seg { c, lo, hi }
    }
    fn near(&self, p: P, tol: f64) -> bool {
        p.x >= self.lo.x - tol
            && p.x <= self.hi.x + tol
            && p.y >= self.lo.y - tol
            && p.y <= self.hi.y + tol
    }
    /// The nearest point of the segment to `p` and its distance.
    fn nearest(&self, p: P) -> (P, f64) {
        let f = |t: f64| bezier::cubic_at(&self.c, t);
        let n = 32;
        let mut best = (0.0, f64::MAX);
        for k in 0..=n {
            let t = k as f64 / n as f64;
            let d = f(t).dist(p);
            if d < best.1 {
                best = (t, d);
            }
        }
        // Refine by halving the step around the best sample.
        let (mut t, mut d) = best;
        let mut step = 1.0 / n as f64;
        for _ in 0..12 {
            step /= 2.0;
            for c in [t - step, t + step] {
                let c = c.clamp(0.0, 1.0);
                let dc = f(c).dist(p);
                if dc < d {
                    (t, d) = (c, dc);
                }
            }
        }
        (f(t), d)
    }
    fn polyline(&self) -> Vec<P> {
        bezier::sample(&self.c, 24)
    }
}

/// What points snap to, gathered from the document (minus the object being
/// edited), rebuilt when the document changes.
pub struct Targets {
    key: (u64, Option<usize>),
    points: Vec<(P, Kind)>,
    segs: Vec<Seg>,
    /// Open paths' ends, which the Pen joins onto.
    pub ends: Vec<End>,
}

/// An open subpath's end: (object uid, subpath, its start?, where).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct End {
    pub uid: usize,
    pub s: usize,
    pub at_start: bool,
    pub p: P,
}

/// A snapped point and the guides that show why.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapped {
    pub p: P,
    pub label: Option<&'static str>,
    /// Guide lines (design units).
    pub lines: Vec<(P, P)>,
}

#[derive(Default)]
pub struct SnapState {
    pub(crate) targets: Option<Targets>,
    /// The snap made this frame, drawn after the tool.
    shown: RefCell<Option<Snapped>>,
}

/// Where two polylines cross, nearest `p`.
fn crossing(a: &[P], b: &[P], p: P, tol: f64) -> Option<P> {
    let mut best: Option<(f64, P)> = None;
    for u in a.windows(2) {
        for v in b.windows(2) {
            let (r, s) = (u[1] - u[0], v[1] - v[0]);
            let den = r.x * s.y - r.y * s.x;
            if den.abs() < 1e-12 {
                continue;
            }
            let w = v[0] - u[0];
            let t = (w.x * s.y - w.y * s.x) / den;
            let k = (w.x * r.y - w.y * r.x) / den;
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&k) {
                let q = u[0] + r * t;
                let d = q.dist(p);
                if d <= tol && best.is_none_or(|b| d < b.0) {
                    best = Some((d, q));
                }
            }
        }
    }
    best.map(|b| b.1)
}

impl Targets {
    fn build(
        doc: &zigzag::document::Document,
        exclude: Option<usize>,
        key: (u64, Option<usize>),
    ) -> Targets {
        let mut points = vec![(P::default(), Kind::Center)]; // the hoop's center
        let mut segs = Vec::new();
        let mut ends = Vec::new();
        let paths = doc.all_edit_paths();
        for (s, path) in doc.shapes.iter().zip(paths) {
            if Some(s.uid) == exclude {
                continue;
            }
            let b = shape_bounds(s);
            if !b.is_empty() {
                points.push((b.center(), Kind::Center));
            }
            let Some(path) = path else { continue };
            for (k, sp) in path.iter().enumerate() {
                if !sp.closed && sp.anchors.len() >= 2 {
                    let last = sp.anchors[sp.anchors.len() - 1].p;
                    ends.push(End {
                        uid: s.uid,
                        s: k,
                        at_start: true,
                        p: sp.anchors[0].p,
                    });
                    ends.push(End {
                        uid: s.uid,
                        s: k,
                        at_start: false,
                        p: last,
                    });
                }
                points.extend(sp.anchors.iter().map(|a| (a.p, Kind::Anchor)));
                segs.extend(
                    sp.segments()
                        .into_iter()
                        .map(|(a, b)| Seg::new(sp.cubic(a, b))),
                );
            }
        }
        // One target per place (an object's anchors are often shared).
        points.dedup_by(|a, b| a.0.dist(b.0) < 1e-6);
        Targets {
            key,
            points,
            segs,
            ends,
        }
    }

    /// Snap `p`. `from`: the point it leaves (for 45° rays); `constrain`:
    /// Shift, keep to the 45° ray from `from`; `extra`: more anchors (the
    /// path being drawn or edited).
    pub fn snap(
        &self,
        p: P,
        from: Option<P>,
        constrain: bool,
        extra: &[P],
        tol: f64,
    ) -> Option<Snapped> {
        let points = || {
            self.points
                .iter()
                .copied()
                .chain(extra.iter().map(|&q| (q, Kind::Anchor)))
        };
        // Shift: keep the angle, slide along it into line with a target.
        if let (true, Some(at)) = (constrain, from) {
            let q = bezier::constrain_45(at, p);
            return Some(match self.slide(at, q - at, q, tol, extra) {
                Some((hit, target)) => self.lined_up(hit, target, extra),
                None => Snapped {
                    p: q,
                    label: None,
                    lines: vec![],
                },
            });
        }
        // An anchor or a center.
        if let Some((q, k)) = points()
            .filter(|(q, _)| q.dist(p) <= tol)
            .min_by(|a, b| a.0.dist(p).total_cmp(&b.0.dist(p)))
        {
            return Some(Snapped {
                p: q,
                label: Some(k.label()),
                lines: vec![],
            });
        }
        // Where two outlines cross.
        let near: Vec<&Seg> = self
            .segs
            .iter()
            .filter(|s| s.near(p, tol) && s.nearest(p).1 <= tol)
            .take(24)
            .collect();
        let lines: Vec<Vec<P>> = near.iter().map(|s| s.polyline()).collect();
        let mut cross: Option<P> = None;
        for i in 0..lines.len() {
            for j in i + 1..lines.len() {
                if let Some(q) = crossing(&lines[i], &lines[j], p, tol)
                    && cross.is_none_or(|c| q.dist(p) < c.dist(p))
                {
                    cross = Some(q);
                }
            }
        }
        if let Some(q) = cross {
            return Some(Snapped {
                p: q,
                label: Some("intersect"),
                lines: vec![],
            });
        }
        // On an outline.
        if let Some((q, _)) = near
            .iter()
            .map(|s| s.nearest(p))
            .min_by(|a, b| a.1.total_cmp(&b.1))
        {
            return Some(Snapped {
                p: q,
                label: Some("path"),
                lines: vec![],
            });
        }
        // A 45° ray from the point it leaves, lined up with a target if one
        // crosses it nearby.
        if let Some(at) = from
            && at.dist(p) > tol
        {
            let ray = (0..8)
                .map(|k| {
                    let a = k as f64 * std::f64::consts::FRAC_PI_4;
                    P::new(a.cos(), a.sin())
                })
                .map(|u| (u, at + u * (p - at).dot(u)))
                .filter(|(u, q)| q.dist(p) <= tol && (p - at).dot(*u) > 0.0)
                .min_by(|a, b| a.1.dist(p).total_cmp(&b.1.dist(p)));
            if let Some((u, q)) = ray {
                return Some(match self.slide(at, u, q, tol, extra) {
                    Some((hit, target)) => {
                        let mut s = self.lined_up(hit, target, extra);
                        s.lines.push((at, hit));
                        s
                    }
                    None => {
                        let deg = (-u.y).atan2(u.x).to_degrees().rem_euclid(360.0).round();
                        Snapped {
                            p: q,
                            label: Some(match deg as i32 {
                                0 | 180 => "horizontal",
                                90 | 270 => "vertical",
                                _ => "45°",
                            }),
                            lines: vec![(at, q)],
                        }
                    }
                });
            }
        }
        // In line with targets, across and down.
        let mut out = p;
        let mut lines = vec![];
        let nearest = |v: f64, of: fn(P) -> f64| {
            points()
                .map(|(q, _)| q)
                .filter(|q| (of(*q) - v).abs() <= tol)
                .min_by(|a, b| {
                    (of(*a) - v)
                        .abs()
                        .total_cmp(&(of(*b) - v).abs())
                        .then(a.dist(p).total_cmp(&b.dist(p)))
                })
        };
        if let Some(q) = nearest(p.x, |q| q.x) {
            out.x = q.x;
            lines.push((q, P::new(q.x, p.y)));
        }
        if let Some(q) = nearest(p.y, |q| q.y) {
            out.y = q.y;
            lines.push((q, P::new(p.x, q.y)));
        }
        // The lines end where the point landed.
        for l in &mut lines {
            if l.1.x == p.x {
                l.1.x = out.x;
            }
            if l.1.y == p.y {
                l.1.y = out.y;
            }
        }
        (!lines.is_empty()).then_some(Snapped {
            p: out,
            label: Some("align"),
            lines,
        })
    }

    /// Slide `q`, on the line from `at` along `dir` (forward only), to where
    /// it lines up with the nearest target, at most twice `tol` away: where it
    /// lands and the target.
    fn slide(&self, at: P, dir: P, q: P, tol: f64, extra: &[P]) -> Option<(P, P)> {
        if dir.len() < 1e-9 {
            return None;
        }
        let reach = 2.0 * tol;
        let mut best: Option<(f64, P, P)> = None;
        for t in self.points.iter().map(|x| x.0).chain(extra.iter().copied()) {
            for vertical in [true, false] {
                let (d, a, v) = if vertical {
                    (dir.x, at.x, t.x)
                } else {
                    (dir.y, at.y, t.y)
                };
                if d.abs() <= 1e-9 {
                    continue;
                }
                let s = (v - a) / d;
                if s <= 0.0 {
                    continue;
                }
                let hit = if vertical {
                    P::new(v, at.y + dir.y * s)
                } else {
                    P::new(at.x + dir.x * s, v)
                };
                let moved = hit.dist(q);
                if moved <= reach && best.is_none_or(|b| moved < b.0) {
                    best = Some((moved, hit, t));
                }
            }
        }
        best.map(|(_, hit, t)| (hit, t))
    }

    /// The guides of a point slid into line with `target`.
    fn lined_up(&self, q: P, target: P, extra: &[P]) -> Snapped {
        let on = self
            .points
            .iter()
            .copied()
            .chain(extra.iter().map(|&p| (p, Kind::Anchor)))
            .find(|(p, _)| p.dist(q) < 1e-6);
        match on {
            Some((p, k)) => Snapped {
                p,
                label: Some(k.label()),
                lines: vec![],
            },
            None => Snapped {
                p: q,
                label: Some("align"),
                lines: vec![(target, q)],
            },
        }
    }
}

impl Studio {
    /// Snap distance in design units (6 screen points).
    pub fn snap_tol(&self) -> f64 {
        6.0 * 10.0 / self.view.scale as f64
    }

    /// Make sure the snap targets are current. `exclude`: the uid of the
    /// object being edited; `keep`: a drag is under way, so keep the targets
    /// it started with (the document changes as it goes).
    pub fn snap_targets(&mut self, exclude: Option<usize>, keep: bool) {
        if keep
            && self
                .snap
                .targets
                .as_ref()
                .is_some_and(|t| t.key.1 == exclude)
        {
            return;
        }
        let key = (self.doc.rev(), exclude);
        if self.snap.targets.as_ref().is_some_and(|t| t.key == key) {
            return;
        }
        self.snap.targets = Some(Targets::build(&self.doc, exclude, key));
    }

    /// Snap a drawn or dragged point: to the grid when it's showing, else to
    /// smart guides (when on). The guides are drawn by `draw_snap`.
    pub fn snap_point(&self, p: P, from: Option<P>, constrain: bool, extra: &[P]) -> P {
        if self.view_tools.grid {
            let q = self.snap_to_grid(p);
            return match (constrain, from) {
                (true, Some(a)) => bezier::constrain_45(a, q),
                _ => q,
            };
        }
        let plain = match (constrain, from) {
            (true, Some(a)) => bezier::constrain_45(a, p),
            _ => p,
        };
        if !self.view_tools.smart_guides {
            return plain;
        }
        let Some(t) = &self.snap.targets else {
            return plain;
        };
        match t.snap(p, from, constrain, extra, self.snap_tol()) {
            Some(s) => {
                let q = s.p;
                *self.snap.shown.borrow_mut() = Some(s);
                q
            }
            None => plain,
        }
    }

    /// Draw this frame's snap (guide lines, a mark and the label).
    pub fn draw_snap(&self, painter: &egui::Painter, rect: Rect) {
        let Some(s) = self.snap.shown.borrow_mut().take() else {
            return;
        };
        let to = |p: P| self.view.to_screen(rect, p);
        let stroke = Stroke::new(1.0, MAGENTA);
        let cross = |q: Pos2| {
            painter.line_segment([q - Vec2::new(3.0, 3.0), q + Vec2::new(3.0, 3.0)], stroke);
            painter.line_segment([q + Vec2::new(-3.0, 3.0), q + Vec2::new(3.0, -3.0)], stroke);
        };
        for (a, b) in &s.lines {
            painter.line_segment([to(*a), to(*b)], stroke);
            cross(to(*a));
        }
        let q = to(s.p);
        cross(q);
        if let Some(label) = s.label {
            let galley = painter.layout_no_wrap(
                label.to_string(),
                egui::FontId::proportional(11.0),
                Color32::WHITE,
            );
            // Above and right of the point: tooltips open below it.
            let size = galley.size() + Vec2::new(8.0, 4.0);
            let bg = Rect::from_min_size(q + Vec2::new(8.0, -8.0 - size.y), size);
            painter.rect_filled(bg, 3.0, MAGENTA);
            painter.galley(bg.min + Vec2::new(4.0, 2.0), galley, Color32::WHITE);
        }
    }

    /// The anchors of a path, except the ones listed (extra snap targets).
    pub fn anchors_except(path: &BezPath, skip: &[bezier::AnchorRef]) -> Vec<P> {
        path.iter()
            .enumerate()
            .flat_map(|(s, sp)| {
                sp.anchors
                    .iter()
                    .enumerate()
                    .filter(move |(k, _)| !skip.contains(&(s, *k)))
                    .map(|(_, a)| a.p)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zigzag::document::Document;

    const ART: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
<rect id="a" x="10" y="10" width="30" height="30" fill="#f00"/>
<rect id="b" x="25" y="25" width="30" height="30" fill="#00f"/>
</svg>"##;

    fn targets() -> (Document, Targets) {
        let doc = Document::from_svg(ART, "t", None).unwrap();
        let t = Targets::build(&doc, None, (0, None));
        assert!(t.ends.is_empty(), "rectangles are closed");
        (doc, t)
    }

    #[test]
    fn points_snap_to_anchors_crossings_paths_and_lines() {
        let (doc, t) = targets();
        let tol = 5.0;
        let paths = doc.all_edit_paths();
        let a = paths[0].as_ref().unwrap()[0].anchors[0].p;
        // Near a corner: the anchor.
        let s = t
            .snap(a + P::new(2.0, -1.5), None, false, &[], tol)
            .unwrap();
        assert_eq!((s.p, s.label), (a, Some("anchor")));
        // Where the two squares' outlines cross (a's right edge, b's top).
        let b0 = paths[1].as_ref().unwrap()[0].anchors[0].p;
        let a_right = paths[0].as_ref().unwrap()[0]
            .anchors
            .iter()
            .map(|x| x.p.x)
            .fold(f64::MIN, f64::max);
        let x = P::new(a_right, b0.y);
        let s = t.snap(x + P::new(2.0, 2.0), None, false, &[], tol).unwrap();
        assert_eq!(s.label, Some("intersect"));
        assert!(s.p.dist(x) < 0.5, "{:?} vs {x:?}", s.p);
        // On an edge, away from corners: the path.
        let mid = P::new(a.x + 120.0, a.y + 2.0);
        let s = t.snap(mid, None, false, &[], tol).unwrap();
        assert_eq!(s.label, Some("path"));
        assert!((s.p.y - a.y).abs() < 1e-6);
        // Far from everything but in line with a corner: align.
        let far = P::new(a.x + 3.0, a.y + 900.0);
        let s = t.snap(far, None, false, &[], tol).unwrap();
        assert_eq!(s.label, Some("align"));
        assert_eq!(s.p.x, a.x);
    }

    #[test]
    fn a_ray_from_the_last_point_and_shift_slides_into_line() {
        let (_, t) = targets();
        let tol = 5.0;
        let from = P::new(-900.0, 600.0);
        // Nearly level from `from`: lands on the horizontal.
        let s = t
            .snap(P::new(-700.0, 603.0), Some(from), false, &[], tol)
            .unwrap();
        assert_eq!(s.label, Some("horizontal"));
        assert_eq!(s.p.y, 600.0);
        // Shift along 45°, near where a target's x crosses it: slides there.
        let extra = [P::new(-500.0, 0.0)];
        let s = t
            .snap(P::new(-505.0, 200.0), Some(from), true, &extra, tol)
            .unwrap();
        assert_eq!(s.label, Some("align"));
        assert!(
            (s.p.x + 500.0).abs() < 1e-9 && (s.p.y - 200.0).abs() < 1e-9,
            "{:?}",
            s.p
        );
    }
}
