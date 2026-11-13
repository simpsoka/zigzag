//! Satin columns for filled shapes that one column can't sew: letters with
//! a stem and a bowl, a bar across a stem, a counter (the hole in a P or an
//! e), a stroke that curls round on itself.
//!
//! A branching shape is cut where its branches part: at its sharp inside
//! corners, across to another such corner or straight on along an edge to
//! the outline opposite. Each cut either takes a piece off or opens a hole
//! onto the outline. A piece whose two ends are cuts is a column between
//! them; one with a single cut end runs from it to its tip. Cuts are tried
//! nearest-first and kept only if every piece left is a column whose
//! stitches stay inside it and aren't too wide; if no set of cuts does that
//! the caller fills the shape, as before. Columns that run straight or
//! curve are tried for first; ones that turn a sharp corner (their
//! stitches fanning round it) only if those won't do.
//!
//! A stroke that only bends (a C, an S) has no such corners: its tips are
//! found as the pair of points that makes the narrowest column.

use super::satin::{AUTO_MAX_WIDTH, Rails, pairs, split_at_tips};
use crate::geom::{FillRule, P, Polyline, cross, point_inside, polyline_length};

/// A corner turns at least this sharply (radians), measured a little way
/// either side of it: the flattened curves of a letter turn less.
const CORNER: f64 = 0.6;
/// How far either side of a vertex its turn is measured, units.
const WINDOW: f64 = 2.5;
/// A cut is a column's end when the outline turns at least this much at
/// both of its ends (else it lies along the column's side).
const END_TURN: f64 = 0.7;
/// A piece smaller than this (square units) isn't worth a column.
const MIN_AREA: f64 = 40.0;
/// How many cuts deep the search goes, and how many pieces it will try.
const MAX_DEPTH: usize = 7;
const BUDGET: usize = 600;
/// Cuts tried at each step, nearest first.
const TRIES: usize = 6;

/// A closed outline with the material on its left (an outer ring one way
/// round, a hole the other), and which of its edges are cuts.
#[derive(Clone, Debug)]
struct Ring {
    pts: Vec<P>,
    /// `cut[i]`: the edge from point `i` to the next is a cut.
    cut: Vec<bool>,
}

#[derive(Clone, Debug)]
struct Piece {
    outer: Ring,
    holes: Vec<Ring>,
}

fn area(pts: &[P]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| cross(pts[i], pts[(i + 1) % n])).sum::<f64>() * 0.5
}

impl Ring {
    /// `pts` without repeated points, turning the way that puts the
    /// material on the left (`outer`: anticlockwise in the plane's own
    /// sense; a hole, clockwise).
    fn new(pts: &[P], outer: bool) -> Option<Ring> {
        let mut p: Vec<P> = Vec::with_capacity(pts.len());
        for &q in pts {
            if p.last().is_none_or(|l| l.dist(q) > 1e-6) {
                p.push(q);
            }
        }
        while p.len() > 1 && p[0].dist(*p.last().unwrap()) <= 1e-6 {
            p.pop();
        }
        if p.len() < 3 {
            return None;
        }
        if (area(&p) > 0.0) != outer {
            p.reverse();
        }
        let n = p.len();
        Some(Ring {
            pts: p,
            cut: vec![false; n],
        })
    }

    fn len(&self) -> usize {
        self.pts.len()
    }

    /// The point `d` along the outline from point `i`, forward or back.
    fn along(&self, i: usize, d: f64, forward: bool) -> P {
        let n = self.len();
        let mut at = i;
        let mut left = d;
        for _ in 0..n {
            let next = if forward {
                (at + 1) % n
            } else {
                (at + n - 1) % n
            };
            let seg = self.pts[at].dist(self.pts[next]);
            if seg >= left && seg > 0.0 {
                return self.pts[at].lerp(self.pts[next], left / seg);
            }
            left -= seg;
            at = next;
        }
        self.pts[i]
    }

    /// How far the outline turns at point `i` (radians; negative: to the
    /// right, into the material: an inside corner), measured between
    /// points [`WINDOW`] either side so a corner rounded off over a few
    /// short edges still counts.
    fn turn(&self, i: usize) -> f64 {
        let v = self.pts[i];
        let (a, b) = (
            v - self.along(i, WINDOW, false),
            self.along(i, WINDOW, true) - v,
        );
        cross(a, b).atan2(a.x * b.x + a.y * b.y)
    }

    /// The outline's direction into and out of point `i` (unit vectors).
    fn directions(&self, i: usize) -> (P, P) {
        let v = self.pts[i];
        (
            (v - self.along(i, WINDOW, false)).norm(),
            (self.along(i, WINDOW, true) - v).norm(),
        )
    }

