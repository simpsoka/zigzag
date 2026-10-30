//! Satin columns between two rails.
//!
//! Every satin, however it was drawn, becomes a pair of rails sewn with zigzag
//! stitches between them:
//! - a single stroke: rails are its centerline offset by ± width/2;
//! - an Ink/Stitch-style satin column: a path with two rails (and optional
//!   rungs that keep the rails in step);
//! - a filled shape ("auto satin"): a ring with a hole uses outer and inner
//!   outlines as rails; a single ring is split at its two tips.
//!
//! The column is sewn as one continuous path: underlay passes (center walk,
//! contour, zigzag) followed by the top satin, with pull compensation and
//! short stitches on the inside of tight curves.

use crate::geom::{FillRule, P, Polyline, cross, point_inside, polyline_length};
use crate::params::underlay;

#[derive(Clone, Debug)]
pub struct Rails {
    pub a: Polyline,
    pub b: Polyline,
    /// Rails are closed loops (e.g. a ring); first and last points coincide.
    pub closed: bool,
    /// Rung positions as (arc length on a, arc length on b), increasing.
    pub sync: Vec<(f64, f64)>,
}

#[derive(Clone, Debug)]
pub struct SatinParams {
    /// Distance between zigzag peaks along the column, units.
    pub spacing: f64,
    /// Extra width per side, units.
    pub pull: f64,
    /// Underlay passes (`params::underlay` flags).
    pub underlay: u8,
    /// Underlay stitch length, units.
    pub run_len: f64,
    /// Shorten alternate stitches where needle points on a rail crowd together.
    pub short_stitches: bool,
    /// Split top stitches longer than this, units (auto-split for wide columns).
    pub split: Option<f64>,
    /// How the top stitches cross the column.
    pub style: SatinStyle,
}

/// The pattern a satin column's top stitches make (Ink/Stitch's satin,
/// S-stitch and zigzag satin).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SatinStyle {
    /// Plain satin: each stitch straight across.
    #[default]
    Plain,
    /// S-stitch: every stitch across is split into a shallow S (three
    /// stitches, the middle one offset), a curvy, less shiny texture.
    SStitch,
    /// Zigzag: a lighter, open column, as a zigzag stitch rather than a
    /// solid satin: the spacing is multiplied up and nothing is split.
    Zigzag,
}

impl SatinStyle {
    pub fn name(self) -> &'static str {
        match self {
            SatinStyle::Plain => "satin",
            SatinStyle::SStitch => "s",
            SatinStyle::Zigzag => "zigzag",
        }
    }
    pub fn parse(s: &str) -> Option<SatinStyle> {
        match s {
            "satin" | "plain" => Some(SatinStyle::Plain),
            "s" | "sstitch" | "s-stitch" => Some(SatinStyle::SStitch),
            "zigzag" | "zz" => Some(SatinStyle::Zigzag),
            _ => None,
        }
    }
}

/// Rail points closer than this (units) count as crowded (Ink/Stitch: 0.25 mm).
const SHORT_DIST: f64 = 2.5;
/// How far crowded alternate stitches are pulled in, as a fraction of width.
const SHORT_INSET: f64 = 0.15;
/// Underlay sits this far inside the rails (units), at most a quarter of the width.
const UNDERLAY_INSET: f64 = 4.0;
/// Zigzag underlay peak spacing, units.
const ZIGZAG_UNDERLAY_SPACING: f64 = 30.0;

struct Arc<'a> {
    pts: &'a [P],
    cum: Vec<f64>,
}

impl<'a> Arc<'a> {
    fn new(pts: &'a [P]) -> Self {
        let mut cum = Vec::with_capacity(pts.len());
        let mut s = 0.0;
        for (i, p) in pts.iter().enumerate() {
            if i > 0 {
                s += pts[i - 1].dist(*p);
            }
            cum.push(s);
        }
        Arc { pts, cum }
    }
    fn len(&self) -> f64 {
        *self.cum.last().unwrap_or(&0.0)
    }
    fn at(&self, s: f64) -> P {
        let n = self.pts.len();
        if n == 1 {
            return self.pts[0];
        }
        let s = s.clamp(0.0, self.len());
        let i = self.cum.partition_point(|&c| c <= s).clamp(1, n - 1);
        let seg = self.cum[i] - self.cum[i - 1];
        let t = if seg > 1e-12 {
            (s - self.cum[i - 1]) / seg
        } else {
            0.0
        };
        self.pts[i - 1].lerp(self.pts[i], t)
    }
}

