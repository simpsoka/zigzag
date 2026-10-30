//! Path clean-up after VectorCraft's (and Illustrator's): the Smooth tool
//! (refit the anchors near a drag into fewer, smoother curves), the Path
//! Eraser (cut away the parts of a path under a drag) and the Simplify
//! slider's levels (0: fewest points … 100: closest to the original).

use crate::bezier::{self, Anchor, BezPath, SubPath};
use crate::geom::P;

/// Design units (0.1 mm) in a typographic point.
pub const PT: f64 = 25.4 / 72.0 * 10.0;

/// The Simplify slider's level when it opens without Auto.
pub const SIMPLIFY_LEVEL: f64 = 50.0;

/// The tolerance (points) of Simplify level `level`: 1 pt at
/// [`SIMPLIFY_LEVEL`], doubled for every 10 levels below it and halved for
/// every 10 above (level 0 is 32 pt, level 100 is 1/32 pt).
pub fn simplify_level_tolerance(level: f64) -> f64 {
    let level = if level.is_finite() {
        level.clamp(0.0, 100.0)
    } else {
        SIMPLIFY_LEVEL
    };
    ((SIMPLIFY_LEVEL - level) / 10.0).exp2()
}

/// Auto-Simplify's level for paths `size` points across (the longer side):
/// the lowest whole level whose tolerance is at most 1/400 of the size.
pub fn auto_simplify_level(size: f64) -> f64 {
    let target = size / 400.0;
    if !(target.is_finite() && target > 0.0) {
        return 100.0;
    }
    (SIMPLIFY_LEVEL - 10.0 * target.log2())
        .ceil()
        .clamp(0.0, 100.0)
}

fn unit(v: P) -> Option<P> {
    let l = v.len();
    (l > 1e-12).then(|| v * (1.0 / l))
}

/// Points along a cubic, straight or not, close enough together to fit to.
fn dense(c: &[P; 4]) -> Vec<P> {
    (0..=16)
        .map(|k| bezier::cubic_at(c, k as f64 / 16.0))
        .collect()
}

/// How far either side (in multiples of the fidelity) the Smooth tool
/// averages a path over.
const SMOOTH_RADIUS: f64 = 4.0;

/// `pts` again, evenly `h` apart along it (fewer when that would be very
/// many); the ends stay.
fn resample(pts: &[P], h: f64) -> Vec<P> {
    let total: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    if pts.len() < 2 || total <= 0.0 {
        return pts.to_vec();
    }
    let n = ((total / h.max(1e-9)).ceil() as usize).clamp(2, 4000);
    let step = total / n as f64;
    let mut out = Vec::with_capacity(n + 1);
    out.push(pts[0]);
    let (mut k, mut acc) = (0, 0.0);
    for i in 1..n {
        let want = i as f64 * step;
        while k + 1 < pts.len() && acc + pts[k].dist(pts[k + 1]) < want {
            acc += pts[k].dist(pts[k + 1]);
            k += 1;
        }
        if k + 1 >= pts.len() {
            break;
        }
        let l = pts[k].dist(pts[k + 1]);
        let t = if l > 0.0 { (want - acc) / l } else { 0.0 };
        out.push(pts[k] + (pts[k + 1] - pts[k]) * t);
    }
    out.push(pts[pts.len() - 1]);
    out
}

/// Evenly spaced `pts` averaged over `radius` either side (a tent, so it's
/// smooth). An open run's ends stay put, the averaging narrowing towards them.
fn blur(pts: &[P], radius: f64, closed: bool) -> Vec<P> {
    let n = pts.len();
    if n < 3 {
        return pts.to_vec();
    }
    let h = if closed {
        let mut l: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
        l += pts[n - 1].dist(pts[0]);
        l / n as f64
    } else {
        pts.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>() / (n - 1) as f64
    };
    let k = ((radius / h.max(1e-9)).round() as usize).min(n / 2);
    (0..n)
        .map(|i| {
            let k = if closed { k } else { k.min(i).min(n - 1 - i) };
            if k == 0 {
                return pts[i];
            }
            let (mut sum, mut wsum) = (P::new(0.0, 0.0), 0.0);
            for d in 0..=2 * k {
                let off = d as isize - k as isize;
                let j = (i as isize + off).rem_euclid(n as isize) as usize;
                let w = (k + 1) as f64 - off.unsigned_abs() as f64;
                sum = sum + pts[j] * w;
                wsum += w;
            }
            sum * (1.0 / wsum)
        })
        .collect()
}