    /// Its sharp inside corners: the sharpest point of each.
    fn inside_corners(&self) -> Vec<usize> {
        let n = self.len();
        let turns: Vec<f64> = (0..n).map(|i| self.turn(i)).collect();
        let mut out = Vec::new();
        for i in 0..n {
            if turns[i] > -CORNER {
                continue;
            }
            // Not if a neighbour within the window turns more (the same
            // corner): the first of equals is the one kept.
            let mut sharpest = true;
            let mut d = 0.0;
            let mut k = i;
            while d < WINDOW {
                let next = (k + 1) % n;
                d += self.pts[k].dist(self.pts[next]);
                k = next;
                if k == i || d >= WINDOW {
                    break;
                }
                if turns[k] < turns[i] {
                    sharpest = false;
                }
            }
            let (mut d, mut k) = (0.0, i);
            while d < WINDOW {
                let prev = (k + n - 1) % n;
                d += self.pts[k].dist(self.pts[prev]);
                k = prev;
                if k == i || d >= WINDOW {
                    break;
                }
                if turns[k] <= turns[i] {
                    sharpest = false;
                }
            }
            if sharpest {
                out.push(i);
            }
        }
        out
    }
}

/// A place on a piece's outline: ring 0 is the outer one, ring k its hole
/// k − 1; the point is `t` of the way along the edge from point `edge`.
#[derive(Clone, Copy, Debug)]
struct At {
    ring: usize,
    edge: usize,
    t: f64,
    p: P,
}

impl Piece {
    fn ring(&self, k: usize) -> &Ring {
        if k == 0 {
            &self.outer
        } else {
            &self.holes[k - 1]
        }
    }

    fn rings(&self) -> impl Iterator<Item = &Ring> {
        std::iter::once(&self.outer).chain(&self.holes)
    }

    fn outlines(&self) -> Vec<Polyline> {
        self.rings().map(|r| r.pts.clone()).collect()
    }

    /// The edges of the outline that are the shape's own (not cuts).
    fn walls(&self) -> Vec<(P, P)> {
        let mut out = Vec::new();
        for r in self.rings() {
            let n = r.len();
            for i in 0..n {
                if !r.cut[i] {
                    out.push((r.pts[i], r.pts[(i + 1) % n]));
                }
            }
        }
        out
    }

    fn inside(&self, outlines: &[Polyline], p: P) -> bool {
        point_inside(outlines, FillRule::EvenOdd, p)
    }

    /// The first place a ray from `from` along `dir` meets the outline (not
    /// at `from` itself), within `reach`.
    fn hit(&self, from: P, dir: P, reach: f64) -> Option<At> {
        let mut best: Option<(f64, At)> = None;
        for (k, r) in self.rings().enumerate() {
            let n = r.len();
            for i in 0..n {
                let (a, b) = (r.pts[i], r.pts[(i + 1) % n]);
                let e = b - a;
                let den = cross(dir, e);
                if den.abs() < 1e-12 {
                    continue;
                }
                let w = a - from;
                let s = cross(w, e) / den; // along the ray
                let t = cross(w, dir) / den; // along the edge
                if s > 1e-3 && s <= reach && (-1e-9..=1.0 + 1e-9).contains(&t) {
                    let t = t.clamp(0.0, 1.0);
                    if best.is_none_or(|(bs, _)| s < bs) {
                        best = Some((
                            s,
                            At {
                                ring: k,
                                edge: i,
                                t,
                                p: a.lerp(b, t),
                            },
                        ));
                    }
                }
            }
        }
        best.map(|(_, at)| at)
    }

    /// Whether the straight line from `a` to `b` stays in the material,
    /// touching the outline only at its ends.
    fn clear(&self, outlines: &[Polyline], a: P, b: P) -> bool {
        let d = b - a;
        let len = d.len();
        if len < 1.0 {
            return false;
        }
        let dir = d.norm();
        // Nothing of the outline in the way…
        if self
            .hit(a, dir, len - 1e-3)
            .is_some_and(|h| h.p.dist(b) > 1e-3 && h.p.dist(a) > 1e-3)
        {
            return false;
        }
        // …and it's material, not a hole or outside, that it crosses.
        [0.25, 0.5, 0.75]
            .iter()
            .all(|&t| self.inside(outlines, a.lerp(b, t)))
    }