/// Matched points on both rails, evenly about `step` apart along the column
/// center. Between sync points (rungs, or the offsets of a centerline) each
/// rail is followed in proportion, so stitches stay square to the column
/// even where one rail is much longer than the other (round a bend).
pub fn pairs(r: &Rails, step: f64) -> Vec<(P, P)> {
    if r.a.len() < 2 || r.b.len() < 2 {
        return Vec::new();
    }
    let (a, b) = (Arc::new(&r.a), Arc::new(&r.b));
    let mut syncs = vec![(0.0, 0.0)];
    for &(sa, sb) in &r.sync {
        let (pa, pb) = *syncs.last().unwrap();
        if sa > pa && sb > pb && sa < a.len() && sb < b.len() {
            syncs.push((sa, sb));
        }
    }
    syncs.push((a.len(), b.len()));
    // The column's center length, window by window.
    let centers: Vec<f64> = syncs
        .windows(2)
        .map(|w| ((w[1].0 - w[0].0) + (w[1].1 - w[0].1)) * 0.5)
        .collect();
    let total: f64 = centers.iter().sum();
    let n = (total / step).ceil().max(1.0) as usize;
    let step = total / n as f64;
    let mut out = Vec::with_capacity(n + 1);
    let mut s = 0.0; // distance into the current window
    for (w, &center) in syncs.windows(2).zip(&centers) {
        let ((a0, b0), (a1, b1)) = (w[0], w[1]);
        while s < center - 1e-9 && out.len() < n {
            let t = if center > 1e-12 { s / center } else { 0.0 };
            out.push((a.at(a0 + t * (a1 - a0)), b.at(b0 + t * (b1 - b0))));
            s += step;
        }
        s -= center;
    }
    out.push((a.at(a.len()), b.at(b.len())));
    out
}

/// Sew a column: underlay passes, then the top satin, as one continuous path.
pub fn column(r: &Rails, p: &SatinParams) -> Polyline {
    let mut out: Polyline = Vec::new();
    // Where the needle is: start (false) or end (true) of the column.
    let mut at_end = false;
    let inset = |a: P, b: P| {
        let w = a.dist(b);
        let d = UNDERLAY_INSET.min(w * 0.25);
        (a + (b - a).norm() * d, b + (a - b).norm() * d)
    };

    if p.underlay & underlay::CENTER != 0 {
        let mut pass: Polyline = pairs(r, p.run_len)
            .into_iter()
            .map(|(a, b)| a.lerp(b, 0.5))
            .collect();
        if at_end {
            pass.reverse();
        }
        out.extend(pass);
        at_end = !at_end;
    }
    if p.underlay & underlay::CONTOUR != 0 {
        let ps: Vec<(P, P)> = pairs(r, p.run_len)
            .into_iter()
            .map(|(a, b)| inset(a, b))
            .collect();
        let side_a: Polyline = ps.iter().map(|q| q.0).collect();
        let side_b: Polyline = ps.iter().map(|q| q.1).collect();
        // Out along one side and back along the other: ends where it started.
        if at_end {
            out.extend(side_a.iter().rev());
            out.extend(side_b.iter());
        } else {
            out.extend(side_a.iter());
            out.extend(side_b.iter().rev());
        }
    }
    if p.underlay & underlay::ZIGZAG != 0 {
        let mut pass: Polyline = Vec::new();
        for (i, (a, b)) in pairs(r, ZIGZAG_UNDERLAY_SPACING)
            .into_iter()
            .map(|(a, b)| inset(a, b))
            .enumerate()
        {
            pass.push(if i % 2 == 0 { a } else { b });
        }
        if at_end {
            pass.reverse();
        }
        out.extend(pass);
        at_end = !at_end;
    }

    // Top stitching.
    let spacing = match p.style {
        SatinStyle::Zigzag => p.spacing * 3.0,
        _ => p.spacing,
    };
    let ps = pairs(r, spacing);
    let mut top: Polyline = Vec::with_capacity(ps.len() * 2);
    for (k, &(a, b)) in ps.iter().enumerate() {
        let dir = (a - b).norm();
        let (mut a2, mut b2) = (a + dir * p.pull, b - dir * p.pull);
        if p.short_stitches && k > 0 && k % 2 == 1 {
            let (pa, pb) = ps[k - 1];
            let w = a2.dist(b2);
            if a.dist(pa) < SHORT_DIST {
                a2 = a2 + (b2 - a2).norm() * (w * SHORT_INSET);
            }
            if b.dist(pb) < SHORT_DIST {
                b2 = b2 + (a2 - b2).norm() * (w * SHORT_INSET);
            }
        }
        top.push(a2);
        top.push(b2);
    }
    match p.style {
        SatinStyle::Plain => {
            if let Some(max) = p.split.filter(|m| *m > 0.0) {
                top = split_long(&top, max);
            }
        }
        SatinStyle::SStitch => top = s_stitch(&top),
        SatinStyle::Zigzag => {}
    }
    if at_end {
        top.reverse();
    }
    out.extend(top);
    out
}