/// The heading of `pts` from its start: towards the point a quarter of the
/// way along.
fn heading(pts: &[P]) -> Option<P> {
    let total: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    let mut acc = 0.0;
    for w in pts.windows(2) {
        acc += w[0].dist(w[1]);
        if acc >= total / 4.0 {
            return unit(w[1] - pts[0]);
        }
    }
    unit(pts[pts.len() - 1] - pts[0])
}

/// Refit the open run `anchors` with as few cubics as stay within `tol`,
/// keeping its end points, their outer handles and the directions the run
/// leaves and arrives at them (so it meets the rest of the path smoothly).
/// Never more anchors than it had.
fn refit_run(anchors: &[Anchor], tol: f64) -> Vec<Anchor> {
    let n = anchors.len();
    if n < 3 {
        return anchors.to_vec();
    }
    let mut pts = vec![anchors[0].p];
    for w in anchors.windows(2) {
        let c = [w[0].p, w[0].h_out, w[1].h_in, w[1].p];
        pts.extend(dense(&c).into_iter().skip(1));
    }
    // Take the wobble out first (fitting alone would keep every wiggle
    // bigger than `tol`), then fit what's left.
    let pts = blur(&resample(&pts, tol / 4.0), SMOOTH_RADIUS * tol, false);
    let tol = tol / 2.0;
    let (first, last) = (anchors[0], anchors[n - 1]);
    // The directions the run leaves and arrives in: along the handles that
    // stay, or (from a corner) the run's general heading near that end.
    let Some(t0) = unit(first.h_out - first.p).or_else(|| heading(&pts)) else {
        return anchors.to_vec();
    };
    let back: Vec<P> = pts.iter().rev().copied().collect();
    let Some(t1) = unit(last.p - last.h_in).or_else(|| heading(&back).map(|t| t * -1.0)) else {
        return anchors.to_vec();
    };
    let cubics = bezier::fit_cubics(&pts, t0, t1, tol);
    if cubics.is_empty() || cubics.len() + 1 > n {
        return anchors.to_vec();
    }
    let mut out = Vec::with_capacity(cubics.len() + 1);
    out.push(Anchor {
        p: first.p,
        h_in: first.h_in,
        h_out: cubics[0][1],
    });
    for k in 1..cubics.len() {
        out.push(Anchor {
            p: cubics[k][0],
            h_in: cubics[k - 1][2],
            h_out: cubics[k][1],
        });
    }
    out.push(Anchor {
        p: last.p,
        h_in: cubics[cubics.len() - 1][2],
        h_out: last.h_out,
    });
    out
}

/// `anchors` (an open run) with each stretch of `marked` ones refitted,
/// together with the segments leading into it.
fn smooth_open(anchors: &[Anchor], marked: &[bool], tol: f64) -> Vec<Anchor> {
    let n = anchors.len();
    let mut runs = Vec::new();
    let mut i = 0;
    while i < n {
        if marked[i] {
            let a = i;
            while i + 1 < n && marked[i + 1] {
                i += 1;
            }
            runs.push((a, i));
        }
        i += 1;
    }
    let mut out = anchors.to_vec();
    for (a, b) in runs.into_iter().rev() {
        let lo = a.saturating_sub(1);
        let hi = (b + 1).min(n - 1);
        if hi - lo < 2 {
            continue;
        }
        let new = refit_run(&out[lo..=hi], tol);
        out.splice(lo..=hi, new);
    }
    out
}