    /// The cuts worth trying, best first: each from a sharp inside corner,
    /// across to another, or on along one of the corner's edges (or
    /// between them) to the outline opposite.
    fn cuts(&self) -> Vec<(At, At)> {
        let outlines = self.outlines();
        let mut corners: Vec<At> = Vec::new();
        for (k, r) in self.rings().enumerate() {
            for i in r.inside_corners() {
                corners.push(At {
                    ring: k,
                    edge: i,
                    t: 0.0,
                    p: r.pts[i],
                });
            }
        }
        // (rank, length, from, to): corner to corner before corner to edge.
        let mut found: Vec<(u8, f64, At, At)> = Vec::new();
        for (x, a) in corners.iter().enumerate() {
            for b in &corners[x + 1..] {
                let len = a.p.dist(b.p);
                if len <= AUTO_MAX_WIDTH && self.clear(&outlines, a.p, b.p) {
                    found.push((0, len, *a, *b));
                }
            }
            let (into, out) = self.ring(a.ring).directions(a.edge);
            let between = (into - out).norm();
            for (rank, dir) in [(1, into), (1, -out), (2, between)] {
                if dir.len() < 0.5 {
                    continue;
                }
                let Some(h) = self.hit(a.p, dir, AUTO_MAX_WIDTH) else {
                    continue;
                };
                // (One that lands on a corner is that corner's own cut.)
                if corners.iter().any(|c| c.p.dist(h.p) < 1.5) {
                    continue;
                }
                let len = a.p.dist(h.p);
                if self.clear(&outlines, a.p, h.p) {
                    found.push((rank, len, *a, h));
                }
            }
        }
        found.sort_by(|x, y| (x.0, x.1).partial_cmp(&(y.0, y.1)).unwrap());
        // Much the same cut twice is one cut.
        let mut out: Vec<(At, At)> = Vec::new();
        for (_, _, a, b) in found {
            let same = |p: &(At, At)| {
                (p.0.p.dist(a.p) < 1.5 && p.1.p.dist(b.p) < 1.5)
                    || (p.0.p.dist(b.p) < 1.5 && p.1.p.dist(a.p) < 1.5)
            };
            if !out.iter().any(same) {
                out.push((a, b));
            }
        }
        out
    }

    /// The pieces a cut from `a` to `b` leaves: two, when both ends are on
    /// the outer outline; one with a hole fewer, when it joins a hole to
    /// the outer outline or to another hole. `None` for a cut that can't
    /// be made (both ends on one hole, or a piece too small to sew).
    fn cut(&self, a: At, b: At) -> Option<Vec<Piece>> {
        let mut piece = self.clone();
        // The places as points of their rings (new points where they fall
        // inside an edge; the later one on a ring first, so the earlier
        // one's number doesn't move).
        let put = |piece: &mut Piece, at: At| -> usize {
            let r = if at.ring == 0 {
                &mut piece.outer
            } else {
                &mut piece.holes[at.ring - 1]
            };
            let n = r.len();
            if at.t < 1e-6 || r.pts[at.edge].dist(at.p) < 1e-6 {
                return at.edge;
            }
            if at.t > 1.0 - 1e-6 || r.pts[(at.edge + 1) % n].dist(at.p) < 1e-6 {
                return (at.edge + 1) % n;
            }
            r.pts.insert(at.edge + 1, at.p);
            let tag = r.cut[at.edge];
            r.cut.insert(at.edge + 1, tag);
            at.edge + 1
        };
        let (ia, ib);
        if a.ring == b.ring {
            let a_later = (a.edge, a.t) > (b.edge, b.t);
            let (later, earlier) = if a_later { (a, b) } else { (b, a) };
            let mut hi = put(&mut piece, later);
            let before = piece.ring(a.ring).len();
            let lo = put(&mut piece, earlier);
            // (A point put in at or before the later one moves it on.)
            if piece.ring(a.ring).len() > before && lo <= hi {
                hi += 1;
            }
            (ia, ib) = if a_later { (hi, lo) } else { (lo, hi) };
        } else {
            ia = put(&mut piece, a);
            ib = put(&mut piece, b);
        }
        if a.ring == b.ring {
            if a.ring != 0 || ia == ib {
                return None;
            }
            let r = &piece.outer;
            let n = r.len();
            let (lo, hi) = (ia.min(ib), ia.max(ib));
            // lo … hi and back across the cut; hi … round … lo and back.
            let one = Ring {
                pts: r.pts[lo..=hi].to_vec(),
                cut: r.cut[lo..hi]
                    .iter()
                    .copied()
                    .chain(std::iter::once(true))
                    .collect(),
            };
            let two = Ring {
                pts: r.pts[hi..n].iter().chain(&r.pts[0..=lo]).copied().collect(),
                cut: r.cut[hi..n]
                    .iter()
                    .chain(&r.cut[0..lo])
                    .copied()
                    .chain(std::iter::once(true))
                    .collect(),
            };
            if one.len() < 3
                || two.len() < 3
                || area(&one.pts) < MIN_AREA
                || area(&two.pts) < MIN_AREA
            {
                return None;
            }
            let mut out = vec![
                Piece {
                    outer: one,
                    holes: Vec::new(),
                },
                Piece {
                    outer: two,
                    holes: Vec::new(),
                },
            ];
            for h in &piece.holes {
                let k = point_inside(
                    std::slice::from_ref(&out[0].outer.pts),
                    FillRule::EvenOdd,
                    h.pts[0],
                );
                out[if k { 0 } else { 1 }].holes.push(h.clone());
            }
            return Some(out);
        }
        // Two rings into one: round the first from the cut and back to it,
        // across, round the second, and back across.
        let (first, second, (ix, iy)) = if a.ring < b.ring {
            (a.ring, b.ring, (ia, ib))
        } else {
            (b.ring, a.ring, (ib, ia))
        };
        let (x, y) = (piece.ring(first).clone(), piece.ring(second).clone());
        let round = |r: &Ring, i: usize| -> (Vec<P>, Vec<bool>) {
            let n = r.len();
            let pts: Vec<P> = (0..=n).map(|k| r.pts[(i + k) % n]).collect();
            let cut: Vec<bool> = (0..n).map(|k| r.cut[(i + k) % n]).collect();
            (pts, cut)
        };
        let (mut pts, mut cuts) = round(&x, ix);
        cuts.push(true);
        let (p2, c2) = round(&y, iy);
        pts.extend(p2);
        cuts.extend(c2);
        cuts.push(true);
        let joined = Ring { pts, cut: cuts };
        piece.holes.remove(second - 1);
        if first == 0 {
            piece.outer = joined;
        } else {
            piece.holes[first - 1] = joined;
        }
        Some(vec![piece])
    }

