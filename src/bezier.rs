//! Editable Bézier paths: anchors with in/out handles, as the pen and
//! direct-selection tools see them. Coordinates are whatever the caller uses
//! (design units in the studio); transforms map them exactly.

use crate::geom::{Affine, P, affine_apply};

/// One point of a path, with its two handles (each equal to `p` when the
/// anchor has no handle on that side).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub p: P,
    pub h_in: P,
    pub h_out: P,
}

impl Anchor {
    pub fn corner(p: P) -> Anchor {
        Anchor {
            p,
            h_in: p,
            h_out: p,
        }
    }

    pub fn has_handles(&self) -> bool {
        self.h_in != self.p || self.h_out != self.p
    }

    /// Both handles on one line through the anchor (a smooth point).
    pub fn is_smooth(&self) -> bool {
        let (a, b) = (self.h_in - self.p, self.h_out - self.p);
        if a.len() < 1e-9 || b.len() < 1e-9 {
            return false;
        }
        let cross = a.x * b.y - a.y * b.x;
        (cross / (a.len() * b.len())).abs() < 1e-3 && a.dot(b) < 0.0
    }

    fn map(&self, m: Affine) -> Anchor {
        Anchor {
            p: affine_apply(m, self.p),
            h_in: affine_apply(m, self.h_in),
            h_out: affine_apply(m, self.h_out),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubPath {
    pub anchors: Vec<Anchor>,
    pub closed: bool,
}

impl SubPath {
    /// Segments as (from, to) anchor indices; a closed path has one more,
    /// back to the start.
    pub fn segments(&self) -> Vec<(usize, usize)> {
        let n = self.anchors.len();
        let mut v: Vec<(usize, usize)> = (1..n).map(|k| (k - 1, k)).collect();
        if self.closed && n > 1 {
            v.push((n - 1, 0));
        }
        v
    }

    /// The cubic for segment `a → b`.
    pub fn cubic(&self, a: usize, b: usize) -> [P; 4] {
        let (s, e) = (self.anchors[a], self.anchors[b]);
        [s.p, s.h_out, e.h_in, e.p]
    }
}

/// A path: one or more subpaths (a compound shape, like a ring).
pub type BezPath = Vec<SubPath>;

/// Which anchor: (subpath, anchor).
pub type AnchorRef = (usize, usize);

pub fn cubic_at(c: &[P; 4], t: f64) -> P {
    let u = 1.0 - t;
    c[0] * (u * u * u) + c[1] * (3.0 * u * u * t) + c[2] * (3.0 * u * t * t) + c[3] * (t * t * t)
}

/// Points along a cubic, for drawing (`n` steps; straight segments use 1).
pub fn sample(c: &[P; 4], n: usize) -> Vec<P> {
    let straight = c[1] == c[0] && c[2] == c[3];
    let n = if straight { 1 } else { n.max(1) };
    (0..=n).map(|k| cubic_at(c, k as f64 / n as f64)).collect()
}

/// Read SVG path data (`d`) into anchors, as drawn (no transform).
pub fn from_svg_d(d: &str) -> BezPath {
    let mut pb = usvg::tiny_skia_path::PathBuilder::new();
    for seg in svgtypes::SimplifyingPathParser::from(d).flatten() {
        use svgtypes::SimplePathSegment as S;
        match seg {
            S::MoveTo { x, y } => pb.move_to(x as f32, y as f32),
            S::LineTo { x, y } => pb.line_to(x as f32, y as f32),
            S::Quadratic { x1, y1, x, y } => pb.quad_to(x1 as f32, y1 as f32, x as f32, y as f32),
            S::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => pb.cubic_to(
                x1 as f32, y1 as f32, x2 as f32, y2 as f32, x as f32, y as f32,
            ),
            S::ClosePath => pb.close(),
        }
    }
    match pb.finish() {
        Some(path) => from_usvg(&path, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        None => Vec::new(),
    }
}

/// Read a usvg path (with transform `m` applied) into anchors.
pub fn from_usvg(data: &usvg::tiny_skia_path::Path, m: Affine) -> BezPath {
    use usvg::tiny_skia_path::PathSegment as S;
    let pt = |p: usvg::tiny_skia_path::Point| affine_apply(m, P::new(p.x as f64, p.y as f64));
    let mut out: BezPath = Vec::new();
    let mut cur: Option<SubPath> = None;
    let finish = |cur: &mut Option<SubPath>, out: &mut BezPath, closed: bool| {
        if let Some(mut sp) = cur.take() {
            if closed {
                // A closing segment that ends on the start point: merge them.
                if sp.anchors.len() > 1 {
                    let (first, last) = (sp.anchors[0], *sp.anchors.last().unwrap());
                    if (first.p - last.p).len() < 1e-6 {
                        sp.anchors[0].h_in = last.h_in;
                        sp.anchors.pop();
                    }
                }
                sp.closed = true;
            }
            if !sp.anchors.is_empty() {
                out.push(sp);
            }
        }
    };
    for seg in data.segments() {
        match seg {
            S::MoveTo(p) => {
                finish(&mut cur, &mut out, false);
                cur = Some(SubPath {
                    anchors: vec![Anchor::corner(pt(p))],
                    closed: false,
                });
            }
            S::LineTo(p) => {
                if let Some(sp) = &mut cur {
                    sp.anchors.push(Anchor::corner(pt(p)));
                }
            }
            S::QuadTo(c, p) => {
                if let Some(sp) = &mut cur {
                    let last = sp.anchors.last_mut().unwrap();
                    let (s, c, e) = (last.p, pt(c), pt(p));
                    last.h_out = s + (c - s) * (2.0 / 3.0);
                    sp.anchors.push(Anchor {
                        p: e,
                        h_in: e + (c - e) * (2.0 / 3.0),
                        h_out: e,
                    });
                }
            }
            S::CubicTo(c1, c2, p) => {
                if let Some(sp) = &mut cur {
                    sp.anchors.last_mut().unwrap().h_out = pt(c1);
                    let e = pt(p);
                    sp.anchors.push(Anchor {
                        p: e,
                        h_in: pt(c2),
                        h_out: e,
                    });
                }
            }
            S::Close => finish(&mut cur, &mut out, true),
        }
    }
    finish(&mut cur, &mut out, false);
    out
}

pub fn transform(path: &BezPath, m: Affine) -> BezPath {
    path.iter()
        .map(|sp| SubPath {
            anchors: sp.anchors.iter().map(|a| a.map(m)).collect(),
            closed: sp.closed,
        })
        .collect()
}

/// Write `v` to 3 decimal places, without trailing zeros (and no "-0").
pub fn write_num(out: &mut String, v: f64) {
    use std::fmt::Write;
    let start = out.len();
    let _ = write!(out, "{v:.3}");
    if out[start..].contains('.') {
        let keep = out.trim_end_matches('0').trim_end_matches('.').len();
        out.truncate(keep);
    }
    if &out[start..] == "-0" {
        out.truncate(start);
        out.push('0');
    }
}

/// SVG path data. Straight segments are written as lines.
pub fn to_svg_d(path: &BezPath) -> String {
    let mut d = String::new();
    let xy = |d: &mut String, p: P| {
        write_num(d, p.x);
        d.push(' ');
        write_num(d, p.y);
    };
    for sp in path {
        let Some(first) = sp.anchors.first() else {
            continue;
        };
        d.push('M');
        xy(&mut d, first.p);
        d.push(' ');
        for (a, b) in sp.segments() {
            let c = sp.cubic(a, b);
            if c[1] == c[0] && c[2] == c[3] {
                if b == 0 {
                    break; // closing line: Z draws it
                }
                d.push('L');
                xy(&mut d, c[3]);
                d.push(' ');
            } else {
                d.push('C');
                xy(&mut d, c[1]);
                d.push(' ');
                xy(&mut d, c[2]);
                d.push(' ');
                xy(&mut d, c[3]);
                d.push(' ');
            }
        }
        if sp.closed {
            d.push_str("Z ");
        }
    }
    d.truncate(d.trim_end().len());
    d
}

/// The anchor nearest `q` within `tol`.
pub fn hit_anchor(path: &BezPath, q: P, tol: f64) -> Option<AnchorRef> {
    let mut best: Option<(f64, AnchorRef)> = None;
    for (s, sp) in path.iter().enumerate() {
        for (k, a) in sp.anchors.iter().enumerate() {
            let d = (a.p - q).len();
            if d <= tol && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, (s, k)));
            }
        }
    }
    best.map(|(_, r)| r)
}

/// The nearest point on the outline within `tol`: (subpath, segment start
/// anchor, t along the segment).
pub fn hit_segment(path: &BezPath, q: P, tol: f64) -> Option<(usize, usize, f64)> {
    let mut best: Option<(f64, (usize, usize, f64))> = None;
    for (s, sp) in path.iter().enumerate() {
        for (a, b) in sp.segments() {
            let c = sp.cubic(a, b);
            const N: usize = 48;
            for k in 0..N {
                let (t0, t1) = (k as f64 / N as f64, (k + 1) as f64 / N as f64);
                let (p0, p1) = (cubic_at(&c, t0), cubic_at(&c, t1));
                let d = p1 - p0;
                let u = if d.dot(d) > 1e-12 {
                    ((q - p0).dot(d) / d.dot(d)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let dist = (p0 + d * u - q).len();
                if dist <= tol && best.is_none_or(|(bd, _)| dist < bd) {
                    best = Some((dist, (s, a, t0 + (t1 - t0) * u)));
                }
            }
        }
    }
    best.map(|(_, r)| r)
}

/// Add an anchor on segment `a → a+1` at `t` without changing the shape.
/// Returns the new anchor.
/// Bend the segment from anchor `a` of `sp` to the next so that its point
/// at `t` moves by `delta` (Illustrator's segment drag): both inner handles
/// move. With `keep_smooth`, smooth anchors stay smooth (the handle on the
/// other side turns with the one moved, keeping its length).
pub fn bend_segment(sp: &mut SubPath, a: usize, t: f64, delta: P, keep_smooth: bool) {
    let n = sp.anchors.len();
    if n < 2 {
        return;
    }
    let b = (a + 1) % n;
    let t = t.clamp(0.08, 0.92);
    // B(t) moves 3t(1-t) times as far as the inner control points.
    let v = delta * (1.0 / (3.0 * t * (1.0 - t)));
    let bend = |anchor: &mut Anchor, out: bool| {
        let smooth = anchor.is_smooth();
        if out {
            anchor.h_out = anchor.h_out + v;
        } else {
            anchor.h_in = anchor.h_in + v;
        }
        if smooth && keep_smooth {
            let (moved, other) = if out {
                (anchor.h_out, anchor.h_in)
            } else {
                (anchor.h_in, anchor.h_out)
            };
            let u = moved - anchor.p;
            if u.len() > 1e-9 {
                let turned = anchor.p - u * ((other - anchor.p).len() / u.len());
                if out {
                    anchor.h_in = turned;
                } else {
                    anchor.h_out = turned;
                }
            }
        }
    };
    bend(&mut sp.anchors[a], true);
    bend(&mut sp.anchors[b], false);
}

pub fn split(path: &mut BezPath, s: usize, a: usize, t: f64) -> AnchorRef {
    let sp = &mut path[s];
    let n = sp.anchors.len();
    let b = (a + 1) % n;
    let [p0, p1, p2, p3] = sp.cubic(a, b);
    let lerp = |x: P, y: P| x + (y - x) * t;
    let (q0, q1, q2) = (lerp(p0, p1), lerp(p1, p2), lerp(p2, p3));
    let (r0, r1) = (lerp(q0, q1), lerp(q1, q2));
    let m = lerp(r0, r1);
    sp.anchors[a].h_out = q0;
    sp.anchors[b].h_in = q2;
    let at = a + 1;
    sp.anchors.insert(
        at,
        Anchor {
            p: m,
            h_in: r0,
            h_out: r1,
        },
    );
    (s, at)
}

/// Remove anchors. Subpaths left with too few points go too (an open path
/// needs 2, a closed one 3).
pub fn delete(path: &mut BezPath, which: &[AnchorRef]) {
    for (s, sp) in path.iter_mut().enumerate() {
        let mut k = 0;
        sp.anchors.retain(|_| {
            let keep = !which.contains(&(s, k));
            k += 1;
            keep
        });
    }
    path.retain(|sp| sp.anchors.len() >= if sp.closed { 3 } else { 2 });
}

/// Smooth handles for anchor `k`, pointing along its neighbours (for
/// converting a corner to a smooth point).
pub fn auto_smooth(sp: &mut SubPath, k: usize) {
    let n = sp.anchors.len();
    if n < 2 {
        return;
    }
    let prev = if k > 0 {
        Some(sp.anchors[k - 1].p)
    } else if sp.closed {
        Some(sp.anchors[n - 1].p)
    } else {
        None
    };
    let next = if k + 1 < n {
        Some(sp.anchors[k + 1].p)
    } else if sp.closed {
        Some(sp.anchors[0].p)
    } else {
        None
    };
    let p = sp.anchors[k].p;
    let dir = match (prev, next) {
        (Some(a), Some(b)) => b - a,
        (Some(a), None) => p - a,
        (None, Some(b)) => b - p,
        _ => return,
    };
    if dir.len() < 1e-9 {
        return;
    }
    let u = dir * (1.0 / dir.len());
    let lin = prev.map_or(0.0, |a| (p - a).len() / 3.0);
    let lout = next.map_or(0.0, |b| (b - p).len() / 3.0);
    sp.anchors[k].h_in = p - u * lin;
    sp.anchors[k].h_out = p + u * lout;
}

/// Join a path being drawn onto an open subpath's end: `drawn`'s last anchor
/// lands on `target`'s start (`at_start`) or end, and the two become one
/// open subpath running through `drawn` into `target`. The joint keeps the
/// handle it arrives with and `target`'s handle leaving it.
pub fn join(drawn: &[Anchor], target: &SubPath, at_start: bool) -> SubPath {
    let t = if at_start {
        target.clone()
    } else {
        reversed(target)
    };
    let mut anchors: Vec<Anchor> = drawn[..drawn.len().saturating_sub(1)].to_vec();
    let mut joint = t.anchors[0];
    if let Some(last) = drawn.last() {
        joint.h_in = last.h_in + (joint.p - last.p);
    }
    anchors.push(joint);
    anchors.extend_from_slice(&t.anchors[1..]);
    SubPath {
        anchors,
        closed: false,
    }
}

/// Polygons (closed rings), such as Pathfinder and Offset results, as smooth
/// paths: turns sharper than 40° stay corners, straight runs stay lines, and
/// the rest is fitted with as few cubics as keep within `tol` of the ring.
/// With `tol` 0 the rings are kept exactly, as corner points.
pub fn from_rings(rings: &[Vec<P>], tol: f64) -> BezPath {
    rings
        .iter()
        .filter(|r| r.len() >= 3)
        .filter_map(|r| {
            if tol <= 0.0 {
                return Some(SubPath {
                    anchors: r.iter().map(|&p| Anchor::corner(p)).collect(),
                    closed: true,
                });
            }
            fit_outline(r, true, tol, 40.0)
        })
        .filter(|sp| sp.anchors.len() >= 2)
        .collect()
}

/// Fewer points, smooth curves (Illustrator's Simplify): each subpath is
/// refitted within `tol`, keeping the corners sharper than 30°, and never
/// gets more points than it had.
pub fn simplify(path: &BezPath, tol: f64) -> BezPath {
    let tol = tol.max(1e-6);
    let mut out = Vec::new();
    for sp in path {
        let segs: Vec<[P; 4]> = sp
            .segments()
            .into_iter()
            .map(|(a, b)| sp.cubic(a, b))
            .collect();
        let n = segs.len();
        if n == 0 {
            out.push(sp.clone());
            continue;
        }
        let turn = |a: &[P; 4], b: &[P; 4]| {
            end_tangent(a)
                .dot(start_tangent(b))
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees()
        };
        // Corners split the fit: segment indices where a run starts.
        let mut starts: Vec<usize> = (1..n)
            .filter(|&k| turn(&segs[k - 1], &segs[k]) > 30.0)
            .collect();
        let closed = sp.closed;
        let mut order: Vec<usize> = (0..n).collect();
        let mut has_corner = !starts.is_empty();
        if closed {
            // Start the loop at a corner, so no fit runs across one.
            let k = (0..n).find(|&k| turn(&segs[(k + n - 1) % n], &segs[k]) > 30.0);
            if let Some(k) = k {
                has_corner = true;
                order.rotate_left(k);
                starts = (1..n)
                    .filter(|&i| turn(&segs[order[i - 1]], &segs[order[i]]) > 30.0)
                    .collect();
            }
        }
        starts.insert(0, 0);
        starts.push(n);
        let mut cubics: Vec<[P; 4]> = Vec::new();
        for w in starts.windows(2) {
            let piece: Vec<[P; 4]> = order[w[0]..w[1]].iter().map(|&k| segs[k]).collect();
            if piece.is_empty() {
                continue;
            }
            let mut pts: Vec<P> = Vec::new();
            for (k, c) in piece.iter().enumerate() {
                let len = cubic_len(c);
                let m = ((len / (tol * 0.5)).ceil() as usize).clamp(4, 256);
                let s = sample_n(c, m);
                pts.extend(if k == 0 { &s[..] } else { &s[1..] });
            }
            let (mut t0, mut t1) = (
                start_tangent(&piece[0]),
                end_tangent(&piece[piece.len() - 1]),
            );
            if closed && !has_corner {
                // A loop without corners: one tangent where it closes.
                let t = unit(t0 + t1).unwrap_or(t0);
                (t0, t1) = (t, t);
            }
            for c in fit_cubics(&pts, t0, t1, tol) {
                cubics.push(if is_straight(&c, tol * 0.02) {
                    [c[0], c[0], c[3], c[3]]
                } else {
                    c
                });
            }
        }
        let fitted = anchors_from_cubics(&cubics, closed);
        // Simplify never adds points.
        if fitted.anchors.len() <= sp.anchors.len() && fitted.anchors.len() >= 2 {
            out.push(fitted);
        } else {
            out.push(sp.clone());
        }
    }
    out
}

// ------------------------------------------------------------ corners
//
// Rounded, inverted-round and chamfered corners on any path (Illustrator's
// Live Corners, cut into the path), after VectorCraft's
// `geom/src/corners.rs` (MIT OR Apache-2.0). A corner is an anchor that
// isn't smooth, where two sides (straight or curved) meet at an angle. Its
// cut meets each side `setback` from the corner; radii are fitted so the cuts
// of neighbouring corners never pass each other, and curved sides are
// trimmed to what the cuts leave of them.

const CEPS: f64 = 1e-9;

/// How a corner is cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum CornerKind {
    /// An arc tangent to both sides.
    #[default]
    Round,
    /// An arc centred on the corner (a scoop).
    Inverted,
    /// A straight cut.
    Chamfer,
}

fn is_straight_side(c: &[P; 4]) -> bool {
    c[1].dist(c[0]) <= CEPS && c[2].dist(c[3]) <= CEPS
}

/// Arc length along a cubic, from a table of `n` chords.
fn arclen_table(c: &[P; 4], n: usize) -> Vec<f64> {
    let pts = sample_n(c, n);
    let mut acc = vec![0.0];
    for w in pts.windows(2) {
        acc.push(acc[acc.len() - 1] + w[0].dist(w[1]));
    }
    acc
}

/// A cubic split at `t`: (before, after).
pub fn split_cubic(c: &[P; 4], t: f64) -> ([P; 4], [P; 4]) {
    let l = |a: P, b: P| a.lerp(b, t);
    let (p01, p12, p23) = (l(c[0], c[1]), l(c[1], c[2]), l(c[2], c[3]));
    let (p012, p123) = (l(p01, p12), l(p12, p23));
    let m = l(p012, p123);
    ([c[0], p01, p012, m], [m, p123, p23, c[3]])
}

/// The part of a cubic between `t0` and `t1`.
fn subsegment(c: &[P; 4], t0: f64, t1: f64) -> [P; 4] {
    if t1 <= CEPS {
        return [c[0]; 4];
    }
    let (left, _) = split_cubic(c, t1.min(1.0));
    let u = (t0 / t1).clamp(0.0, 1.0);
    split_cubic(&left, u).1
}

fn deriv_at(c: &[P; 4], t: f64) -> P {
    let s = 1.0 - t;
    (c[1] - c[0]) * (3.0 * s * s) + (c[2] - c[1]) * (6.0 * s * t) + (c[3] - c[2]) * (3.0 * t * t)
}

/// A side of a corner: the segment from the corner to its neighbour, as a
/// cubic leaving the corner.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Side {
    c: [P; 4],
}

impl Side {
    fn direction(&self) -> Option<P> {
        let c = self.c;
        [c[1], c[2], c[3]]
            .into_iter()
            .map(|q| q - c[0])
            .find(|d| d.len() > CEPS)
            .map(|d| d * (1.0 / d.len()))
    }
    fn length(&self) -> f64 {
        if is_straight_side(&self.c) {
            self.c[0].dist(self.c[3])
        } else {
            *arclen_table(&self.c, 128).last().unwrap_or(&0.0)
        }
    }
    /// The parameter `s` along the side from the corner, the point there,
    /// and the direction the side runs on in.
    fn at_length(&self, s: f64, u: P) -> (f64, P, P) {
        let c = self.c;
        if is_straight_side(&c) {
            let len = c[0].dist(c[3]);
            let t = if len > CEPS {
                (s / len).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return (t, c[0] + u * s, u);
        }
        let n = 128;
        let table = arclen_table(&c, n);
        let k = table.partition_point(|&l| l < s).clamp(1, n);
        let (l0, l1) = (table[k - 1], table[k]);
        let f = if l1 > l0 { (s - l0) / (l1 - l0) } else { 0.0 };
        let t = ((k - 1) as f64 + f.clamp(0.0, 1.0)) / n as f64;
        let d = deriv_at(&c, t);
        let dir = if d.len() > CEPS {
            d * (1.0 / d.len())
        } else {
            u
        };
        (t, cubic_at(&c, t), dir)
    }
}

/// A corner a cut can round: (subpath, anchor), where the sides meet, the
/// unit directions they leave in (towards the previous anchor, then the
/// next), and their lengths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corner {
    pub at_ref: AnchorRef,
    pub at: P,
    pub u: P,
    pub v: P,
    pub lu: f64,
    pub lv: f64,
    side_u: Side,
    side_v: Side,
    before: AnchorRef,
    after: AnchorRef,
}

impl Corner {
    fn of(sp: &SubPath, s: usize, i: usize) -> Option<Corner> {
        let n = sp.anchors.len();
        let (prev_i, next_i) = if sp.closed {
            ((i + n - 1) % n, (i + 1) % n)
        } else {
            (i.checked_sub(1)?, i + 1)
        };
        let (a, prev, next) = (
            sp.anchors.get(i)?,
            sp.anchors.get(prev_i)?,
            sp.anchors.get(next_i)?,
        );
        if a.is_smooth() {
            return None;
        }
        let side_u = Side {
            c: [a.p, a.h_in, prev.h_out, prev.p],
        };
        let side_v = Side {
            c: [a.p, a.h_out, next.h_in, next.p],
        };
        let (u, v) = (side_u.direction()?, side_v.direction()?);
        let (lu, lv) = (side_u.length(), side_v.length());
        if !(lu > CEPS && lv > CEPS && lu.is_finite() && lv.is_finite()) {
            return None;
        }
        let c = Corner {
            at_ref: (s, i),
            at: a.p,
            u,
            v,
            lu,
            lv,
            side_u,
            side_v,
            before: (s, prev_i),
            after: (s, next_i),
        };
        (c.sin() > 1e-6).then_some(c)
    }
    pub fn cos(&self) -> f64 {
        self.u.dot(self.v)
    }
    pub fn sin(&self) -> f64 {
        cross(self.u, self.v).abs()
    }
    /// How far along each side a round cut of radius `r` meets it.
    pub fn setback(&self, r: f64) -> f64 {
        r * (1.0 + self.cos()) / self.sin()
    }
    pub fn radius_at_setback(&self, s: f64) -> f64 {
        s * self.sin() / (1.0 + self.cos())
    }
    /// The largest radius, its cut reaching halfway along the shorter side.
    pub fn max_radius(&self) -> f64 {
        self.radius_at_setback(self.lu.min(self.lv) / 2.0)
    }
    /// The largest radius when the neighbours' cuts reach `before` and
    /// `after` along the sides: halfway, or on up to where they begin.
    fn max_radius_beside(&self, before: f64, after: f64) -> f64 {
        let room = |len: f64, other: f64| (len - other.max(0.0)).max(len / 2.0);
        self.radius_at_setback(room(self.lu, before).min(room(self.lv, after)))
    }
    /// The point `k` along the bisector (in u + v): a round cut of radius `r`
    /// is centred at `k = r / sin`.
    pub fn on_bisector(&self, k: f64) -> P {
        self.at + (self.u + self.v) * k
    }
    /// How much the radius grows when a widget on the bisector moves by `d`.
    pub fn radius_change(&self, d: P) -> f64 {
        d.dot(self.u + self.v) * self.sin() / (2.0 + 2.0 * self.cos())
    }