fn smooth_subpath(sp: &SubPath, near: &dyn Fn(P) -> bool, tol: f64) -> Option<SubPath> {
    let n = sp.anchors.len();
    let marked: Vec<bool> = sp.anchors.iter().map(|a| near(a.p)).collect();
    if n < 3 || !marked.iter().any(|m| *m) {
        return None;
    }
    if !sp.closed {
        return Some(SubPath {
            anchors: smooth_open(&sp.anchors, &marked, tol),
            closed: false,
        });
    }
    match marked.iter().position(|m| !m) {
        // All of it in reach: refit the whole loop (keeping only real corners).
        None => {
            let mut pts = Vec::new();
            for (a, b) in sp.segments() {
                let c = sp.cubic(a, b);
                pts.extend(dense(&c).into_iter().skip(usize::from(!pts.is_empty())));
            }
            let pts = blur(&resample(&pts, tol / 4.0), SMOOTH_RADIUS * tol, true);
            let fitted = bezier::fit_outline(&pts, true, tol / 2.0, 150.0)?;
            (fitted.anchors.len() <= n).then_some(fitted)
        }
        // Start at an anchor out of reach, open the loop there, smooth, close.
        Some(k) => {
            let mut open = sp.anchors.clone();
            open.rotate_left(k);
            let mut m = marked.clone();
            m.rotate_left(k);
            open.push(open[0]);
            m.push(false);
            let mut res = smooth_open(&open, &m, tol);
            let end = res.pop()?;
            if let Some(r0) = res.first_mut() {
                r0.h_in = end.h_in;
            }
            Some(SubPath {
                anchors: res,
                closed: true,
            })
        }
    }
}

/// The Smooth tool: `path` with the anchors where `near` holds refitted into
/// fewer, smoother curves within `tol` (design units). `None` when nothing
/// there changes.
pub fn smooth_near(path: &BezPath, near: &dyn Fn(P) -> bool, tol: f64) -> Option<BezPath> {
    let mut changed = false;
    let out: BezPath = path
        .iter()
        .map(|sp| match smooth_subpath(sp, near, tol) {
            Some(s) if s != *sp && s.anchors.len() >= 2 => {
                changed = true;
                s
            }
            _ => sp.clone(),
        })
        .collect();
    changed.then_some(out)
}

/// A point of segment `c` at `t` (a straight segment evenly, so its parts stay straight).
fn eval(c: &[P; 4], straight: bool, t: f64) -> P {
    if straight {
        c[0] + (c[3] - c[0]) * t
    } else {
        bezier::cubic_at(c, t)
    }
}

/// Segment `c` between `t0` and `t1`.
fn part(c: &[P; 4], straight: bool, t0: f64, t1: f64) -> [P; 4] {
    if straight {
        let (a, b) = (eval(c, true, t0), eval(c, true, t1));
        return [a, a, b, b];
    }
    let (_, right) = bezier::split_cubic(c, t0);
    if t0 >= 1.0 - 1e-12 {
        return [c[3]; 4];
    }
    let (left, _) = bezier::split_cubic(&right, (t1 - t0) / (1.0 - t0));
    left
}

/// The stretches of segment `c` where `near` doesn't hold, as `t` ranges.
fn kept_ranges(c: &[P; 4], straight: bool, near: &dyn Fn(P) -> bool) -> Vec<(f64, f64)> {
    const N: usize = 48;
    let gone = |t: f64| near(eval(c, straight, t));
    // The boundary between `a` (state sa) and `b`, found by halving.
    let edge = |mut a: f64, mut b: f64, sa: bool| {
        for _ in 0..24 {
            let m = (a + b) / 2.0;
            if gone(m) == sa {
                a = m;
            } else {
                b = m;
            }
        }
        (a + b) / 2.0
    };
    let mut out = Vec::new();
    let mut start: Option<f64> = (!gone(0.0)).then_some(0.0);
    let mut prev = (0.0, gone(0.0));
    for k in 1..=N {
        let t = k as f64 / N as f64;
        let g = gone(t);
        if g != prev.1 {
            let e = edge(prev.0, t, prev.1);
            if g {
                if let Some(s) = start.take() {
                    out.push((s, e));
                }
            } else {
                start = Some(e);
            }
        }
        prev = (t, g);
    }
    if let Some(s) = start {
        out.push((s, 1.0));
    }
    out
}