/// S-stitch: each stitch across the column becomes three, the middle
/// point pushed sideways (alternating sides), so the column reads as a
/// run of shallow S curves.
fn s_stitch(pts: &[P]) -> Polyline {
    let mut out = Vec::with_capacity(pts.len() * 3);
    for (k, w) in pts.windows(2).enumerate() {
        if k == 0 {
            out.push(w[0]);
        }
        let d = w[1] - w[0];
        let len = d.len();
        if len > 6.0 {
            let n = d.norm();
            let side = P::new(-n.y, n.x) * (len * 0.18).min(8.0);
            let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
            out.push(w[0].lerp(w[1], 0.3) + side * sign);
            out.push(w[0].lerp(w[1], 0.7) - side * sign);
        }
        out.push(w[1]);
    }
    if pts.len() == 1 {
        out.push(pts[0]);
    }
    out
}

/// Auto-split: break stitches longer than `max` into shorter ones, staggering
/// the split points from stitch to stitch so they don't line up into a groove.
fn split_long(pts: &[P], max: f64) -> Polyline {
    let mut out = Vec::with_capacity(pts.len() * 2);
    for (k, w) in pts.windows(2).enumerate() {
        if k == 0 {
            out.push(w[0]);
        }
        let len = w[0].dist(w[1]);
        let n = (len / max).ceil() as usize;
        if n > 1 {
            let shift = [0.0, 0.33, -0.33][k % 3] / n as f64;
            for j in 1..n {
                out.push(w[0].lerp(w[1], j as f64 / n as f64 + shift));
            }
        }
        out.push(w[1]);
    }
    if pts.len() == 1 {
        out.push(pts[0]);
    }
    out
}

/// The strip of a column between fractions `t0` and `t1` of its width
/// (rail a at 0, rail b at 1), as rails of its own: the rungs are kept,
/// so the strip's zigzags line up with the whole column's.
pub fn strip(r: &Rails, t0: f64, t1: f64) -> Rails {
    // Matched points along both rails, finely.
    let ps = pairs(r, 5.0);
    let a: Polyline = ps.iter().map(|(a, b)| a.lerp(*b, t0)).collect();
    let b: Polyline = ps.iter().map(|(a, b)| a.lerp(*b, t1)).collect();
    Rails {
        a,
        b,
        closed: r.closed,
        sync: Vec::new(),
    }
}

