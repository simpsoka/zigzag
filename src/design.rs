//! Turns imported shapes into an ordered, machine-independent stitch plan:
//! color blocks made of continuous runs, with trims between runs.
//!
//! Steps: (1) optionally knock out fill hidden under objects sewn later,
//! (2) choose the sewing order (document order, or cap order: center-out and
//! bottom-up), (3) generate each object's stitches, (4) chain them together,
//! sewing short gaps straight across and trimming long ones.

use crate::boolean;
use crate::geom::{Bounds, FillRule, P, Polyline, dedupe, segment_inside};
use crate::params::{Fabric, Kind, defaults, underlay};
use crate::stitch::fill::{FillParams, tatami};
use crate::stitch::satin::{self, Rails, SatinParams};
use crate::stitch::stroke::running;
use crate::svg::{Geometry, Shape};
use std::collections::HashMap;

/// A continuous sequence of stitches.
#[derive(Clone, Debug)]
pub struct Run {
    pub pts: Polyline,
    /// Cut the thread before moving to this run's first point.
    pub trim_before: bool,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub color: [u8; 3],
    pub runs: Vec<Run>,
}

/// Why the machine halts before a color block (see [`Design::block_start`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockStart {
    /// The thread differs from the block before (true for the first block).
    pub thread_change: bool,
    /// The input shape whose "stop after" asked for a pause here.
    pub stop_after: Option<usize>,
    /// A pause inside an appliqué: (input shape, 1: lay the fabric, 2: trim it).
    pub applique: Option<(usize, u8)>,
    /// What to do at the pause, when the object says (`note=`).
    pub note: Option<String>,
}