/// A run of cubics as an open subpath.
fn from_cubics(cs: &[[P; 4]]) -> SubPath {
    let mut anchors = vec![Anchor {
        p: cs[0][0],
        h_in: cs[0][0],
        h_out: cs[0][1],
    }];
    for w in cs.windows(2) {
        anchors.push(Anchor {
            p: w[1][0],
            h_in: w[0][2],
            h_out: w[1][1],
        });
    }
    let l = cs[cs.len() - 1];
    anchors.push(Anchor {
        p: l[3],
        h_in: l[2],
        h_out: l[3],
    });
    SubPath {
        anchors,
        closed: false,
    }
}

/// The Path Eraser: `path` with the parts where `near` holds cut away,
/// what's left as open subpaths (bits under `min_len` dropped). `None` when
/// nothing is under it.
pub fn erase_near(path: &BezPath, near: &dyn Fn(P) -> bool, min_len: f64) -> Option<BezPath> {
    let mut changed = false;
    let mut out = BezPath::new();
    for sp in path {
        let segs: Vec<[P; 4]> = sp
            .segments()
            .into_iter()
            .map(|(a, b)| sp.cubic(a, b))
            .collect();
        // Runs of kept cubics; whether each run reaches the subpath's start / end.
        let mut runs: Vec<(Vec<[P; 4]>, bool, bool)> = Vec::new();
        let mut cur: Option<(Vec<[P; 4]>, bool)> = None;
        let mut touched = false;
        for (k, c) in segs.iter().enumerate() {
            let straight = c[1] == c[0] && c[2] == c[3];
            let kept = kept_ranges(c, straight, near);
            if !(kept.len() == 1 && kept[0] == (0.0, 1.0)) {
                touched = true;
            }
            if kept.is_empty()
                && let Some((run, at_start)) = cur.take()
            {
                runs.push((run, at_start, false));
            }
            for (t0, t1) in kept {
                let piece = part(c, straight, t0, t1);
                match &mut cur {
                    Some((run, _)) if t0 == 0.0 => run.push(piece),
                    _ => {
                        if let Some((run, at_start)) = cur.take() {
                            runs.push((run, at_start, false));
                        }
                        cur = Some((vec![piece], k == 0 && t0 == 0.0));
                    }
                }
                if t1 < 1.0
                    && let Some((run, at_start)) = cur.take()
                {
                    runs.push((run, at_start, false));
                }
            }
        }
        if let Some((run, at_start)) = cur.take() {
            runs.push((run, at_start, true));
        }
        if !touched {
            out.push(sp.clone());
            continue;
        }
        changed = true;
        // A closed loop cut once or more: its last run carries on into its first.
        if sp.closed
            && runs.len() >= 2
            && runs.last().is_some_and(|r| r.2)
            && runs.first().is_some_and(|r| r.1)
        {
            let (first, _, _) = runs.remove(0);
            if let Some(last) = runs.last_mut() {
                last.0.extend(first);
            }
        }
        for (run, _, _) in runs {
            let len: f64 = run
                .iter()
                .map(|c| {
                    bezier::sample(c, 8)
                        .windows(2)
                        .map(|w| w[0].dist(w[1]))
                        .sum::<f64>()
                })
                .sum();
            if len >= min_len {
                out.push(from_cubics(&run));
            }
        }
    }
    changed.then_some(out)
}

/// A corner point with straight sides into and out of it.
fn straight_through(prev: &Anchor, a: &Anchor, next: &Anchor) -> bool {
    prev.h_out == prev.p && a.h_in == a.p && a.h_out == a.p && next.h_in == next.p
}