/// Rails for a satin of `width` along a single centerline.
pub fn rails_from_centerline(line: &[P], closed: bool, width: f64) -> Rails {
    let mut base: Polyline = line.to_vec();
    if closed && base.len() > 2 {
        base.push(base[0]);
    }
    // Resample finely so offset rails follow curves smoothly.
    let c = crate::geom::resample_keep_corners(&base, 5.0);
    let n = c.len();
    let half = width * 0.5;
    let tangent = |i: usize| -> P {
        let prev = if i > 0 {
            c[i] - c[i - 1]
        } else if closed && n > 2 {
            c[0] - c[n - 2]
        } else {
            c[1] - c[0]
        };
        let next = if i + 1 < n {
            c[i + 1] - c[i]
        } else if closed && n > 2 {
            c[1] - c[0]
        } else {
            c[n - 1] - c[n - 2]
        };
        let (tp, tn) = (prev.norm(), next.norm());
        let avg = (tp + tn).norm();
        if avg.len() < 1e-9 { tn } else { avg }
    };
    let mut a: Polyline = Vec::with_capacity(n);
    let mut b: Polyline = Vec::with_capacity(n);
    // Each point's two offsets belong together: a sync at every one keeps the
    // stitches square to the centerline round bends, where the inner rail is
    // shorter than the outer.
    let mut sync = Vec::with_capacity(n);
    let (mut la, mut lb) = (0.0, 0.0);
    for i in 0..n {
        let t = tangent(i);
        let seg = if i + 1 < n {
            (c[i + 1] - c[i]).norm()
        } else {
            (c[i] - c[i.saturating_sub(1)]).norm()
        };
        let miter = (1.0 / t.dot(seg).abs().max(0.5)).min(2.0);
        let nrm = t.perp() * (half * miter);
        let (pa, pb) = (c[i] + nrm, c[i] - nrm);
        if let (Some(&qa), Some(&qb)) = (a.last(), b.last()) {
            la += pa.dist(qa);
            lb += pb.dist(qb);
            if i + 1 < n {
                sync.push((la, lb));
            }
        }
        a.push(pa);
        b.push(pb);
    }
    Rails { a, b, closed, sync }
}

/// Two-rail satin from a stroke path with several subpaths (Ink/Stitch style):
/// the two longest subpaths are rails, the rest are rungs. Returns `None` if
/// the subpaths don't look like a pair of rails.
pub fn rails_from_subpaths(lines: &[(Polyline, bool)]) -> Option<Rails> {
    if lines.len() < 2 {
        return None;
    }
    let mut idx: Vec<usize> = (0..lines.len()).collect();
    idx.sort_by(|&i, &j| polyline_length(&lines[j].0).total_cmp(&polyline_length(&lines[i].0)));
    let (ia, ib) = (idx[0], idx[1]);
    let (la, lb) = (polyline_length(&lines[ia].0), polyline_length(&lines[ib].0));
    if lb < la * 0.4 {
        return None;
    }
    let closed = lines[ia].1 && lines[ib].1;
    let close = |l: &Polyline| {
        let mut l = l.clone();
        if closed && l.len() > 2 {
            l.push(l[0]);
        }
        l
    };
    let a = close(&lines[ia].0);
    let mut b = close(&lines[ib].0);
    if !closed {
        let same = a[0].dist(b[0]) + a.last().unwrap().dist(*b.last().unwrap());
        let flipped = a[0].dist(*b.last().unwrap()) + a.last().unwrap().dist(b[0]);
        if flipped < same {
            b.reverse();
        }
    } else {
        align_loop(&a, &mut b);
    }
    // Rails must be close relative to their length to be one column.
    let mid_a = Arc::new(&a).at(la * 0.5);
    let mid_b = Arc::new(&b).at(lb * 0.5);
    if mid_a.dist(mid_b) > 150.0 {
        return None;
    }
    let mut sync: Vec<(f64, f64)> = idx[2..]
        .iter()
        .filter_map(|&k| {
            let rung = &lines[k].0;
            Some((first_hit(&a, rung)?, first_hit(&b, rung)?))
        })
        .collect();
    sync.sort_by(|x, y| x.0.total_cmp(&y.0));
    // Keep only rungs that advance along both rails.
    let mut kept: Vec<(f64, f64)> = Vec::new();
    for s in sync {
        if kept.last().is_none_or(|k| s.0 > k.0 && s.1 > k.1) {
            kept.push(s);
        }
    }
    Some(Rails {
        a,
        b,
        closed,
        sync: kept,
    })
}

/// Arc length along `rail` of its first intersection with `rung`.
fn first_hit(rail: &[P], rung: &[P]) -> Option<f64> {
    let mut s = 0.0;
    for w in rail.windows(2) {
        for r in rung.windows(2) {
            if let Some(t) = seg_intersect(w[0], w[1], r[0], r[1]) {
                return Some(s + t * w[0].dist(w[1]));
            }
        }
        s += w[0].dist(w[1]);
    }
    None
}