    /// This piece as one column, if it is one: between its two cut ends,
    /// from its one cut end to its tip, between its own two tips, or (a
    /// ring) round between its outer outline and its hole.
    /// With `turns`, a column may turn sharp corners (its stitches fanning
    /// round each); without, only straight and curving columns will do.
    fn column(&self, turns: bool) -> Option<Rails> {
        let outlines = self.outlines();
        let walls = self.walls();
        let rails = match self.holes.len() {
            0 => squared(self.open_column(&outlines, &walls)?),
            1 if self.rings().all(|r| !r.cut.contains(&true)) => {
                return super::satin::auto_rails(&outlines).ok();
            }
            _ => return None,
        };
        if !turns && !rails.sync.is_empty() {
            return None;
        }
        valid(&rails, &outlines, &walls).then_some(rails)
    }

    fn open_column(&self, outlines: &[Polyline], walls: &[(P, P)]) -> Option<Rails> {
        let r = &self.outer;
        let n = r.len();
        // The cuts that are ends of the column: (first point, last point).
        // (Cut edges one after another are one cut while they run straight
        // on; where they turn a corner, two that meet there.)
        let dir = |a: usize, b: usize| (r.pts[b] - r.pts[a]).norm();
        let turn = |a: P, b: P| cross(a, b).atan2(a.x * b.x + a.y * b.y);
        let edge = |i: usize| dir(i, (i + 1) % n);
        let runs_on = |i: usize| {
            let next = (i + 1) % n;
            r.cut[i] && r.cut[next] && turn(edge(i), edge(next)).abs() < END_TURN
        };
        let mut ends: Vec<(usize, usize)> = Vec::new();
        for i in 0..n {
            if !r.cut[i] || runs_on((i + n - 1) % n) {
                continue;
            }
            let mut j = i;
            let mut steps = 0;
            while runs_on(j) {
                j = (j + 1) % n;
                steps += 1;
                if steps > n {
                    return None;
                }
            }
            // The cut is from point i to the point after edge j.
            let j = (j + 1) % n;
            // The outline turns onto the cut and off it again, or the cut
            // lies along the column's side.
            let before = edge((i + n - 1) % n);
            let after = edge(j);
            let face = (r.pts[j] - r.pts[i]).norm();
            if turn(before, face) > END_TURN && turn(face, after) > END_TURN {
                ends.push((i, j));
            }
        }
        // The outline from point `from` on to point `to`.
        let path = |from: usize, to: usize| -> Polyline {
            let mut out = vec![r.pts[from]];
            let mut k = from;
            while k != to {
                k = (k + 1) % n;
                out.push(r.pts[k]);
            }
            out
        };
        match ends.as_slice() {
            [] => tip_rails(&r.pts, outlines, walls),
            &[(i, j)] => {
                // From the cut's far point round to its near one, with the
                // column's other end somewhere between: a flat end (an edge
                // between two corners) or a point, whichever makes the
                // narrowest column.
                let whole = path(j, i);
                let rails = |a: &[P], b: &[P]| Rails {
                    a: a.to_vec(),
                    b: b.iter().rev().copied().collect(),
                    closed: false,
                    sync: Vec::new(),
                };
                let mut tries: Vec<Rails> = Vec::new();
                let m = whole.len();
                for k in 1..m.saturating_sub(2) {
                    let d = |a: usize, b: usize| (whole[b] - whole[a]).norm();
                    let turn = |a: P, b: P| cross(a, b).atan2(a.x * b.x + a.y * b.y);
                    if whole[k].dist(whole[k + 1]) <= AUTO_MAX_WIDTH
                        && turn(d(k - 1, k), d(k, k + 1)) > END_TURN
                        && turn(d(k, k + 1), d(k + 1, k + 2)) > END_TURN
                    {
                        tries.push(rails(&whole[..=k], &whole[k + 1..]));
                    }
                }
                let flat = tries.len();
                let step = (polyline_length(&whole) / 200.0).max(1.0);
                let pts = crate::geom::resample_keep_corners(&whole, step);
                for k in 1..pts.len().saturating_sub(1) {
                    tries.push(rails(&pts[..=k], &pts[k..]));
                }
                narrowest(tries, flat, outlines, walls)
            }
            &[(i, j), (k, l)] => {
                let a = path(j, k);
                let b: Polyline = path(l, i).into_iter().rev().collect();
                Some(Rails {
                    a,
                    b,
                    closed: false,
                    sync: Vec::new(),
                })
            }
            _ => None,
        }
    }
}