    fn cut(&self, radius: f64, kind: CornerKind) -> Cut {
        let s = self.setback(radius);
        let (tu, start, du) = self.side_u.at_length(s, self.u);
        let (tv, end, dv) = self.side_v.at_length(s, self.v);
        let cos = du.dot(dv);
        let (h_out, h_in) = match kind {
            CornerKind::Round => {
                let (hu, hv) = self.round_handles(radius, (start, du), (end, dv));
                (start - du * hu, end - dv * hv)
            }
            CornerKind::Inverted => {
                let h = s * arc_handle(cos);
                (start + square_to(du, dv) * h, end + square_to(dv, du) * h)
            }
            CornerKind::Chamfer => (start, end),
        };
        Cut {
            tu,
            tv,
            start,
            h_out,
            h_in,
            end,
        }
    }

    /// Handle lengths of a round cut. Between straight sides, the arc's; a
    /// curved side has turned by the time the cut meets it, so each handle is
    /// the share an arc takes of its tangent up to where the tangents cross.
    fn round_handles(&self, radius: f64, (start, du): (P, P), (end, dv): (P, P)) -> (f64, f64) {
        let cos = du.dot(dv);
        let arc = arc_handle(-cos);
        if is_straight_side(&self.side_u.c) && is_straight_side(&self.side_v.c) {
            return (radius * arc, radius * arc);
        }
        let (cr, w) = (cross(du, dv), start - end);
        let (a, b) = (cross(w, dv) / cr, cross(w, du) / cr);
        if cr.abs() > 1e-6 && a > CEPS && b > CEPS && a.is_finite() && b.is_finite() {
            let share = arc * (1.0 - cos) / cr.abs();
            (a * share, b * share)
        } else {
            let third = start.dist(end) / 3.0;
            (third, third)
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Cut {
    tu: f64,
    tv: f64,
    start: P,
    h_out: P,
    h_in: P,
    end: P,
}

/// The unit direction square to unit `a`, on `b`'s side.
fn square_to(a: P, b: P) -> P {
    let w = b - a * a.dot(b);
    let l = w.len();
    if l > CEPS {
        w * (1.0 / l)
    } else {
        P::default()
    }
}

/// Handle length per unit radius of a cubic arc spanning an angle with
/// cosine `cos`: 4/3 · tan(angle / 4).
fn arc_handle(cos: f64) -> f64 {
    if cos.abs() < 1e-12 {
        return KAPPA;
    }
    let cos = cos.clamp(-1.0, 1.0);
    let (sin_half, cos_half) = (
        ((1.0 - cos) / 2.0).max(0.0).sqrt(),
        ((1.0 + cos) / 2.0).max(0.0).sqrt(),
    );
    4.0 / 3.0 * sin_half / (1.0 + cos_half)
}

/// The corners of `path` that can be cut, in anchor order.
pub fn path_corners(path: &BezPath) -> Vec<Corner> {
    path.iter()
        .enumerate()
        .flat_map(|(s, sp)| (0..sp.anchors.len()).filter_map(move |i| Corner::of(sp, s, i)))
        .collect()
}

/// `path` with its corners cut: `radius(corner)` gives each one's radius
/// (0: leave it), fitted so that neighbouring cuts never pass each other.
pub fn cut_corners(path: &BezPath, radius: impl Fn(AnchorRef) -> f64, kind: CornerKind) -> BezPath {
    let corners = path_corners(path);
    let find = |r: AnchorRef| corners.iter().find(|c| c.at_ref == r);
    let asked = |r: AnchorRef| find(r).map_or(0.0, |c| c.setback(radius(r).max(0.0)));
    let fitted: Vec<(AnchorRef, f64)> = corners
        .iter()
        .map(|c| {
            let own = radius(c.at_ref).max(0.0);
            (
                c.at_ref,
                own.min(c.max_radius_beside(asked(c.before), asked(c.after))),
            )
        })
        .collect();
    let mut out = Vec::with_capacity(path.len());
    for (s, sp) in path.iter().enumerate() {
        let n = sp.anchors.len();
        let cuts: Vec<Option<Cut>> = (0..n)
            .map(|i| {
                let (_, r) = fitted.iter().find(|(at, _)| *at == (s, i))?;
                let c = find((s, i))?;
                (*r > CEPS).then(|| c.cut(*r, kind))
            })
            .collect();
        // Each segment from anchor j, trimmed to what the cuts at its ends
        // leave: its handles (out of j, into j+1); None when untouched.
        let trimmed: Vec<Option<(P, P)>> = (0..n)
            .map(|j| {
                let k = (j + 1) % n;
                if k == 0 && !sp.closed {
                    return None;
                }
                let (a, b) = (sp.anchors[j], sp.anchors[k]);
                let (from, to) = (cuts[j], cuts[k]);
                if from.is_none() && to.is_none() {
                    return None;
                }
                if a.h_out == a.p && b.h_in == b.p {
                    return Some((from.map_or(a.p, |c| c.end), to.map_or(b.p, |c| c.start)));
                }
                let (s0, s1) = (from.map_or(0.0, |c| c.tv), to.map_or(1.0, |c| 1.0 - c.tu));
                let sub = subsegment(&[a.p, a.h_out, b.h_in, b.p], s0, s1.max(s0));
                Some((sub[1], sub[2]))
            })
            .collect();
        let mut anchors = Vec::with_capacity(n + 4);
        let mut closing = None;
        for (i, a) in sp.anchors.iter().enumerate() {
            let h_in = if i > 0 || sp.closed {
                trimmed[(i + n - 1) % n].map_or(a.h_in, |(_, h)| h)
            } else {
                a.h_in
            };
            let h_out = trimmed[i].map_or(a.h_out, |(h, _)| h);
            match cuts[i] {
                Some(c) => {
                    let start = Anchor {
                        p: c.start,
                        h_in,
                        h_out: c.h_out,
                    };
                    let end = Anchor {
                        p: c.end,
                        h_in: c.h_in,
                        h_out,
                    };
                    if i == 0 && sp.closed {
                        closing = Some(start);
                    } else {
                        anchors.push(start);
                    }
                    anchors.push(end);
                }
                None => anchors.push(Anchor {
                    p: a.p,
                    h_in,
                    h_out,
                }),
            }
        }
        if let Some(a) = closing {
            anchors.push(a);
        }
        out.push(SubPath {
            anchors,
            closed: sp.closed,
        });
    }
    out
}

// ------------------------------------------------------------ freehand

/// A freehand stroke (pointer samples) as a smooth open path, after
/// VectorCraft's Paintbrush (`fit_freehand`, MIT OR Apache-2.0): samples
/// closer than a third of `tol` are dropped, `smooth` relaxation passes take
/// out the hand's jitter (ends fixed), turns sharper than ~100° stay corners,
/// and the rest is fitted with as few curves as keep within `tol`. With
/// `closed`, the end joins the start.
pub fn fit_stroke(samples: &[P], tol: f64, smooth: usize, closed: bool) -> Option<SubPath> {
    let tol = tol.max(1e-3);
    let min = tol / 3.0;
    let mut pts: Vec<P> = Vec::with_capacity(samples.len());
    for &p in samples {
        if pts.last().is_none_or(|q: &P| q.dist(p) >= min) {
            pts.push(p);
        }
    }
    // The last sample always ends the stroke.
    if let (Some(&end), true) = (samples.last(), pts.len() > 1) {
        let n = pts.len();
        pts[n - 1] = end;
    } else if samples.len() > 1 {
        pts.push(samples[samples.len() - 1]);
    }
    if closed && pts.len() > 2 && pts[0].dist(pts[pts.len() - 1]) < tol * 4.0 {
        pts.pop();
    }
    let n = pts.len();
    if n < 2 {
        return None;
    }
    // Corners: where the stroke doubles back (turn over ~100°), measured
    // across two samples so jitter doesn't count.
    let turn_at = |pts: &[P], i: usize| -> f64 {
        let m = pts.len();
        let (a, b) = if closed {
            (pts[(i + m - 2) % m], pts[(i + 2) % m])
        } else {
            if i < 2 || i + 2 >= m {
                return 0.0;
            }
            (pts[i - 2], pts[i + 2])
        };
        match (unit(pts[i] - a), unit(b - pts[i])) {
            (Some(u), Some(w)) => u.dot(w).clamp(-1.0, 1.0).acos().to_degrees(),
            _ => 0.0,
        }
    };
    let corner: Vec<bool> = (0..n)
        .map(|i| {
            let t = turn_at(&pts, i);
            t > 100.0
                && (1..=2)
                    .all(|d| t >= turn_at(&pts, (i + d) % n) && t >= turn_at(&pts, (i + n - d) % n))
        })
        .collect();
    // Relax: each point toward the middle of its neighbours (not the ends or
    // corners), a few times.
    for _ in 0..smooth {
        let prev = pts.clone();
        for i in 0..n {
            if corner[i] || (!closed && (i == 0 || i + 1 == n)) {
                continue;
            }
            let (a, b) = (prev[(i + n - 1) % n], prev[(i + 1) % n]);
            pts[i] = prev[i] * 0.5 + (a + b) * 0.25;
        }
    }
    let mut cuts: Vec<usize> = (0..n).filter(|&i| corner[i]).collect();
    let mut cubics: Vec<[P; 4]> = Vec::new();
    let fit = |run: &[P], cubics: &mut Vec<[P; 4]>, t: Option<P>| {
        if run.len() < 2 {
            return;
        }
        let m = run.len();
        let t0 = t
            .or_else(|| unit(run[1.min(m - 1)] - run[0]))
            .unwrap_or(P::new(1.0, 0.0));
        let t1 = t.or_else(|| unit(run[m - 1] - run[m - 2])).unwrap_or(t0);
        for c in fit_cubics(run, t0, t1, tol) {
            cubics.push(if is_straight(&c, tol * 0.05) {
                [c[0], c[0], c[3], c[3]]
            } else {
                c
            });
        }
    };
    if closed {
        if cuts.is_empty() {
            let t = unit(pts[1] - pts[n - 1]);
            let mut run = pts.clone();
            run.push(pts[0]);
            fit(&run, &mut cubics, t);
        } else {
            cuts.push(cuts[0] + n);
            for w in cuts.windows(2) {
                let run: Vec<P> = (w[0]..=w[1]).map(|k| pts[k % n]).collect();
                fit(&run, &mut cubics, None);
            }
        }
    } else {
        cuts.insert(0, 0);
        cuts.push(n - 1);
        cuts.dedup();
        for w in cuts.windows(2) {
            fit(&pts[w[0]..=w[1]], &mut cubics, None);
        }
    }
    let sp = anchors_from_cubics(&cubics, closed);
    (sp.anchors.len() >= 2).then_some(sp)
}

// ------------------------------------------------------------ curve fitting
//
// Least-squares cubic fitting: fit one cubic with fixed end tangents,
// reparameterise with Newton steps, split at the worst point (Schneider,
// "An Algorithm for Automatically Fitting Digitized Curves", Graphics Gems).
// Ported from VectorCraft's `pathops/src/fit.rs` (MIT OR Apache-2.0).

const MAX_REPARAM: usize = 6;

fn unit(v: P) -> Option<P> {
    let l = v.len();
    (l > 1e-12 && l.is_finite()).then(|| v * (1.0 / l))
}

fn cross(a: P, b: P) -> f64 {
    a.x * b.y - a.y * b.x
}

/// The direction a cubic leaves its start (robust to retracted handles).
fn start_tangent(c: &[P; 4]) -> P {
    unit(c[1] - c[0])
        .or_else(|| unit(c[2] - c[0]))
        .or_else(|| unit(c[3] - c[0]))
        .unwrap_or(P::new(1.0, 0.0))
}

/// The direction a cubic arrives at its end (pointing forward).
fn end_tangent(c: &[P; 4]) -> P {
    unit(c[3] - c[2])
        .or_else(|| unit(c[3] - c[1]))
        .or_else(|| unit(c[3] - c[0]))
        .unwrap_or(P::new(1.0, 0.0))
}

fn sample_n(c: &[P; 4], n: usize) -> Vec<P> {
    (0..=n).map(|k| cubic_at(c, k as f64 / n as f64)).collect()
}

fn cubic_len(c: &[P; 4]) -> f64 {
    sample_n(c, 16).windows(2).map(|w| w[0].dist(w[1])).sum()
}

/// A chain of G1-continuous cubics through `pts`, each within `tol` of
/// them. `t0` is the unit tangent leaving the first point, `t1` the one
/// arriving at the last (forward).
pub fn fit_cubics(pts: &[P], t0: P, t1: P, tol: f64) -> Vec<[P; 4]> {
    let mut v: Vec<P> = Vec::with_capacity(pts.len());
    for &p in pts {
        if v.last().is_none_or(|q: &P| q.dist(p) > 1e-12) {
            v.push(p);
        }
    }
    let tol = tol.max(1e-9);
    let mut pieces: Vec<(usize, usize, [P; 4])> = Vec::new();
    if v.len() >= 2 {
        fit_rec(&v, 0, t0, t1, tol, &mut pieces, 0);
    }
    // Splitting where the error is worst can leave two pieces one curve
    // could have drawn: merge neighbours while one fits within `tol`.
    loop {
        let mut merged = false;
        let mut k = 0;
        while k + 1 < pieces.len() {
            let (a, _, ca) = pieces[k];
            let (_, b, cb) = pieces[k + 1];
            let (c, err, _) = fit_single(&v[a..=b], start_tangent(&ca), end_tangent(&cb));
            if err <= tol {
                pieces[k] = (a, b, c);
                pieces.remove(k + 1);
                merged = true;
            } else {
                k += 1;
            }
        }
        if !merged {
            break;
        }
    }
    pieces.into_iter().map(|p| p.2).collect()
}

/// Fit `pts` (starting at index `at` of the whole run), pushing (first
/// index, last index, cubic) pieces.
fn fit_rec(
    pts: &[P],
    at: usize,
    t0: P,
    t1: P,
    tol: f64,
    out: &mut Vec<(usize, usize, [P; 4])>,
    depth: usize,
) {
    let n = pts.len();
    if n == 2 {
        let d = pts[0].dist(pts[1]) / 3.0;
        out.push((
            at,
            at + 1,
            [pts[0], pts[0] + t0 * d, pts[1] - t1 * d, pts[1]],
        ));
        return;
    }
    let (c, err, split) = fit_single(pts, t0, t1);
    if err <= tol || depth > 40 {
        out.push((at, at + n - 1, c));
        return;
    }
    // Split at the worst point (its tangent from its neighbours), but within
    // the middle third, so that no sliver of a curve is left at either end.
    let split = split.clamp(n / 3, n - 1 - n / 3).clamp(1, n - 2);
    let tc = unit(pts[split + 1] - pts[split - 1]).unwrap_or(t0);
    fit_rec(&pts[..=split], at, t0, tc, tol, out, depth + 1);
    fit_rec(&pts[split..], at + split, tc, t1, tol, out, depth + 1);
}

fn fit_single(pts: &[P], t0: P, t1: P) -> ([P; 4], f64, usize) {
    let mut u = chord_params(pts);
    let mut best = generate(pts, &u, t0, t1);
    let (mut best_err, mut best_split) = max_error(pts, &best, &u);
    for _ in 0..MAX_REPARAM {
        for (i, ui) in u.iter_mut().enumerate() {
            *ui = newton(&best, pts[i], *ui);
        }
        let c = generate(pts, &u, t0, t1);
        let (e, s) = max_error(pts, &c, &u);
        if e < best_err {
            best = c;
            best_err = e;
            best_split = s;
        } else {
            break;
        }
    }
    (best, best_err, best_split)
}

fn chord_params(pts: &[P]) -> Vec<f64> {
    let mut u = Vec::with_capacity(pts.len());
    let mut acc = 0.0;
    u.push(0.0);
    for w in pts.windows(2) {
        acc += w[0].dist(w[1]);
        u.push(acc);
    }
    if acc > 0.0 {
        for x in &mut u {
            *x /= acc;
        }
    }
    u
}

fn bern(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t]
}

/// The handle lengths along the fixed tangents (2 × 2 normal equations).
fn generate(pts: &[P], u: &[f64], t0: P, t1: P) -> [P; 4] {
    let (p0, p3) = (pts[0], pts[pts.len() - 1]);
    let dir_in = -t1;
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (p, &t) in pts.iter().zip(u) {
        let b = bern(t);
        let (a0, a1) = (t0 * b[1], dir_in * b[2]);
        c00 += a0.dot(a0);
        c01 += a0.dot(a1);
        c11 += a1.dot(a1);
        let tmp = *p - (p0 * (b[0] + b[1]) + p3 * (b[2] + b[3]));
        x0 += a0.dot(tmp);
        x1 += a1.dot(tmp);
    }
    let det = c00 * c11 - c01 * c01;
    let seg = p0.dist(p3);
    let eps = 1e-6 * seg;
    let (mut al, mut ar) = if det.abs() > 1e-12 {
        ((x0 * c11 - x1 * c01) / det, (c00 * x1 - c01 * x0) / det)
    } else {
        (seg / 3.0, seg / 3.0)
    };
    if !al.is_finite() || !ar.is_finite() || al < eps || ar < eps {
        al = seg / 3.0;
        ar = seg / 3.0;
    }
    // No wild handles on nearly degenerate data.
    let cap = seg * 4.0 + 1e-9;
    [p0, p0 + t0 * al.min(cap), p3 + dir_in * ar.min(cap), p3]
}

fn max_error(pts: &[P], c: &[P; 4], u: &[f64]) -> (f64, usize) {
    let (mut e, mut idx) = (0.0, pts.len() / 2);
    for (i, (p, &t)) in pts.iter().zip(u).enumerate() {
        let d = cubic_at(c, t).dist(*p);
        if d > e {
            e = d;
            idx = i;
        }
    }
    (e, idx)
}

fn newton(c: &[P; 4], p: P, t: f64) -> f64 {
    // First and second derivatives of the cubic.
    let d1 = [
        (c[1] - c[0]) * 3.0,
        (c[2] - c[1]) * 3.0,
        (c[3] - c[2]) * 3.0,
    ];
    let d2 = [(d1[1] - d1[0]) * 2.0, (d1[2] - d1[1]) * 2.0];
    let s = 1.0 - t;
    let q1 = d1[0] * (s * s) + d1[1] * (2.0 * s * t) + d1[2] * (t * t);
    let q2 = d2[0] * s + d2[1] * t;
    let q = cubic_at(c, t) - p;
    let den = q1.dot(q1) + q.dot(q2);
    if den.abs() < 1e-12 {
        return t;
    }
    let nt = t - q.dot(q1) / den;
    if nt.is_finite() {
        nt.clamp(0.0, 1.0)
    } else {
        t
    }
}

/// Whether a cubic is (within `tol`) the straight line between its ends,
/// handles inside the chord.
fn is_straight(c: &[P; 4], tol: f64) -> bool {
    let chord = c[3] - c[0];
    let len = chord.len();
    if len < 1e-12 {
        return c[1].dist(c[0]) <= tol && c[2].dist(c[0]) <= tol;
    }
    let dir = chord * (1.0 / len);
    [c[1], c[2]].iter().all(|&h| {
        let v = h - c[0];
        let along = v.dot(dir);
        cross(v, dir).abs() <= tol && along >= -tol && along <= len + tol
    })
}

/// Consecutive cubics as anchors (a closed loop's last end is its start).
fn anchors_from_cubics(cubics: &[[P; 4]], closed: bool) -> SubPath {
    let mut anchors: Vec<Anchor> = Vec::new();
    for c in cubics {
        match anchors.last_mut() {
            Some(last) => last.h_out = c[1],
            None => anchors.push(Anchor {
                p: c[0],
                h_in: c[0],
                h_out: c[1],
            }),
        }
        anchors.push(Anchor {
            p: c[3],
            h_in: c[2],
            h_out: c[3],
        });
    }
    if closed && anchors.len() > 1 {
        let last = anchors.pop().unwrap();
        anchors[0].h_in = last.h_in;
    }
    SubPath { anchors, closed }
}

/// Smooth curves through a traced outline (points, as booleans and offsets
/// make them). Corners stay corners (a turn of at least half `corner_deg`
/// that stands out from its neighbours', or any over 75°); between them, as
/// few cubics as keep within `tol`; straight runs become lines.
pub fn fit_outline(pts: &[P], closed: bool, tol: f64, corner_deg: f64) -> Option<SubPath> {
    let mut v: Vec<P> = Vec::with_capacity(pts.len());
    for &p in pts {
        if v.last().is_none_or(|q: &P| q.dist(p) > 1e-9) {
            v.push(p);
        }
    }
    if closed && v.len() > 1 && v[0].dist(v[v.len() - 1]) <= 1e-9 {
        v.pop();
    }
    let n = v.len();
    if n < if closed { 3 } else { 2 } {
        return None;
    }
    let at = |i: isize| -> P {
        if closed {
            v[i.rem_euclid(n as isize) as usize]
        } else {
            v[i.clamp(0, n as isize - 1) as usize]
        }
    };
    // How sharply the outline turns at each point.
    let turn: Vec<f64> = (0..n as isize)
        .map(|i| {
            if !closed && (i == 0 || i == n as isize - 1) {
                return 0.0;
            }
            let p = at(i);
            match (unit(p - at(i - 1)), unit(at(i + 1) - p)) {
                (Some(u), Some(w)) => u.dot(w).clamp(-1.0, 1.0).acos().to_degrees(),
                _ => 0.0,
            }
        })
        .collect();
    // A corner is a spike: a sharp turn whose neighbours hardly turn (a
    // flattened curve turns evenly, a little at every point, and small ones
    // turn up to 40° each), or one too sharp for any curve (over 75°).
    let mut corners: Vec<usize> = (0..n)
        .filter(|&i| {
            let (prev, next) = if closed {
                (turn[(i + n - 1) % n], turn[(i + 1) % n])
            } else {
                (
                    if i > 0 { turn[i - 1] } else { 0.0 },
                    if i + 1 < n { turn[i + 1] } else { 0.0 },
                )
            };
            turn[i] > 75.0
                || (turn[i] > corner_deg.min(75.0) * 0.5 && turn[i] >= 1.5 * prev.max(next))
        })
        .collect();
    let mut cubics: Vec<[P; 4]> = Vec::new();
    // A run's end direction: the chord to the next point is off a curve's
    // tangent by half the turn there, so take the slope at `a` of the
    // parabola through the next two points (spaced by arc length, as
    // flattening and intersections leave them uneven).
    let end_dir = |a: P, b: P, c: Option<P>| -> Option<P> {
        let d1 = unit(b - a)?;
        let Some(c) = c else { return Some(d1) };
        let (h1, h2) = (a.dist(b), b.dist(c));
        let d2 = unit(c - a)?;
        if d1.dot(d2) < 0.95 || h1 < 1e-9 || h2 < 1e-9 {
            return Some(d1);
        }
        let s2 = h1 + h2;
        let slope = a * -(1.0 / h1 + 1.0 / s2) + b * (s2 / (h1 * h2)) - c * (h1 / (s2 * h2));
        unit(slope).filter(|t| t.dot(d1) > 0.9).or(Some(d1))
    };
    let fit_run = |run: &[P], t0: Option<P>, t1: Option<P>, cubics: &mut Vec<[P; 4]>| {
        if run.len() < 2 {
            return;
        }
        let m = run.len();
        let t0 = t0
            .or_else(|| end_dir(run[0], run[1], run.get(2).copied()))
            .unwrap_or(P::new(1.0, 0.0));
        let t1 = t1
            .or_else(|| end_dir(run[m - 1], run[m - 2], (m >= 3).then(|| run[m - 3])).map(|d| -d))
            .unwrap_or(t0);
        for c in fit_cubics(run, t0, t1, tol) {
            cubics.push(if is_straight(&c, tol * 0.05) {
                [c[0], c[0], c[3], c[3]]
            } else {
                c
            });
        }
    };
    if closed {
        if corners.is_empty() {
            // A smooth loop: start anywhere, leave and arrive the same way.
            let t = unit(v[1] - v[n - 1]);
            let mut run = v.clone();
            run.push(v[0]);
            fit_run(&run, t, t, &mut cubics);
        } else {
            corners.push(corners[0] + n);
            for w in corners.windows(2) {
                let run: Vec<P> = (w[0]..=w[1]).map(|k| v[k % n]).collect();
                fit_run(&run, None, None, &mut cubics);
            }
        }
    } else {
        let mut cuts = vec![0];
        cuts.extend(corners);
        cuts.push(n - 1);
        cuts.dedup();
        for w in cuts.windows(2) {
            fit_run(&v[w[0]..=w[1]], None, None, &mut cubics);
        }
    }
    let sp = anchors_from_cubics(&cubics, closed);
    (sp.anchors.len() >= 2).then_some(sp)
}

// ------------------------------------------------------------ shapes

/// Handle length for a quarter circle of radius 1 drawn with one cubic.
const KAPPA: f64 = 0.552_284_749_8;

fn closed(anchors: Vec<Anchor>) -> BezPath {
    vec![SubPath {
        anchors,
        closed: true,
    }]
}

/// A rectangle from `a` to `b` (any two corners), corners rounded by `r`.
pub fn rect(a: P, b: P, r: f64) -> BezPath {
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    let r = r.max(0.0).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    if r < 1e-9 {
        return closed(
            [
                P::new(x0, y0),
                P::new(x1, y0),
                P::new(x1, y1),
                P::new(x0, y1),
            ]
            .into_iter()
            .map(Anchor::corner)
            .collect(),
        );
    }
    let k = r * KAPPA;
    // Clockwise from the top edge; each corner is two anchors joined by a curve.
    let pt = |x: f64, y: f64, hin: (f64, f64), hout: (f64, f64)| Anchor {
        p: P::new(x, y),
        h_in: P::new(x + hin.0, y + hin.1),
        h_out: P::new(x + hout.0, y + hout.1),
    };
    closed(vec![
        pt(x0 + r, y0, (-k, 0.0), (0.0, 0.0)),
        pt(x1 - r, y0, (0.0, 0.0), (k, 0.0)),
        pt(x1, y0 + r, (0.0, -k), (0.0, 0.0)),
        pt(x1, y1 - r, (0.0, 0.0), (0.0, k)),
        pt(x1 - r, y1, (k, 0.0), (0.0, 0.0)),
        pt(x0 + r, y1, (0.0, 0.0), (-k, 0.0)),
        pt(x0, y1 - r, (0.0, k), (0.0, 0.0)),
        pt(x0, y0 + r, (0.0, 0.0), (0.0, -k)),
    ])
}

/// An ellipse filling the box from `a` to `b`.
pub fn ellipse(a: P, b: P) -> BezPath {
    let c = (a + b) * 0.5;
    let (rx, ry) = ((b.x - a.x).abs() / 2.0, (b.y - a.y).abs() / 2.0);
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let pt = |x: f64, y: f64, d: (f64, f64)| Anchor {
        p: P::new(x, y),
        h_in: P::new(x - d.0, y - d.1),
        h_out: P::new(x + d.0, y + d.1),
    };
    closed(vec![
        pt(c.x, c.y - ry, (kx, 0.0)),
        pt(c.x + rx, c.y, (0.0, ky)),
        pt(c.x, c.y + ry, (-kx, 0.0)),
        pt(c.x - rx, c.y, (0.0, -ky)),
    ])
}

/// A regular polygon: `n` corners on a circle of radius `r`, the first at
/// angle `rot` (radians, 0 = right, clockwise on screen).
pub fn polygon(c: P, r: f64, n: usize, rot: f64) -> BezPath {
    let n = n.max(3);
    closed(
        (0..n)
            .map(|k| {
                let a = rot + k as f64 * std::f64::consts::TAU / n as f64;
                Anchor::corner(c + P::new(a.cos(), a.sin()) * r)
            })
            .collect(),
    )
}

/// A star: `n` points at radius `r`, the notches between them at `inner`.
pub fn star(c: P, r: f64, inner: f64, n: usize, rot: f64) -> BezPath {
    let n = n.max(3);
    closed(
        (0..2 * n)
            .map(|k| {
                let a = rot + k as f64 * std::f64::consts::PI / n as f64;
                let rr = if k % 2 == 0 { r } else { inner };
                Anchor::corner(c + P::new(a.cos(), a.sin()) * rr)
            })
            .collect(),
    )
}

/// A straight line.
pub fn line(a: P, b: P) -> BezPath {
    vec![SubPath {
        anchors: vec![Anchor::corner(a), Anchor::corner(b)],
        closed: false,
    }]
}

/// The anchors of a subpath in the other direction (handles swap sides).
pub fn reversed(sp: &SubPath) -> SubPath {
    SubPath {
        anchors: sp
            .anchors
            .iter()
            .rev()
            .map(|a| Anchor {
                p: a.p,
                h_in: a.h_out,
                h_out: a.h_in,
            })
            .collect(),
        closed: sp.closed,
    }
}

/// `q` moved onto the nearest 45° line through `from` (Shift).
pub fn constrain_45(from: P, q: P) -> P {
    let d = q - from;
    if d.len() < 1e-12 {
        return q;
    }
    let a = (d.y.atan2(d.x) / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
    let u = P::new(a.cos(), a.sin());
    from + u * d.dot(u)
}

pub fn bounds(path: &BezPath) -> crate::geom::Bounds {
    let mut b = crate::geom::Bounds::empty();
    for sp in path {
        for (a, z) in sp.segments() {
            for q in sample(&sp.cubic(a, z), 16) {
                b.add(q);
            }
        }
        if let Some(a) = sp.anchors.first() {
            b.add(a.p);
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::IDENTITY;

    /// A circle flattened the way imports flatten curves (0.3 units).
    fn flat_circle(c: P, r: f64) -> Vec<P> {
        let step = 2.0 * (1.0 - 0.3 / r).acos();
        let n = ((std::f64::consts::TAU / step).ceil() as usize).max(8);
        (0..n)
            .map(|k| {
                let a = k as f64 / n as f64 * std::f64::consts::TAU;
                c + P::new(a.cos(), a.sin()) * r
            })
            .collect()
    }

    fn max_dev(path: &BezPath, truth: impl Fn(P) -> f64) -> f64 {
        path.iter()
            .flat_map(|sp| sp.segments().into_iter().map(move |(a, b)| sp.cubic(a, b)))
            .flat_map(|c| sample_n(&c, 20))
            .map(truth)
            .fold(0.0, f64::max)
    }

    fn poly(pts: &[(f64, f64)], closed: bool) -> BezPath {
        vec![SubPath {
            anchors: pts
                .iter()
                .map(|&(x, y)| Anchor::corner(P::new(x, y)))
                .collect(),
            closed,
        }]
    }

    #[test]
    fn a_square_gets_round_corners_tangent_to_its_sides() {
        let sq = poly(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        );
        assert_eq!(path_corners(&sq).len(), 4);
        let cut = cut_corners(&sq, |_| 10.0, CornerKind::Round);
        let sp = &cut[0];
        assert_eq!(sp.anchors.len(), 8);
        // Each cut runs from 10 along one side to 10 along the next, and
        // stays 10 from the centre of its round.
        for p in [
            P::new(10.0, 0.0),
            P::new(90.0, 0.0),
            P::new(100.0, 10.0),
            P::new(0.0, 90.0),
        ] {
            assert!(sp.anchors.iter().any(|a| a.p.dist(p) < 1e-9), "{p:?}");
        }
        let arc = sp
            .segments()
            .into_iter()
            .map(|(a, b)| sp.cubic(a, b))
            .find(|c| c[0].dist(P::new(90.0, 0.0)) < 1e-9)
            .unwrap();
        let mid = cubic_at(&arc, 0.5);
        assert!(
            (mid.dist(P::new(90.0, 10.0)) - 10.0).abs() < 0.05,
            "{mid:?}"
        );
        // The new anchors are smooth joins of arc and side.
        let rounded = cut_corners(&sq, |_| 200.0, CornerKind::Round);
        // Radii stop at half the side: a circle-ish shape, 8 anchors.
        assert_eq!(rounded[0].anchors.len(), 8);
    }

    #[test]
    fn chamfers_and_inverted_rounds_and_only_some_corners() {
        let tri = poly(&[(0.0, 0.0), (100.0, 0.0), (0.0, 100.0)], true);
        let ch = cut_corners(
            &tri,
            |r| if r == (0, 0) { 10.0 } else { 0.0 },
            CornerKind::Chamfer,
        );
        assert_eq!(ch[0].anchors.len(), 4);
        assert!(ch[0].anchors.iter().all(|a| !a.has_handles()));
        let inv = cut_corners(&tri, |_| 5.0, CornerKind::Inverted);
        assert_eq!(inv[0].anchors.len(), 6);
        // An inverted cut scoops in: its middle is nearer the corner.
        let sp = &inv[0];
        let c = sp
            .segments()
            .into_iter()
            .map(|(a, b)| sp.cubic(a, b))
            .find(|c| c[0].y.abs() < 1e-9 && c[0].x > 0.0 && c[0].x < 10.0 && c[3].x.abs() < 1e-9)
            .or_else(|| {
                sp.segments()
                    .into_iter()
                    .map(|(a, b)| sp.cubic(a, b))
                    .find(|c| c[3].y.abs() < 1e-9 && c[0].x.abs() < 1e-9)
            })
            .unwrap();
        let mid = cubic_at(&c, 0.5);
        assert!(mid.len() < c[0].len().max(c[3].len()), "{mid:?} in {c:?}");
    }

    #[test]
    fn corners_next_to_a_curve_trim_it_along_itself() {
        let mut sq = poly(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        );
        sq[0].anchors[1].h_out = P::new(140.0, 30.0);
        sq[0].anchors[2].h_in = P::new(140.0, 70.0);
        let side = [
            P::new(100.0, 0.0),
            P::new(140.0, 30.0),
            P::new(140.0, 70.0),
            P::new(100.0, 100.0),
        ];
        let on_side = |p: P| {
            (0..=1000)
                .map(|i| cubic_at(&side, i as f64 / 1000.0).dist(p))
                .fold(f64::MAX, f64::min)
        };
        let cut = cut_corners(&sq, |_| 10.0, CornerKind::Round);
        let sp = &cut[0];
        assert_eq!(sp.anchors.len(), 8);
        let (a, b) = (sp.anchors[2], sp.anchors[3]);
        assert!(on_side(a.p) < 0.1 && on_side(b.p) < 0.1, "{a:?} {b:?}");
        let trimmed = [a.p, a.h_out, b.h_in, b.p];
        for t in [0.25, 0.5, 0.75] {
            assert!(on_side(cubic_at(&trimmed, t)) < 0.1);
        }
        // Smooth curves and open ends aren't corners.
        let open = poly(
            &[(0.0, 0.0), (50.0, 0.0), (100.0, 0.0), (100.0, 50.0)],
            false,
        );
        assert_eq!(
            path_corners(&open)
                .iter()
                .map(|c| c.at_ref)
                .collect::<Vec<_>>(),
            [(0, 2)]
        );
        assert!(path_corners(&ellipse(P::new(0.0, 0.0), P::new(10.0, 10.0))).is_empty());
    }

    #[test]
    fn simplify_keeps_a_teardrops_one_corner() {
        // A circle-ish body with one sharp tip at the top, as corner points.
        let mut anchors = vec![Anchor::corner(P::new(0.0, -100.0))];
        let n = 60;
        for k in 1..n {
            let a = -std::f64::consts::FRAC_PI_2
                + 0.5
                + (std::f64::consts::TAU - 1.0) * k as f64 / n as f64;
            anchors.push(Anchor::corner(P::new(
                a.cos() * 50.0,
                a.sin() * 50.0 + 10.0,
            )));
        }
        let path = vec![SubPath {
            anchors,
            closed: true,
        }];
        let out = simplify(&path, 1.0);
        let tip = out[0]
            .anchors
            .iter()
            .find(|a| a.p.dist(P::new(0.0, -100.0)) < 1e-6)
            .expect("the tip stays");
        assert!(!tip.is_smooth(), "{tip:?}");
    }

    /// A wobbly hand-drawn S curve, then a sharp V.
    fn wobbly(n: usize) -> Vec<P> {
        (0..=n)
            .map(|k| {
                let t = k as f64 / n as f64;
                let x = t * 400.0;
                let y = (t * std::f64::consts::TAU).sin() * 80.0;
                // Hand jitter: a fast wobble of ±1.5 units.
                let j = ((k * 7919) % 13) as f64 / 13.0 * 3.0 - 1.5;
                P::new(x, y + j)
            })
            .collect()
    }

    #[test]
    fn a_freehand_stroke_becomes_a_few_smooth_curves() {
        let pts = wobbly(300);
        let sp = fit_stroke(&pts, 4.0, 2, false).unwrap();
        assert!(!sp.closed);
        assert!(sp.anchors.len() <= 8, "{} anchors", sp.anchors.len());
        assert!(
            sp.anchors[1..sp.anchors.len() - 1]
                .iter()
                .all(|a| a.is_smooth()),
            "{:?}",
            sp.anchors
        );
        // Ends where the stroke did; stays near the drawn line.
        assert!(sp.anchors[0].p.dist(pts[0]) < 1e-9);
        assert!(sp.anchors[sp.anchors.len() - 1].p.dist(pts[300]) < 1e-9);
        let truth = |p: P| (p.y - (p.x / 400.0 * std::f64::consts::TAU).sin() * 80.0).abs();
        let dev = max_dev(&vec![sp], truth);
        assert!(dev < 7.0, "{dev}");
    }

    #[test]
    fn a_freehand_v_keeps_its_point_and_a_loop_closes() {
        let mut pts: Vec<P> = (0..=100)
            .map(|k| P::new(k as f64 * 2.0, k as f64 * 3.0))
            .collect();
        pts.extend((1..=100).map(|k| P::new(200.0 + k as f64 * 2.0, 300.0 - k as f64 * 3.0)));
        let sp = fit_stroke(&pts, 3.0, 2, false).unwrap();
        let tip = sp
            .anchors
            .iter()
            .find(|a| a.p.dist(P::new(200.0, 300.0)) < 1.0)
            .expect("the tip stays");
        assert!(!tip.is_smooth());
        let ring: Vec<P> = (0..200)
            .map(|k| {
                let a = k as f64 / 200.0 * std::f64::consts::TAU;
                P::new(a.cos(), a.sin()) * 100.0
            })
            .collect();
        let sp = fit_stroke(&ring, 2.0, 1, true).unwrap();
        assert!(sp.closed);
        assert!(
            sp.anchors.len() <= 8 && sp.anchors.iter().all(|a| a.is_smooth()),
            "{:?}",
            sp.anchors
        );
    }

    #[test]
    fn fit_cubics_recovers_a_cubic() {
        let c = [
            P::new(0.0, 0.0),
            P::new(30.0, 60.0),
            P::new(70.0, 60.0),
            P::new(100.0, 0.0),
        ];
        let pts = sample_n(&c, 40);
        let out = fit_cubics(&pts, start_tangent(&c), end_tangent(&c), 0.05);
        assert_eq!(out.len(), 1);
        assert!(out[0][1].dist(c[1]) < 1.0, "{:?}", out[0]);
    }

    #[test]
    fn a_flattened_circle_comes_back_as_a_few_smooth_curves() {
        let ring = flat_circle(P::new(0.0, 0.0), 100.0);
        let path = from_rings(&[ring], 0.2);
        let sp = &path[0];
        assert!(sp.anchors.len() <= 8, "{} anchors", sp.anchors.len());
        assert!(
            sp.anchors.iter().all(|a| a.is_smooth()),
            "no corners on a circle"
        );
        let dev = max_dev(&path, |p| (p.len() - 100.0).abs());
        assert!(dev < 0.6, "{dev}");
    }

    #[test]
    fn small_curves_are_not_mistaken_for_corners() {
        // 0.6 mm radius: each flattened step turns about 35°.
        let path = from_rings(&[flat_circle(P::new(0.0, 0.0), 6.0)], 0.2);
        assert!(
            path[0].anchors.iter().all(|a| a.is_smooth()),
            "{:?}",
            path[0].anchors
        );
    }

    #[test]
    fn two_united_circles_keep_their_two_corners() {
        use crate::boolean::{PathOp, pathfinder};
        use crate::geom::FillRule;
        let a = flat_circle(P::new(-60.0, 0.0), 100.0);
        let b = flat_circle(P::new(60.0, 0.0), 100.0);
        let rings = pathfinder(
            &[(vec![a], FillRule::NonZero), (vec![b], FillRule::NonZero)],
            PathOp::Unite,
        );
        let path = from_rings(&rings, 0.2);
        assert_eq!(path.len(), 1);
        let sp = &path[0];
        let corners = sp.anchors.iter().filter(|a| !a.is_smooth()).count();
        assert_eq!(corners, 2, "{:?}", sp.anchors);
        assert!(sp.anchors.len() <= 8, "{} anchors", sp.anchors.len());
        let dev = max_dev(&path, |p| {
            ((p - P::new(-60.0, 0.0)).len() - 100.0)
                .abs()
                .min(((p - P::new(60.0, 0.0)).len() - 100.0).abs())
        });
        assert!(dev < 0.6, "{dev}");
    }

    #[test]
    fn a_square_stays_four_straight_corners() {
        let sq = vec![
            P::new(0.0, 0.0),
            P::new(50.0, 0.0),
            P::new(100.0, 0.0),
            P::new(100.0, 100.0),
            P::new(0.0, 100.0),
        ];
        let path = from_rings(&[sq], 0.2);
        let sp = &path[0];
        assert_eq!(sp.anchors.len(), 4, "{:?}", sp.anchors);
        assert!(sp.anchors.iter().all(|a| !a.has_handles()));
    }

    #[test]
    fn simplify_fits_curves_not_corner_points() {
        // A circle drawn as 40 corner points simplifies to a few smooth ones.
        let ring = vec![SubPath {
            anchors: (0..40)
                .map(|k| {
                    let a = k as f64 / 40.0 * std::f64::consts::TAU;
                    Anchor::corner(P::new(a.cos(), a.sin()) * 100.0)
                })
                .collect(),
            closed: true,
        }];
        let out = simplify(&ring, 1.5);
        assert!(out[0].anchors.len() <= 8, "{}", out[0].anchors.len());
        assert!(out[0].anchors.iter().all(|a| a.is_smooth()));
        let dev = max_dev(&out, |p| (p.len() - 100.0).abs());
        assert!(dev < 2.5, "{dev}");
        // A square keeps its corners and gets no more points.
        let sq = vec![SubPath {
            anchors: [
                P::new(0.0, 0.0),
                P::new(10.0, 0.0),
                P::new(10.0, 10.0),
                P::new(0.0, 10.0),
            ]
            .into_iter()
            .map(Anchor::corner)
            .collect(),
            closed: true,
        }];
        let out = simplify(&sq, 1.0);
        assert_eq!(out[0].anchors.len(), 4);
        assert!(out[0].anchors.iter().all(|a| !a.has_handles()));
    }

    fn parse(d: &str) -> BezPath {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><path id="p" d="{d}" fill="black"/></svg>"#
        );
        let tree = crate::svg::parse_tree(svg.as_bytes()).unwrap();
        let Some(usvg::Node::Path(p)) = tree.node_by_id("p") else {
            panic!()
        };
        from_usvg(p.data(), IDENTITY)
    }

    #[test]
    fn reads_lines_curves_and_closes_cleanly() {
        let p = parse("M10 10 L50 10 C60 10 60 40 50 40 Z");
        assert_eq!(p.len(), 1);
        assert!(p[0].closed);
        assert_eq!(p[0].anchors.len(), 3);
        assert_eq!(p[0].anchors[1].h_out, P::new(60.0, 10.0));
        assert_eq!(p[0].anchors[2].h_in, P::new(60.0, 40.0));
        assert_eq!(to_svg_d(&p), "M10 10 L50 10 C60 10 60 40 50 40 Z");
        // A circle comes in as four smooth anchors and writes back as curves.
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><circle id="c" cx="50" cy="50" r="20"/></svg>"#;
        let tree = crate::svg::parse_tree(svg.as_bytes()).unwrap();
        let Some(usvg::Node::Path(c)) = tree.node_by_id("c") else {
            panic!()
        };
        let c = from_usvg(c.data(), IDENTITY);
        assert_eq!(c[0].anchors.len(), 4);
        assert!(c[0].closed && c[0].anchors.iter().all(|a| a.is_smooth()));
    }

    #[test]
    fn split_keeps_the_shape_and_delete_removes_points() {
        let mut p = parse("M0 0 C0 50 100 50 100 0");
        let before: Vec<P> = sample(&p[0].cubic(0, 1), 10);
        let (s, k) = split(&mut p, 0, 0, 0.5);
        assert_eq!((s, k), (0, 1));
        assert_eq!(p[0].anchors.len(), 3);
        let mid = cubic_at(&p[0].cubic(0, 1), 1.0);
        assert!(
            (mid - before[5]).len() < 1e-9,
            "the new point is on the old curve"
        );
        let q = cubic_at(&p[0].cubic(1, 2), 0.5);
        assert!(
            (q - cubic_at(
                &[
                    before[0],
                    P::new(0.0, 50.0),
                    P::new(100.0, 50.0),
                    before[10]
                ],
                0.75
            ))
            .len()
                < 1e-9
        );
        delete(&mut p, &[(0, 1)]);
        assert_eq!(p[0].anchors.len(), 2);
        delete(&mut p, &[(0, 0)]);
        assert!(p.is_empty(), "one point isn't a path");
    }

    #[test]
    fn simplify_keeps_the_shape_with_fewer_points() {
        // A circle drawn with 64 corner points.
        let ring: Vec<P> = (0..64)
            .map(|k| {
                let a = k as f64 / 64.0 * std::f64::consts::TAU;
                P::new(100.0 * a.cos(), 100.0 * a.sin())
            })
            .collect();
        let p = vec![SubPath {
            anchors: ring.iter().map(|&q| Anchor::corner(q)).collect(),
            closed: true,
        }];
        let s = simplify(&p, 3.0);
        let n = s[0].anchors.len();
        assert!(n < 20, "{n} points");
        for (a, b) in s[0].segments() {
            for q in sample(&s[0].cubic(a, b), 8) {
                assert!((q.len() - 100.0).abs() < 6.0, "{}", q.len());
            }
        }
        // A square keeps its corners.
        let sq = from_rings(
            &[vec![
                P::new(0.0, 0.0),
                P::new(50.0, 0.0),
                P::new(100.0, 0.0),
                P::new(100.0, 100.0),
                P::new(0.0, 100.0),
            ]],
            0.5,
        );
        assert_eq!(sq[0].anchors.len(), 4);
        assert_eq!(simplify(&sq, 0.5)[0].anchors.len(), 4);
        assert!(
            simplify(&sq, 0.5)[0]
                .anchors
                .iter()
                .all(|a| !a.has_handles())
        );
    }

    #[test]
    fn shapes_have_the_right_size() {
        let r = rect(P::new(10.0, 10.0), P::new(-10.0, -20.0), 0.0);
        assert_eq!(r[0].anchors.len(), 4);
        let b = bounds(&r);
        assert_eq!((b.width(), b.height()), (20.0, 30.0));
        let rr = rect(P::new(0.0, 0.0), P::new(100.0, 50.0), 10.0);
        assert_eq!(rr[0].anchors.len(), 8);
        let b = bounds(&rr);
        assert!((b.width() - 100.0).abs() < 1e-6 && (b.height() - 50.0).abs() < 1e-6);
        let e = ellipse(P::new(0.0, 0.0), P::new(100.0, 60.0));
        let b = bounds(&e);
        assert!((b.width() - 100.0).abs() < 0.5 && (b.height() - 60.0).abs() < 0.5);
        // A circle drawn this way is within 0.03% of round.
        for (a, z) in e[0].segments() {
            for q in sample(&e[0].cubic(a, z), 8) {
                let d = ((q.x - 50.0) / 50.0).powi(2) + ((q.y - 30.0) / 30.0).powi(2);
                assert!((d - 1.0).abs() < 0.003, "{d}");
            }
        }
        let h = polygon(P::new(0.0, 0.0), 10.0, 6, 0.0);
        assert_eq!(h[0].anchors.len(), 6);
        let st = star(P::new(0.0, 0.0), 10.0, 4.0, 5, 0.0);
        assert_eq!(st[0].anchors.len(), 10);
        assert!((st[0].anchors[1].p.len() - 4.0).abs() < 1e-9);
        assert_eq!(
            constrain_45(P::new(0.0, 0.0), P::new(10.0, 1.0)),
            P::new(10.0, 0.0)
        );
        let rev = reversed(&rr[0]);
        assert_eq!(rev.anchors[0].p, rr[0].anchors[7].p);
        assert_eq!(rev.anchors[0].h_out, rr[0].anchors[7].h_in);
    }

    #[test]
    fn hit_tests_find_points_and_segments() {
        let p = parse("M0 0 L100 0 L100 100 Z");
        assert_eq!(hit_anchor(&p, P::new(99.0, 1.0), 3.0), Some((0, 1)));
        let (s, a, t) = hit_segment(&p, P::new(50.0, 1.0), 3.0).unwrap();
        assert_eq!((s, a), (0, 0));
        assert!((t - 0.5).abs() < 0.03);
        let (_, a, _) = hit_segment(&p, P::new(49.0, 51.0), 3.0).unwrap();
        assert_eq!(a, 2, "the closing segment");
    }
}