fn seg_intersect(p: P, p2: P, q: P, q2: P) -> Option<f64> {
    let (r, s) = (p2 - p, q2 - q);
    let d = cross(r, s);
    if d.abs() < 1e-12 {
        return None;
    }
    let t = cross(q - p, s) / d;
    let u = cross(q - p, r) / d;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some(t)
}

fn signed_area(r: &[P]) -> f64 {
    let n = r.len();
    (0..n).map(|i| cross(r[i], r[(i + 1) % n])).sum::<f64>() * 0.5
}

/// Make closed rail `b` run the same way round as `a`, starting nearest `a[0]`.
fn align_loop(a: &[P], b: &mut Polyline) {
    let open = |r: &[P]| -> Polyline {
        let mut v = r.to_vec();
        if v.len() > 1 && v[0].dist(*v.last().unwrap()) < 1e-9 {
            v.pop();
        }
        v
    };
    let a0 = open(a);
    let mut b0 = open(b);
    if signed_area(&a0).signum() != signed_area(&b0).signum() {
        b0.reverse();
    }
    let start = (0..b0.len())
        .min_by(|&i, &j| b0[i].dist(a0[0]).total_cmp(&b0[j].dist(a0[0])))
        .unwrap_or(0);
    b0.rotate_left(start);
    b0.push(b0[0]);
    *b = b0;
}

/// Why a filled shape couldn't become a satin column.
#[derive(Clone, Debug, PartialEq)]
pub enum AutoSatinError {
    TooManyHoles,
    TooComplex,
    TooWide(f64),
}

impl std::fmt::Display for AutoSatinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AutoSatinError::TooManyHoles => write!(
                f,
                "has more than one hole, so it can't be a single satin column"
            ),
            AutoSatinError::TooComplex => {
                write!(
                    f,
                    "branches or bends too much for an automatic satin; split it into simple strokes, or use Fill"
                )
            }
            AutoSatinError::TooWide(w) => write!(
                f,
                "is {:.1} mm wide in places; satin over ~10 mm snags. Use Fill",
                w / 10.0
            ),
        }
    }
}

/// Widest stitch allowed in an automatic satin, units.
const AUTO_MAX_WIDTH: f64 = 80.0;

/// Group rings into components: each outer ring with the rings directly inside it.
pub fn components(rings: &[Polyline], rule: FillRule) -> Vec<Vec<Polyline>> {
    let n = rings.len();
    let depth: Vec<usize> = (0..n)
        .map(|i| {
            (0..n)
                .filter(|&j| {
                    j != i
                        && rings[j].len() >= 3
                        && point_inside(&rings[j..=j], FillRule::NonZero, rings[i][0])
                })
                .count()
        })
        .collect();
    let mut comps: Vec<Vec<Polyline>> = Vec::new();
    for i in 0..n {
        if depth[i].is_multiple_of(2) {
            let mut comp = vec![rings[i].clone()];
            for j in 0..n {
                if depth[j] == depth[i] + 1
                    && point_inside(&rings[i..=i], FillRule::NonZero, rings[j][0])
                {
                    comp.push(rings[j].clone());
                }
            }
            comps.push(comp);
        }
    }
    let _ = rule;
    comps
}

/// Rails for one filled component (outer ring + holes).
pub fn auto_rails(comp: &[Polyline]) -> Result<Rails, AutoSatinError> {
    let rails = match comp.len() {
        1 => split_at_tips(&comp[0]).ok_or(AutoSatinError::TooComplex)?,
        2 => {
            let mut a = comp[0].clone();
            a.push(a[0]);
            let mut b = comp[1].clone();
            align_loop(&a, &mut b);
            Rails {
                a,
                b,
                closed: true,
                sync: Vec::new(),
            }
        }
        _ => return Err(AutoSatinError::TooManyHoles),
    };
    // Validate: stitches must stay inside the shape and not get too wide.
    let ps = pairs(&rails, 10.0);
    let mut bad = 0;
    let mut widest: f64 = 0.0;
    for &(a, b) in &ps {
        let w = a.dist(b);
        widest = widest.max(w);
        if w > 1.0 && !point_inside(comp, FillRule::EvenOdd, a.lerp(b, 0.5)) {
            bad += 1;
        }
    }
    if widest > AUTO_MAX_WIDTH {
        return Err(AutoSatinError::TooWide(widest));
    }
    if bad as f64 > ps.len() as f64 * 0.08 {
        return Err(AutoSatinError::TooComplex);
    }
    Ok(rails)
}