/// Where a ray from `from` along `dir` first meets the line `line`: how far
/// along the ray, and how far along the line.
fn ray_on_line(from: P, dir: P, line: &[P]) -> Option<(f64, f64)> {
    let mut best: Option<(f64, f64)> = None;
    let mut arc = 0.0;
    for w in line.windows(2) {
        let e = w[1] - w[0];
        let len = e.len();
        let den = cross(dir, e);
        if den.abs() > 1e-12 {
            let d = w[0] - from;
            let s = cross(d, e) / den;
            let t = cross(d, dir) / den;
            if s > 1e-6 && (0.0..=1.0).contains(&t) && best.is_none_or(|(bs, _)| s < bs) {
                best = Some((s, arc + t * len));
            }
        }
        arc += len;
    }
    best
}

/// Rungs for a column that turns sharp corners, so its stitches stay
/// square to it along the straights and fan only round each corner: at an
/// inside corner, the places on the far rail square across from it, on the
/// way in and on the way out. (Without them each rail is followed in
/// proportion to its length, and a corner slants every stitch before it.)
fn corner_rungs(r: &Rails) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (near, far, swap) in [(&r.a, &r.b, false), (&r.b, &r.a, true)] {
        let mut arc = 0.0;
        for k in 1..near.len().saturating_sub(1) {
            arc += near[k - 1].dist(near[k]);
            let (d1, d2) = (
                (near[k] - near[k - 1]).norm(),
                (near[k + 1] - near[k]).norm(),
            );
            if cross(d1, d2).atan2(d1.x * d2.x + d1.y * d2.y).abs() < CORNER {
                continue;
            }
            // Square across from the corner, by the leg before it and by
            // the leg after: the nearer of the two sides each time (which
            // side, and how far along the far rail).
            let across = |d: P| -> Option<(P, f64)> {
                [P::new(-d.y, d.x), P::new(d.y, -d.x)]
                    .into_iter()
                    .filter_map(|n| ray_on_line(near[k], n, far).map(|(s, t)| (s, n, t)))
                    .filter(|(s, _, _)| *s <= AUTO_MAX_WIDTH * 1.2)
                    .min_by(|x, y| x.0.total_cmp(&y.0))
                    .map(|(_, n, t)| (n, t))
            };
            if let (Some((n1, t1)), Some((_, t2))) = (across(d1), across(d2))
                && t2 > t1 + 1.0
                // An inside corner: the rail turns away from the far rail.
                // (A corner of the column's own end turns toward it.)
                && cross(d1, n1) * cross(d1, d2) < 0.0
            {
                let pair = |near_arc: f64, far_arc: f64| {
                    if swap {
                        (far_arc, near_arc)
                    } else {
                        (near_arc, far_arc)
                    }
                };
                out.push(pair(arc - 0.25, t1));
                out.push(pair(arc + 0.25, t2));
            }
        }
    }
    out.sort_by(|x, y| x.0.total_cmp(&y.0));
    // In step along both rails, or not a rung.
    let mut kept: Vec<(f64, f64)> = Vec::new();
    for (a, b) in out {
        if kept.last().is_none_or(|l| a > l.0 + 0.1 && b > l.1 + 0.1) {
            kept.push((a, b));
        }
    }
    kept
}

/// `rails` with the rungs its corners want.
fn squared(mut rails: Rails) -> Rails {
    rails.sync = corner_rungs(&rails);
    rails
}