/// `a` on the straight line from `prev` to `next` (within `eps`), between them.
fn on_line(prev: P, a: P, next: P, eps: f64) -> bool {
    let d = next - prev;
    let l = d.len();
    if l < 1e-9 {
        return a.dist(prev) <= eps;
    }
    let u = d * (1.0 / l);
    let v = a - prev;
    let along = v.dot(u);
    let off = (v.x * u.y - v.y * u.x).abs();
    off <= eps && along > -eps && along < l + eps
}

/// Clean Up for one outline: points on top of each other become one, a
/// corner point in the middle of a straight line goes, and subpaths left
/// with nothing to them (under two points, or a closed one with no area) are
/// dropped. Returns the cleaned path and how many points went, or `None`
/// when there's nothing to clean.
pub fn clean_path(path: &BezPath, eps: f64) -> Option<(BezPath, usize)> {
    let mut removed = 0;
    let mut out = BezPath::new();
    for sp in path {
        let mut v: Vec<Anchor> = Vec::with_capacity(sp.anchors.len());
        for &a in &sp.anchors {
            match v.last_mut() {
                // Coincident: keep one point, with the handles either side.
                Some(l) if l.p.dist(a.p) <= eps => {
                    l.h_out = a.h_out + (l.p - a.p);
                    removed += 1;
                }
                _ => v.push(a),
            }
        }
        if sp.closed && v.len() > 1 && v[0].p.dist(v[v.len() - 1].p) <= eps {
            let l = v.pop().unwrap();
            v[0].h_in = l.h_in + (v[0].p - l.p);
            removed += 1;
        }
        // Points in the middle of straight lines.
        let mut k = 0;
        while v.len() > 2 && k < v.len() {
            let n = v.len();
            let inner = sp.closed || (k > 0 && k + 1 < n);
            if inner {
                let (prev, next) = (v[(k + n - 1) % n], v[(k + 1) % n]);
                if straight_through(&prev, &v[k], &next) && on_line(prev.p, v[k].p, next.p, eps) {
                    v.remove(k);
                    removed += 1;
                    continue;
                }
            }
            k += 1;
        }
        let empty = v.len() < 2
            || (sp.closed && v.len() == 2 && v.iter().all(|a| a.h_in == a.p && a.h_out == a.p));
        if empty {
            removed += v.len();
            continue;
        }
        out.push(SubPath {
            anchors: v,
            closed: sp.closed,
        });
    }
    (removed > 0).then_some((out, removed))
}

/// One end of an open subpath: (subpath, at its start).
pub type End = (usize, bool);

/// Join two open ends: `a` and `b` of `path` (Illustrator's Join, and
/// VectorCraft's Join tool). The two ends of one subpath close it; ends of
/// two subpaths become one subpath through the join. Ends closer than
/// `merge` become a single point; otherwise a straight segment joins them.
pub fn join_ends(path: &BezPath, a: End, b: End, merge: f64) -> Option<BezPath> {
    let (sa, sb) = (path.get(a.0)?, path.get(b.0)?);
    if sa.closed || sb.closed || a == b {
        return None;
    }
    let mut out = path.clone();
    if a.0 == b.0 {
        if a.1 == b.1 || sa.anchors.len() < 2 {
            return None;
        }
        let sp = &mut out[a.0];
        let n = sp.anchors.len();
        if n > 2 && sp.anchors[0].p.dist(sp.anchors[n - 1].p) <= merge {
            let l = sp.anchors.pop().unwrap();
            sp.anchors[0].h_in = l.h_in + (sp.anchors[0].p - l.p);
        }
        sp.closed = true;
        return Some(out);
    }
    // `a`'s subpath running into the join, then `b`'s running away from it.
    let first = if a.1 {
        bezier::reversed(sa)
    } else {
        sa.clone()
    };
    let second = if b.1 {
        sb.clone()
    } else {
        bezier::reversed(sb)
    };
    let mut anchors = first.anchors;
    let mut rest = second.anchors.into_iter();
    if let (Some(l), Some(f)) = (anchors.last_mut(), rest.as_slice().first())
        && l.p.dist(f.p) <= merge
    {
        l.h_out = f.h_out + (l.p - f.p);
        rest.next();
    }
    anchors.extend(rest);
    let (keep, gone) = (a.0.min(b.0), a.0.max(b.0));
    out[keep] = SubPath {
        anchors,
        closed: false,
    };
    out.remove(gone);
    Some(out)
}