impl BlockStart {
    /// A pause someone has to act on, not just a change of thread.
    pub fn is_pause(&self) -> bool {
        self.stop_after.is_some() || self.applique.is_some()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Design {
    pub name: String,
    pub blocks: Vec<Block>,
    /// Stitches each input shape contributed (same order as the input).
    pub per_shape: Vec<usize>,
    /// Input shape indices in the order they're sewn.
    pub sew_order: Vec<usize>,
    /// Problems found while planning: (shape index, message).
    pub notes: Vec<(usize, String)>,
    /// Input shapes sewn in each color block (same order as `blocks`).
    pub block_shapes: Vec<Vec<usize>>,
    /// Stitches in the basting box (the first run of the first block), 0
    /// when the design has none. They belong to no object.
    pub basting_stitches: usize,
    /// Why each color block starts (same order as `blocks`; empty for a
    /// design that wasn't planned from shapes, such as an opened machine
    /// file). Read it with [`Design::block_start`].
    pub starts: Vec<BlockStart>,
    /// Input shapes left with nothing to fill: covered by what's sewn on
    /// top (so the checks needn't work the overlaps out again).
    pub covered: Vec<bool>,
}

#[derive(Clone, Debug)]
pub struct PlanOptions {
    /// Gaps up to this length (units) are sewn straight across instead of trimmed,
    /// when the straight line stays inside the shape being filled.
    pub travel_inside: f64,
    /// Gaps up to this length (units) between runs are always sewn across.
    pub max_jump_no_trim: f64,
    /// Shortest stitch kept, units. Must stay below fill row spacing, or row
    /// starts get dropped and the edges turn ragged.
    pub min_stitch: f64,
    pub fabric: Fabric,
    /// Don't fill areas that later objects will cover.
    pub remove_overlaps: bool,
    /// How far fills still extend under the covering object, units.
    pub overlap: f64,
    /// Cap order: center-out, bottom-up, fewest color changes.
    pub hat_order: bool,
    /// Gaps between objects up to this long (units) are sewn across with
    /// running stitches rather than cut (Ink/Stitch's "jump to stroke").
    pub connect: f64,
    /// When a straight line between two parts of a fill would leave it,
    /// travel stitches follow the inside of its outline instead, up to
    /// this far (units); further than that, the thread is cut.
    pub travel_route: f64,
    /// A basting box: one line of long running stitches round the whole
    /// design, this far outside it (units), sewn before anything else to
    /// hold the fabric to the stabilizer. `None`: no basting.
    pub basting: Option<f64>,
}

impl Default for PlanOptions {
    fn default() -> Self {
        PlanOptions {
            travel_inside: 60.0,
            max_jump_no_trim: 10.0,
            min_stitch: 1.0,
            fabric: *Fabric::standard(),
            remove_overlaps: true,
            overlap: 5.0,
            hat_order: false,
            connect: 0.0,
            travel_route: 500.0,
            basting: None,
        }
    }
}

/// Top thread a stitch takes beyond its own length, mm: down through the
/// fabric and back up. With stitches around 4 mm this comes to about 5 m
/// per 1,000 stitches, in the range thread makers quote (4.6 to 6 m).
pub const THREAD_PER_STITCH_MM: f64 = 1.0;

/// Bobbin thread as a share of the top thread: thread makers' figures put
/// it between a third and a half (about 2.3 m per 1,000 stitches).
pub const BOBBIN_SHARE: f64 = 0.45;

/// The top thread a run of stitches takes, mm: an estimate from its length
/// and its stitch count. Tension, fabric thickness and thread weight all
/// move the real figure, so treat it as "about".
pub fn thread_mm(pts: &[P]) -> f64 {
    let len: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    len / 10.0 + pts.len().saturating_sub(1) as f64 * THREAD_PER_STITCH_MM
}

/// A thread length for people: metres to one decimal ("12.4 m"), or
/// centimetres under a metre ("85 cm").
pub fn fmt_thread(mm: f64) -> String {
    if mm < 995.0 {
        format!("{:.0} cm", (mm / 10.0).max(0.0))
    } else {
        format!("{:.1} m", mm / 1000.0)
    }
}

/// A run plus an equivalent alternative that starts at the other end.
#[derive(Clone)]
struct Piece {
    pts: Polyline,
    alt: Option<Polyline>,
    /// Closed outline (single pass): may start at any vertex.
    ring: bool,
}

impl Piece {
    fn reversible(pts: Polyline) -> Piece {
        let mut alt = pts.clone();
        alt.reverse();
        Piece {
            pts,
            alt: Some(alt),
            ring: false,
        }
    }
    fn fixed(pts: Polyline) -> Piece {
        Piece {
            pts,
            alt: None,
            ring: false,
        }
    }
    /// A closed loop (first point repeated at the end).
    fn ring(pts: Polyline) -> Piece {
        let closed = pts.len() > 3 && pts[0].dist(*pts.last().unwrap()) < 1e-6;
        if !closed {
            return Piece::reversible(pts);
        }
        Piece {
            pts,
            alt: None,
            ring: true,
        }
    }
    /// Distance from `here` to where this piece would start, and that start.
    fn nearest_start(&self, here: P) -> (f64, usize) {
        if self.ring {
            let (i, d) = self.pts[..self.pts.len() - 1]
                .iter()
                .enumerate()
                .map(|(i, p)| (i, here.dist(*p)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            return (d, i);
        }
        (here.dist(self.pts[0]), 0)
    }
}

/// Rotate a closed loop to start (and end) at vertex `i`.
fn rotate_ring(pts: &[P], i: usize) -> Polyline {
    let n = pts.len() - 1;
    let mut out: Polyline = (0..=n).map(|k| pts[(i + k) % n]).collect();
    out[n] = pts[i];
    out
}

/// Runs produced by one shape, in groups whose order must be preserved
/// (e.g. underlay before top stitching). Within a group, pieces may be reordered.
#[derive(Clone)]
struct ShapeRuns {
    groups: Vec<Vec<Piece>>,
    /// For fills: the outline, used to keep travel stitches inside.
    area: Option<(Vec<Polyline>, FillRule)>,
    /// Group indices that start after a machine stop (appliqué steps).
    stops: Vec<usize>,
    /// Groups sewn in a thread other than the object's (tartan stripes,
    /// multicolor satin): (group index, color). Each starts a color block.
    colors: Vec<(usize, [u8; 3])>,
}

impl ShapeRuns {
    fn new(groups: Vec<Vec<Piece>>, area: Option<(Vec<Polyline>, FillRule)>) -> Self {
        ShapeRuns {
            groups,
            area,
            stops: Vec::new(),
            colors: Vec::new(),
        }
    }
}

/// The point of `ring` nearest `p`: (distance, distance along the ring).
fn nearest_along(ring: &[P], p: P) -> (f64, f64) {
    let (mut best_d, mut best_along, mut along) = (f64::INFINITY, 0.0, 0.0);
    for w in ring.windows(2) {
        let d = w[0].dist(w[1]);
        let t = if d > 0.0 {
            ((p - w[0]).dot(w[1] - w[0]) / (d * d)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let dist = w[0].lerp(w[1], t).dist(p);
        if dist < best_d {
            best_d = dist;
            best_along = along + t * d;
        }
        along += d;
    }
    (best_d, best_along)
}

/// The points of closed `ring` from `a` to `b` (distances along it),
/// forward (`a` to `b`, wrapping) or backward.
fn ring_walk(ring: &[P], a: f64, b: f64, forward: bool) -> Polyline {
    let total = crate::geom::polyline_length(ring);
    let at = |s: f64| {
        let mut want = s.rem_euclid(total);
        for w in ring.windows(2) {
            let d = w[0].dist(w[1]);
            if want <= d {
                return if d > 0.0 {
                    w[0].lerp(w[1], want / d)
                } else {
                    w[0]
                };
            }
            want -= d;
        }
        *ring.last().unwrap()
    };
    let mut out = vec![at(a)];
    // The vertices strictly between a and b, in walking order.
    let mut along = 0.0;
    let mut verts: Vec<(f64, P)> = Vec::new();
    for w in ring.windows(2) {
        along += w[0].dist(w[1]);
        verts.push((along, w[1]));
    }
    let between = |s: f64| {
        if forward {
            if a <= b {
                s > a && s < b
            } else {
                s > a || s < b
            }
        } else if a >= b {
            s < a && s > b
        } else {
            s < a || s > b
        }
    };
    let mut mid: Vec<(f64, P)> = verts.into_iter().filter(|(s, _)| between(*s)).collect();
    if forward {
        // Wrapping: the vertices past a, then those before b.
        mid.sort_by(|x, y| {
            let kx = if x.0 > a { x.0 - a } else { x.0 + total - a };
            let ky = if y.0 > a { y.0 - a } else { y.0 + total - a };
            kx.total_cmp(&ky)
        });
    } else {
        mid.sort_by(|x, y| {
            let kx = if x.0 < a { a - x.0 } else { a + total - x.0 };
            let ky = if y.0 < a { a - y.0 } else { a + total - y.0 };
            kx.total_cmp(&ky)
        });
    }
    out.extend(mid.into_iter().map(|(_, p)| p));
    out.push(at(b));
    out
}

/// Travel stitches from `from` to `to` that stay inside the fill: along
/// the inside of its outline (0.5 mm in), the shorter way round, when
/// that's no longer than `max_len`. The points returned exclude `from`.
pub fn route_inside(
    rings: &[Polyline],
    rule: FillRule,
    from: P,
    to: P,
    max_len: f64,
) -> Option<Polyline> {
    route_along(&travel_ring(rings, rule), from, to, max_len)
}

/// The rings travel stitches follow: the fill's outline, 0.5 mm in.
pub fn travel_ring(rings: &[Polyline], rule: FillRule) -> Vec<Polyline> {
    let mut inset = boolean::offset(rings, rule, -5.0);
    for r in &mut inset {
        if r.len() >= 3 && r.first() != r.last() {
            r.push(r[0]);
        }
    }
    inset
}

/// [`route_inside`] along rings already worked out by [`travel_ring`].
fn route_along(inset: &[Polyline], from: P, to: P, max_len: f64) -> Option<Polyline> {
    // Both ends must be nearest the same inset ring.
    let pick = |p: P| {
        inset
            .iter()
            .enumerate()
            .filter(|(_, r)| r.len() >= 3)
            .map(|(i, r)| {
                let (d, along) = nearest_along(r, p);
                (i, d, along)
            })
            .min_by(|x, y| x.1.total_cmp(&y.1))
    };
    let (ia, _, a) = pick(from)?;
    let (ib, _, b) = pick(to)?;
    if ia != ib {
        return None;
    }
    let ring = &inset[ia];
    let total = crate::geom::polyline_length(ring);
    let fwd = (b - a).rem_euclid(total);
    let forward = fwd <= total - fwd;
    let mut path = ring_walk(ring, a, b, forward);
    let len =
        from.dist(path[0]) + crate::geom::polyline_length(&path) + path.last().unwrap().dist(to);
    if len > max_len {
        return None;
    }
    path.push(to);
    // Stitches every 3 mm along it.
    let mut out = Vec::new();
    let mut prev = from;
    for q in path {
        let d = prev.dist(q);
        let n = (d / 30.0).ceil().max(1.0) as usize;
        for k in 1..=n {
            out.push(prev.lerp(q, k as f64 / n as f64));
        }
        prev = q;
    }
    Some(out)
}

/// The outline a shape's start and end points are measured along: a
/// fill's biggest ring (closed), or a stroke's first line.
fn outline_of(g: &Geometry) -> Option<std::borrow::Cow<'_, Polyline>> {
    use std::borrow::Cow;
    match g {
        Geometry::Fill { rings, .. } => {
            let r = rings
                .iter()
                .filter(|r| r.len() >= 2)
                .max_by(|a, b| boolean::ring_area(a).total_cmp(&boolean::ring_area(b)))?;
            if r.first() == r.last() {
                Some(Cow::Borrowed(r))
            } else {
                let mut c = r.clone();
                c.push(r[0]);
                Some(Cow::Owned(c))
            }
        }
        Geometry::Stroke { lines, .. } => {
            lines.iter().find(|(l, _)| l.len() >= 2).map(|(l, closed)| {
                if *closed && l.first() != l.last() {
                    let mut c = l.clone();
                    c.push(l[0]);
                    Cow::Owned(c)
                } else {
                    Cow::Borrowed(l)
                }
            })
        }
    }
}

/// The point `t` (0–1) of the way along a shape's outline (where its start
/// or end point is; see `Params::start`).
pub fn outline_at(g: &Geometry, t: f64) -> Option<P> {
    let pts = outline_of(g)?;
    let total = crate::geom::polyline_length(&pts);
    if total <= 0.0 {
        return pts.first().copied();
    }
    let mut want = t.rem_euclid(1.0) * total;
    for w in pts.windows(2) {
        let d = w[0].dist(w[1]);
        if want <= d {
            return Some(if d > 0.0 {
                w[0].lerp(w[1], want / d)
            } else {
                w[0]
            });
        }
        want -= d;
    }
    pts.last().copied()
}

/// Where along a shape's outline (0–1) the point nearest `p` is: the
/// inverse of [`outline_at`].
pub fn outline_param(g: &Geometry, p: P) -> Option<f64> {
    let pts = outline_of(g)?;
    let total = crate::geom::polyline_length(&pts);
    if total <= 0.0 {
        return Some(0.0);
    }
    let (mut best, mut best_d, mut along) = (0.0, f64::INFINITY, 0.0);
    for w in pts.windows(2) {
        let d = w[0].dist(w[1]);
        let t = if d > 0.0 {
            ((p - w[0]).dot(w[1] - w[0]) / (d * d)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = w[0].lerp(w[1], t);
        let dist = q.dist(p);
        if dist < best_d {
            best_d = dist;
            best = along + t * d;
        }
        along += d;
    }
    Some((best / total).clamp(0.0, 0.9999))
}

fn mm(v: f64) -> f64 {
    v * 10.0
}

/// The satin rails a shape will be sewn with, or why a part couldn't be.
/// Parts that fail fall back to fill (returned as `Err` with the component).
pub fn satin_rails(shape: &Shape) -> Vec<Result<Rails, (Vec<Polyline>, String)>> {
    let p = &shape.params;
    match &shape.geometry {
        Geometry::Stroke { lines, width } => {
            let w = p.width.map(mm).unwrap_or(*width);
            if let Some(r) = satin::rails_from_subpaths(lines) {
                return vec![Ok(r)];
            }
            // (A subpath without length, a dot drawn as `M p L p`, has no
            // column to sew.)
            lines
                .iter()
                .map(|(l, c)| satin::rails_from_centerline(l, *c, w))
                .filter(|r| r.a.len() > 1)
                .map(Ok)
                .collect()
        }
        Geometry::Fill { rings, rule } => {
            if p.is_border() {
                let w = p.width.map(mm).unwrap_or(mm(defaults::OUTLINE_SATIN_WIDTH));
                return rings
                    .iter()
                    .map(|r| satin::rails_from_centerline(r, true, w))
                    .filter(|r| r.a.len() > 1)
                    .map(Ok)
                    .collect();
            }
            satin::components(rings, *rule)
                .into_iter()
                .map(|comp| satin::auto_rails(&comp).map_err(|e| (comp, e.to_string())))
                .collect()
        }
    }
}

fn mean_width(r: &Rails) -> f64 {
    let ps = satin::pairs(r, 10.0);
    ps.iter().map(|(a, b)| a.dist(*b)).sum::<f64>() / ps.len().max(1) as f64
}

fn fill_runs(
    rings: &[Polyline],
    rule: FillRule,
    shape: &Shape,
    opt: &PlanOptions,
) -> Vec<Vec<Piece>> {
    let p = &shape.params;
    let f = &opt.fabric;
    let angle = p.fill_angle().to_radians();
    let mut groups = Vec::new();
    if p.underlay_on() {
        if f.fill_edge_walk {
            let inset = boolean::offset(rings, rule, -5.0);
            let walk: Vec<Piece> = inset
                .iter()
                .filter(|r| r.len() >= 3 && boolean::ring_area(r) > 100.0)
                .map(|r| Piece::reversible(running(r, true, mm(2.0), 1)))
                .collect();
            if !walk.is_empty() {
                groups.push(walk);
            }
        }
        let under = FillParams {
            angle: angle + std::f64::consts::FRAC_PI_2,
            spacing: mm(f.fill_underlay_spacing),
            len: mm(3.0),
            inset: mm(0.4),
            pull: 0.0,
            stagger: 1,
            bottom_up: opt.hat_order,
        };
        let u: Vec<Piece> = tatami(rings, rule, &under)
            .into_iter()
            .map(Piece::reversible)
            .collect();
        if !u.is_empty() {
            groups.push(u);
        }
    }
    let top = FillParams {
        angle,
        spacing: mm(p.fill_spacing(f)),
        len: mm(p.fill_len(f)),
        inset: 0.0,
        pull: mm(p.fill_pull(f)),
        stagger: 4,
        bottom_up: opt.hat_order,
    };
    groups.push(
        tatami(rings, rule, &top)
            .into_iter()
            .map(Piece::reversible)
            .collect(),
    );
    groups
}

/// Knockdown: an open lattice, rows one way and then rows across them, with
/// no underlay and no pull compensation (it's there to hold the pile down,
/// not to cover).
fn knockdown_runs(
    rings: &[Polyline],
    rule: FillRule,
    shape: &Shape,
    opt: &PlanOptions,
) -> Vec<Vec<Piece>> {
    let p = &shape.params;
    let angle = p.knockdown_angle().to_radians();
    [angle, angle + std::f64::consts::FRAC_PI_2]
        .into_iter()
        .map(|a| {
            let rows = FillParams {
                angle: a,
                spacing: mm(p.knockdown_spacing()).max(5.0),
                len: mm(p.fill_len(&opt.fabric)),
                inset: 0.0,
                pull: 0.0,
                stagger: 1,
                bottom_up: opt.hat_order,
            };
            tatami(rings, rule, &rows)
                .into_iter()
                .map(Piece::reversible)
                .collect::<Vec<Piece>>()
        })
        .filter(|g| !g.is_empty())
        .collect()
}

fn satin_pieces(
    shape: &Shape,
    opt: &PlanOptions,
    notes: &mut Vec<String>,
) -> (Vec<Piece>, Vec<Vec<Piece>>) {
    satin_pieces_strip(shape, opt, notes, 0.0, 1.0)
}

/// [`satin_pieces`] for the strip of the column between fractions `t0`
/// and `t1` of its width (rail a at 0, rail b at 1): a multicolor satin
/// is several of these.
fn satin_pieces_strip(
    shape: &Shape,
    opt: &PlanOptions,
    notes: &mut Vec<String>,
    t0: f64,
    t1: f64,
) -> (Vec<Piece>, Vec<Vec<Piece>>) {
    let p = &shape.params;
    let f = &opt.fabric;
    let mut pieces = Vec::new();
    let mut fallback = Vec::new();
    let whole = t0 <= 0.0 && t1 >= 1.0;
    for r in satin_rails(shape) {
        match r {
            Ok(rails) => {
                let rails = if whole {
                    rails
                } else {
                    satin::strip(&rails, t0, t1)
                };
                let w = mean_width(&rails);
                let sp = SatinParams {
                    spacing: mm(p.satin_spacing_for(f, w / 10.0)),
                    pull: mm(p.satin_pull(f)),
                    underlay: p.satin_underlay_for(w / 10.0),
                    run_len: mm(2.5),
                    short_stitches: p.short_stitches_on(),
                    split: p.satin_split().map(mm),
                    style: p.satin_style(),
                };
                let fwd = satin::column(&rails, &sp);
                let alt = (!rails.closed).then(|| {
                    let rev = Rails {
                        a: rails.a.iter().rev().copied().collect(),
                        b: rails.b.iter().rev().copied().collect(),
                        closed: false,
                        sync: {
                            let (la, lb) = (
                                crate::geom::polyline_length(&rails.a),
                                crate::geom::polyline_length(&rails.b),
                            );
                            rails
                                .sync
                                .iter()
                                .rev()
                                .map(|&(a, b)| (la - a, lb - b))
                                .collect()
                        },
                    };
                    satin::column(&rev, &sp)
                });
                pieces.push(Piece {
                    pts: fwd,
                    alt,
                    ring: false,
                });
            }
            Err((comp, why)) => {
                notes.push(format!("Part of this shape {why}; it's filled instead."));
                fallback.extend(fill_runs(&comp, FillRule::EvenOdd, shape, opt));
            }
        }
    }
    (pieces, fallback)
}

/// Concentric rows following the outline, outside in.
fn contour_runs(
    rings: &[Polyline],
    rule: FillRule,
    shape: &Shape,
    opt: &PlanOptions,
) -> Vec<Vec<Piece>> {
    let p = &shape.params;
    let sp = mm(p.contour_spacing()).max(2.0);
    let len = mm(p.run_len(&opt.fabric));
    let mut groups = Vec::new();
    let mut d = sp * 0.5;
    for _ in 0..1000 {
        let level: Vec<Piece> = boolean::offset(rings, rule, -d)
            .iter()
            .filter(|r| r.len() >= 3 && boolean::ring_area(r) > sp * sp)
            .map(|r| Piece::ring(running(r, true, len, 1)))
            .collect();
        if level.is_empty() {
            break;
        }
        groups.push(level);
        d += sp;
    }
    groups
}

/// One spiral out from the shape's center (its start point, if set), the
/// turns `spacing` apart, clipped to the shape: Ink/Stitch's circular
/// fill. The pieces outside the shape are dropped; the planner joins what's
/// left inside it.
fn spiral_runs(
    rings: &[Polyline],
    rule: FillRule,
    shape: &Shape,
    opt: &PlanOptions,
) -> Vec<Vec<Piece>> {
    let p = &shape.params;
    let sp = mm(p.contour_spacing()).max(2.0);
    let len = mm(p.run_len(&opt.fabric)).max(5.0);
    let b = geometry_bounds(&shape.geometry);
    let center = p
        .start
        .and_then(|t| outline_at(&shape.geometry, t))
        .unwrap_or_else(|| {
            // The centroid of the biggest ring.
            rings
                .iter()
                .max_by(|a, c| boolean::ring_area(a).total_cmp(&boolean::ring_area(c)))
                .map(|r| {
                    let n = r.len().max(1) as f64;
                    r.iter().fold(P::default(), |acc, q| acc + *q) * (1.0 / n)
                })
                .unwrap_or(b.center())
        });
    let reach = [
        b.min,
        P::new(b.max.x, b.min.y),
        b.max,
        P::new(b.min.x, b.max.y),
    ]
    .iter()
    .map(|c| c.dist(center))
    .fold(0.0, f64::max)
        + sp;
    // Archimedean spiral, points about `len` apart along it.
    let mut pts: Polyline = vec![center];
    let mut theta: f64 = 0.0;
    while theta * sp / std::f64::consts::TAU <= reach {
        let r = theta * sp / std::f64::consts::TAU;
        let step = (len / r.max(len)).min(std::f64::consts::FRAC_PI_3);
        theta += step;
        let r = theta * sp / std::f64::consts::TAU;
        pts.push(center + P::new(r * theta.cos(), r * theta.sin()));
    }
    // Keep the parts inside the shape.
    let mut pieces = Vec::new();
    let mut cur: Polyline = Vec::new();
    for w in pts.windows(2) {
        if crate::geom::segment_inside(rings, rule, w[0], w[1], 2.0)
            && crate::geom::point_inside(rings, rule, w[0])
            && crate::geom::point_inside(rings, rule, w[1])
        {
            if cur.is_empty() {
                cur.push(w[0]);
            }
            cur.push(w[1]);
        } else if cur.len() >= 2 {
            pieces.push(Piece::reversible(std::mem::take(&mut cur)));
        } else {
            cur.clear();
        }
    }
    if cur.len() >= 2 {
        pieces.push(Piece::reversible(cur));
    }
    if pieces.is_empty() {
        return Vec::new();
    }
    vec![pieces]
}

/// Blanket (E-stitch): a running line with prongs of `width` every `spacing`,
/// on the `side` (+1 left of travel, -1 right).
pub fn blanket(line: &[P], closed: bool, spacing: f64, width: f64, side: f64) -> Polyline {
    let mut base: Polyline = line.to_vec();
    if closed && base.len() > 2 {
        base.push(base[0]);
    }
    let samples = crate::geom::sample_by_arclength(&base, spacing);
    let mut out = Vec::with_capacity(samples.len() * 3);
    for (k, (q, t)) in samples.iter().enumerate() {
        out.push(*q);
        if closed && k == samples.len() - 1 {
            break;
        }
        out.push(*q + t.perp() * (width * side));
        out.push(*q);
    }
    out
}

fn blanket_pieces(shape: &Shape, geometry: &Geometry) -> Vec<Piece> {
    let p = &shape.params;
    let sp = mm(p.spacing_set().unwrap_or(defaults::BLANKET_SPACING));
    let w = mm(p.width.unwrap_or(defaults::BLANKET_WIDTH));
    match geometry {
        Geometry::Fill { rings, rule } => rings
            .iter()
            .filter(|r| r.len() >= 3)
            .map(|r| {
                // Prongs point into the shape.
                let probe = crate::geom::sample_by_arclength(r, 20.0);
                let side = probe
                    .get(probe.len() / 2)
                    .map(|(q, t)| {
                        if crate::geom::point_inside(rings, *rule, *q + t.perp() * 3.0) {
                            1.0
                        } else {
                            -1.0
                        }
                    })
                    .unwrap_or(1.0);
                Piece::fixed(blanket(r, true, sp, w, side))
            })
            .collect(),
        Geometry::Stroke { lines, .. } => lines
            .iter()
            .map(|(l, c)| Piece::fixed(blanket(l, *c, sp, w, 1.0)))
            .collect(),
    }
}

/// Appliqué: placement line, stop, tack-down, stop, satin cover.
fn applique_runs(rings: &[Polyline], shape: &Shape, opt: &PlanOptions) -> ShapeRuns {
    let p = &shape.params;
    let f = &opt.fabric;
    let closed: Vec<&Polyline> = rings.iter().filter(|r| r.len() >= 3).collect();
    let placement: Vec<Piece> = closed
        .iter()
        .map(|r| Piece::ring(running(r, true, mm(2.5), 1)))
        .collect();
    let tack_params = SatinParams {
        spacing: 25.0,
        pull: 0.0,
        underlay: 0,
        run_len: 25.0,
        short_stitches: false,
        split: None,
        style: Default::default(),
    };
    let tack: Vec<Piece> = closed
        .iter()
        .map(|r| {
            Piece::fixed(satin::column(
                &satin::rails_from_centerline(r, true, 12.0),
                &tack_params,
            ))
        })
        .collect();
    let w = mm(p.width.unwrap_or(defaults::APPLIQUE_WIDTH));
    let cover_params = SatinParams {
        spacing: mm(p.satin_spacing_for(f, w / 10.0)),
        pull: mm(p.satin_pull(f)),
        underlay: if p.underlay_on() { underlay::CENTER } else { 0 },
        run_len: mm(2.5),
        short_stitches: p.short_stitches_on(),
        split: None,
        style: Default::default(),
    };
    let cover: Vec<Piece> = closed
        .iter()
        .map(|r| {
            Piece::fixed(satin::column(
                &satin::rails_from_centerline(r, true, w),
                &cover_params,
            ))
        })
        .collect();
    ShapeRuns {
        groups: vec![placement, tack, cover],
        area: None,
        stops: vec![1, 2],
        colors: Vec::new(),
    }
}

/// The decorative fills (see `stitch::patterns`): each a group of pieces
/// the planner joins, plus a tartan's per-colour groups.
fn pattern_runs(
    rings: &[Polyline],
    rule: FillRule,
    shape: &Shape,
    kind: Kind,
    opt: &PlanOptions,
) -> ShapeRuns {
    use crate::stitch::patterns;
    let p = &shape.params;
    let f = &opt.fabric;
    let len = mm(p.run_len(f));
    let area = Some((rings.to_vec(), rule));
    match kind {
        Kind::Meander => {
            let seed = shape.uid as u64 + 1;
            let lines =
                patterns::meander(rings, rule, mm(p.meander_spacing()).max(10.0), len, seed);
            ShapeRuns::new(
                vec![lines.into_iter().map(Piece::reversible).collect()],
                area,
            )
        }
        Kind::Ripple => {
            let rows = patterns::ripple(
                rings,
                rule,
                mm(p.ripple_spacing()).max(5.0),
                len,
                p.spiral == Some(true),
            );
            let pieces = rows
                .into_iter()
                .map(|r| {
                    if p.spiral == Some(true) {
                        Piece::reversible(r)
                    } else {
                        Piece::ring(r)
                    }
                })
                .collect();
            ShapeRuns::new(vec![pieces], area)
        }
        Kind::Cross => {
            let rows = patterns::cross_stitch(rings, rule, mm(p.cross_cell()).max(10.0));
            ShapeRuns::new(
                vec![rows.into_iter().map(Piece::reversible).collect()],
                area,
            )
        }
        Kind::Tartan => {
            let sett: Vec<patterns::Stripe> = p
                .tartan_sett(shape.color)
                .into_iter()
                .map(|(color, w)| patterns::Stripe {
                    color,
                    width: mm(w),
                })
                .collect();
            let per = patterns::tartan(
                rings,
                rule,
                &sett,
                mm(p.fill_spacing(f)),
                mm(p.fill_len(f)),
                p.fill_angle().to_radians(),
            );
            let mut sr = ShapeRuns::new(Vec::new(), area);
            for (gi, (color, rows)) in per.into_iter().enumerate() {
                sr.groups
                    .push(rows.into_iter().map(Piece::reversible).collect());
                sr.colors.push((gi, color));
            }
            sr
        }
        _ => ShapeRuns::new(Vec::new(), None),
    }
}

fn manual_pieces(geometry: &Geometry) -> Vec<Vec<Piece>> {
    let closed_loop = |l: &Polyline| {
        let mut v = l.clone();
        if v.len() > 2 {
            v.push(v[0]);
        }
        v
    };
    // One group per path: manual stitching keeps its order exactly.
    match geometry {
        Geometry::Fill { rings, .. } => rings
            .iter()
            .map(|r| vec![Piece::fixed(closed_loop(r))])
            .collect(),
        Geometry::Stroke { lines, .. } => lines
            .iter()
            .map(|(l, c)| vec![Piece::fixed(if *c { closed_loop(l) } else { l.clone() })])
            .collect(),
    }
}

/// [`shape_runs`], but an object whose stitching hits an internal error
/// (a panic in the fill, satin or boolean code on some odd outline) is
/// left out with a note, rather than taking the whole plan (and the app,
/// and the recovered document on the next launch) down with it.
fn shape_runs_guarded(
    shape: &Shape,
    geometry: &Geometry,
    opt: &PlanOptions,
    notes: &mut Vec<String>,
) -> ShapeRuns {
    let mut tried = Vec::new();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        shape_runs(shape, geometry, opt, &mut tried)
    }));
    match r {
        Ok(mut sr) => {
            notes.append(&mut tried);
            // A piece without stitches (a subpath without length) has no
            // start or end to route to. The groups stay: stops count them.
            for g in &mut sr.groups {
                g.retain(|p| !p.pts.is_empty());
            }
            sr
        }
        Err(e) => {
            let what = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown error".into());
            notes.push(format!(
                "Couldn't work out this object's stitches ({what}); it's left out. \
                 Try another stitch type, Object › Path › Clean Up, or Simplify."
            ));
            ShapeRuns::new(Vec::new(), None)
        }
    }
}

fn shape_runs(
    shape: &Shape,
    geometry: &Geometry,
    opt: &PlanOptions,
    notes: &mut Vec<String>,
) -> ShapeRuns {
    let p = &shape.params;
    let f = &opt.fabric;
    let kind = p.kind.unwrap_or(Kind::Fill);
    match (geometry, kind) {
        (_, Kind::Skip) => ShapeRuns::new(Vec::new(), None),
        (_, Kind::Manual) => ShapeRuns::new(manual_pieces(geometry), None),
        (_, Kind::Blanket) => ShapeRuns::new(vec![blanket_pieces(shape, geometry)], None),
        (Geometry::Fill { rings, rule }, Kind::Fill) => ShapeRuns::new(
            fill_runs(rings, *rule, shape, opt),
            Some((rings.clone(), *rule)),
        ),
        (Geometry::Fill { rings, rule }, Kind::Contour) => ShapeRuns::new(
            if p.spiral == Some(true) {
                spiral_runs(rings, *rule, shape, opt)
            } else {
                contour_runs(rings, *rule, shape, opt)
            },
            Some((rings.clone(), *rule)),
        ),
        (Geometry::Fill { rings, .. }, Kind::Applique) => applique_runs(rings, shape, opt),
        (Geometry::Fill { rings, rule }, Kind::Knockdown) => ShapeRuns::new(
            knockdown_runs(rings, *rule, shape, opt),
            Some((rings.clone(), *rule)),
        ),
        (
            Geometry::Fill { rings, rule },
            Kind::Meander | Kind::Ripple | Kind::Cross | Kind::Tartan,
        ) => pattern_runs(rings, *rule, shape, kind, opt),
        (_, Kind::Satin) => {
            let area = match geometry {
                Geometry::Fill { rings, rule } => Some((rings.clone(), *rule)),
                _ => None,
            };
            if let Some(colors) = p.colors.as_ref().filter(|c| c.len() >= 2) {
                // Multicolor: the column as side-by-side strips, one per
                // thread, each its own color group.
                let mut sr = ShapeRuns::new(Vec::new(), area);
                let n = colors.len();
                for (k, &color) in colors.iter().enumerate() {
                    let (t0, t1) = (k as f64 / n as f64, (k + 1) as f64 / n as f64);
                    let (pieces, fallback) = satin_pieces_strip(shape, opt, notes, t0, t1);
                    let mut groups = fallback;
                    if !pieces.is_empty() {
                        groups.push(pieces);
                    }
                    for g in groups {
                        sr.colors.push((sr.groups.len(), color));
                        sr.groups.push(g);
                    }
                }
                return sr;
            }
            let (pieces, mut fallback) = satin_pieces(shape, opt, notes);
            if !pieces.is_empty() {
                fallback.push(pieces);
            }
            ShapeRuns::new(fallback, area)
        }
        (Geometry::Fill { rings, .. }, _) => {
            // Running stitch along each outline.
            let pieces = rings
                .iter()
                .map(|r| {
                    let run = running(r, true, mm(p.run_len(f)), p.run_repeats());
                    if p.run_repeats() == 1 {
                        Piece::ring(run)
                    } else {
                        Piece::reversible(run)
                    }
                })
                .collect();
            ShapeRuns::new(vec![pieces], None)
        }
        (Geometry::Stroke { lines, .. }, _) => {
            let pieces = lines
                .iter()
                .map(|(l, c)| {
                    let run = running(l, *c, mm(p.run_len(f)), p.run_repeats());
                    if *c && p.run_repeats() == 1 {
                        Piece::ring(run)
                    } else {
                        Piece::reversible(run)
                    }
                })
                .collect();
            ShapeRuns::new(vec![pieces], None)
        }
    }
}

/// Area a shape's stitches will cover, for overlap removal (None for runs).
pub fn coverage(shape: &Shape) -> Option<(Vec<Polyline>, FillRule)> {
    match (shape.params.kind.unwrap_or(Kind::Fill), &shape.geometry) {
        (Kind::Fill | Kind::Contour | Kind::Applique, Geometry::Fill { rings, rule }) => {
            Some((rings.clone(), *rule))
        }
        (Kind::Satin, _) => {
            let mut rings = Vec::new();
            for r in satin_rails(shape) {
                match r {
                    Ok(rails) if rails.closed => {
                        rings.push(rails.a.clone());
                        rings.push(rails.b.clone());
                    }
                    Ok(rails) => {
                        let mut ring = rails.a.clone();
                        ring.extend(rails.b.iter().rev());
                        rings.push(ring);
                    }
                    Err((comp, _)) => rings.extend(comp),
                }
            }
            Some((rings, FillRule::EvenOdd))
        }
        _ => None,
    }
}

fn geometry_bounds(g: &Geometry) -> Bounds {
    let mut b = Bounds::empty();
    match g {
        Geometry::Fill { rings, .. } => rings.iter().flatten().for_each(|&p| b.add(p)),
        Geometry::Stroke { lines, width } => {
            lines.iter().flat_map(|(l, _)| l).for_each(|&p| b.add(p));
            if !b.is_empty() {
                b.add(b.min - P::new(width / 2.0, width / 2.0));
                b.add(b.max + P::new(width / 2.0, width / 2.0));
            }
        }
    }
    b
}

fn overlaps(a: &Bounds, b: &Bounds) -> bool {
    !a.is_empty()
        && !b.is_empty()
        && a.min.x <= b.max.x
        && b.min.x <= a.max.x
        && a.min.y <= b.max.y
        && b.min.y <= a.max.y
}

/// Sewing order: document order, or cap order (center-out, bottom-up, grouping
/// colors) that still sews anything underneath an object before the object.
fn sew_order(shapes: &[Shape], opt: &PlanOptions) -> Vec<usize> {
    let n = shapes.len();
    if !opt.hat_order {
        return (0..n).collect();
    }
    let bounds: Vec<Bounds> = shapes
        .iter()
        .map(|s| geometry_bounds(&s.geometry))
        .collect();
    let mut all = Bounds::empty();
    for b in &bounds {
        if !b.is_empty() {
            all.add(b.min);
            all.add(b.max);
        }
    }
    let bottom = all.max.y;
    // deps[j] = shapes that must come before j (earlier in the document and overlapping).
    let deps: Vec<Vec<usize>> = (0..n)
        .map(|j| {
            (0..j)
                .filter(|&i| overlaps(&bounds[i], &bounds[j]))
                .collect()
        })
        .collect();
    let mut done = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut color: Option<[u8; 3]> = None;
    while order.len() < n {
        let pick = (0..n)
            .filter(|&j| !done[j] && deps[j].iter().all(|&i| done[i]))
            .min_by(|&a, &b| {
                let key = |j: usize| {
                    let c = bounds[j].center();
                    let change = (color.is_some() && Some(shapes[j].color) != color) as u8;
                    (change, c.x.abs() + 0.5 * (bottom - c.y))
                };
                let (ka, kb) = (key(a), key(b));
                ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
            })
            .expect("dependencies only point backwards, so something is always available");
        done[pick] = true;
        color = Some(shapes[pick].color);
        order.push(pick);
    }
    order
}

/// Geometry each shape is actually stitched with, after overlap removal.
pub fn effective_geometry(shapes: &[Shape], order: &[usize], opt: &PlanOptions) -> Vec<Geometry> {
    effective_geometry_cached(shapes, order, opt, &mut HashMap::new())
}

/// [`effective_geometry`], reusing `cache` (keyed by what each result
/// depends on) for every shape whose outline, and the outlines and stitch
/// types of what's sewn over it, haven't changed. Moving one object redoes
/// only the objects it overlaps, not the whole design.
fn effective_geometry_cached(
    shapes: &[Shape],
    order: &[usize],
    opt: &PlanOptions,
    cache: &mut HashMap<u64, Geometry>,
) -> Vec<Geometry> {
    use std::hash::{Hash, Hasher};
    let mut out: Vec<Geometry> = shapes.iter().map(|s| s.geometry.clone()).collect();
    if !opt.remove_overlaps {
        return out;
    }
    let bounds: Vec<Bounds> = shapes
        .iter()
        .map(|s| geometry_bounds(&s.geometry))
        .collect();
    // What each shape covers (a satin's coverage comes from its rails, so
    // it's worked out only for shapes that are actually on top of another),
    // normalized once for all the shapes under it.
    struct Cover {
        rings: Vec<Polyline>,
        bounds: Bounds,
        /// `rings` simplified, for where it's within reach whole.
        simple: Option<Vec<Polyline>>,
    }
    let mut cover: Vec<Option<Option<Cover>>> = (0..shapes.len()).map(|_| None).collect();
    let geo_keys: Vec<u64> = shapes.iter().map(|s| geometry_key(&s.geometry)).collect();
    let cover_keys: Vec<u64> = shapes
        .iter()
        .zip(&geo_keys)
        .map(|(s, k)| {
            let mut h = hasher();
            k.hash(&mut h);
            let kind = s.params.kind.unwrap_or(Kind::Fill);
            kind.hash(&mut h);
            if kind == Kind::Satin {
                format!("{:?}", s.params).hash(&mut h);
            }
            h.finish()
        })
        .collect();
    let mut used = std::collections::HashSet::new();
    for (pos, &i) in order.iter().enumerate() {
        let s = &shapes[i];
        let Geometry::Fill { rings, rule } = &s.geometry else {
            continue;
        };
        if !matches!(
            s.params.kind.unwrap_or(Kind::Fill),
            Kind::Fill | Kind::Contour
        ) {
            continue;
        }
        let on_top: Vec<usize> = order[pos + 1..]
            .iter()
            .copied()
            .filter(|&j| overlaps(&bounds[i], &bounds[j]))
            .collect();
        if on_top.is_empty() {
            continue;
        }
        let key = {
            let mut h = hasher();
            geo_keys[i].hash(&mut h);
            (*rule == FillRule::EvenOdd).hash(&mut h);
            opt.overlap.to_bits().hash(&mut h);
            for &j in &on_top {
                cover_keys[j].hash(&mut h);
            }
            h.finish()
        };
        used.insert(key);
        if let Some(g) = cache.get(&key) {
            out[i] = g.clone();
            continue;
        }
        // Only what's on top within reach of this shape matters (clipping
        // first gives the same result, faster), and shrinking an outline
        // traced to a few hundredths of a millimetre by half a millimetre
        // doesn't need every one of its points.
        let reach = opt.overlap.abs() + 2.0;
        let (lo, hi) = (
            P::new(bounds[i].min.x - reach, bounds[i].min.y - reach),
            P::new(bounds[i].max.x + reach, bounds[i].max.y + reach),
        );
        let mut above: Vec<Vec<Polyline>> = Vec::new();
        for &j in &on_top {
            let c = cover[j].get_or_insert_with(|| {
                coverage(&shapes[j]).map(|(r, f)| {
                    let rings = boolean::normalize(&r, f);
                    let mut bounds = Bounds::empty();
                    rings.iter().flatten().for_each(|&p| bounds.add(p));
                    Cover {
                        rings,
                        bounds,
                        simple: None,
                    }
                })
            });
            let Some(c) = c else { continue };
            let b = &c.bounds;
            if b.is_empty() || b.min.x > hi.x || b.max.x < lo.x || b.min.y > hi.y || b.max.y < lo.y
            {
                continue;
            }
            let within = b.min.x >= lo.x && b.max.x <= hi.x && b.min.y >= lo.y && b.max.y <= hi.y;
            let piece = if within {
                c.simple
                    .get_or_insert_with(|| boolean::simplify_rings(&c.rings, 0.5))
                    .clone()
            } else {
                boolean::simplify_rings(&boolean::clip_normalized_to_rect(&c.rings, lo, hi), 0.5)
            };
            if !piece.is_empty() {
                above.push(piece);
            }
        }
        if above.is_empty() {
            cache.insert(key, out[i].clone());
            continue;
        }
        let covered = boolean::simplify_rings(&boolean::union_normalized(&above), 0.5);
        let shrunk = boolean::offset(&covered, FillRule::EvenOdd, -opt.overlap);
        if shrunk.is_empty() {
            cache.insert(key, out[i].clone());
            continue;
        }
        let remaining: Vec<Polyline> = boolean::difference(rings, *rule, &shrunk)
            .into_iter()
            .filter(|r| r.len() >= 3 && boolean::ring_area(r) > 150.0)
            .collect();
        out[i] = Geometry::Fill {
            rings: remaining,
            rule: FillRule::EvenOdd,
        };
        cache.insert(key, out[i].clone());
    }
    // Forget shapes no longer there (or no longer under the same things).
    cache.retain(|k, _| used.contains(k));
    out
}

/// What planning keeps between runs, so a change re-plans only what it
/// touched: the overlap removal (each shape's redone only when its outline,
/// or the outlines and stitch types of what's sewn over it, change) and
/// each shape's stitches (redone only when that shape, its settings, or
/// what's sewn over it change).
#[derive(Default)]
pub struct PlanCache {
    overlaps: Option<(u64, Vec<Geometry>)>,
    /// Each shape's geometry after overlap removal, by what it depends on.
    overlap_shapes: HashMap<u64, Geometry>,
    runs: std::collections::HashMap<u64, (ShapeRuns, Vec<String>)>,
}

fn hasher() -> std::collections::hash_map::DefaultHasher {
    std::collections::hash_map::DefaultHasher::new()
}

/// A fingerprint of a geometry's every coordinate.
pub(crate) fn geometry_key(g: &Geometry) -> u64 {
    use std::hash::{Hash, Hasher};
    fn eat(h: &mut impl Hasher, pts: &[P]) {
        pts.len().hash(h);
        for p in pts {
            p.x.to_bits().hash(h);
            p.y.to_bits().hash(h);
        }
    }
    let mut h = hasher();
    match g {
        Geometry::Fill { rings, rule } => {
            0u8.hash(&mut h);
            (*rule == FillRule::EvenOdd).hash(&mut h);
            for r in rings {
                eat(&mut h, r);
            }
        }
        Geometry::Stroke { lines, width } => {
            1u8.hash(&mut h);
            width.to_bits().hash(&mut h);
            for (l, closed) in lines {
                closed.hash(&mut h);
                eat(&mut h, l);
            }
        }
    }
    h.finish()
}

/// Build the stitch plan. Consecutive shapes of the same color share a color block.
pub fn plan(name: &str, shapes: &[Shape], opt: &PlanOptions) -> Design {
    plan_cached(name, shapes, opt, &mut PlanCache::default())
}

/// [`plan`], reusing what `cache` kept from the last run where nothing it
/// depends on changed.
pub fn plan_cached(
    name: &str,
    shapes: &[Shape],
    opt: &PlanOptions,
    cache: &mut PlanCache,
) -> Design {
    use std::hash::{Hash, Hasher};
    let order = sew_order(shapes, opt);
    let geo_keys: Vec<u64> = shapes.iter().map(|s| geometry_key(&s.geometry)).collect();
    let opt_key = format!("{opt:?}");
    // Overlap removal depends on outlines, stitch types (a satin's coverage
    // on its settings too), the sewing order and the overlap options.
    let overlap_key = {
        let mut h = hasher();
        order.hash(&mut h);
        opt.remove_overlaps.hash(&mut h);
        opt.overlap.to_bits().hash(&mut h);
        for (s, k) in shapes.iter().zip(&geo_keys) {
            k.hash(&mut h);
            let kind = s.params.kind.unwrap_or(Kind::Fill);
            format!("{kind:?}").hash(&mut h);
            if kind == Kind::Satin {
                format!("{:?}", s.params).hash(&mut h);
            }
        }
        h.finish()
    };
    let geometry = match &cache.overlaps {
        Some((k, g)) if *k == overlap_key && g.len() == shapes.len() => g.clone(),
        _ => {
            let g = effective_geometry_cached(shapes, &order, opt, &mut cache.overlap_shapes);
            cache.overlaps = Some((overlap_key, g.clone()));
            g
        }
    };
    let mut used = std::collections::HashSet::new();
    let mut design = Design {
        name: name.to_string(),
        blocks: Vec::new(),
        per_shape: vec![0; shapes.len()],
        sew_order: order.clone(),
        notes: Vec::new(),
        block_shapes: Vec::new(),
        basting_stitches: 0,
        starts: Vec::new(),
        covered: shapes
            .iter()
            .zip(&geometry)
            .map(|(s, g)| {
                matches!(
                    (&s.geometry, g),
                    (Geometry::Fill { rings: a, .. }, Geometry::Fill { rings: b, .. })
                        if !a.is_empty() && b.is_empty()
                )
            })
            .collect(),
    };
    let mut cursor: Option<P> = None;
    // Set by the last object: cut before the next run, or pause before
    // the next object.
    let mut force_trim = false;
    let mut stop_next = false;
    // Which object asked for that pause, and why every block (the empty
    // ones too, until they're dropped below) was started.
    let mut stop_from: Option<usize> = None;
    let mut starts: Vec<BlockStart> = Vec::new();
    // The last run sewn belongs to the object being sewn (gaps inside an
    // object are never connected by the document's `connect`).
    let mut same_shape = false;
    for &si in &order {
        let shape = &shapes[si];
        if shape.params.kind == Some(Kind::Skip) {
            continue;
        }
        // The fill's travel ring, worked out when a gap first needs it.
        let mut travel: Option<Vec<Polyline>> = None;
        let start_at = shape
            .params
            .start
            .and_then(|t| outline_at(&shape.geometry, t));
        let end_at = shape
            .params
            .end
            .and_then(|t| outline_at(&shape.geometry, t));
        // This shape's stitches: kept from last time when its outline, what's
        // left of it after overlaps, its settings and the options are the same.
        let key = {
            let mut h = hasher();
            geo_keys[si].hash(&mut h);
            geometry_key(&geometry[si]).hash(&mut h);
            format!("{:?}{:?}", shape.params, shape.part).hash(&mut h);
            // A tartan's default sett comes from the thread color, and a
            // meander's seed from the uid.
            shape.color.hash(&mut h);
            shape.uid.hash(&mut h);
            opt_key.hash(&mut h);
            h.finish()
        };
        used.insert(key);
        let (sr, notes) = match cache.runs.get(&key) {
            Some(hit) => hit.clone(),
            None => {
                let mut notes = Vec::new();
                let sr = shape_runs_guarded(shape, &geometry[si], opt, &mut notes);
                cache.runs.insert(key, (sr.clone(), notes.clone()));
                (sr, notes)
            }
        };
        design.notes.extend(notes.into_iter().map(|n| (si, n)));
        if sr.groups.iter().all(|g| g.is_empty()) {
            continue;
        }
        if design.blocks.last().is_none_or(|b| b.color != shape.color)
            || (stop_next && design.blocks.last().is_some_and(|b| !b.runs.is_empty()))
        {
            design.blocks.push(Block {
                color: shape.color,
                runs: Vec::new(),
            });
            design.block_shapes.push(Vec::new());
            starts.push(BlockStart::default());
        }
        if stop_next
            && design.blocks.last().is_some_and(|b| b.runs.is_empty())
            && let Some(s) = starts.last_mut()
        {
            s.stop_after = stop_from;
        }
        let last_group = sr.groups.iter().rposition(|g| !g.is_empty()).unwrap_or(0);
        for (gi, mut group) in sr.groups.into_iter().enumerate() {
            // The group's thread: the object's, unless the group names one.
            let group_color = sr
                .colors
                .iter()
                .find(|(g, _)| *g == gi)
                .map(|(_, c)| *c)
                .unwrap_or(shape.color);
            let stop_here = sr.stops.contains(&gi);
            let last_nonempty = design.blocks.last().is_some_and(|b| !b.runs.is_empty());
            if (stop_here && last_nonempty)
                || design.blocks.last().is_none_or(|b| b.color != group_color)
            {
                // A machine stop (same thread, new block) or a change of thread.
                // A block left empty (a pause before an object whose first
                // group is in another thread) hands its pause on.
                let carried = match (design.blocks.last(), starts.last()) {
                    (Some(b), Some(s)) if b.runs.is_empty() => s.stop_after,
                    _ => None,
                };
                design.blocks.push(Block {
                    color: group_color,
                    runs: Vec::new(),
                });
                design.block_shapes.push(Vec::new());
                starts.push(BlockStart {
                    stop_after: carried,
                    applique: (stop_here && last_nonempty)
                        .then(|| sr.stops.iter().position(|&g| g == gi))
                        .flatten()
                        .map(|k| (si, k as u8 + 1)),
                    ..BlockStart::default()
                });
            }
            let bs = design.block_shapes.last_mut().unwrap();
            if !bs.contains(&si) {
                bs.push(si);
            }
            let block = design.blocks.last_mut().unwrap();
            // The object's end point: in its last group, the piece that can
            // end nearest it is kept for last, and sewn so it does.
            let mut ender = (gi == last_group).then_some(end_at).flatten().map(|e| {
                group
                    .iter()
                    .enumerate()
                    .map(|(i, piece)| {
                        let d = piece.pts.last().map_or(f64::INFINITY, |q| q.dist(e));
                        let d_alt = piece
                            .alt
                            .as_ref()
                            .and_then(|a| a.last())
                            .map_or(f64::INFINITY, |q| q.dist(e));
                        if piece.ring {
                            // A ring ends where it starts: nearest vertex to e.
                            (i, piece.nearest_start(e).0, false)
                        } else {
                            (i, d.min(d_alt), d_alt < d)
                        }
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(i, _, alt)| (i, alt))
                    .unwrap_or((0, false))
            });
            let mut first = gi == 0;
            // Greedy nearest-neighbour ordering within the group.
            while !group.is_empty() {
                // The object's start point stands in for where the needle is
                // when choosing its first piece.
                let here_is_start = first && start_at.is_some();
                let here = match (first, start_at, cursor) {
                    (true, Some(s), _) => s,
                    (_, _, Some(c)) => c,
                    _ => group[0].pts[0],
                };
                first = false;
                let (mut best, mut best_d, mut best_alt, mut best_at) =
                    (0, f64::INFINITY, false, 0);
                let from_start = here_is_start;
                let finishing = group.len() == 1 && ender.is_some() && !from_start;
                let held = if group.len() > 1 {
                    ender.map(|(i, _)| i)
                } else {
                    None
                };
                for (i, piece) in group.iter().enumerate() {
                    if held == Some(i) {
                        continue;
                    }
                    if finishing {
                        // Whichever way ends nearest the end point.
                        let (_, alt) = ender.unwrap();
                        (best, best_alt, best_at) = (i, alt && piece.alt.is_some(), 0);
                        if piece.ring
                            && let Some(e) = end_at
                        {
                            best_at = piece.nearest_start(e).1;
                        }
                        break;
                    }
                    let (d0, at) = piece.nearest_start(here);
                    if d0 < best_d {
                        (best, best_d, best_alt, best_at) = (i, d0, false, at);
                    }
                    if let Some(alt) = &piece.alt {
                        let d1 = here.dist(alt[0]);
                        if d1 < best_d {
                            (best, best_d, best_alt, best_at) = (i, d1, true, 0);
                        }
                    }
                }
                let last_piece = group.len() == 1;
                let piece = group.swap_remove(best);
                // (That moved the last piece into the gap: the one kept
                // for the end, if it was last.)
                if let Some((held, _)) = &mut ender
                    && *held == group.len()
                {
                    *held = best;
                }
                let mut pts = if best_alt {
                    piece.alt.unwrap()
                } else if piece.ring && best_at > 0 {
                    rotate_ring(&piece.pts, best_at)
                } else {
                    piece.pts
                };
                dedupe(&mut pts, opt.min_stitch);
                if pts.is_empty() {
                    continue;
                }
                // Begin at the start point: a few stitches from it to the
                // first row, hidden under the fill (a satin or run just
                // starts at its nearest end).
                if from_start
                    && let Some(st) = start_at
                    && let Some((rings, rule)) = sr.area.as_ref()
                    && st.dist(pts[0]) > opt.min_stitch
                    && segment_inside(rings, *rule, st, pts[0], 2.0)
                {
                    let n = (st.dist(pts[0]) / 30.0).ceil() as usize;
                    let lead: Vec<P> = (0..n)
                        .map(|k| st.lerp(pts[0], k as f64 / n as f64))
                        .collect();
                    pts.splice(0..0, lead);
                }
                // Finish at the end point: a few stitches on to it.
                if last_piece
                    && gi == last_group
                    && let Some(e) = end_at
                    && let Some(&q) = pts.last()
                    && q.dist(e) > opt.min_stitch
                {
                    let n = (q.dist(e) / 30.0).ceil() as usize;
                    for k in 1..=n {
                        pts.push(q.lerp(e, k as f64 / n as f64));
                    }
                }
                design.per_shape[si] += pts.len();
                let start = pts[0];
                let mut trim_before = false;
                if let (Some(prev), Some(run)) = (cursor, block.runs.last_mut()) {
                    let gap = prev.dist(start);
                    let inside = sr.area.as_ref().is_some_and(|(rings, rule)| {
                        gap <= opt.travel_inside && segment_inside(rings, *rule, prev, start, 2.0)
                    });
                    let across =
                        opt.max_jump_no_trim
                            .max(if same_shape { 0.0 } else { opt.connect });
                    if !force_trim && (gap <= across || inside) {
                        // Sew straight across: append to the previous run.
                        let n = (gap / 30.0).ceil() as usize;
                        for i in 1..n {
                            run.pts.push(prev.lerp(start, i as f64 / n as f64));
                        }
                        run.pts.extend(pts.iter().copied());
                        cursor = run.pts.last().copied();
                        force_trim = false;
                        same_shape = true;
                        continue;
                    }
                    // Within a fill: travel round the inside of its outline
                    // rather than cutting (Ink/Stitch's underpath).
                    if !force_trim
                        && same_shape
                        && opt.travel_route > 0.0
                        && let Some((rings, rule)) = sr.area.as_ref()
                        && let Some(route) = route_along(
                            travel.get_or_insert_with(|| travel_ring(rings, *rule)),
                            prev,
                            start,
                            opt.travel_route,
                        )
                    {
                        run.pts
                            .extend(route.into_iter().filter(|q| q.dist(start) > 1e-6));
                        run.pts.extend(pts.iter().copied());
                        cursor = run.pts.last().copied();
                        continue;
                    }
                    trim_before = true;
                }
                force_trim = false;
                same_shape = true;
                cursor = pts.last().copied();
                block.runs.push(Run { pts, trim_before });
            }
        }
        force_trim = shape.params.trim == Some(true);
        stop_next = shape.params.stop == Some(true);
        stop_from = stop_next.then_some(si);
        same_shape = false;
    }
    // Forget shapes no longer there (or no longer like that).
    cache.runs.retain(|k, _| used.contains(k));
    let keep: Vec<bool> = design.blocks.iter().map(|b| !b.runs.is_empty()).collect();
    let mut k = keep.iter();
    design.block_shapes.retain(|_| *k.next().unwrap());
    let mut k = keep.iter();
    starts.retain(|_| *k.next().unwrap());
    design.blocks.retain(|b| !b.runs.is_empty());
    for (k, s) in starts.iter_mut().enumerate() {
        s.thread_change = k == 0 || design.blocks[k - 1].color != design.blocks[k].color;
        // (A pause asked for before anything was sewn isn't one.)
        if k == 0 {
            s.stop_after = None;
        }
        s.note = s
            .stop_after
            .and_then(|i| shapes[i].params.stop_note.clone())
            .filter(|n| !n.trim().is_empty());
    }
    design.starts = starts;
    // The basting box: before everything else, in the first thread.
    if let Some(margin) = opt.basting {
        let b = design.bounds();
        if let Some(first) = design.blocks.first_mut()
            && !b.is_empty()
        {
            let m = margin.max(0.0);
            let (x0, y0, x1, y1) = (b.min.x - m, b.min.y - m, b.max.x + m, b.max.y + m);
            let corners = [
                P::new(x0, y0),
                P::new(x1, y0),
                P::new(x1, y1),
                P::new(x0, y1),
            ];
            let pts = running(&corners, true, mm(defaults::BASTING_LEN), 1);
            design.basting_stitches = pts.len();
            // (Whatever came first now follows the box: cut in between.)
            if let Some(r) = first.runs.first_mut() {
                r.trim_before = true;
            }
            first.runs.insert(
                0,
                Run {
                    pts,
                    trim_before: false,
                },
            );
        }
    }
    design
}

/// A sewing order with fewer color changes: objects of the current color go
/// first, but nothing moves above an object it overlaps that was meant to be
/// on top. Returns shape indices.
pub fn grouped_by_color(shapes: &[Shape]) -> Vec<usize> {
    let n = shapes.len();
    let bounds: Vec<Bounds> = shapes
        .iter()
        .map(|s| geometry_bounds(&s.geometry))
        .collect();
    let deps: Vec<Vec<usize>> = (0..n)
        .map(|j| {
            (0..j)
                .filter(|&i| overlaps(&bounds[i], &bounds[j]))
                .collect()
        })
        .collect();
    let mut done = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut color: Option<[u8; 3]> = None;
    while order.len() < n {
        let ready = |j: usize| !done[j] && deps[j].iter().all(|&i| done[i]);
        let pick = (0..n)
            .find(|&j| ready(j) && Some(shapes[j].color) == color)
            .or_else(|| (0..n).find(|&j| ready(j)))
            .expect("dependencies only point backwards");
        done[pick] = true;
        color = Some(shapes[pick].color);
        order.push(pick);
    }
    order
}

/// Default satin width (mm) for a shape: its stroke width, or the outline default.
pub fn default_satin_width(shape: &Shape) -> f64 {
    match &shape.geometry {
        Geometry::Stroke { width, .. } => width / 10.0,
        Geometry::Fill { .. } => defaults::OUTLINE_SATIN_WIDTH,
    }
}

impl Design {
    /// Why the machine halts before block `k`: a change of thread, a pause
    /// an object asked for, an appliqué step, or (same thread twice running
    /// in a design that doesn't say) a plain stop.
    pub fn block_start(&self, k: usize) -> BlockStart {
        match self.starts.get(k) {
            Some(s) => s.clone(),
            None => BlockStart {
                thread_change: k == 0
                    || self
                        .blocks
                        .get(k)
                        .zip(self.blocks.get(k - 1))
                        .is_none_or(|(b, a)| a.color != b.color),
                ..BlockStart::default()
            },
        }
    }

    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds::empty();
        for blk in &self.blocks {
            for r in &blk.runs {
                for &p in &r.pts {
                    b.add(p);
                }
            }
        }
        b
    }

    /// Move the design so its bounding box is centered on the origin.
    pub fn center(&mut self) {
        let c = self.bounds().center();
        for blk in &mut self.blocks {
            for r in &mut blk.runs {
                for p in &mut r.pts {
                    *p = *p - c;
                }
            }
        }
    }

    pub fn stitch_count(&self) -> usize {
        self.blocks
            .iter()
            .flat_map(|b| &b.runs)
            .map(|r| r.pts.len())
            .sum()
    }

    /// How much top thread block `k` takes, mm: an estimate (see
    /// [`thread_mm`]).
    pub fn block_thread_mm(&self, k: usize) -> f64 {
        self.blocks
            .get(k)
            .map_or(0.0, |b| b.runs.iter().map(|r| thread_mm(&r.pts)).sum())
    }

    /// Top thread for the whole design, mm (an estimate).
    pub fn thread_mm(&self) -> f64 {
        (0..self.blocks.len())
            .map(|k| self.block_thread_mm(k))
            .sum()
    }

    /// Bobbin thread for the whole design, mm (an estimate: a share of the
    /// top thread, see [`BOBBIN_SHARE`]).
    pub fn bobbin_mm(&self) -> f64 {
        self.thread_mm() * BOBBIN_SHARE
    }

    /// Top thread per thread color, mm, in the order each is first sewn:
    /// what to have on the spool for each.
    pub fn thread_by_color(&self) -> Vec<([u8; 3], f64)> {
        let mut out: Vec<([u8; 3], f64)> = Vec::new();
        for (k, b) in self.blocks.iter().enumerate() {
            let mm = self.block_thread_mm(k);
            match out.iter_mut().find(|(c, _)| *c == b.color) {
                Some((_, total)) => *total += mm,
                None => out.push((b.color, mm)),
            }
        }
        out
    }

    pub fn trim_count(&self) -> usize {
        self.blocks
            .iter()
            .flat_map(|b| &b.runs)
            .filter(|r| r.trim_before)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Params;
    use crate::svg::Part;

    fn shape(geometry: Geometry, kind: Kind, color: [u8; 3]) -> Shape {
        Shape {
            id: String::new(),
            part: Part::Fill,
            label: String::new(),
            color,
            paint: color,
            geometry,
            params: Params {
                kind: Some(kind),
                underlay: Some(false),
                ..Params::default()
            },
            uid: 0,
            groups: Vec::new(),
        }
    }

    fn sq(x: f64, y: f64, w: f64) -> Polyline {
        vec![
            P::new(x, y),
            P::new(x + w, y),
            P::new(x + w, y + w),
            P::new(x, y + w),
        ]
    }

    fn fill(r: Polyline) -> Geometry {
        Geometry::Fill {
            rings: vec![r],
            rule: FillRule::NonZero,
        }
    }

    fn same_stitches(a: &Design, b: &Design) -> bool {
        a.blocks.len() == b.blocks.len()
            && a.blocks.iter().zip(&b.blocks).all(|(x, y)| {
                x.color == y.color
                    && x.runs.len() == y.runs.len()
                    && x.runs.iter().zip(&y.runs).all(|(r, s)| {
                        r.trim_before == s.trim_before
                            && r.pts.len() == s.pts.len()
                            && r.pts.iter().zip(&s.pts).all(|(p, q)| p.dist(*q) < 1e-9)
                    })
            })
    }

    /// Overlap removal is cached per shape: after moving one object, the
    /// cached plan must still equal a plan made from scratch (what moved
    /// and what it overlapped are redone; the rest is reused).
    #[test]
    fn cached_overlap_removal_matches_a_fresh_plan_after_a_move() {
        let grid = |dx: f64| -> Vec<Shape> {
            let mut shapes = Vec::new();
            for k in 0..40 {
                let (x, y) = ((k % 8) as f64 * 90.0, (k / 8) as f64 * 90.0);
                let c = [(k * 37 % 256) as u8, 60, 120];
                shapes.push(shape(fill(sq(x, y, 100.0)), Kind::Fill, c));
            }
            // One on top of the first row, to be moved along it.
            shapes.push(shape(
                fill(sq(40.0 + dx, 40.0, 120.0)),
                Kind::Fill,
                [200, 10, 10],
            ));
            shapes
        };
        let opt = PlanOptions {
            remove_overlaps: true,
            ..PlanOptions::default()
        };
        let mut cache = PlanCache::default();
        let first = plan_cached("t", &grid(0.0), &opt, &mut cache);
        assert!(same_stitches(&first, &plan("t", &grid(0.0), &opt)));
        let moved = grid(150.0);
        let t = std::time::Instant::now();
        let cached = plan_cached("t", &moved, &opt, &mut cache);
        let warm = t.elapsed();
        let t = std::time::Instant::now();
        let fresh = plan("t", &moved, &opt);
        let cold = t.elapsed();
        assert!(same_stitches(&cached, &fresh), "the cached plan differs");
        eprintln!("warm replan {warm:?}, cold {cold:?}");
        // And once nothing has changed, nothing is redone.
        let again = plan_cached("t", &moved, &opt, &mut cache);
        assert!(same_stitches(&again, &fresh));
    }

    #[test]
    fn grouping_colors_keeps_layers() {
        let red = [200, 0, 0];
        let blue = [0, 0, 200];
        // red A, blue B (on top of A), red C (apart), blue D (apart).
        let a = shape(fill(sq(0.0, 0.0, 100.0)), Kind::Fill, red);
        let b = shape(fill(sq(50.0, 50.0, 100.0)), Kind::Fill, blue);
        let c = shape(fill(sq(500.0, 0.0, 50.0)), Kind::Fill, red);
        let d = shape(fill(sq(800.0, 0.0, 50.0)), Kind::Fill, blue);
        let order = grouped_by_color(&[a.clone(), b.clone(), c.clone(), d.clone()]);
        assert_eq!(order, vec![0, 2, 1, 3]);
        // C on top of B must stay after it.
        let c2 = shape(fill(sq(120.0, 120.0, 50.0)), Kind::Fill, red);
        assert_eq!(grouped_by_color(&[a, b, c2, d]), vec![0, 1, 3, 2]);
    }

    #[test]
    fn contour_fill_rows_follow_the_outline_inside() {
        let s = shape(fill(sq(0.0, 0.0, 200.0)), Kind::Contour, [0, 0, 0]);
        let d = plan("t", &[s], &PlanOptions::default());
        let pts: Vec<P> = d
            .blocks
            .iter()
            .flat_map(|b| &b.runs)
            .flat_map(|r| r.pts.iter().copied())
            .collect();
        assert!(
            pts.iter()
                .all(|p| p.x >= -0.5 && p.x <= 200.5 && p.y >= -0.5 && p.y <= 200.5)
        );
        // ~0.45 mm rows across a 20 mm square: about 22 concentric rings, sewn in one piece.
        let near_center = pts
            .iter()
            .filter(|p| p.dist(P::new(100.0, 100.0)) < 15.0)
            .count();
        assert!(near_center > 0, "rows reach the middle");
        assert_eq!(d.trim_count(), 0, "rings connect without trims");
        assert!(d.stitch_count() > 300, "{}", d.stitch_count());
    }

    #[test]
    fn ring_walks_follow_the_ring_both_ways_round() {
        let n = 36;
        let ring: Polyline = (0..=n)
            .map(|k| {
                let a = k as f64 / n as f64 * std::f64::consts::TAU;
                P::new(100.0 + 100.0 * a.cos(), 100.0 + 100.0 * a.sin())
            })
            .collect();
        let total = crate::geom::polyline_length(&ring);
        let edge = ring[0].dist(ring[1]);
        for (a, b) in [
            (0.1 * total, 0.4 * total),
            (0.4 * total, 0.1 * total),
            (0.95 * total, 0.05 * total),
            (0.05 * total, 0.95 * total),
            (0.5 * total, 0.5 * total + 1.0),
        ] {
            for forward in [true, false] {
                let w = ring_walk(&ring, a, b, forward);
                for q in w.windows(2) {
                    assert!(
                        q[0].dist(q[1]) <= edge + 1e-6,
                        "{a} -> {b} forward={forward}: jump {:?} -> {:?}",
                        q[0],
                        q[1]
                    );
                }
                let want = if forward {
                    (b - a).rem_euclid(total)
                } else {
                    (a - b).rem_euclid(total)
                };
                let len = crate::geom::polyline_length(&w);
                assert!(
                    (len - want).abs() < 1.0,
                    "{a} -> {b} forward={forward}: {len} vs {want}"
                );
            }
        }
    }

    #[test]
    fn spiral_fill_winds_out_from_the_center_inside_the_shape() {
        let mut s = shape(fill(sq(0.0, 0.0, 200.0)), Kind::Contour, [0, 0, 0]);
        s.params.spiral = Some(true);
        let d = plan("t", &[s], &PlanOptions::default());
        let pts: Vec<P> = d
            .blocks
            .iter()
            .flat_map(|b| &b.runs)
            .flat_map(|r| r.pts.iter().copied())
            .collect();
        assert!(pts.len() > 300, "{}", pts.len());
        assert!(
            pts.iter()
                .all(|p| p.x >= -0.5 && p.x <= 200.5 && p.y >= -0.5 && p.y <= 200.5)
        );
        assert!(
            pts[0].dist(P::new(100.0, 100.0)) < 5.0,
            "starts at the center: {:?}",
            pts[0]
        );
        // Winds out: distance from the center grows along the stitches.
        let d0 = pts[20].dist(P::new(100.0, 100.0));
        let d1 = pts[pts.len() / 2].dist(P::new(100.0, 100.0));
        assert!(d1 > d0 + 20.0, "{d0} {d1}");
        // Inside a square, the spiral is clipped at the corners into a few
        // pieces joined by travel, not cuts.
        assert_eq!(d.trim_count(), 0);
    }

    #[test]
    fn decorative_fills_stay_inside_and_tartan_splits_into_threads() {
        for kind in [Kind::Meander, Kind::Ripple, Kind::Cross] {
            let s = shape(fill(sq(0.0, 0.0, 200.0)), kind, [0, 0, 0]);
            let d = plan("t", &[s], &PlanOptions::default());
            let pts: Vec<P> = d
                .blocks
                .iter()
                .flat_map(|b| &b.runs)
                .flat_map(|r| r.pts.iter().copied())
                .collect();
            assert!(pts.len() > 50, "{kind:?}: {}", pts.len());
            assert!(
                pts.iter()
                    .all(|p| p.x >= -0.5 && p.x <= 200.5 && p.y >= -0.5 && p.y <= 200.5),
                "{kind:?} leaves the shape"
            );
            assert_eq!(d.blocks.len(), 1);
            // A tatami fill of the same square is denser than any of these.
            let t = plan(
                "t",
                &[shape(fill(sq(0.0, 0.0, 200.0)), Kind::Fill, [0, 0, 0])],
                &PlanOptions::default(),
            );
            assert!(
                d.stitch_count() < t.stitch_count(),
                "{kind:?} is lighter than tatami"
            );
        }
        let mut s = shape(fill(sq(0.0, 0.0, 300.0)), Kind::Tartan, [200, 0, 0]);
        s.params.sett = Some(vec![([200, 0, 0], 4.0), ([0, 0, 200], 2.0)]);
        let d = plan("t", &[s], &PlanOptions::default());
        assert_eq!(d.blocks.len(), 2, "one block per stripe colour");
        assert_eq!(d.blocks[0].color, [200, 0, 0]);
        assert_eq!(d.blocks[1].color, [0, 0, 200]);
        assert_eq!(d.block_shapes, vec![vec![0], vec![0]]);
    }

    #[test]
    fn multicolor_satin_and_styles() {
        // A 4 mm wide bar as a satin column.
        let bar = fill(vec![
            P::new(0.0, 0.0),
            P::new(300.0, 0.0),
            P::new(300.0, 40.0),
            P::new(0.0, 40.0),
        ]);
        let mut s = shape(bar.clone(), Kind::Satin, [0, 0, 0]);
        let plain = plan("t", &[s.clone()], &PlanOptions::default());
        assert_eq!(plain.blocks.len(), 1);
        s.params.colors = Some(vec![[255, 0, 0], [0, 0, 255]]);
        let d = plan("t", &[s.clone()], &PlanOptions::default());
        assert_eq!(d.blocks.len(), 2, "two threads across the column");
        assert_eq!(d.blocks[0].color, [255, 0, 0]);
        // The red strip is the top half (y in 0..20), the blue the bottom.
        let red_y: Vec<f64> = d.blocks[0]
            .runs
            .iter()
            .flat_map(|r| r.pts.iter().map(|p| p.y))
            .collect();
        let blue_y: Vec<f64> = d.blocks[1]
            .runs
            .iter()
            .flat_map(|r| r.pts.iter().map(|p| p.y))
            .collect();
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        assert!(
            (mean(&red_y) - mean(&blue_y)).abs() > 12.0,
            "{} {}",
            mean(&red_y),
            mean(&blue_y)
        );
        // Zigzag is lighter than plain; S-stitch has more points.
        let mut z = shape(bar.clone(), Kind::Satin, [0, 0, 0]);
        z.params.style = Some(crate::stitch::satin::SatinStyle::Zigzag);
        let mut ss = shape(bar, Kind::Satin, [0, 0, 0]);
        ss.params.style = Some(crate::stitch::satin::SatinStyle::SStitch);
        let (zp, sp) = (
            plan("t", &[z], &PlanOptions::default()),
            plan("t", &[ss], &PlanOptions::default()),
        );
        assert!(
            zp.stitch_count() < plain.stitch_count() / 2,
            "{} vs {}",
            zp.stitch_count(),
            plain.stitch_count()
        );
        assert!(
            sp.stitch_count() > plain.stitch_count(),
            "{} vs {}",
            sp.stitch_count(),
            plain.stitch_count()
        );
    }

    #[test]
    fn blanket_prongs_point_into_the_shape() {
        let s = shape(fill(sq(0.0, 0.0, 200.0)), Kind::Blanket, [0, 0, 0]);
        let d = plan("t", &[s], &PlanOptions::default());
        let pts: Vec<P> = d.blocks[0]
            .runs
            .iter()
            .flat_map(|r| r.pts.iter().copied())
            .collect();
        let inner = pts
            .iter()
            .filter(|p| p.x > 10.0 && p.x < 190.0 && p.y > 10.0 && p.y < 190.0)
            .count();
        assert!(inner > 20, "prong tips land 2.5 mm inside: {inner}");
        assert!(
            pts.iter()
                .all(|p| p.x > -1.0 && p.y > -1.0 && p.x < 201.0 && p.y < 201.0),
            "none point outward"
        );
        // Reversed ring direction gives the same inward prongs.
        let mut ring = sq(0.0, 0.0, 200.0);
        ring.reverse();
        let d2 = plan(
            "t",
            &[shape(fill(ring), Kind::Blanket, [0, 0, 0])],
            &PlanOptions::default(),
        );
        assert!(
            d2.blocks[0]
                .runs
                .iter()
                .flat_map(|r| &r.pts)
                .all(|p| p.x > -1.0 && p.y > -1.0 && p.x < 201.0 && p.y < 201.0)
        );
    }

    #[test]
    fn applique_stops_twice_in_the_same_thread() {
        let s = shape(fill(sq(0.0, 0.0, 300.0)), Kind::Applique, [10, 20, 30]);
        let d = plan("t", &[s], &PlanOptions::default());
        assert_eq!(d.blocks.len(), 3, "placement | tack-down | cover");
        assert!(d.blocks.iter().all(|b| b.color == [10, 20, 30]));
        let n: Vec<usize> = d
            .blocks
            .iter()
            .map(|b| b.runs.iter().map(|r| r.pts.len()).sum())
            .collect();
        assert!(
            n[0] < n[1] && n[1] < n[2],
            "placement is lightest, cover heaviest: {n:?}"
        );
        let pes = crate::formats::pes::write_pes(&d);
        let back = crate::formats::read::read_stitch_file(&pes, "pes").unwrap();
        assert_eq!(back.blocks.len(), 3, "machine stops survive export");
    }

    #[test]
    fn start_and_end_points_place_the_first_and_last_stitch() {
        // A 20 mm square: start at the top-left corner (t = 0), end at the
        // bottom-right (t = 0.5).
        let mut sh = shape(fill(sq(0.0, 0.0, 200.0)), Kind::Fill, [0; 3]);
        let plain = plan("p", &[sh.clone()], &PlanOptions::default());
        sh.params.start = Some(0.0);
        sh.params.end = Some(0.5);
        let d = plan("p", &[sh.clone()], &PlanOptions::default());
        let first = d.blocks[0].runs[0].pts[0];
        let last = *d.blocks[0].runs.last().unwrap().pts.last().unwrap();
        assert!(
            first.dist(P::new(0.0, 0.0)) < 15.0,
            "starts near the corner: {first:?}"
        );
        assert!(
            last.dist(P::new(200.0, 200.0)) < 2.0,
            "ends at the corner: {last:?}"
        );
        assert_eq!(outline_at(&sh.geometry, 0.5), Some(P::new(200.0, 200.0)));
        assert!((outline_param(&sh.geometry, P::new(205.0, 205.0)).unwrap() - 0.5).abs() < 1e-6);
        // The other way round: the same rows, sewn from the other corner.
        sh.params.start = Some(0.5);
        sh.params.end = Some(0.0);
        let d = plan("p", &[sh.clone()], &PlanOptions::default());
        let first = d.blocks[0].runs[0].pts[0];
        assert!(first.dist(P::new(200.0, 200.0)) < 15.0, "{first:?}");
        assert!(d.stitch_count() >= plain.stitch_count());
    }

    /// The piece kept back to end on stays kept back while the others are
    /// sewn (taking one out of the list moves the last one into its place).
    #[test]
    fn the_end_point_piece_is_sewn_last_among_several() {
        // A 30 mm square with a hole: rows above, either side of and below
        // the hole. The rows below finish at the bottom-left corner, so
        // ending there, they go last.
        let mut sh = shape(
            Geometry::Fill {
                rings: vec![sq(0.0, 0.0, 300.0), sq(100.0, 100.0, 100.0)],
                rule: FillRule::EvenOdd,
            },
            Kind::Fill,
            [0; 3],
        );
        let corner = P::new(0.0, 300.0);
        sh.params.end = outline_param(&sh.geometry, corner);
        let d = plan("p", &[sh], &PlanOptions::default());
        let pts: Vec<P> = d.blocks[0]
            .runs
            .iter()
            .flat_map(|r| r.pts.clone())
            .collect();
        assert!(pts.last().unwrap().dist(corner) < 2.0, "ends at the corner");
        let below = pts.iter().position(|q| q.y > 230.0).expect("rows below");
        assert!(
            pts[below..].iter().all(|q| q.y > 195.0),
            "once below the hole, it stays there to the end"
        );
    }

    #[test]
    fn a_subpath_without_length_is_left_out_not_fatal() {
        let dot = (vec![P::new(50.0, 50.0), P::new(50.0, 50.0)], false);
        let line = (vec![P::new(0.0, 0.0), P::new(200.0, 0.0)], false);
        for kind in [Kind::Satin, Kind::Blanket, Kind::Run] {
            let both = shape(
                Geometry::Stroke {
                    lines: vec![dot.clone(), line.clone()],
                    width: 30.0,
                },
                kind,
                [0; 3],
            );
            let only_dot = shape(
                Geometry::Stroke {
                    lines: vec![dot.clone()],
                    width: 30.0,
                },
                kind,
                [0; 3],
            );
            let shapes = [both, only_dot];
            let opt = PlanOptions::default();
            let d = plan("p", &shapes, &opt);
            assert!(d.notes.is_empty(), "{kind:?}: {:?}", d.notes);
            assert!(d.per_shape[0] > 0, "{kind:?}: the line is sewn");
            // The checks read the same rails.
            crate::checks::check(&shapes, &d, &opt, None);
        }
    }

    #[test]
    fn a_zero_spacing_or_length_is_floored_not_endless() {
        for kind in [Kind::Fill, Kind::Satin, Kind::Run, Kind::Contour] {
            let mut sh = shape(fill(sq(-20.0, -20.0, 40.0)), kind, [0; 3]);
            sh.params.spacing = Some(0.0);
            sh.params.len = Some(0.0);
            let d = plan("p", &[sh], &PlanOptions::default());
            assert!(d.stitch_count() < 200_000, "{kind:?}");
        }
    }

    #[test]
    fn a_new_thread_color_replans_a_tartan_from_the_cache() {
        let mut sh = shape(fill(sq(0.0, 0.0, 200.0)), Kind::Tartan, [200, 30, 30]);
        let opt = PlanOptions::default();
        let mut cache = PlanCache::default();
        plan_cached("p", &[sh.clone()], &opt, &mut cache);
        sh.color = [30, 30, 200];
        let cached = plan_cached("p", &[sh.clone()], &opt, &mut cache);
        assert!(same_stitches(&cached, &plan("p", &[sh], &opt)));
    }

    #[test]
    fn trim_stop_and_connect_between_objects() {
        let a = shape(fill(sq(0.0, 0.0, 100.0)), Kind::Fill, [0; 3]);
        let b = shape(fill(sq(130.0, 0.0, 100.0)), Kind::Fill, [0; 3]);
        // 3 mm apart: cut by default.
        let d = plan("p", &[a.clone(), b.clone()], &PlanOptions::default());
        assert_eq!(d.blocks.len(), 1);
        assert_eq!(d.trim_count(), 1);
        // Connect gaps up to 15 mm: sewn across, no cut.
        let opt = PlanOptions {
            connect: 150.0,
            ..PlanOptions::default()
        };
        let d = plan("p", &[a.clone(), b.clone()], &opt);
        assert_eq!(d.trim_count(), 0, "sewn across the gap");
        assert_eq!(d.blocks[0].runs.len(), 1);
        // Trim after A cuts even though they'd connect.
        let mut at = a.clone();
        at.params.trim = Some(true);
        let d = plan("p", &[at, b.clone()], &opt);
        assert_eq!(d.trim_count(), 1);
        // Stop after A: a second block in the same thread.
        let mut ast = a.clone();
        ast.params.stop = Some(true);
        let d = plan("p", &[ast, b.clone()], &PlanOptions::default());
        assert_eq!(d.blocks.len(), 2);
        assert_eq!(d.blocks[0].color, d.blocks[1].color);
        assert_eq!(d.block_shapes, vec![vec![0], vec![1]]);
    }

    #[test]
    fn basting_box_goes_round_the_design_first() {
        let a = shape(fill(sq(0.0, 0.0, 200.0)), Kind::Fill, [0; 3]);
        let b = shape(fill(sq(300.0, 100.0, 100.0)), Kind::Fill, [200, 0, 0]);
        let plain = plan("p", &[a.clone(), b.clone()], &PlanOptions::default());
        assert_eq!(plain.basting_stitches, 0);
        let opt = PlanOptions {
            basting: Some(50.0),
            ..PlanOptions::default()
        };
        let d = plan("p", &[a, b], &opt);
        // The same blocks and objects' stitches, plus one run at the start.
        assert_eq!(d.blocks.len(), plain.blocks.len());
        assert_eq!(d.per_shape, plain.per_shape);
        assert_eq!(d.block_shapes, plain.block_shapes);
        assert_eq!(d.blocks[0].runs.len(), plain.blocks[0].runs.len() + 1);
        let box_run = &d.blocks[0].runs[0];
        assert_eq!(d.basting_stitches, box_run.pts.len());
        assert_eq!(d.stitch_count(), plain.stitch_count() + box_run.pts.len());
        assert!(!box_run.trim_before);
        assert!(d.blocks[0].runs[1].trim_before, "cut before the design");
        // 5 mm outside the stitches all round, closed, in long stitches.
        let pb = plain.bounds();
        let mut bb = Bounds::empty();
        box_run.pts.iter().for_each(|&p| bb.add(p));
        assert!((bb.min.x - (pb.min.x - 50.0)).abs() < 1e-6);
        assert!((bb.max.y - (pb.max.y + 50.0)).abs() < 1e-6);
        assert!(box_run.pts[0].dist(*box_run.pts.last().unwrap()) < 1e-6);
        let longest = box_run
            .pts
            .windows(2)
            .map(|w| w[0].dist(w[1]))
            .fold(0.0, f64::max);
        assert!(longest <= 60.0 + 1e-6 && longest > 40.0, "{longest}");
        // Nothing to baste round: no box.
        let none = plan("p", &[], &opt);
        assert!(none.blocks.is_empty());
        assert_eq!(none.basting_stitches, 0);
    }

    #[test]
    fn knockdown_is_a_light_lattice_sewn_in_full() {
        let under = shape(fill(sq(0.0, 0.0, 400.0)), Kind::Knockdown, [9; 3]);
        let top = shape(fill(sq(50.0, 50.0, 300.0)), Kind::Fill, [0; 3]);
        // On its own, and under a fill that covers most of it: the same
        // stitches, because a knockdown is never cut away.
        let alone = plan("p", std::slice::from_ref(&under), &PlanOptions::default());
        let covered = plan("p", &[under.clone(), top.clone()], &PlanOptions::default());
        assert_eq!(alone.per_shape[0], covered.per_shape[0]);
        assert!(alone.per_shape[0] > 50);
        assert!(!covered.covered[0]);
        // Far lighter than a fill of the same square.
        let as_fill = shape(fill(sq(0.0, 0.0, 400.0)), Kind::Fill, [9; 3]);
        let solid = plan("p", &[as_fill], &PlanOptions::default());
        assert!(
            alone.per_shape[0] * 3 < solid.per_shape[0],
            "{} vs {}",
            alone.per_shape[0],
            solid.per_shape[0]
        );
        // Two sets of rows, across each other: stitches run both ways.
        let dirs: Vec<f64> = alone
            .blocks
            .iter()
            .flat_map(|b| &b.runs)
            .flat_map(|r| r.pts.windows(2))
            .filter(|w| w[0].dist(w[1]) > 20.0)
            .map(|w| {
                let v = w[1] - w[0];
                (v.y.atan2(v.x).to_degrees() + 180.0) % 180.0
            })
            .collect();
        let near = |a: f64| dirs.iter().filter(|d| (**d - a).abs() < 3.0).count();
        assert!(
            near(45.0) > 10 && near(135.0) > 10,
            "{} {}",
            near(45.0),
            near(135.0)
        );
        // Wider spacing, fewer stitches; and it doesn't cut what's under it.
        let mut wide = under.clone();
        wide.params.spacing = Some(4.0);
        let w = plan("p", &[wide], &PlanOptions::default());
        assert!(w.per_shape[0] < alone.per_shape[0]);
        let d = plan("p", &[top, under], &PlanOptions::default());
        assert!(!d.covered[0] && d.per_shape[0] > 0);
    }

    #[test]
    fn thread_estimates_follow_the_stitches() {
        // 10 stitches of 4 mm: 40 mm of path plus 1 mm each through the fabric.
        let line: Polyline = (0..=10).map(|k| P::new(k as f64 * 40.0, 0.0)).collect();
        assert!((thread_mm(&line) - 50.0).abs() < 1e-9);
        assert_eq!(thread_mm(&[]), 0.0);
        assert_eq!(thread_mm(&[P::new(1.0, 1.0)]), 0.0);
        let red = [200, 0, 0];
        let a = shape(fill(sq(0.0, 0.0, 200.0)), Kind::Fill, [0; 3]);
        let b = shape(fill(sq(300.0, 0.0, 100.0)), Kind::Fill, red);
        let c = shape(fill(sq(500.0, 0.0, 200.0)), Kind::Fill, [0; 3]);
        let d = plan("p", &[a, b, c], &PlanOptions::default());
        assert_eq!(d.blocks.len(), 3);
        let per: Vec<f64> = (0..3).map(|k| d.block_thread_mm(k)).collect();
        assert!(per.iter().all(|m| *m > 0.0));
        assert!((d.thread_mm() - per.iter().sum::<f64>()).abs() < 1e-6);
        assert!((d.bobbin_mm() - d.thread_mm() * BOBBIN_SHARE).abs() < 1e-9);
        // The two black blocks are one thread to have on the spool.
        let by = d.thread_by_color();
        assert_eq!(by.len(), 2);
        assert_eq!(by[0].0, [0; 3]);
        assert!((by[0].1 - (per[0] + per[2])).abs() < 1e-6);
        assert!((by[1].1 - per[1]).abs() < 1e-6);
        // In the range thread makers quote: 4 to 7 m per 1,000 stitches.
        let per_thousand = d.thread_mm() / d.stitch_count() as f64;
        assert!((3.0..=7.0).contains(&per_thousand), "{per_thousand}");
        assert_eq!(d.block_thread_mm(9), 0.0);
        assert_eq!(fmt_thread(12_440.0), "12.4 m");
        assert_eq!(fmt_thread(850.0), "85 cm");
        assert_eq!(fmt_thread(999.0), "1.0 m");
        assert_eq!(fmt_thread(0.0), "0 cm");
    }

    #[test]
    fn every_block_says_why_the_machine_halts_before_it() {
        let red = [200, 0, 0];
        let mut a = shape(fill(sq(0.0, 0.0, 100.0)), Kind::Fill, [0; 3]);
        a.params.stop = Some(true);
        a.params.stop_note = Some("Lay the foam".into());
        let b = shape(fill(sq(130.0, 0.0, 100.0)), Kind::Fill, [0; 3]);
        let mut c = shape(fill(sq(260.0, 0.0, 100.0)), Kind::Fill, red);
        c.params.stop = Some(true);
        let e = shape(fill(sq(390.0, 0.0, 100.0)), Kind::Fill, [0; 3]);
        let d = plan("p", &[a, b, c, e], &PlanOptions::default());
        assert_eq!(d.blocks.len(), 4);
        assert_eq!(d.starts.len(), 4);
        // The first block: the first thread, nothing to do before it.
        assert_eq!(
            d.block_start(0),
            BlockStart {
                thread_change: true,
                ..BlockStart::default()
            }
        );
        // After A: a pause in the same thread, with its note.
        let s = d.block_start(1);
        assert!(!s.thread_change && s.is_pause());
        assert_eq!(s.stop_after, Some(0));
        assert_eq!(s.note.as_deref(), Some("Lay the foam"));
        // Before C: only a change of thread.
        let s = d.block_start(2);
        assert!(s.thread_change && !s.is_pause());
        // After C: a pause that is also a change of thread (no note).
        let s = d.block_start(3);
        assert!(s.thread_change);
        assert_eq!((s.stop_after, s.note.clone()), (Some(2), None));

        // An appliqué's two pauses name their steps.
        let ap = shape(fill(sq(0.0, 0.0, 300.0)), Kind::Applique, [0; 3]);
        let d = plan("p", &[ap], &PlanOptions::default());
        assert_eq!(d.blocks.len(), 3);
        assert_eq!(d.block_start(1).applique, Some((0, 1)));
        assert_eq!(d.block_start(2).applique, Some((0, 2)));

        // A design that wasn't planned (an opened machine file) still says
        // where the thread changes.
        let mut bare = d.clone();
        bare.starts.clear();
        assert!(bare.block_start(0).thread_change);
        assert!(!bare.block_start(1).thread_change);
        assert!(!bare.block_start(1).is_pause());
    }

    #[test]
    fn travel_follows_the_inside_of_a_fill_instead_of_cutting() {
        // A C shape: the two arms' rows can't be joined by a straight line
        // inside it, but a walk round the inside of the outline joins them.
        let c: Polyline = vec![
            P::new(0.0, 0.0),
            P::new(300.0, 0.0),
            P::new(300.0, 60.0),
            P::new(60.0, 60.0),
            P::new(60.0, 240.0),
            P::new(300.0, 240.0),
            P::new(300.0, 300.0),
            P::new(0.0, 300.0),
        ];
        let mut sh = shape(fill(c.clone()), Kind::Fill, [0; 3]);
        sh.params.angle = Some(90.0);
        let routed = plan(
            "c",
            &[sh.clone()],
            &PlanOptions {
                travel_route: 1000.0,
                ..PlanOptions::default()
            },
        );
        let cut = plan(
            "c",
            &[sh],
            &PlanOptions {
                travel_route: 0.0,
                ..PlanOptions::default()
            },
        );
        assert!(cut.trim_count() > 0, "the arms need a cut without routing");
        assert_eq!(routed.trim_count(), 0, "routed round the inside instead");
        // Every travel stitch stays inside the shape.
        for r in routed.blocks.iter().flat_map(|b| &b.runs) {
            for w in r.pts.windows(2) {
                assert!(
                    crate::geom::segment_inside(
                        std::slice::from_ref(&c),
                        FillRule::NonZero,
                        w[0],
                        w[1],
                        5.0
                    ) || w[0].dist(w[1]) < 1.0,
                    "{:?} -> {:?} leaves the shape",
                    w[0],
                    w[1]
                );
            }
        }
        let route = route_inside(
            std::slice::from_ref(&c),
            FillRule::NonZero,
            P::new(290.0, 30.0),
            P::new(290.0, 270.0),
            1000.0,
        )
        .unwrap();
        assert!(route.len() > 10, "stitched every 3 mm: {}", route.len());
        assert_eq!(*route.last().unwrap(), P::new(290.0, 270.0));
    }

    #[test]
    fn manual_stitches_are_kept_exactly() {
        let pts = vec![
            P::new(0.0, 0.0),
            P::new(17.0, 3.0),
            P::new(5.0, 40.0),
            P::new(60.0, 41.0),
        ];
        let mut s = shape(
            Geometry::Stroke {
                lines: vec![(pts.clone(), false)],
                width: 4.0,
            },
            Kind::Manual,
            [0, 0, 0],
        );
        s.part = Part::Stroke;
        let d = plan("t", &[s], &PlanOptions::default());
        assert_eq!(d.blocks[0].runs[0].pts, pts);
    }

    #[test]
    fn wide_satin_is_auto_split() {
        let line = vec![P::new(0.0, 0.0), P::new(300.0, 0.0)];
        let mut s = shape(
            Geometry::Stroke {
                lines: vec![(line, false)],
                width: 90.0,
            },
            Kind::Satin,
            [0, 0, 0],
        );
        s.part = Part::Stroke;
        s.params.underlay = Some(false);
        let longest = |d: &Design| {
            d.blocks[0]
                .runs
                .iter()
                .flat_map(|r| r.pts.windows(2).map(|w| w[0].dist(w[1])))
                .fold(0.0, f64::max)
        };
        let split = plan("t", std::slice::from_ref(&s), &PlanOptions::default());
        assert!(longest(&split) <= 75.0, "{}", longest(&split));
        s.params.split = Some(0.0);
        let whole = plan("t", &[s], &PlanOptions::default());
        assert!(longest(&whole) > 85.0);
    }

    #[test]
    fn dense_fill_keeps_every_row_start() {
        // 10 mm square, 0.25 mm rows: every row's first and last stitch must survive planning.
        let s = shape(fill(sq(0.0, 0.0, 100.0)), Kind::Fill, [0, 0, 0]);
        let d = plan("t", &[s], &PlanOptions::default());
        let pts: Vec<P> = d.blocks[0]
            .runs
            .iter()
            .flat_map(|r| r.pts.iter().copied())
            .collect();
        let edge_hits = pts
            .iter()
            .filter(|p| p.x.abs() < 1e-6 || (p.x - 100.0).abs() < 1e-6)
            .count();
        assert_eq!(edge_hits, 80, "40 rows × 2 ends");
        assert_eq!(d.per_shape, vec![pts.len()]);
    }

    #[test]
    fn overlap_removal_skips_fill_under_later_objects() {
        let base = shape(fill(sq(0.0, 0.0, 200.0)), Kind::Fill, [0, 0, 80]);
        let top = shape(fill(sq(50.0, 50.0, 100.0)), Kind::Fill, [255, 200, 0]);
        let mut opt = PlanOptions::default();
        let with = plan("t", &[base.clone(), top.clone()], &opt);
        opt.remove_overlaps = false;
        let without = plan("t", &[base, top], &opt);
        let hidden = |d: &Design| {
            d.blocks[0]
                .runs
                .iter()
                .flat_map(|r| &r.pts)
                .filter(|p| p.x > 60.0 && p.x < 140.0 && p.y > 60.0 && p.y < 140.0)
                .count()
        };
        assert!(hidden(&without) > 30, "{}", hidden(&without));
        assert_eq!(
            hidden(&with),
            0,
            "no base stitches deep under the top object"
        );
        // ...but the base still reaches 0.5 mm under its edge, so no gap shows.
        let near_edge = with.blocks[0]
            .runs
            .iter()
            .flat_map(|r| &r.pts)
            .any(|p| p.x > 52.0 && p.x < 56.0 && p.y > 80.0 && p.y < 120.0);
        assert!(near_edge);
    }

    #[test]
    fn hat_order_sews_center_then_out_and_keeps_layering() {
        let left = shape(fill(sq(-300.0, 0.0, 50.0)), Kind::Fill, [1, 1, 1]);
        let center = shape(fill(sq(-25.0, 0.0, 50.0)), Kind::Fill, [1, 1, 1]);
        let under = shape(fill(sq(200.0, 0.0, 80.0)), Kind::Fill, [2, 2, 2]);
        let over = shape(fill(sq(220.0, 20.0, 40.0)), Kind::Fill, [1, 1, 1]);
        let opt = PlanOptions {
            hat_order: true,
            ..PlanOptions::default()
        };
        let d = plan("t", &[left, center, under, over], &opt);
        let pos = |i: usize| d.sew_order.iter().position(|&x| x == i).unwrap();
        assert_eq!(d.sew_order[0], 1, "center first");
        assert!(pos(2) < pos(3), "the object underneath still sews first");
    }

    #[test]
    fn satin_on_filled_bar_is_a_column() {
        let bar = vec![
            P::new(0.0, 0.0),
            P::new(200.0, 0.0),
            P::new(200.0, 30.0),
            P::new(0.0, 30.0),
        ];
        let s = shape(fill(bar), Kind::Satin, [0, 0, 0]);
        let d = plan("t", &[s], &PlanOptions::default());
        assert!(d.notes.is_empty(), "{:?}", d.notes);
        let pts: Vec<P> = d.blocks[0]
            .runs
            .iter()
            .flat_map(|r| r.pts.iter().copied())
            .collect();
        // Satin stitches cross the bar: consecutive points alternate between the long edges.
        let crossings = pts
            .windows(2)
            .filter(|w| (w[0].y - w[1].y).abs() > 25.0)
            .count();
        assert!(crossings > pts.len() / 2, "{crossings} of {}", pts.len());
    }

    #[test]
    fn satin_on_a_blob_falls_back_to_fill_with_a_note() {
        let s = shape(fill(sq(0.0, 0.0, 300.0)), Kind::Satin, [0, 0, 0]);
        let d = plan("t", &[s], &PlanOptions::default());
        assert_eq!(d.notes.len(), 1);
        assert!(d.stitch_count() > 100);
    }
}

#[cfg(test)]
mod concave_fill_tests {
    use super::*;
    use crate::params::Params;
    use crate::svg::Part;

    /// An "H": stems meet a crossbar, so fill rows change length abruptly.
    /// No stitch may cut across the notches above or below the crossbar.
    #[test]
    fn fill_never_cuts_across_notches() {
        let pts = [
            (27.5, 3.0),
            (30.1, 3.0),
            (30.1, 10.6),
            (33.5, 10.6),
            (33.5, 3.0),
            (36.1, 3.0),
            (36.1, 21.0),
            (33.5, 21.0),
            (33.5, 13.2),
            (30.1, 13.2),
            (30.1, 21.0),
            (27.5, 21.0),
        ];
        let ring: Polyline = pts
            .iter()
            .map(|&(x, y)| P::new(x * 10.0, y * 10.0))
            .collect();
        for angle in [0.0, 30.0, 90.0] {
            let s = Shape {
                id: String::new(),
                part: Part::Fill,
                label: String::new(),
                color: [0, 0, 0],
                paint: [0, 0, 0],
                geometry: Geometry::Fill {
                    rings: vec![ring.clone()],
                    rule: FillRule::NonZero,
                },
                params: Params {
                    kind: Some(Kind::Fill),
                    angle: Some(angle),
                    ..Params::default()
                },
                uid: 0,
                groups: Vec::new(),
            };
            let d = plan("t", &[s], &PlanOptions::default());
            for r in d.blocks.iter().flat_map(|b| &b.runs) {
                for w in r.pts.windows(2) {
                    let m = w[0].lerp(w[1], 0.5);
                    let in_notch = m.x > 302.0 && m.x < 334.0 && (m.y < 104.0 || m.y > 134.0);
                    assert!(
                        !in_notch,
                        "angle {angle}: stitch {:?} -> {:?} crosses a notch",
                        w[0], w[1]
                    );
                }
            }
        }
    }
}