/// Whether a column's stitches stay inside the shape, aren't too wide and
/// each cross one stroke of it: what an automatic satin has to be.
/// (`walls`: the shape's own outline, without the cuts made in it.)
fn valid(rails: &Rails, outlines: &[Polyline], walls: &[(P, P)]) -> bool {
    let ps = pairs(rails, 8.0);
    if ps.len() < 2 {
        return false;
    }
    let mut bad = 0;
    // How far each stitch's middle is from the shape's outline (for the
    // stitches long enough for it to matter): see below.
    let mut nears: Vec<Option<f64>> = Vec::with_capacity(ps.len());
    let mut widths: Vec<f64> = Vec::with_capacity(ps.len());
    let last = ps.len() - 1;
    for (k, &(a, b)) in ps.iter().enumerate() {
        let w = a.dist(b);
        if w > AUTO_MAX_WIDTH {
            return false;
        }
        widths.push(w);
        // (The first and last stitch lie along the column's ends: on the
        // outline, neither in nor out.)
        if k > 0
            && k < last
            && w > 1.0
            && [0.25, 0.5, 0.75]
                .iter()
                .any(|&t| !point_inside(outlines, FillRule::EvenOdd, a.lerp(b, t)))
        {
            bad += 1;
        }
        nears.push((w > 6.0).then(|| {
            let mid = a.lerp(b, 0.5);
            walls
                .iter()
                .map(|&(p, q)| {
                    let e = q - p;
                    let len2 = e.x * e.x + e.y * e.y;
                    let t = if len2 > 0.0 {
                        (((mid.x - p.x) * e.x + (mid.y - p.y) * e.y) / len2).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    p.lerp(q, t).dist(mid)
                })
                .fold(f64::INFINITY, f64::min)
        }));
    }
    // A column is about as wide all along: a stitch several times the
    // usual length is one side doubling back on itself (round a branch
    // that should have been cut off), not a wide place.
    let mut sorted = widths.clone();
    sorted.sort_by(f64::total_cmp);
    let (median, widest) = (sorted[sorted.len() / 2], sorted[sorted.len() - 1]);
    if widest > (median * 3.0).max(12.0) {
        return false;
    }
    // A stitch across a stroke is as long as the stroke is thick, so its
    // middle is half its length from the outline. One whose middle is much
    // nearer the outline than that spans two strokes and the notch between
    // them (a tail and the stem it leaves): not a column. (Not counting
    // the stitches within half a width of the column's own ends, which
    // are that near the outline there.)
    let margin = (median * 0.5 / 8.0).ceil() as usize + 1;
    let spans = (margin..ps.len().saturating_sub(margin))
        .filter(|&k| nears[k].is_some_and(|near| widths[k] > near * 3.6))
        .count();
    if spans as f64 > ps.len() as f64 * 0.1 {
        return false;
    }
    (bad as f64) <= ps.len() as f64 * 0.06
}

/// The narrowest of `tries` that's a column. The first `flat` of them end
/// square (not at a point): preferred unless a pointed one is clearly
/// narrower.
fn narrowest(
    tries: Vec<Rails>,
    flat: usize,
    outlines: &[Polyline],
    walls: &[(P, P)],
) -> Option<Rails> {
    let mut ranked: Vec<(f64, usize)> = tries
        .iter()
        .enumerate()
        .map(|(k, rails)| {
            let (la, lb) = (polyline_length(&rails.a), polyline_length(&rails.b));
            // (Sides too unlike each other aren't the two sides of a column.)
            if la.min(lb) < la.max(lb) * 0.45 {
                return (f64::INFINITY, k);
            }
            let ps = pairs(rails, (la.max(lb) / 24.0).max(1.0));
            let w = ps.iter().map(|(a, b)| a.dist(*b)).sum::<f64>() / ps.len().max(1) as f64;
            (if k < flat { w * 0.8 } else { w }, k)
        })
        .collect();
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut tries: Vec<Option<Rails>> = tries.into_iter().map(Some).collect();
    ranked
        .into_iter()
        .take(8)
        .filter(|(w, _)| w.is_finite())
        .filter_map(|(_, k)| tries[k].take())
        .map(squared)
        .find(|rails| valid(rails, outlines, walls))
}

/// A column for a ring with no cuts: split at its tips as ever, or (a
/// stroke that curls round, whose tips are near each other) at the pair of
/// points that makes the narrowest column.
fn tip_rails(ring: &[P], outlines: &[Polyline], walls: &[(P, P)]) -> Option<Rails> {
    if let Some(r) = split_at_tips(ring).filter(|r| valid(r, outlines, walls)) {
        return Some(r);
    }
    let mut closed = ring.to_vec();
    closed.push(ring[0]);
    let step = (polyline_length(&closed) / 96.0).max(1.0);
    let mut pts = crate::geom::resample_keep_corners(&closed, step);
    pts.pop();
    let n = pts.len();
    if n < 8 {
        return None;
    }
    let rails_for = |i: usize, j: usize| -> Rails {
        let a: Polyline = (i..=j).map(|k| pts[k]).collect();
        let b: Polyline = (0..=(n - j + i)).map(|k| pts[(i + n - k) % n]).collect();
        Rails {
            a,
            b,
            closed: false,
            sync: Vec::new(),
        }
    };
    let mut ranked: Vec<(f64, usize, usize)> = Vec::new();
    for i in 0..n {
        for j in i + 2..n {
            let rails = rails_for(i, j);
            let (la, lb) = (polyline_length(&rails.a), polyline_length(&rails.b));
            if la.min(lb) < la.max(lb) * 0.45 {
                continue;
            }
            let ps = pairs(&rails, la.max(lb) / 12.0);
            let w = ps.iter().map(|(a, b)| a.dist(*b)).sum::<f64>() / ps.len().max(1) as f64;
            ranked.push((w, i, j));
        }
    }
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    ranked
        .into_iter()
        .take(12)
        .map(|(_, i, j)| rails_for(i, j))
        .find(|rails| valid(rails, outlines, walls))
}

/// The columns `piece` can be sewn as, cutting it up to `depth` more times
/// (`turns`: see [`Piece::column`]).
fn solve(piece: &Piece, depth: usize, budget: &mut usize, turns: bool) -> Option<Vec<Rails>> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    if let Some(rails) = piece.column(turns) {
        return Some(vec![rails]);
    }
    if depth == 0 {
        return None;
    }
    let mut tried = 0;
    for (a, b) in piece.cuts() {
        let Some(pieces) = piece.cut(a, b) else {
            continue;
        };
        tried += 1;
        if tried > TRIES {
            break;
        }
        let mut all = Vec::new();
        let mut ok = true;
        for p in &pieces {
            match solve(p, depth - 1, budget, turns) {
                Some(rails) => all.extend(rails),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            return Some(all);
        }
    }
    None
}

/// The satin columns for one filled component (its outer ring, then its
/// holes) that isn't a single column: `None` if it can't be made into
/// columns whose stitches stay inside it.
pub fn split(comp: &[Polyline]) -> Option<Vec<Rails>> {
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::sync::{Mutex, OnceLock};
    // Asked for again and again (planning, the checks, the rails drawn on
    // the canvas): worked out once per outline.
    static MEMO: OnceLock<Mutex<HashMap<u64, Option<Vec<Rails>>>>> = OnceLock::new();
    let key = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for ring in comp {
            ring.len().hash(&mut h);
            for p in ring {
                p.x.to_bits().hash(&mut h);
                p.y.to_bits().hash(&mut h);
            }
        }
        h.finish()
    };
    let memo = MEMO.get_or_init(Default::default);
    if let Some(hit) = memo.lock().ok().and_then(|m| m.get(&key).cloned()) {
        return hit;
    }
    let found = (|| {
        let outer = Ring::new(comp.first()?, true)?;
        let holes: Vec<Ring> = comp[1..]
            .iter()
            .filter_map(|h| Ring::new(h, false))
            .collect();
        // Straight and curving columns, butted where they meet, if the
        // shape can be cut into those: cleaner than a column turning a
        // sharp corner. Else columns that turn corners too.
        let piece = Piece { outer, holes };
        [false, true].into_iter().find_map(|turns| {
            let mut budget = BUDGET;
            solve(&piece, MAX_DEPTH, &mut budget, turns)
        })
    })();
    if let Ok(mut m) = memo.lock() {
        if m.len() > 2048 {
            m.clear();
        }
        m.insert(key, found.clone());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poly(pts: &[(f64, f64)]) -> Polyline {
        pts.iter().map(|&(x, y)| P::new(x, y)).collect()
    }

    /// Every column's stitches inside the shape, and the columns together
    /// covering about all of it.
    fn covers(comp: &[Polyline], rails: &[Rails]) {
        let total: f64 =
            area(&comp[0]).abs() - comp[1..].iter().map(|h| area(h).abs()).sum::<f64>();
        let mut sewn = 0.0;
        for r in rails {
            let walls: Vec<(P, P)> = comp
                .iter()
                .flat_map(|ring| {
                    (0..ring.len()).map(move |i| (ring[i], ring[(i + 1) % ring.len()]))
                })
                .collect();
            assert!(valid(r, comp, &walls), "a column leaves the shape");
            let ring: Polyline = r.a.iter().chain(r.b.iter().rev()).copied().collect();
            sewn += area(&ring).abs();
        }
        assert!(
            (sewn - total).abs() < total * 0.03,
            "columns cover {sewn} of {total}"
        );
    }

    #[test]
    fn a_t_is_a_bar_and_a_stem() {
        // Bar 100 × 16 on top, stem 16 wide below its middle.
        let t = vec![poly(&[
            (0.0, 0.0),
            (100.0, 0.0),
            (100.0, 16.0),
            (58.0, 16.0),
            (58.0, 120.0),
            (42.0, 120.0),
            (42.0, 16.0),
            (0.0, 16.0),
        ])];
        assert!(
            super::super::satin::auto_rails(&t).is_err(),
            "not one column"
        );
        let rails = split(&t).expect("columns");
        assert_eq!(rails.len(), 2);
        covers(&t, &rails);
        // One runs the bar's length, the other the stem's.
        let spans: Vec<(f64, f64)> = rails
            .iter()
            .map(|r| {
                let xs = r.a.iter().chain(&r.b).map(|p| p.x);
                let ys = r.a.iter().chain(&r.b).map(|p| p.y);
                (
                    xs.clone().fold(f64::MIN, f64::max) - xs.fold(f64::MAX, f64::min),
                    ys.clone().fold(f64::MIN, f64::max) - ys.fold(f64::MAX, f64::min),
                )
            })
            .collect();
        assert!(spans.contains(&(100.0, 16.0)), "{spans:?}");
        assert!(spans.contains(&(16.0, 104.0)), "{spans:?}");
    }

    #[test]
    fn a_p_is_a_stem_and_a_bowl_round_its_counter() {
        // Stem 16 wide and 140 tall; a square bowl to its right, 16 thick.
        let outer = poly(&[
            (0.0, 0.0),
            (80.0, 0.0),
            (80.0, 76.0),
            (16.0, 76.0),
            (16.0, 140.0),
            (0.0, 140.0),
        ]);
        let counter = poly(&[(16.0, 16.0), (64.0, 16.0), (64.0, 60.0), (16.0, 60.0)]);
        let p = vec![outer, counter];
        assert!(super::super::satin::auto_rails(&p).is_err());
        let rails = split(&p).expect("columns");
        covers(&p, &rails);
        assert!(rails.len() >= 2 && rails.len() <= 4, "{}", rails.len());
        // No stitch wider than the strokes are thick (give or take a mitre).
        for r in &rails {
            let widest = pairs(r, 5.0)
                .iter()
                .map(|(a, b)| a.dist(*b))
                .fold(0.0, f64::max);
            assert!(widest < 16.0 * 1.6, "{widest}");
        }
    }

    #[test]
    fn an_e_with_three_bars_is_four_columns() {
        let e = vec![poly(&[
            (0.0, 0.0),
            (70.0, 0.0),
            (70.0, 16.0),
            (16.0, 16.0),
            (16.0, 52.0),
            (60.0, 52.0),
            (60.0, 68.0),
            (16.0, 68.0),
            (16.0, 104.0),
            (70.0, 104.0),
            (70.0, 120.0),
            (0.0, 120.0),
        ])];
        let rails = split(&e).expect("columns");
        covers(&e, &rails);
        assert_eq!(rails.len(), 4, "a stem and three bars");
    }

    #[test]
    fn a_stroke_that_curls_round_is_one_column_between_its_ends() {
        // A C: three quarters of a ring 12 wide, its ends near each other.
        let mut c: Polyline = Vec::new();
        let (from, to) = (0.6f64, std::f64::consts::TAU - 0.6);
        for k in 0..=60 {
            let t = from + (to - from) * k as f64 / 60.0;
            c.push(P::new(50.0 * t.cos(), 50.0 * t.sin()));
        }
        for k in (0..=60).rev() {
            let t = from + (to - from) * k as f64 / 60.0;
            c.push(P::new(38.0 * t.cos(), 38.0 * t.sin()));
        }
        let c = vec![c];
        let rails = split(&c).expect("a column");
        assert_eq!(rails.len(), 1);
        covers(&c, &rails);
        let mean = {
            let ps = pairs(&rails[0], 5.0);
            ps.iter().map(|(a, b)| a.dist(*b)).sum::<f64>() / ps.len() as f64
        };
        assert!((mean - 12.0).abs() < 1.0, "as wide as the stroke: {mean}");
    }

    #[test]
    fn a_blob_is_still_not_a_satin() {
        // 30 mm across: wider than any satin stitch should be.
        let blob: Polyline = (0..40)
            .map(|k| {
                let t = k as f64 / 40.0 * std::f64::consts::TAU;
                P::new(150.0 * t.cos(), 150.0 * t.sin())
            })
            .collect();
        assert!(split(&[blob]).is_none());
    }
}