/// The open ends of `path`, with where they are.
pub fn open_ends(path: &BezPath) -> Vec<(End, P)> {
    let mut v = Vec::new();
    for (s, sp) in path.iter().enumerate() {
        if sp.closed || sp.anchors.is_empty() {
            continue;
        }
        v.push(((s, true), sp.anchors[0].p));
        if sp.anchors.len() > 1 {
            v.push(((s, false), sp.anchors[sp.anchors.len() - 1].p));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> BezPath {
        let c = |x: f64, y: f64| Anchor::corner(P::new(x, y));
        vec![SubPath {
            anchors: vec![c(0.0, 0.0), c(100.0, 0.0), c(100.0, 100.0), c(0.0, 100.0)],
            closed: true,
        }]
    }

    fn open(pts: &[(f64, f64)]) -> SubPath {
        SubPath {
            anchors: pts
                .iter()
                .map(|&(x, y)| Anchor::corner(P::new(x, y)))
                .collect(),
            closed: false,
        }
    }

    #[test]
    fn clean_up_takes_out_doubled_and_straight_line_points() {
        let mut sq = square();
        // A doubled corner, and a point in the middle of the bottom side.
        sq[0].anchors.insert(1, Anchor::corner(P::new(50.0, 0.0)));
        sq[0].anchors.insert(3, Anchor::corner(P::new(100.0, 0.01)));
        // A stray single point, and a flat closed sliver.
        sq.push(open(&[(5.0, 5.0)]));
        sq.push(SubPath {
            closed: true,
            ..open(&[(0.0, 0.0), (10.0, 0.0)])
        });
        let (out, removed) = clean_path(&sq, 0.05).expect("cleaned");
        assert_eq!(out, square());
        assert_eq!(removed, 5);
        assert!(clean_path(&square(), 0.05).is_none());
    }

    #[test]
    fn join_connects_closes_and_merges() {
        let path = vec![
            open(&[(0.0, 0.0), (10.0, 0.0)]),
            open(&[(20.0, 0.0), (30.0, 0.0)]),
        ];
        // End of the first to the start of the second: one line, four points.
        let j = join_ends(&path, (0, false), (1, true), 0.1).unwrap();
        assert_eq!(j.len(), 1);
        assert_eq!(j[0].anchors.len(), 4);
        // Start to start: the first runs backwards into the second.
        let j = join_ends(&path, (0, true), (1, true), 0.1).unwrap();
        assert_eq!(j[0].anchors[0].p, P::new(10.0, 0.0));
        assert_eq!(j[0].anchors[3].p, P::new(30.0, 0.0));
        // Touching ends merge into one point.
        let touching = vec![
            open(&[(0.0, 0.0), (10.0, 0.0)]),
            open(&[(10.0, 0.0), (10.0, 10.0)]),
        ];
        let j = join_ends(&touching, (0, false), (1, true), 0.1).unwrap();
        assert_eq!(j[0].anchors.len(), 3);
        // A subpath's own two ends: closed.
        let u = vec![open(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)])];
        let j = join_ends(&u, (0, true), (0, false), 0.1).unwrap();
        assert!(j[0].closed && j[0].anchors.len() == 4);
        assert_eq!(open_ends(&j).len(), 0);
    }

    #[test]
    fn levels_map_to_tolerances_as_vectorcrafts() {
        assert!((simplify_level_tolerance(50.0) - 1.0).abs() < 1e-12);
        assert!((simplify_level_tolerance(0.0) - 32.0).abs() < 1e-9);
        assert!((simplify_level_tolerance(100.0) - 1.0 / 32.0).abs() < 1e-12);
        assert_eq!(auto_simplify_level(400.0), 50.0);
        assert_eq!(auto_simplify_level(0.0), 100.0);
    }

    #[test]
    fn smoothing_a_wobbly_line_leaves_fewer_points() {
        // A hand-drawn wobble: 40 corner points along a gentle wave.
        let anchors: Vec<Anchor> = (0..40)
            .map(|k| {
                let x = k as f64 * 10.0;
                let y = (x / 60.0).sin() * 30.0 + if k % 2 == 0 { 0.6 } else { -0.6 };
                Anchor::corner(P::new(x, y))
            })
            .collect();
        let path = vec![SubPath {
            anchors,
            closed: false,
        }];
        let out = smooth_near(&path, &|_| true, 2.5).expect("smoothed");
        let n = out[0].anchors.len();
        assert!(n < 15, "{n} points");
        // The ends stay put.
        assert_eq!(out[0].anchors[0].p, path[0].anchors[0].p);
        assert_eq!(out[0].anchors[n - 1].p, path[0].anchors[39].p);
        // Only near the drag: the far half keeps its points.
        let half = smooth_near(&path, &|p| p.x < 150.0, 2.5).unwrap();
        assert!(half[0].anchors.len() > n && half[0].anchors.len() < 40);
        assert!(half[0].anchors.iter().any(|a| a.p == path[0].anchors[30].p));
    }

    #[test]
    fn smoothing_part_of_a_zigzag_stays_within_the_fidelity() {
        let anchors: Vec<Anchor> = (0..41)
            .map(|k| {
                let x = 100.0 + 20.0 * k as f64;
                let y = 500.0 + 40.0 * (x / 60.0).sin() + if k % 2 == 0 { 6.0 } else { -6.0 };
                Anchor::corner(P::new(x, y))
            })
            .collect();
        let orig: Vec<P> = anchors.iter().map(|a| a.p).collect();
        let path = vec![SubPath {
            anchors,
            closed: false,
        }];
        // No hooks or bulges: every bit of the result near the original.
        for (lo, w) in [
            (200.0, 60.0),
            (300.0, 120.0),
            (420.0, 120.0),
            (100.0, 900.0),
        ] {
            let out = smooth_near(&path, &|p| p.x > lo && p.x < lo + w, 8.0).unwrap();
            assert!(out[0].anchors.len() < 41);
            for sp in &out {
                for (a, b) in sp.segments() {
                    for q in bezier::sample(&sp.cubic(a, b), 32) {
                        let d = orig
                            .windows(2)
                            .map(|s| {
                                let d = s[1] - s[0];
                                let t = ((q - s[0]).dot(d) / d.dot(d)).clamp(0.0, 1.0);
                                (s[0] + d * t).dist(q)
                            })
                            .fold(f64::INFINITY, f64::min);
                        assert!(d <= 8.5, "{lo} {w}: {d:.1} off");
                    }
                }
            }
        }
    }

    #[test]
    fn erasing_across_a_side_of_a_square_opens_it_there() {
        let near = |p: P| (p - P::new(50.0, 0.0)).len() <= 10.0;
        let out = erase_near(&square(), &near, 1.0).expect("erased");
        assert_eq!(out.len(), 1, "{out:?}");
        let sp = &out[0];
        assert!(!sp.closed);
        // From the gap's right edge round to its left edge.
        let (a, b) = (sp.anchors[0].p, sp.anchors[sp.anchors.len() - 1].p);
        assert!((a - P::new(60.0, 0.0)).len() < 0.01, "{a:?}");
        assert!((b - P::new(40.0, 0.0)).len() < 0.01, "{b:?}");
        assert_eq!(sp.anchors.len(), 6);
        // Nothing under it: no change.
        assert!(erase_near(&square(), &|p| p.x > 500.0, 1.0).is_none());
        // Two cuts: two pieces.
        let two =
            |p: P| (p - P::new(50.0, 0.0)).len() <= 10.0 || (p - P::new(50.0, 100.0)).len() <= 10.0;
        assert_eq!(erase_near(&square(), &two, 1.0).unwrap().len(), 2);
    }
}