/// Resample a closed ring to roughly even spacing.
fn resample_ring(ring: &[P], step: f64) -> Polyline {
    let mut closed = ring.to_vec();
    closed.push(ring[0]);
    let mut out = crate::geom::resample_keep_corners(&closed, step);
    out.pop();
    out
}

/// Split a single ring at its two tips into rails running tip to tip.
fn split_at_tips(ring: &[P]) -> Option<Rails> {
    if ring.len() < 3 {
        return None;
    }
    let total = polyline_length(&[ring, &ring[..1]].concat());
    let r = resample_ring(ring, (total / 240.0).max(2.0));
    let n = r.len();
    if n < 6 {
        return None;
    }
    let mut cum = vec![0.0; n + 1];
    for i in 0..n {
        cum[i + 1] = cum[i] + r[i].dist(r[(i + 1) % n]);
    }
    let per = cum[n];
    // Candidate tips: far apart, splitting the outline into similar halves.
    let stride = (n / 120).max(1);
    let mut cands: Vec<(f64, usize, usize)> = Vec::new();
    for i in (0..n).step_by(stride) {
        for j in (i + 1..n).step_by(stride) {
            let arc = cum[j] - cum[i];
            let bal = arc.min(per - arc) / arc.max(per - arc);
            cands.push((r[i].dist(r[j]) * bal, i, j));
        }
    }
    let best = cands.iter().map(|c| c.0).fold(0.0, f64::max);
    if best <= 0.0 {
        return None;
    }
    // Among near-best candidates, prefer the one giving the narrowest stitches
    // (e.g. a bar splits at its short ends, not its corners).
    let rails_for = |i: usize, j: usize| -> Rails {
        let a: Polyline = (i..=j).map(|k| r[k]).collect();
        let b: Polyline = (0..=(n - j + i)).map(|k| r[(i + n - k) % n]).collect();
        Rails {
            a,
            b,
            closed: false,
            sync: Vec::new(),
        }
    };
    let mean_width = |rails: &Rails| -> f64 {
        let step = polyline_length(&rails.a).max(polyline_length(&rails.b)) / 24.0;
        let ps = pairs(rails, step.max(1.0));
        ps.iter().map(|(a, b)| a.dist(*b)).sum::<f64>() / ps.len().max(1) as f64
    };
    // Each candidate's width worked out once (not twice per comparison).
    cands
        .iter()
        .filter(|c| c.0 >= best * 0.85)
        .map(|&(_, i, j)| {
            let rails = rails_for(i, j);
            (mean_width(&rails), rails)
        })
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .map(|(_, rails)| rails)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(underlay: u8) -> SatinParams {
        SatinParams {
            spacing: 4.0,
            pull: 0.0,
            underlay,
            run_len: 25.0,
            short_stitches: true,
            split: None,
            style: Default::default(),
        }
    }

    #[test]
    fn centerline_satin_zigzags_across_width() {
        let r = rails_from_centerline(&[P::new(0.0, 0.0), P::new(100.0, 0.0)], false, 30.0);
        let s = column(&r, &params(0));
        assert!(s.len() > 40);
        for q in &s {
            assert!((q.y.abs() - 15.0).abs() < 1e-6, "{q:?}");
        }
        assert!(s.windows(2).all(|w| (w[0].y > 0.0) != (w[1].y > 0.0)));
    }

    #[test]
    fn underlay_is_sewn_first_and_path_is_continuous() {
        let r = rails_from_centerline(&[P::new(0.0, 0.0), P::new(200.0, 0.0)], false, 40.0);
        let all = underlay::CENTER | underlay::CONTOUR | underlay::ZIGZAG;
        let s = column(&r, &params(all));
        // Center walk first: points on the centerline.
        assert!(s[..5].iter().all(|q| q.y.abs() < 1e-6));
        // No jumps longer than the zigzag underlay diagonal.
        assert!(s.windows(2).all(|w| w[0].dist(w[1]) < 55.0));
        // Underlay stays inside the rails.
        let top_start = s.len() - pairs(&r, 4.0).len() * 2;
        assert!(s[..top_start].iter().all(|q| q.y.abs() <= 16.0 + 1e-6));
    }

    #[test]
    fn two_rails_with_reversed_second_rail() {
        let a = vec![P::new(0.0, 0.0), P::new(100.0, 0.0)];
        let b = vec![P::new(100.0, 20.0), P::new(0.0, 20.0)];
        let rung = vec![P::new(50.0, -5.0), P::new(50.0, 25.0)];
        let r = rails_from_subpaths(&[(a, false), (b, false), (rung, false)]).unwrap();
        assert_eq!(r.b[0], P::new(0.0, 20.0));
        assert_eq!(r.sync.len(), 1);
        assert!((r.sync[0].0 - 50.0).abs() < 1e-9 && (r.sync[0].1 - 50.0).abs() < 1e-9);
        for (pa, pb) in pairs(&r, 4.0) {
            assert!((pa.x - pb.x).abs() < 1e-6, "stitches stay perpendicular");
        }
    }

    #[test]
    fn short_stitches_pull_in_on_tight_inner_curve() {
        // Tight arc: inner rail radius 15, outer 45.
        let line: Polyline = (0..=40)
            .map(|i| {
                let t = i as f64 / 40.0 * std::f64::consts::PI;
                P::new(30.0 * t.cos(), 30.0 * t.sin())
            })
            .collect();
        let r = rails_from_centerline(&line, false, 30.0);
        let mut p = params(0);
        let with = column(&r, &p);
        p.short_stitches = false;
        let without = column(&r, &p);
        let inner_mean = |s: &Polyline| {
            let v: Vec<f64> = s.iter().map(|q| q.len()).filter(|&l| l < 30.0).collect();
            v.iter().sum::<f64>() / v.len() as f64
        };
        assert!(
            inner_mean(&with) > inner_mean(&without) + 1.5,
            "alternate inner stitches inset"
        );
        // The outer rail has room, so it's untouched.
        let outer_max = |s: &Polyline| s.iter().map(|q| q.len()).fold(0.0, f64::max);
        assert!((outer_max(&with) - outer_max(&without)).abs() < 1e-6);
    }

    #[test]
    fn auto_satin_splits_a_bar_at_its_short_ends() {
        let bar = vec![
            P::new(0.0, 0.0),
            P::new(200.0, 0.0),
            P::new(200.0, 30.0),
            P::new(0.0, 30.0),
        ];
        let r = auto_rails(&[bar]).unwrap();
        let ps = pairs(&r, 10.0);
        let mid = &ps[ps.len() / 2];
        assert!(
            (mid.0.dist(mid.1) - 30.0).abs() < 3.0,
            "stitches cross the bar's width: {mid:?}"
        );
    }

    #[test]
    fn auto_satin_ring_uses_inner_and_outer_outline() {
        let circle = |r: f64| -> Polyline {
            (0..64)
                .map(|i| {
                    let t = i as f64 / 64.0 * std::f64::consts::TAU;
                    P::new(r * t.cos(), r * t.sin())
                })
                .collect()
        };
        let comps = components(&[circle(60.0), circle(40.0)], FillRule::EvenOdd);
        assert_eq!(comps.len(), 1);
        let r = auto_rails(&comps[0]).unwrap();
        assert!(r.closed);
        for (a, b) in pairs(&r, 10.0) {
            assert!((a.dist(b) - 20.0).abs() < 2.0);
        }
    }

    #[test]
    fn auto_satin_refuses_wide_and_branched_shapes() {
        let square = vec![
            P::new(0.0, 0.0),
            P::new(300.0, 0.0),
            P::new(300.0, 300.0),
            P::new(0.0, 300.0),
        ];
        assert!(auto_rails(&[square]).is_err());
        // A plus sign: four arms can't be one column.
        let plus = vec![
            P::new(40.0, 0.0),
            P::new(60.0, 0.0),
            P::new(60.0, 40.0),
            P::new(100.0, 40.0),
            P::new(100.0, 60.0),
            P::new(60.0, 60.0),
            P::new(60.0, 100.0),
            P::new(40.0, 100.0),
            P::new(40.0, 60.0),
            P::new(0.0, 60.0),
            P::new(0.0, 40.0),
            P::new(40.0, 40.0),
        ];
        assert!(
            auto_rails(&[plus]).is_err(),
            "a 10 mm wide cross can't be one satin"
        );
    }
}
