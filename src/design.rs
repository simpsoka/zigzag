//! Turns imported shapes into an ordered, machine-independent stitch plan:
//! color blocks made of continuous runs, with trims between runs.
//!
//! Steps: (1) optionally knock out fill hidden under objects sewn later,
//! (2) choose the sewing order (document order, or cap order: center-out and
//! bottom-up), (3) generate each object's stitches, (4) chain them together,
//! sewing short gaps straight across and trimming long ones.

use crate::boolean;
use crate::geom::{Bounds, FillRule, P, Polyline, dedupe, segment_inside};
use crate::params::{Fabric, Kind, Params, defaults, underlay};
use crate::route::Zone;
use crate::stitch::fill::{FillParams, tatami};
use crate::stitch::satin::{self, Rails, SatinParams};
use crate::stitch::stroke::running;
use crate::svg::{Geometry, Shape};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub mod measure;

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
    /// Input shapes left with nothing to fill: covered by what's sewn on
    /// top (so the checks needn't work the overlaps out again).
    pub covered: Vec<bool>,
    /// Gaps sewn straight across in the open because the document connects
    /// gaps that short (from, to): each would be a cut without it. (Travel
    /// that's out of sight, under a fill or what's sewn on top, isn't here.)
    pub bridges: Vec<(P, P)>,
    /// Gaps sewn across where the thread blends into what it lies on (the
    /// document's Blend), each a cut without it: the way taken, straight
    /// or round by what it's like in color, from where the thread was to
    /// where the next piece begins.
    pub blends: Vec<Vec<P>>,
    /// The ways sewn as drawn by hand (an object's `via`): the input
    /// shape each leads to, and the way, from where the thread was to
    /// where that object begins.
    pub routes: Vec<(usize, Vec<P>)>,
    /// Which input shape each stitch is for: per block and run (as
    /// `blocks`), the places in the run where another shape's stitches
    /// begin, as (index of the first such point, shape index), in order.
    /// Travel to a shape counts as its. Empty for a design that wasn't
    /// planned from shapes (a machine file read back).
    pub stitch_shapes: Vec<Vec<Vec<(usize, usize)>>>,
    /// The stitches that only take the thread somewhere (from one piece
    /// of stitching to the start of the next, without a cut), longer than
    /// a hop: where each is, for checking that none lies over finished
    /// stitching in plain sight ([`crate::checks::exposed_travel`]).
    pub travels: Vec<Travel>,
    /// A satin's columns in the order they're sewn: (input shape, column
    /// by its place among the shape's columns that are columns, as
    /// [`satin_rails`] gives them, whether sewn from its end).
    pub satin_columns: Vec<(usize, usize, bool)>,
}

/// How a stretch of travel gets where it's going.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TravelHow {
    /// Straight across the inside of a fill.
    Across,
    /// Round the inside of a fill's outline.
    Edge,
    /// Wherever it's out of sight: under the object's own stitches still
    /// to come, and under what's sewn later.
    Under,
    /// Straight across open cloth (the document's Connect).
    Open,
    /// Straight across what the thread is close enough in color to that
    /// it doesn't show as a line (the document's Blend): another object's
    /// stitches in a like thread, or cloth of its own color.
    Blend,
    /// The way drawn for it by hand (the object's `via`).
    Routed,
}

/// How much a thread of color `a` stands out lying on color `b`, from 0
/// (can't be told apart) to 1 (as plain as it gets): from how different
/// they look (CIEDE2000), a shade apart counting for a little, a lighter
/// or darker thread of the same color for something under half, and
/// another color altogether in full.
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    /// Nearer than this, a difference isn't seen in thread.
    const SAME: f64 = 3.0;
    /// This far apart it's as plain as it gets (white on black is 100;
    /// a light red on red, or navy on royal blue, 20 to 30).
    const OTHER: f64 = 60.0;
    ((crate::formats::perceptual_distance(a, b) - SAME) / (OTHER - SAME)).clamp(0.0, 1.0)
}

/// What a thread sewn across the open lies on: the objects sewn before
/// (the last first: their bounds, what each covers and its thread), the
/// cloth, and the shape it's being sewn to (whose own stitches are the
/// document's Sew Over's to say, not Blend's).
pub struct Backdrop<'a> {
    pub under: &'a [(Bounds, Vec<Polyline>, FillRule, [u8; 3])],
    pub own: Option<&'a (Vec<Polyline>, FillRule)>,
    pub cloth: [u8; 3],
    pub thread: [u8; 3],
}

impl Backdrop<'_> {
    /// The color under point `q`: the last-sewn object there, or the cloth.
    pub fn color_at(&self, q: P) -> [u8; 3] {
        self.under
            .iter()
            .find(|(b, rings, rule, _)| {
                q.x >= b.min.x
                    && q.x <= b.max.x
                    && q.y >= b.min.y
                    && q.y <= b.max.y
                    && crate::geom::point_inside(rings, *rule, q)
            })
            .map_or(self.cloth, |u| u.3)
    }

    /// How much the thread stands out at `q` (see [`contrast`]), on the
    /// way to `start` (the last few points of it are the shape's own
    /// edge, where it's about to sew).
    pub fn shows_at(&self, q: P, start: P) -> f64 {
        let own = q.dist(start) > 6.0
            && self
                .own
                .is_some_and(|(rings, rule)| crate::geom::point_inside(rings, *rule, q));
        if own {
            1.0
        } else {
            contrast(self.thread, self.color_at(q))
        }
    }

    /// What sewing along `line` (to its last point) spends of a budget of
    /// 1: each stretch as a share of how far it could go there (`reach`,
    /// units, by how much it shows). `None`: it crosses somewhere it
    /// can't go at all.
    pub fn spends(&self, line: &[P], reach: &dyn Fn(f64) -> f64) -> Option<f64> {
        let start = *line.last()?;
        let mut spent = 0.0;
        for w in line.windows(2) {
            let len = w[0].dist(w[1]);
            let n = (len / 5.0).ceil().max(1.0) as usize;
            for k in 0..n {
                let q = w[0].lerp(w[1], (k as f64 + 0.5) / n as f64);
                let r = reach(self.shows_at(q, start));
                if r <= 0.0 {
                    return None;
                }
                spent += len / n as f64 / r;
            }
        }
        Some(spent)
    }

    /// The way from `prev` to `start` that shows least: round by what the
    /// thread is like in color where straight across isn't, no longer
    /// than `most` (units) and spending no more than `budget` (see
    /// [`Backdrop::spends`]). With what it spends.
    pub fn least_seen(
        &self,
        prev: P,
        start: P,
        reach: &dyn Fn(f64) -> f64,
        most: f64,
        budget: f64,
    ) -> Option<(Vec<P>, f64)> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let gap = prev.dist(start);
        if gap > most {
            return None;
        }
        if let Some(c) = self.spends(&[prev, start], reach)
            && c <= budget
        {
            return Some((vec![prev, start], c));
        }
        // A grid of places to go through, from `prev`; fine enough to
        // follow an edge, and no more of it than the length allows.
        let step = (gap / 50.0).clamp(5.0, 12.0);
        let best_reach = reach(0.0).max(1e-9);
        let at = |c: (i32, i32)| prev + P::new(c.0 as f64 * step, c.1 as f64 * step);
        let fixed = |x: f64| (x * 1e7) as u64;
        // (Nodes: grid places, and the end itself.)
        const END: (i32, i32) = (i32::MAX, i32::MAX);
        let place = |c: (i32, i32)| if c == END { start } else { at(c) };
        let mut cost: HashMap<(i32, i32), f64> = HashMap::new();
        let mut from: HashMap<(i32, i32), (i32, i32)> = HashMap::new();
        let mut heap = BinaryHeap::new();
        cost.insert((0, 0), 0.0);
        heap.push(Reverse((fixed(gap / best_reach), 0u64, (0, 0))));
        let mut found = None;
        let mut looked = 0;
        while let Some(Reverse((_, g, c))) = heap.pop() {
            let g = g as f64 / 1e7;
            if cost.get(&c).is_some_and(|&best| g > best + 1e-6) {
                continue;
            }
            if c == END {
                found = Some(g);
                break;
            }
            looked += 1;
            if looked > 20_000 {
                break;
            }
            let p = at(c);
            let mut next: Vec<(i32, i32)> = Vec::with_capacity(9);
            if p.dist(start) <= step * 1.5 {
                next.push(END);
            }
            for (dx, dy) in [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                next.push((c.0 + dx, c.1 + dy));
            }
            for n in next {
                let q = place(n);
                if n != END && prev.dist(q) + q.dist(start) > most {
                    continue;
                }
                // (Each half of the step by how much it shows there: one
                // look at its middle misses a corner it cuts.)
                let len = p.dist(q);
                let (r0, r1) = (
                    reach(self.shows_at(p.lerp(q, 0.25), start)),
                    reach(self.shows_at(p.lerp(q, 0.75), start)),
                );
                if r0 <= 0.0 || r1 <= 0.0 {
                    continue;
                }
                let ng = g + len * 0.5 / r0 + len * 0.5 / r1;
                if ng > budget + 1e-9 || cost.get(&n).is_some_and(|&best| ng >= best - 1e-9) {
                    continue;
                }
                cost.insert(n, ng);
                from.insert(n, c);
                heap.push(Reverse((
                    fixed(ng + q.dist(start) / best_reach),
                    fixed(ng),
                    n,
                )));
            }
        }
        found?;
        let mut line = vec![start];
        let mut c = END;
        while let Some(&p) = from.get(&c) {
            line.push(at(p));
            c = p;
        }
        line.reverse();
        // In as few points as go the same way, if that's still within it.
        let tidy = simplify_line(&line, step * 0.75);
        for l in [tidy, line] {
            if let Some(c) = self.spends(&l, reach)
                && c <= budget + 1e-6
            {
                return Some((l, c));
            }
        }
        None
    }
}

/// A line with the points that don't change where it goes (within `tol`)
/// left out (Douglas–Peucker).
pub fn simplify_line(pts: &[P], tol: f64) -> Vec<P> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let len = a.dist(b);
    let off = |q: P| {
        if len < 1e-9 {
            a.dist(q)
        } else {
            ((b.x - a.x) * (a.y - q.y) - (a.x - q.x) * (b.y - a.y)).abs() / len
        }
    };
    let (k, far) = pts[1..pts.len() - 1]
        .iter()
        .enumerate()
        .map(|(k, &q)| (k + 1, off(q)))
        .fold((0, 0.0), |m, x| if x.1 > m.1 { x } else { m });
    if far <= tol {
        return vec![a, b];
    }
    let mut out = simplify_line(&pts[..=k], tol);
    out.pop();
    out.extend(simplify_line(&pts[k..], tol));
    out
}

/// Running stitches along `line`, from its first point (not given) to
/// just short of its last (the first of what's sewn next).
fn sewn_along(line: &[P]) -> Vec<P> {
    let mut way = Vec::new();
    for w in line.windows(2) {
        let n = (w[0].dist(w[1]) / 30.0).ceil().max(1.0) as usize;
        way.extend((1..=n).map(|k| w[0].lerp(w[1], k as f64 / n as f64)));
    }
    way.pop();
    way
}

/// How far (units) a gap may be sewn across where the thread stands out
/// `w` (see [`contrast`]) from what it lies on: the document's Connect
/// where it shows in full, its Blend where it can't be seen, and
/// evenly in between.
pub fn blend_reach(opt: &PlanOptions, w: f64) -> f64 {
    opt.connect + (opt.blend - opt.connect).max(0.0) * (1.0 - w)
}

/// One stretch of travel in a [`Design`]: `blocks[block].runs[run].pts`
/// from point `from` (where the last piece ended) to point `to` (where
/// the next begins), for input shape `shape`.
#[derive(Clone, Debug, PartialEq)]
pub struct Travel {
    /// Which of the planner's ways this is.
    pub how: TravelHow,
    pub shape: usize,
    pub block: usize,
    pub run: usize,
    pub from: usize,
    pub to: usize,
}

#[derive(Clone, Debug)]
pub struct PlanOptions {
    /// Gaps up to this length (units) are sewn straight across instead of trimmed,
    /// when the straight line stays inside the shape being filled.
    pub travel_inside: f64,
    /// Gaps up to this length (units) between runs are always sewn across.
    pub max_jump_no_trim: f64,
    /// Shortest stitch kept, units: shorter ones are merged into their
    /// neighbours, in each piece and where pieces join. (Fill rows closer
    /// together than this are sewn in passes, so their turns aren't short:
    /// see [`FillParams::min_stitch`].)
    pub min_stitch: f64,
    pub fabric: Fabric,
    /// Don't fill areas that later objects will cover.
    pub remove_overlaps: bool,
    /// How far fills still extend under the covering object, units.
    pub overlap: f64,
    /// Cap order: center-out, bottom-up, fewest color changes.
    pub hat_order: bool,
    /// Gaps up to this long (units), between objects or the separate parts
    /// of one, are sewn across with running stitches rather than cut
    /// (Ink/Stitch's "jump to stroke"), where there's no way round out of
    /// sight. These show: [`Design::bridges`] lists them.
    pub connect: f64,
    /// Gaps up to this long (units) are sewn across where the thread is
    /// close in color to what it would lie on (stitches sewn before, or
    /// the cloth), and less far the more it would stand out, down to
    /// `connect` where it shows in full (see [`blend_reach`]). 0: off.
    pub blend: f64,
    /// The cloth's color, for what a thread in the open lies on.
    pub cloth: [u8; 3],
    /// Where the way from one part of an object to the next can't be out
    /// of sight, sew it over the stitches already there (round the inside
    /// of the fill's edge, or across it) instead of cutting the thread:
    /// fewer cuts, and a line of running stitch on top of the fill.
    pub over_sewn: bool,
    /// Travel stitches that stay out of sight (round the inside of a fill's
    /// outline, through it, or under what's sewn later on top) go up to
    /// this far (units); further than that, the thread is cut.
    pub travel_route: f64,
    /// A satin narrower than this on average (mm) gets a warning from the
    /// checks: how narrow a column still sews cleanly depends on the fabric
    /// and thread, so the document says. (Stitches don't depend on it.)
    pub narrow_satin: f64,
    /// Sew each stretch of objects that follow one another in the same
    /// thread in a short travelling order rather than strictly as listed
    /// (see [`shorten_travel`]). What's sewn on top of what never changes.
    pub short_travel: bool,
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
            over_sewn: false,
            connect: 0.0,
            blend: 0.0,
            cloth: [244, 241, 234],
            travel_route: 500.0,
            narrow_satin: 1.0,
            short_travel: true,
        }
    }
}

/// A run plus an equivalent alternative that starts at the other end.
#[derive(Clone)]
struct Piece {
    pts: Polyline,
    alt: Option<Polyline>,
    /// Closed outline (single pass): may start at any vertex.
    ring: bool,
    /// What's sewn for `pts` and for `alt`, where that's more than they
    /// are: a satin's column with its stitches run on under the column it
    /// ends against (see [`laps`]). The same from its first stitch to its
    /// last, which is all that's planned with.
    sewn: Option<Polyline>,
    alt_sewn: Option<Polyline>,
    /// A satin's column this sews (its place among the shape's columns
    /// that are columns, as [`satin_rails`] gives them), and whether
    /// `pts` sews it from its end.
    column: Option<(usize, bool)>,
}

impl Piece {
    fn reversible(pts: Polyline) -> Piece {
        let mut alt = pts.clone();
        alt.reverse();
        Piece {
            pts,
            alt: Some(alt),
            ring: false,
            sewn: None,
            alt_sewn: None,
            column: None,
        }
    }
    fn fixed(pts: Polyline) -> Piece {
        Piece {
            pts,
            alt: None,
            ring: false,
            sewn: None,
            alt_sewn: None,
            column: None,
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
            sewn: None,
            alt_sewn: None,
            column: None,
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
    /// For a fill in several separate parts: which part each piece of each
    /// group is in, so a part's underlay and top are sewn together (rather
    /// than every part's underlay, then back round them all for the top).
    islands: Vec<Vec<usize>>,
    /// Groups besides the last whose stitches are top stitching too, not
    /// underlay: the rows of each part of a satin that couldn't be
    /// columns and is filled instead (its columns are the last group).
    /// Travel across them shows as it would across the last group's.
    tops: Vec<usize>,
}

impl ShapeRuns {
    fn new(groups: Vec<Vec<Piece>>, area: Option<(Vec<Polyline>, FillRule)>) -> Self {
        ShapeRuns {
            groups,
            area,
            stops: Vec::new(),
            colors: Vec::new(),
            islands: Vec::new(),
            tops: Vec::new(),
        }
    }

    /// Work out [`ShapeRuns::islands`] (left empty when the fill is all
    /// one part, or its groups aren't simply underlay and top).
    fn find_islands(&mut self) {
        self.islands.clear();
        let Some((rings, rule)) = &self.area else {
            return;
        };
        if rings.len() < 2
            || !self.stops.is_empty()
            || !self.colors.is_empty()
            || self.groups.iter().filter(|g| !g.is_empty()).count() < 2
        {
            return;
        }
        let parts = boolean::parts(rings, *rule);
        if parts.len() < 2 {
            return;
        }
        let part_of = |p: P| {
            parts
                .iter()
                .position(|part| crate::geom::point_inside(part, FillRule::EvenOdd, p))
                .unwrap_or_else(|| {
                    // On the outline, or just past it: the nearest part.
                    let near = |part: &Vec<Polyline>| {
                        part.iter()
                            .filter(|r| r.len() >= 2)
                            .map(|r| {
                                let mut closed = r.clone();
                                closed.push(r[0]);
                                nearest_along(&closed, p).0
                            })
                            .fold(f64::INFINITY, f64::min)
                    };
                    (0..parts.len())
                        .min_by(|&a, &b| near(&parts[a]).total_cmp(&near(&parts[b])))
                        .unwrap_or(0)
                })
        };
        self.islands = self
            .groups
            .iter()
            .map(|g| {
                g.iter()
                    .map(|piece| part_of(piece.pts[piece.pts.len() / 2]))
                    .collect()
            })
            .collect();
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

/// The furthest a travel ring can be from where travel along it begins and
/// ends, units.
const RING_HOP: f64 = 15.0;

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
    // The ring is half a millimetre in from the outline, so the hop onto
    // it and off it is short. A long one means the ring nearest isn't this
    // part's (a stroke too thin to have one of its own): that hop would
    // cross open cloth.
    if from.dist(path[0]) > RING_HOP || path.last().unwrap().dist(to) > RING_HOP {
        return None;
    }
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

/// What a shape's stitches cover (none, for a run), once it's been asked.
type Cover = Option<(Vec<Polyline>, FillRule)>;

/// A fill's rows no further apart than this (mm) hide travel under them.
const HIDING_SPACING: f64 = 1.0;

/// Where travel within the shape being sewn is out of sight, as worked
/// out from some point with some of a satin's columns done (they're kept
/// by both: see [`hidden_zone`], which takes in the point): the ground it
/// may run over (`ways`), and what of that covers it once sewn (`cover`:
/// the same, but for a satin's parts that are filled instead, whose rows
/// already sewn nothing covers).
struct Ground {
    ways: Option<Rc<Zone>>,
    cover: Option<Rc<Zone>>,
}

/// Where travel to and within the shape sewn at `order[pos]` is out of
/// sight: under its own fill (`own`, when its stitches hide what's under
/// them; `thin`: its hairlines, as wide as travel needs, see
/// [`satin_thin`]) and under whatever's sewn later on top. `from` is where the
/// needle's coming from: taken in when it's within `reach` (the last
/// object, close by).
#[allow(clippy::too_many_arguments)]
fn hidden_zone(
    shapes: &[Shape],
    order: &[usize],
    pos: usize,
    own: Option<&(Vec<Polyline>, FillRule)>,
    thin: &[&Polyline],
    from: P,
    reach: f64,
    covers: &mut [Option<Cover>],
    shape_bounds: &mut Option<Vec<Bounds>>,
    kept: &Kept,
) -> Option<Rc<Zone>> {
    let bounds = shape_bounds.get_or_insert_with(|| {
        shapes
            .iter()
            .map(|s| geometry_bounds(&s.geometry))
            .collect()
    });
    let mut b = match own {
        Some((rings, _)) => {
            let mut b = Bounds::empty();
            rings.iter().flatten().for_each(|&p| b.add(p));
            b
        }
        None => bounds[order[pos]],
    };
    if b.is_empty() {
        return None;
    }
    if from.x > b.min.x - reach
        && from.x < b.max.x + reach
        && from.y > b.min.y - reach
        && from.y < b.max.y + reach
    {
        b.add(from);
    }
    // Room to go round the outside of a part, under what covers it there.
    let grow = |b: &Bounds, by: f64| Bounds {
        min: P::new(b.min.x - by, b.min.y - by),
        max: P::new(b.max.x + by, b.max.y + by),
    };
    let b = grow(&b, 20.0);
    // (A satin's bounds are its centerline's: its width reaches further.)
    let near = grow(&b, 50.0);
    let mut later: Vec<usize> = Vec::new();
    for &j in &order[pos + 1..] {
        if bounds[j].is_empty() || !overlaps(&near, &bounds[j]) {
            continue;
        }
        if covers[j]
            .get_or_insert_with(|| kept.coverage(&shapes[j]))
            .is_some()
        {
            later.push(j);
        }
    }
    let mut parts: Vec<(&[Polyline], FillRule)> = Vec::new();
    if let Some((rings, rule)) = own {
        parts.push((rings.as_slice(), *rule));
    }
    // (Each on its own: where one lies over a column beside it, that's
    // ground twice over, not a hole.)
    for ring in thin {
        parts.push((std::slice::from_ref(*ring), FillRule::EvenOdd));
    }
    for j in later {
        if let Some(Some((rings, rule))) = &covers[j] {
            parts.push((rings.as_slice(), *rule));
        }
    }
    if parts.is_empty() {
        return None;
    }
    kept.zone(&parts, b)
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

/// A filled shape's rings, the ones that cross themselves or each other
/// told from the rest (see [`untangled`]).
struct Untangled<'a> {
    /// The rings that cross nothing, as they were and in their order.
    plain: std::borrow::Cow<'a, [Polyline]>,
    /// Each group of rings that cross (and the ones inside those): as
    /// they were, and as the outline of what they fill together.
    tangles: Vec<(Vec<Polyline>, Vec<Polyline>)>,
}

/// Rings, each that touches itself at a point as the rings either side of
/// that point, and the points: `None` if none of them does.
///
/// The arm and leg of a k may meet its stem at a point: one outline that
/// goes through that point twice, of a shape that's two, touching. Taken
/// as one, the join between them is a neck of no width, and the cut across
/// it lands wherever the last bit of its coordinates puts it: the same
/// letter comes out clean in one place and leaning in another.
fn unpinched(rings: &[Polyline]) -> Option<(Vec<Polyline>, Vec<P>)> {
    // Where a ring first comes back to a point it has been at (not a
    // point doubled, or a spike out and back: a loop each side of it).
    let pinch = |ring: &[P]| -> Option<(usize, usize)> {
        let n = ring.len();
        let mut seen: HashMap<(i64, i64), usize> = HashMap::with_capacity(n);
        for (j, p) in ring.iter().enumerate() {
            let at = ((p.x * 1e6).round() as i64, (p.y * 1e6).round() as i64);
            match seen.get(&at) {
                Some(&i) if j - i >= 3 && n - (j - i) >= 3 => return Some((i, j)),
                Some(_) => {}
                None => {
                    seen.insert(at, j);
                }
            }
        }
        None
    };
    if rings.iter().all(|r| pinch(r).is_none()) {
        return None;
    }
    let mut out = Vec::with_capacity(rings.len() + 1);
    let mut at = Vec::new();
    let mut todo: Vec<Polyline> = rings.iter().rev().cloned().collect();
    while let Some(ring) = todo.pop() {
        match pinch(&ring) {
            Some((i, j)) => {
                // (Each from the point after the one they share, which
                // is on the other's outline.)
                let mut near: Polyline = ring[i..j].to_vec();
                let mut far: Polyline = ring[j..].iter().chain(&ring[..i]).copied().collect();
                near.rotate_left(1);
                far.rotate_left(1);
                at.push(ring[i]);
                todo.push(far);
                todo.push(near);
            }
            None => out.push(ring),
        }
    }
    Some((out, at))
}

/// A filled shape's rings, with the overlaps found in them: the rings
/// that cross themselves or each other (and the ones inside those), in
/// groups, each with the outline of what it fills.
///
/// A variable font draws a letter in overlapping pieces (a b as a stem
/// laid over a bowl, an e as one outline that crosses itself), which
/// fill as one shape by the nonzero rule but aren't the outline of one:
/// taken for it, a stem is a part of its own and the bowl a ring with a
/// bite out of it. The letters of a script that run into each other
/// overlap as well, but are letters each of its own: which a tangle is,
/// [`satin_rails`] tells by which way it can be sewn.
fn untangled(rings: &[Polyline], rule: FillRule) -> Untangled<'_> {
    use std::borrow::Cow;
    let n = rings.len();
    // Every edge, by the ring it's of, sorted by where it starts across:
    // only edges that overlap across can cross.
    struct Edge {
        ring: usize,
        at: usize,
        a: P,
        b: P,
    }
    let mut edges: Vec<Edge> = Vec::new();
    for (ring, r) in rings.iter().enumerate() {
        let m = r.len();
        if m < 3 {
            continue;
        }
        edges.extend((0..m).map(|at| Edge {
            ring,
            at,
            a: r[at],
            b: r[(at + 1) % m],
        }));
    }
    edges.sort_by(|x, y| x.a.x.min(x.b.x).total_cmp(&y.a.x.min(y.b.x)));
    let side = |a: P, b: P, c: P| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    // The groups of rings that cross (a ring that crosses itself is a
    // group of one).
    let mut group: Vec<usize> = (0..n).collect();
    fn root(group: &mut [usize], mut k: usize) -> usize {
        while group[k] != k {
            group[k] = group[group[k]];
            k = group[k];
        }
        k
    }
    let mut tangled = vec![false; n];
    for (i, e) in edges.iter().enumerate() {
        let reach = e.a.x.max(e.b.x);
        for f in edges[i + 1..]
            .iter()
            .take_while(|f| f.a.x.min(f.b.x) <= reach)
        {
            // (Edges next to each other on a ring meet, and don't cross.)
            let m = rings[e.ring].len();
            if e.ring == f.ring && ((e.at + 1) % m == f.at || (f.at + 1) % m == e.at) {
                continue;
            }
            if e.a.y.max(e.b.y) < f.a.y.min(f.b.y) || f.a.y.max(f.b.y) < e.a.y.min(e.b.y) {
                continue;
            }
            if side(e.a, e.b, f.a) * side(e.a, e.b, f.b) < 0.0
                && side(f.a, f.b, e.a) * side(f.a, f.b, e.b) < 0.0
            {
                let (x, y) = (root(&mut group, e.ring), root(&mut group, f.ring));
                group[x] = y;
                tangled[e.ring] = true;
                tangled[f.ring] = true;
            }
        }
    }
    if !tangled.contains(&true) {
        return Untangled {
            plain: Cow::Borrowed(rings),
            tangles: Vec::new(),
        };
    }
    for k in 0..n {
        if tangled[k] {
            let r = root(&mut group, k);
            tangled[r] = true;
        }
    }
    // A ring inside one of a group's is that group's too (a counter, for
    // the fill rule to make of what it will), and so is one that has one
    // of a group's inside it (a letter's outer outline, round counters
    // drawn as one contour that crosses itself): until there are no more.
    let inside = |j: usize, k: usize| {
        rings[j].len() >= 3
            && rings[k].len() >= 3
            && crate::geom::point_inside(&rings[j..=j], FillRule::NonZero, rings[k][0])
    };
    loop {
        let mut joined = false;
        for k in 0..n {
            for j in 0..n {
                let (own, of) = (root(&mut group, k), root(&mut group, j));
                if own != of && tangled[of] && (inside(j, k) || inside(k, j)) {
                    group[own] = of;
                    joined = true;
                }
            }
        }
        if !joined {
            break;
        }
    }
    let mut plain: Vec<Polyline> = Vec::with_capacity(n);
    let mut tangles = Vec::new();
    let mut done = vec![false; n];
    for k in 0..n {
        let own = root(&mut group, k);
        if !tangled[own] {
            plain.push(rings[k].clone());
        } else if !done[own] {
            done[own] = true;
            let pieces: Vec<Polyline> = (0..n)
                .filter(|&j| root(&mut group, j) == own)
                .map(|j| rings[j].clone())
                .collect();
            let whole = boolean::normalize(&pieces, rule);
            tangles.push((pieces, whole));
        }
    }
    Untangled {
        plain: Cow::Owned(plain),
        tangles,
    }
}

/// The satin rails a shape will be sewn with, or why a part couldn't be.
/// Parts that fail fall back to fill (returned as `Err` with the component).
pub fn satin_rails(shape: &Shape) -> Vec<Result<Rails, (Vec<Polyline>, String)>> {
    satin_rails_sewn(shape)
        .into_iter()
        .map(|r| r.map(|(rails, _)| rails))
        .collect()
}

/// A column, and where its stitches run on past its ends.
type Sewn = (Rails, satin::Laps);

/// [`satin_rails`], and where each column's stitches run on past its
/// ends: one that ends against the side of another is sewn a little way
/// under it (see [`laps`]). The columns themselves, which are what's
/// shown, measured and planned with, end where they meet.
fn satin_rails_sewn(shape: &Shape) -> Vec<Result<Sewn, (Vec<Polyline>, String)>> {
    let alone = |r: Rails| Ok((r, satin::NO_LAPS));
    let p = &shape.params;
    match &shape.geometry {
        Geometry::Stroke { lines, width } => {
            // (Satin columns sewn as one satin: each its own subpaths',
            // lapped where it ends against another as a shape's are, the
            // satin being all of them.)
            if !shape.joined.is_empty() {
                let mut rest = &lines[..];
                let columns: Vec<Rails> = shape
                    .joined
                    .iter()
                    .filter_map(|&n| {
                        let (own, after) = rest.split_at(n.min(rest.len()));
                        rest = after;
                        satin::rails_from_subpaths(own, p.satin_rails())
                    })
                    .collect();
                let outlines: Vec<(Polyline, Bounds)> = columns
                    .iter()
                    .filter(|r| !r.closed)
                    .map(|r| {
                        let ring = r.outline().remove(0);
                        let mut b = Bounds::empty();
                        ring.iter().for_each(|&q| b.add(q));
                        (ring, b)
                    })
                    .collect();
                let inside = |q: P| {
                    outlines.iter().any(|(ring, b)| {
                        q.x >= b.min.x
                            && q.x <= b.max.x
                            && q.y >= b.min.y
                            && q.y <= b.max.y
                            && crate::geom::point_inside(
                                std::slice::from_ref(ring),
                                FillRule::EvenOdd,
                                q,
                            )
                    })
                };
                let laps = laps(&columns, &inside);
                return columns.into_iter().zip(laps).map(Ok).collect();
            }
            let w = p.width.map(mm).unwrap_or(*width);
            // (A shape's outline is sewn along each of its edges: a shape
            // in two parts, or with a hole, isn't a column between them.)
            if !shape.outline
                && let Some(r) = satin::rails_from_subpaths(lines, p.satin_rails())
            {
                return vec![alone(r)];
            }
            // (A subpath without length, a dot drawn as `M p L p`, has no
            // column to sew.)
            lines
                .iter()
                .map(|(l, c)| satin::rails_from_centerline(l, *c, w))
                .filter(|r| r.a.len() > 1)
                .map(alone)
                .collect()
        }
        Geometry::Fill { rings, rule } => {
            let Untangled { plain, tangles } = untangled(rings, *rule);
            if p.is_border() {
                let w = p.width.map(mm).unwrap_or(mm(defaults::OUTLINE_SATIN_WIDTH));
                return plain
                    .iter()
                    .chain(tangles.iter().flat_map(|(_, whole)| whole))
                    .map(|r| satin::rails_from_centerline(r, true, w))
                    .filter(|r| r.a.len() > 1)
                    .map(alone)
                    .collect();
            }
            // Each part as one column if it is one; else as the columns
            // it can be cut into (a stem and a bowl, a bar and a stem); and
            // failing that, filled.
            let how = Strokes::of(p);
            // (The parts, letters, each on its own: on all the computer's
            // cores at once, a line of text being many.)
            let columns = |rings: &[Polyline]| -> Vec<_> {
                in_parallel(satin::components(rings, *rule), |comp| {
                    component_columns(comp, how)
                })
                .into_iter()
                .flatten()
                .collect()
            };
            // How much of what came of some rings is filled, by area.
            let filled = |parts: &[Result<Sewn, (Vec<Polyline>, String)>]| -> f64 {
                parts
                    .iter()
                    .filter_map(|c| c.as_ref().err())
                    .filter_map(|(comp, _)| comp.first())
                    .map(|outer| boolean::ring_area(outer).abs())
                    .sum()
            };
            let mut out = columns(&plain);
            for (pieces, whole) in &tangles {
                // Rings that overlap, as the one shape they fill, if that
                // can be sewn in columns: a letter drawn in pieces is. If
                // it can't, and less of them would be filled taken as
                // they are, as they are: the letters of a script that run
                // into each other, each lapped over the next.
                let together = columns(whole);
                if filled(&together) > 0.0 {
                    let apart = columns(pieces);
                    if filled(&apart) < filled(&together) {
                        out.extend(apart);
                        continue;
                    }
                }
                out.extend(together);
            }
            out
        }
    }
}

/// Two outlines that differ by less than this, a design unit over it, are
/// the same outline to [`component_columns`] (a hundredth of a micron:
/// the rounding of putting one somewhere else is a billionth of that).
const COLUMN_GRAIN: f64 = 1e-4;

/// The columns one part of a filled shape (its outer ring, then its
/// holes) is sewn in, or the part and why it's filled instead.
///
/// Found for the part's shape, not for where it lies: its outline is read
/// from its own first point, the columns found for that, and moved back.
/// So a letter has the same columns wherever it's put, and they're found
/// once (they're kept by that outline): the second l of a word, or a line
/// of text moved across the design, costs nothing more. Finding them is
/// most of the work of planning a satin, and the rails on the canvas and
/// the checks ask for them as well.
///
/// `how`: the way its strokes are read (see [`Strokes`]).
/// `f` of each of `items`, in their order, the work shared among the
/// computer's cores (each takes the next item as it's free). With one
/// item, or one core, just in turn. A panic in `f` is the caller's, as
/// it would be in turn.
fn in_parallel<T: Send, R: Send>(items: Vec<T>, f: impl Fn(T) -> R + Sync) -> Vec<R> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let n = items.len();
    let cores = std::thread::available_parallelism()
        .map_or(1, |c| c.get())
        .min(n);
    if cores <= 1 {
        return items.into_iter().map(f).collect();
    }
    let items: Vec<Mutex<Option<T>>> = items.into_iter().map(|t| Mutex::new(Some(t))).collect();
    let out: Vec<Mutex<Option<R>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    // (A panic is the caller's: as much caught on a worker as it is here,
    // for a panic hook to tell (see [`catching`]), and handed back whole
    // rather than as a thread's having panicked.)
    let caller_catching = CATCHING.with(|c| c.get());
    let panicked: Mutex<Option<Box<dyn std::any::Any + Send>>> = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..cores {
            s.spawn(|| {
                CATCHING.with(|c| c.set(caller_catching));
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let Some(t) = items[i].lock().ok().and_then(|mut t| t.take()) else {
                        continue;
                    };
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(t))) {
                        Ok(r) => {
                            if let Ok(mut slot) = out[i].lock() {
                                *slot = Some(r);
                            }
                        }
                        Err(why) => {
                            if let Ok(mut first) = panicked.lock() {
                                first.get_or_insert(why);
                            }
                            // (No more begun: the rest is the caller's to
                            // drop.)
                            next.store(n, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            });
        }
    });
    if let Some(why) = panicked.into_inner().ok().flatten() {
        std::panic::resume_unwind(why);
    }
    out.into_iter()
        .filter_map(|m| m.into_inner().ok().flatten())
        .collect()
}

fn component_columns(
    comp: Vec<Polyline>,
    how: Strokes,
) -> Vec<Result<Sewn, (Vec<Polyline>, String)>> {
    use std::hash::{Hash, Hasher};
    use std::sync::{Mutex, OnceLock};
    // (In two halves, the older dropped when the newer fills up: what was
    // asked for lately, the letters on the canvas now, is never what goes.)
    type Memo = HashMap<u64, Result<Vec<Sewn>, String>>;
    static MEMO: OnceLock<Mutex<(Memo, Memo)>> = OnceLock::new();
    let Some(&origin) = comp.first().and_then(|r| r.first()) else {
        return Vec::new();
    };
    // (Known by its points to the grain, so that the same outline
    // somewhere else, a few bits of rounding different, is known again.)
    let mut h = hasher();
    how.hash(&mut h);
    let mut corners: HashMap<(i64, i64), P> = HashMap::new();
    let home: Vec<Polyline> = comp
        .iter()
        .map(|ring| {
            ring.len().hash(&mut h);
            ring.iter()
                .map(|&p| {
                    let d = (p - origin) * (1.0 / COLUMN_GRAIN);
                    let at = (d.x.round() as i64, d.y.round() as i64);
                    at.hash(&mut h);
                    corners.insert(at, p);
                    p - origin
                })
                .collect()
        })
        .collect();
    let key = h.finish();
    let memo = MEMO.get_or_init(Default::default);
    let kept = memo.lock().ok().and_then(|mut m| {
        if let Some(hit) = m.0.get(&key) {
            return Some(hit.clone());
        }
        let hit = m.1.remove(&key)?;
        m.0.insert(key, hit.clone());
        Some(hit)
    });
    let found = kept.unwrap_or_else(|| {
        let found = columns_at_home(&home, how).map(|columns| {
            let laps = laps(&columns, &|p| {
                crate::geom::point_inside(&home, FillRule::EvenOdd, p)
            });
            columns.into_iter().zip(laps).collect::<Vec<Sewn>>()
        });
        if let Ok(mut m) = memo.lock() {
            if m.0.len() >= 1024 {
                m.1 = std::mem::take(&mut m.0);
            }
            m.0.insert(key, found.clone());
        }
        found
    });
    match found {
        Ok(columns) => columns
            .into_iter()
            .map(|(mut r, mut laps)| {
                // (A point of the outline is that point again, to the
                // bit, not a rounding away from it.)
                r.a.iter_mut().chain(&mut r.b).for_each(|p| {
                    let d = *p * (1.0 / COLUMN_GRAIN);
                    let corner = corners
                        .get(&(d.x.round() as i64, d.y.round() as i64))
                        .filter(|c| (**c - origin).dist(*p) < 1e-9);
                    *p = corner.copied().unwrap_or(*p + origin);
                });
                laps.iter_mut().flatten().for_each(|row| {
                    *row = (row.0 + origin, row.1 + origin);
                });
                Ok((r, laps))
            })
            .collect(),
        Err(why) => vec![Err((comp, why))],
    }
}

/// [`component_columns`] of a part read from its own first point: each
/// part as one column if it is one; else as the columns it can be cut
/// into (a stem and a bowl, a bar and a stem); and failing that, why not.
fn columns_at_home(comp: &[Polyline], how: Strokes) -> Result<Vec<Rails>, String> {
    // A part that touches itself at a point (see [`unpinched`]) is cut
    // into columns as the parts it's made of, and those sewn as the one
    // letter they are: the thread goes through from one to the next
    // where they touch. (If any of them can't be cut into columns, as
    // the one part after all.)
    if let Some((rings, touching)) = unpinched(comp) {
        let parts = satin::components(&rings, FillRule::EvenOdd);
        let each: Result<Vec<Vec<Rails>>, String> =
            parts.iter().map(|part| part_columns(part, how)).collect();
        if let (true, Ok(each)) = (parts.len() > 1, each) {
            return Ok(through(each.concat(), &touching));
        }
    }
    part_columns(comp, how).map(|columns| through(columns, &[]))
}

/// How a filled satin's strokes are read into columns (its settings).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Strokes {
    /// By its chordal axis (see [`crate::stitch::cat`]): each stroke a
    /// column, corners mitred, joins butted; unless a corner is too sharp
    /// to mitre cleanly or a join can't be read clearly, when it's `Cut`.
    /// The default.
    Auto,
    /// By its chordal axis, whatever its corners (the `mitred` setting).
    Mitred,
    /// Cut where its strokes part (see [`crate::stitch::columns::split`]):
    /// corners squared, one stroke run on and the other ended against it
    /// (the `cut` setting).
    Cut,
    /// Its strokes as whole columns laid over each other where they meet
    /// (the `overlap` setting, no longer offered).
    Overlap,
}

impl Strokes {
    fn of(p: &Params) -> Strokes {
        if p.satin_overlap() {
            Strokes::Overlap
        } else if p.satin_cut() {
            Strokes::Cut
        } else if p.satin_mitred() {
            Strokes::Mitred
        } else {
            Strokes::Auto
        }
    }
}

/// [`columns_at_home`], of a part that's one shape and not two that
/// touch, and before any of its columns is cut in two for the thread.
/// Whether every end of `was` that `now` has moved came to a point:
/// narrower than [`POINTED`] of the column's usual width.
fn only_pointed_ends_moved(was: &Rails, now: &Rails) -> bool {
    let ps = satin::pairs(was, 3.0);
    let mut widths: Vec<f64> = ps.iter().map(|(a, b)| a.dist(*b)).collect();
    widths.sort_by(f64::total_cmp);
    let Some(&usual) = widths.get(widths.len() / 2) else {
        return false;
    };
    let end = |r: &Rails, last: bool| {
        if last {
            (r.a[r.a.len() - 1], r.b[r.b.len() - 1])
        } else {
            (r.a[0], r.b[0])
        }
    };
    [false, true].iter().all(|&last| {
        let ((a, b), (c, d)) = (end(was, last), end(now, last));
        let moved = a.dist(c) > 1e-6 || b.dist(d) > 1e-6;
        !moved || a.dist(b) < POINTED * usual
    })
}

/// A column's end narrower than this share of its usual width comes to
/// a point.
const POINTED: f64 = 0.25;

fn part_columns(comp: &[Polyline], how: Strokes) -> Result<Vec<Rails>, String> {
    // Its strokes laid over each other, when that's asked for and they
    // can be read (as they're found: their ends are their own); or by its
    // chordal axis (the default), where that reads them and they cover it;
    // else cut into columns as below (asked for, or a square, a dot, a
    // shape whose axis is all corners).
    if how == Strokes::Overlap
        && let Some(strokes) = crate::stitch::columns::overlapping(comp)
    {
        return Ok(strokes);
    }
    if matches!(how, Strokes::Auto | Strokes::Mitred)
        && let Some(columns) = crate::stitch::cat::columns(comp, how == Strokes::Auto)
    {
        // (A stroke cut off at a slant, the top of a v, that the axis
        // brings to a point at the cut's sharp corner, the stitches
        // fanned in to it: ended on the cut instead, as below, its
        // stitches turned to meet it. Only an end that came to a point:
        // one already across the stroke, a serif's foot, is left.)
        return Ok(columns
            .into_iter()
            .map(|r| match satin::flat_ends(&r, comp) {
                Some(flat)
                    if crate::stitch::columns::holds(comp, &flat, &r)
                        && only_pointed_ends_moved(&r, &flat) =>
                {
                    flat
                }
                _ => r,
            })
            .collect());
    }

    let one = satin::auto_rails(comp);
    // A ring whose stitches lean (one with corners: a triangle
    // and the triangle cut out of it) is better cut into
    // columns too, where those come out clearly squarer. (Only
    // a ring: a column between two tips is left as it is.)
    let leaning = one
        .as_ref()
        .ok()
        .filter(|r| r.closed)
        .map(|r| satin::lean(std::slice::from_ref(r)))
        .filter(|l| *l > satin::RING_LEAN);
    // So is one taken for a column whose stitches would cross
    // each other or span two strokes (a V, serif to serif
    // round its foot): its strokes, if it can be cut into them.
    let unsound = one
        .as_ref()
        .is_ok_and(|r| !crate::stitch::columns::sound(comp, r));
    let cut = if one.is_err() || leaning.is_some() || unsound {
        crate::stitch::columns::split(comp)
            .filter(|c| leaning.is_none_or(|l| satin::lean(c) < l * 0.7))
    } else {
        None
    };
    // A line through the shape, off its outline: a cut between columns.
    let is_cut = |a: P, b: P| {
        [0.25, 0.5, 0.75].iter().all(|&t| {
            let q = a.lerp(b, t);
            crate::geom::point_inside(comp, FillRule::EvenOdd, q)
                && comp.iter().all(|ring| dist_to_ring(ring, q) > 1.0)
        })
    };
    // Each column's ends where the shape's are, however the
    // column was come by.
    // And its stitches square to it where nothing has said how they
    // cross it (a column found by its corners has rungs only there; one
    // between two tips has none), unless that takes them off the shape.
    // (`cuts`: whether a flat end may be made along a cut between
    // columns too, or only along the shape's own outline.)
    let ended = |r: Rails, cuts: bool| {
        let on_cut = |flat: &Rails| {
            [true, false].iter().any(|&last| {
                let end = |x: &Rails| {
                    if last {
                        (x.a[x.a.len() - 1], x.b[x.b.len() - 1])
                    } else {
                        (x.a[0], x.b[0])
                    }
                };
                let ((fa, fb), (ra, rb)) = (end(flat), end(&r));
                (fa.dist(ra) > 1e-6 || fb.dist(rb) > 1e-6) && is_cut(fa, fb)
            })
        };
        let r = match satin::flat_ends(&r, comp) {
            Some(flat)
                if crate::stitch::columns::holds(comp, &flat, &r) && (cuts || !on_cut(&flat)) =>
            {
                flat
            }
            _ => r,
        };
        let square = satin::squared(&r);
        if crate::stitch::columns::astray(comp, &square) > crate::stitch::columns::astray(comp, &r)
        {
            r
        } else {
            square
        }
    };
    // A column that ends on a steep cut against another (two diagonals
    // cut apart on a mitre, an arm against a stem at a sharp angle) comes
    // to a point there, the cut its side (see [`satin::pointed`]); and
    // one that comes to a point against another already isn't given a
    // flat end along the cut. Each only where it leaves the column's
    // stitches squarer to the shape's own edge, and none of them off the
    // shape: not where the cut is long (a bowl leaving its stem all but
    // along it), and the wedge it makes is sewn no squarer than the end
    // was.
    let finished = |r: Rails| -> Rails {
        // (Not closed up on itself at an end first: see
        // [`satin::unfolded`].)
        let r = satin::unfolded(&r);
        if r.closed || r.a.len() < 2 || r.b.len() < 2 {
            return ended(r, true);
        }
        let ends = [(r.a[r.a.len() - 1], r.b[r.b.len() - 1]), (r.a[0], r.b[0])];
        let score = |x: &Rails| {
            (
                crate::stitch::columns::astray(comp, x),
                crate::stitch::columns::off_square(comp, x),
            )
        };
        let better = |now: &Rails, was: &Rails| {
            let (now, was) = (score(now), score(was));
            now.0 <= was.0 && now.1 < was.1
        };
        let mut best = ended(r.clone(), true);
        let plain = ended(r.clone(), false);
        if better(&plain, &best) {
            best = plain;
        }
        let mut raw = r;
        for (ea, eb) in ends {
            let this = |a: P, b: P| a.dist(ea) < 1e-9 && b.dist(eb) < 1e-9 && is_cut(a, b);
            let point = satin::pointed(&raw, satin::POINT_SHEAR, this);
            if point.a.len() == raw.a.len() && point.b.len() == raw.b.len() {
                continue;
            }
            let pointed = ended(point.clone(), false);
            if better(&pointed, &best) {
                (raw, best) = (point, pointed);
            }
        }
        best
    };
    match (cut, one) {
        (Some(columns), _) => Ok(columns.into_iter().map(finished).collect()),
        (None, Ok(rails)) => Ok(vec![ended(rails, true)]),
        (None, Err(e)) => {
            // "Too wide" as one column may only mean it
            // isn't one: say so only of a shape that's
            // wide whichever way it's sewn (its area over
            // half its outline's length is how thick it
            // is, as a stroke).
            let area: f64 = comp
                .iter()
                .enumerate()
                .map(|(k, r)| {
                    let a = boolean::ring_area(r).abs();
                    if k == 0 { a } else { -a }
                })
                .sum();
            let around: f64 = comp
                .iter()
                .map(|r| crate::geom::polyline_length(r) + r.last().map_or(0.0, |l| l.dist(r[0])))
                .sum();
            let thick = 2.0 * area / around.max(1.0);
            let why = match e {
                satin::AutoSatinError::TooWide(_) if thick < 50.0 => {
                    satin::AutoSatinError::TooComplex
                }
                e => e,
            };
            Err(why.to_string())
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
            min_stitch: opt.min_stitch,
            origin: shape.origin,
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
        min_stitch: opt.min_stitch,
        origin: shape.origin,
    };
    groups.push(
        tatami(rings, rule, &top)
            .into_iter()
            .map(Piece::reversible)
            .collect(),
    );
    groups
}

fn satin_pieces(
    shape: &Shape,
    opt: &PlanOptions,
    notes: &mut Vec<String>,
) -> (Vec<Piece>, Vec<Vec<Piece>>, Vec<usize>) {
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
) -> (Vec<Piece>, Vec<Vec<Piece>>, Vec<usize>) {
    let p = &shape.params;
    let f = &opt.fabric;
    let mut pieces = Vec::new();
    let mut fallback = Vec::new();
    // (Which of the fallback's groups are a filled part's top rows: the
    // last of each part's.)
    let mut tops = Vec::new();
    let whole = t0 <= 0.0 && t1 >= 1.0;
    for r in satin_rails_sewn(shape) {
        match r {
            Ok((rails, laps)) => {
                // (Part of a column, sewn for show, ends where it's cut.)
                let (rails, laps) = if whole {
                    (rails, laps)
                } else {
                    (satin::strip(&rails, t0, t1), satin::NO_LAPS)
                };
                let w = mean_width(&rails);
                let sp = SatinParams {
                    spacing: mm(p.satin_spacing_for(f, w / 10.0)),
                    pull: mm(p.satin_pull(f)),
                    underlay: p.satin_underlay_for(f, w / 10.0),
                    run_len: mm(2.5),
                    short_stitches: p.short_stitches_on(),
                    split: p.satin_split().map(mm),
                    style: p.satin_style(),
                    lean: p.satin_lean().to_radians(),
                    even: p.satin_even(),
                };
                let fwd = satin::column(&rails, &sp);
                let rev = (!rails.closed).then(|| Rails {
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
                });
                let alt = rev.as_ref().map(|rev| satin::column(rev, &sp));
                let lapped = laps != satin::NO_LAPS;
                let sewn = lapped.then(|| satin::column_lapped(&rails, &sp, &laps));
                let alt_sewn = rev
                    .as_ref()
                    .filter(|_| lapped)
                    .map(|rev| satin::column_lapped(rev, &sp, &[laps[1], laps[0]]));

                pieces.push(Piece {
                    pts: fwd,
                    alt,
                    ring: false,
                    sewn,
                    alt_sewn,
                    column: Some((pieces.len(), false)),
                });
            }
            Err((comp, why)) => {
                notes.push(format!("Part of this shape {why}; it's filled instead."));
                fallback.extend(fill_runs(&comp, FillRule::EvenOdd, shape, opt));
                tops.push(fallback.len() - 1);
            }
        }
    }
    (pieces, fallback, tops)
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
/// on the `side` (+1 left of travel, -1 right). The line keeps its corners
/// (each stretch between two is spaced out on its own, so the line goes
/// through every corner rather than across it), and a corner's prong
/// points between its two sides.
pub fn blanket(line: &[P], closed: bool, spacing: f64, width: f64, side: f64) -> Polyline {
    let mut base: Polyline = Vec::with_capacity(line.len() + 1);
    for &q in line {
        if base.last().is_none_or(|l: &P| l.dist(q) > 1e-6) {
            base.push(q);
        }
    }
    if closed && base.len() > 2 && base[0].dist(base[base.len() - 1]) > 1e-6 {
        base.push(base[0]);
    }
    if base.len() < 2 {
        return Vec::new();
    }
    // A turn sharper than this is a corner (a curve's points turn far less).
    let corner = |a: P, b: P, c: P| (b - a).norm().dot((c - b).norm()) < 0.82;
    let n = base.len();
    // (Not one closer than half a prong's spacing to the corner before
    // it or to the line's end: a tight little curve turns that sharply at
    // every point, and would get a prong at each.)
    let mut along = vec![0.0; n];
    for k in 1..n {
        along[k] = along[k - 1] + base[k - 1].dist(base[k]);
    }
    let apart = spacing * 0.5;
    let mut cuts: Vec<usize> = Vec::new();
    let mut last = 0.0;
    for k in 1..n - 1 {
        if corner(base[k - 1], base[k], base[k + 1])
            && along[k] - last >= apart
            && along[n - 1] - along[k] >= apart
        {
            cuts.push(k);
            last = along[k];
        }
    }
    // (A closed line's first point is a corner too if the line turns there.)
    let wraps = closed && n > 3 && corner(base[n - 2], base[0], base[1]);
    cuts.insert(0, 0);
    cuts.push(n - 1);
    let stretches: Vec<Vec<(P, P)>> = cuts
        .windows(2)
        .map(|w| crate::geom::sample_by_arclength(&base[w[0]..=w[1]], spacing))
        .filter(|s| s.len() >= 2)
        .collect();
    let prong = |out: &mut Polyline, q: P, dir: P| {
        out.push(q);
        out.push(q + dir * (width * side));
        out.push(q);
    };
    // The prong at a corner: between the two sides' (or the one before
    // the corner's, where the line doubles back on itself).
    let between = |before: P, after: P| {
        let d = before.perp() + after.perp();
        if d.len() > 1e-6 {
            d.norm()
        } else {
            before.perp()
        }
    };
    let mut out = Vec::new();
    for (j, st) in stretches.iter().enumerate() {
        let last = st.len() - 1;
        for (k, (q, t)) in st.iter().enumerate() {
            if k == 0 {
                // A stretch begins where the one before ended, with that
                // corner's prong: only the line's first point is new.
                if j == 0 {
                    match stretches.last().filter(|_| wraps) {
                        Some(prev) => prong(&mut out, *q, between(prev[prev.len() - 1].1, *t)),
                        None => prong(&mut out, *q, t.perp()),
                    }
                }
            } else if k < last {
                prong(&mut out, *q, t.perp());
            } else if let Some(next) = stretches.get(j + 1) {
                prong(&mut out, *q, between(*t, next[0].1));
            } else if closed {
                // Back at the start, whose prong is sewn already.
                out.push(*q);
            } else {
                prong(&mut out, *q, t.perp());
            }
        }
    }
    out
}

/// Which side of closed line `r` (one of `rings`) a blanket's prongs go on
/// to point into the shape the rings enclose (see [`blanket`]).
fn prongs_inward(rings: &[Polyline], rule: FillRule, r: &[P]) -> f64 {
    let probe = crate::geom::sample_by_arclength(r, 20.0);
    probe
        .get(probe.len() / 2)
        .map(|(q, t)| {
            if crate::geom::point_inside(rings, rule, *q + t.perp() * 3.0) {
                1.0
            } else {
                -1.0
            }
        })
        .unwrap_or(1.0)
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
                Piece::fixed(blanket(r, true, sp, w, prongs_inward(rings, *rule, r)))
            })
            .collect(),
        Geometry::Stroke { lines, .. } => {
            // A shape's outline: prongs into the shape too, whichever way
            // its path was drawn (a hole's count as outside: where one
            // part of the path is inside another). A line's are on its left.
            let rings: Vec<Polyline> = if shape.outline {
                lines
                    .iter()
                    .filter(|(l, c)| *c && l.len() >= 3)
                    .map(|(l, _)| l.clone())
                    .collect()
            } else {
                Vec::new()
            };
            lines
                .iter()
                .map(|(l, c)| {
                    let side = if shape.outline && *c && l.len() >= 3 {
                        prongs_inward(&rings, FillRule::EvenOdd, l)
                    } else {
                        1.0
                    };
                    Piece::fixed(blanket(l, *c, sp, w, side))
                })
                .collect()
        }
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
        lean: 0.0,
        even: false,
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
        lean: 0.0,
        even: p.satin_even(),
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
        islands: Vec::new(),
        tops: Vec::new(),
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
                opt.min_stitch,
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

thread_local! {
    /// How many [`caught`] calls this thread is inside.
    static CATCHING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Run `f`, catching a panic in it (an internal error on some odd
/// geometry) so the caller can carry on without that piece. While it runs,
/// [`catching`] is true on this thread: a panic hook can tell a failure
/// that's being handled from one that ends the program.
pub fn caught<R>(f: impl FnOnce() -> R) -> std::thread::Result<R> {
    CATCHING.with(|c| c.set(c.get() + 1));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    CATCHING.with(|c| c.set(c.get().saturating_sub(1)));
    r
}

/// Whether a panic on this thread now would be caught by [`caught`].
pub fn catching() -> bool {
    CATCHING.with(|c| c.get() > 0)
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
    let r = caught(|| shape_runs(shape, geometry, opt, &mut tried));
    match r {
        Ok(mut sr) => {
            notes.append(&mut tried);
            // A piece without stitches (a subpath without length) has no
            // start or end to route to. The groups stay: stops count them.
            for g in &mut sr.groups {
                g.retain(|p| !p.pts.is_empty());
            }
            if shape.params.kind.unwrap_or(Kind::Fill) == Kind::Fill {
                sr.find_islands();
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
                    let (pieces, fallback, tops) = satin_pieces_strip(shape, opt, notes, t0, t1);
                    sr.tops.extend(tops.iter().map(|k| sr.groups.len() + k));
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
            let (pieces, mut fallback, tops) = satin_pieces(shape, opt, notes);
            if !pieces.is_empty() {
                fallback.push(pieces);
            }
            let mut sr = ShapeRuns::new(fallback, area);
            sr.tops = tops;
            sr
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
        (Kind::Satin, _) => Some((
            satin_columns(shape).into_iter().flatten().collect(),
            FillRule::NonZero,
        )),
        _ => None,
    }
}

/// What each of a satin's columns covers, column by column: a column
/// between two rails is one ring, a closed one (a ring of satin) its two
/// rails, and a part that's filled instead its outline.
///
/// Each column's rings are wound alike (see [`wound_alike`]), so that any
/// of them together are what they cover by the nonzero rule: columns that
/// overlap (see the `overlap` setting) cover where they overlap. (By the
/// even-odd rule, that would be a hole in what they cover, between every
/// two strokes; one column alone is the same by either rule.)
fn satin_columns(shape: &Shape) -> Vec<Vec<Polyline>> {
    satin_rails(shape)
        .into_iter()
        .map(|r| match r {
            Ok(rails) => rails.outline(),
            Err((comp, _)) => comp,
        })
        .map(wound_alike)
        .collect()
}

/// One part's rings, its outermost wound one way and the rest (its holes)
/// the other: what's inside it by the even-odd rule, and by the nonzero
/// rule as well, alone or with other parts wound so.
fn wound_alike(mut rings: Vec<Polyline>) -> Vec<Polyline> {
    let signed = |r: &[P]| -> f64 {
        (0..r.len())
            .map(|i| crate::geom::cross(r[i], r[(i + 1) % r.len()]))
            .sum()
    };
    let outer = (0..rings.len())
        .max_by(|&i, &j| signed(&rings[i]).abs().total_cmp(&signed(&rings[j]).abs()));
    for (i, ring) in rings.iter_mut().enumerate() {
        if (signed(ring) > 0.0) != (Some(i) == outer) {
            ring.reverse();
        }
    }
    rings
}

/// A satin's column (by its place among those that are columns) by its
/// place among all its parts, the filled ones too (`filled`: which are,
/// see [`satin_filled`]).
fn satin_part(filled: &[bool], column: usize) -> Option<usize> {
    filled
        .iter()
        .enumerate()
        .filter(|(_, f)| !**f)
        .nth(column)
        .map(|(i, _)| i)
}

/// Which of a satin's parts, as [`satin_columns`] has them, couldn't be
/// columns and are filled instead.
fn satin_filled(shape: &Shape) -> Vec<bool> {
    satin_rails(shape).iter().map(|r| r.is_err()).collect()
}

/// A column narrower than this (units) is taken to be this wide as ground
/// for travel: thread run down the middle of a hairline lies under its
/// stitches, but the hairline itself is narrower than the cells travel is
/// routed over, and no way is found along it.
const GROUND_WIDTH: f64 = 6.0;

/// A satin's hairlines as ground for travel, column by column as
/// [`satin_columns`] has them: the outline of each open column that's
/// narrower than [`GROUND_WIDTH`] somewhere, widened to that about its
/// middle; `None` for the rest.
fn satin_thin(shape: &Shape) -> Vec<Option<Polyline>> {
    satin_rails(shape)
        .into_iter()
        .map(|r| thin_ground(&r.ok()?))
        .collect()
}

/// An open column's outline with its hairlines widened to
/// [`GROUND_WIDTH`] about their middle; `None` if it has none (or is a
/// ring).
fn thin_ground(rails: &Rails) -> Option<Polyline> {
    if rails.closed {
        return None;
    }
    let pairs = satin::pairs(rails, 5.0);
    if pairs.iter().all(|(a, b)| a.dist(*b) >= GROUND_WIDTH) {
        return None;
    }
    let wide: Vec<(P, P)> = pairs
        .iter()
        .map(|&(a, b)| {
            let w = a.dist(b);
            // (Not where it comes to a point: there's no saying which way
            // is across it.)
            if w >= GROUND_WIDTH || w < 0.5 {
                return (a, b);
            }
            let (mid, out) = (a.lerp(b, 0.5), (a - b).norm() * (GROUND_WIDTH * 0.5));
            (mid + out, mid - out)
        })
        .collect();
    let mut ring: Polyline = wide.iter().map(|w| w.0).collect();
    ring.extend(wide.iter().rev().map(|w| w.1));
    Some(ring)
}

/// What one plan works out that the next can use as it is, kept in the
/// [`PlanCache`]: each satin's columns (found from its rails: most of the
/// work of planning a design with satins in it, and the same until that
/// satin's outline or settings change), and the zones travel can hide in
/// (the same while what covers the ground there is) and each fill's
/// travel ring (the same while its area is). Moving one object leaves
/// nearly all of them as they were.
#[derive(Default)]
struct Kept {
    /// By [`satin_key`].
    satins: RefCell<HashMap<u64, Rc<Vec<Vec<Polyline>>>>>,
    /// Their hairlines ([`satin_thin`]), by the same.
    thins: RefCell<HashMap<u64, Rc<Vec<Option<Polyline>>>>>,
    /// And which of each one's parts are filled instead of sewn as
    /// columns ([`satin_filled`]), by the same.
    filled: RefCell<HashMap<u64, Rc<Vec<bool>>>>,
    /// By what the zone is made of and the box it's in.
    zones: RefCell<HashMap<u64, Option<Rc<Zone>>>>,
    /// By the area the ring is inset from (see [`travel_ring`]).
    travels: RefCell<HashMap<u64, Rc<Vec<Polyline>>>>,
    /// The zones and rings the plan under way has asked for: the rest are
    /// forgotten when it's done.
    used: RefCell<HashSet<u64>>,
}

/// A fingerprint of every coordinate of `parts`, with their fill rules.
fn parts_key(h: &mut impl std::hash::Hasher, parts: &[(&[Polyline], FillRule)]) {
    use std::hash::Hash;
    parts.len().hash(h);
    for (rings, rule) in parts {
        (*rule == FillRule::EvenOdd).hash(h);
        rings.len().hash(h);
        for r in rings.iter() {
            r.len().hash(h);
            for p in r {
                p.x.to_bits().hash(h);
                p.y.to_bits().hash(h);
            }
        }
    }
}

/// What a satin's columns depend on: its outline, its settings, and
/// whether it's a shape's outline (see [`satin_rails`]).
fn satin_key(shape: &Shape) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = hasher();
    geometry_key(&shape.geometry).hash(&mut h);
    format!("{:?}", shape.params).hash(&mut h);
    shape.outline.hash(&mut h);
    shape.joined.hash(&mut h);
    h.finish()
}

impl Kept {
    /// [`satin_columns`], from last time when the satin is as it was.
    fn satin_columns(&self, shape: &Shape) -> Rc<Vec<Vec<Polyline>>> {
        let key = satin_key(shape);
        if let Some(c) = self.satins.borrow().get(&key) {
            return c.clone();
        }
        let c = Rc::new(satin_columns(shape));
        self.satins.borrow_mut().insert(key, c.clone());
        c
    }

    /// [`satin_thin`], from last time when the satin is as it was.
    fn satin_thin(&self, shape: &Shape) -> Rc<Vec<Option<Polyline>>> {
        let key = satin_key(shape);
        if let Some(t) = self.thins.borrow().get(&key) {
            return t.clone();
        }
        let t = Rc::new(satin_thin(shape));
        self.thins.borrow_mut().insert(key, t.clone());
        t
    }

    /// [`satin_filled`], from last time when the satin is as it was.
    fn satin_filled(&self, shape: &Shape) -> Rc<Vec<bool>> {
        let key = satin_key(shape);
        if let Some(f) = self.filled.borrow().get(&key) {
            return f.clone();
        }
        let f = Rc::new(satin_filled(shape));
        self.filled.borrow_mut().insert(key, f.clone());
        f
    }

    /// [`coverage`], a satin's from its kept columns.
    fn coverage(&self, shape: &Shape) -> Cover {
        if shape.params.kind.unwrap_or(Kind::Fill) == Kind::Satin {
            return Some((
                self.satin_columns(shape)
                    .iter()
                    .flatten()
                    .cloned()
                    .collect(),
                FillRule::NonZero,
            ));
        }
        coverage(shape)
    }

    /// [`Zone::new`], from last time when it was asked for the same ground.
    fn zone(&self, parts: &[(&[Polyline], FillRule)], bounds: Bounds) -> Option<Rc<Zone>> {
        use std::hash::{Hash, Hasher};
        let key = {
            let mut h = hasher();
            for v in [bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y] {
                v.to_bits().hash(&mut h);
            }
            parts_key(&mut h, parts);
            h.finish()
        };
        self.used.borrow_mut().insert(key);
        if let Some(z) = self.zones.borrow().get(&key) {
            return z.clone();
        }
        let z = Zone::new(parts, bounds).map(Rc::new);
        self.zones.borrow_mut().insert(key, z.clone());
        z
    }

    /// [`travel_ring`], from last time when the area is as it was.
    fn travel_ring(&self, rings: &[Polyline], rule: FillRule) -> Rc<Vec<Polyline>> {
        use std::hash::Hasher;
        let key = {
            let mut h = hasher();
            parts_key(&mut h, &[(rings, rule)]);
            h.finish()
        };
        self.used.borrow_mut().insert(key);
        if let Some(t) = self.travels.borrow().get(&key) {
            return t.clone();
        }
        let t = Rc::new(travel_ring(rings, rule));
        self.travels.borrow_mut().insert(key, t.clone());
        t
    }

    /// A plan of `shapes` is starting: forget the satins that aren't
    /// among them any more (or aren't as they were).
    fn begin(&self, shapes: &[Shape]) {
        let live: HashSet<u64> = shapes
            .iter()
            .filter(|s| s.params.kind == Some(Kind::Satin))
            .map(satin_key)
            .collect();
        self.satins.borrow_mut().retain(|k, _| live.contains(k));
        self.thins.borrow_mut().retain(|k, _| live.contains(k));
        self.filled.borrow_mut().retain(|k, _| live.contains(k));
        self.used.borrow_mut().clear();
    }

    /// The plan is made: forget the zones and rings it didn't ask for.
    fn end(&self) {
        let used = self.used.borrow();
        self.zones.borrow_mut().retain(|k, _| used.contains(k));
        self.travels.borrow_mut().retain(|k, _| used.contains(k));
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
/// Satin columns sewn as one satin: two or more two-rail satins one after
/// another in the sewing order, in the same group, in the same thread and
/// with the same settings, none of them saying where it begins or ends.
/// One that says to cut or stop after it is the last of them, and the
/// satin they make cuts or stops there. What Break into Columns makes, and columns
/// drawn by hand and grouped. They're planned as a satin shape's columns
/// are, the thread going on under the columns still to come, so a satin
/// broken into its columns sews as it did; but in the order and direction
/// they're drawn, which is what drawing them is for.
///
/// The shapes from `pos` in `order` that are, and the one satin they
/// make: their subpaths one after another, [`Shape::joined`] saying how
/// many are each's. `None` if fewer than two are.
/// A two-rail satin column that may be sewn as one satin with the columns
/// next to it (see [`joined_satin`]): its subpaths and width.
fn satin_column(s: &Shape) -> Option<(&Vec<(Polyline, bool)>, f64)> {
    let p = &s.params;
    match &s.geometry {
        Geometry::Stroke { lines, width }
            if p.kind == Some(Kind::Satin)
                && !s.outline
                && !s.groups.is_empty()
                && p.start.is_none()
                && p.end.is_none()
                && p.colors.as_ref().is_none_or(|c| c.len() < 2)
                && satin::rails_from_subpaths(lines, p.satin_rails()).is_some() =>
        {
            Some((lines, *width))
        }
        _ => None,
    }
}

fn joined_satin(shapes: &[Shape], order: &[usize], pos: usize) -> Option<(Vec<usize>, Shape)> {
    let column = satin_column;
    let first = &shapes[*order.get(pos)?];
    let (_, width) = column(first)?;
    // (Settings but for what happens after: that's the last one's to say.)
    let settings_of = |s: &Shape| {
        let mut p = s.params.clone();
        (p.trim, p.stop) = (None, None);
        format!("{p:?}")
    };
    let ends = |s: &Shape| s.params.trim == Some(true) || s.params.stop == Some(true);
    let settings = settings_of(first);
    let mut members: Vec<usize> = Vec::new();
    for &j in &order[pos..] {
        let s = &shapes[j];
        if column(s).is_none()
            || s.groups != first.groups
            || s.color != first.color
            || settings_of(s) != settings
        {
            break;
        }
        members.push(j);
        if ends(s) {
            break;
        }
    }
    if members.len() < 2 {
        return None;
    }
    let last = &shapes[*members.last()?];
    let mut params = first.params.clone();
    (params.trim, params.stop) = (last.params.trim, last.params.stop);
    let mut lines = Vec::new();
    let mut joined = Vec::new();
    for &j in &members {
        let (own, _) = column(&shapes[j])?;
        joined.push(own.len());
        lines.extend(own.iter().cloned());
    }
    Some((
        members,
        Shape {
            geometry: Geometry::Stroke { lines, width },
            joined,
            params,
            ..first.clone()
        },
    ))
}

fn sew_order(shapes: &[Shape], opt: &PlanOptions) -> Vec<usize> {
    let n = shapes.len();
    if !opt.hat_order {
        let mut order: Vec<usize> = (0..n).collect();
        if opt.short_travel {
            shorten_travel(shapes, &mut order);
        }
        return order;
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

/// The gap between two boxes (0 where they touch or overlap).
fn box_gap(a: &Bounds, b: &Bounds) -> f64 {
    let dx = (a.min.x - b.max.x).max(b.min.x - a.max.x).max(0.0);
    let dy = (a.min.y - b.max.y).max(b.min.y - a.max.y).max(0.0);
    dx.hypot(dy)
}

/// How far `p` is from the segment `a`–`b`.
fn dist_to_segment(p: P, a: P, b: P) -> f64 {
    let ab = b - a;
    let len2 = ab.dot(ab);
    let t = if len2 > 0.0 {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    a.lerp(b, t).dist(p)
}

/// How far apart two segments are (0 where they cross).
fn segment_gap((a, b): (P, P), (c, d): (P, P)) -> f64 {
    use crate::geom::cross;
    let (d1, d2) = (cross(b - a, c - a), cross(b - a, d - a));
    let (d3, d4) = (cross(d - c, a - c), cross(d - c, b - c));
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return 0.0;
    }
    dist_to_segment(a, c, d)
        .min(dist_to_segment(b, c, d))
        .min(dist_to_segment(c, a, b))
        .min(dist_to_segment(d, a, b))
}

/// A geometry's edges: a fill's outlines, a stroke's lines; and how far
/// its stitches reach either side of them (half a stroke's width).
fn edges(g: &Geometry) -> (Vec<(P, P)>, f64) {
    let mut out = Vec::new();
    let mut add = |l: &[P], closed: bool| {
        out.extend(l.windows(2).map(|w| (w[0], w[1])));
        if closed && l.len() > 2 {
            out.push((l[l.len() - 1], l[0]));
        }
        if l.len() == 1 {
            out.push((l[0], l[0]));
        }
    };
    match g {
        Geometry::Fill { rings, .. } => {
            rings.iter().for_each(|r| add(r, true));
            (out, 0.0)
        }
        Geometry::Stroke { lines, width } => {
            lines.iter().for_each(|(l, closed)| add(l, *closed));
            (out, width / 2.0)
        }
    }
}

/// How far apart two objects' stitches are: 0 where one is over the
/// other (edges crossing, or one inside a fill).
fn object_gap(a: &Geometry, b: &Geometry) -> f64 {
    let inside = |f: &Geometry, o: &Geometry| match f {
        Geometry::Fill { rings, rule } => {
            let (e, _) = edges(o);
            e.first()
                .is_some_and(|&(p, _)| crate::geom::point_inside(rings, *rule, p))
        }
        Geometry::Stroke { .. } => false,
    };
    if inside(a, b) || inside(b, a) {
        return 0.0;
    }
    let ((ea, wa), (eb, wb)) = (edges(a), edges(b));
    let mut best = f64::INFINITY;
    for &x in &ea {
        for &y in &eb {
            best = best.min(segment_gap(x, y));
            if best == 0.0 {
                return 0.0;
            }
        }
    }
    (best - wa - wb).max(0.0)
}

/// Objects nearer than this (units) may be over one another once sewn
/// (pull compensation, underlay, a fill's edge stitches): their order is
/// kept.
const LAYERED: f64 = 5.0;

/// Objects whose boxes are further apart than this (units) are told apart
/// by their boxes alone: the exact gap matters only to choose among near
/// ones.
const NEAR: f64 = 100.0;

/// Stretches longer than this are ordered by their boxes alone, without
/// the exact gaps or 2-opt's passes, which grow with the square of the
/// stretch.
const TOUR_MAX: usize = 300;

/// Reorder each stretch of `order` that's objects one after another in the
/// same thread so the thread travels less between them: the open-path
/// travelling-salesman heuristics, a nearest-neighbour tour (as the pieces
/// of one object are chosen, in [`plan_shown`]) improved by 2-opt (Croes,
/// 1958: reverse a stretch of the tour wherever that shortens it), the
/// distance between two objects the gap between their stitches.
///
/// Only where nothing visible can change: two objects of a stretch that
/// are over one another, or nearly (see [`LAYERED`]), keep their order
/// (which is on top, and so what overlap removal cuts out of what), as
/// [`sew_order`]'s cap order does; and the first of a stretch stays first.
/// An object that says where it begins or ends, cuts or stops after it,
/// sews more than one thread, is a satin column sewn as one satin with its
/// neighbours, is turned off or has no geometry stays where it is and
/// ends the stretch. Traced artwork lists a color's small pieces in the
/// tracer's order, which jumps between them.
fn shorten_travel(shapes: &[Shape], order: &mut [usize]) {
    let bounds: Vec<Bounds> = shapes
        .iter()
        .map(|s| geometry_bounds(&s.geometry))
        .collect();
    let movable = |i: usize| {
        let s = &shapes[i];
        let p = &s.params;
        p.kind != Some(Kind::Skip)
            && p.start.is_none()
            && p.end.is_none()
            && p.trim != Some(true)
            && p.stop != Some(true)
            && p.colors.as_ref().is_none_or(|c| c.len() < 2)
            && satin_column(s).is_none()
            && !bounds[i].is_empty()
    };
    let mut at = 0;
    while at < order.len() {
        let mut end = at + 1;
        if movable(order[at]) {
            while end < order.len()
                && movable(order[end])
                && shapes[order[end]].color == shapes[order[at]].color
            {
                end += 1;
            }
        }
        if end - at >= 3 {
            short_tour(shapes, &bounds, &mut order[at..end]);
        }
        at = end;
    }
}

/// [`shorten_travel`] for one stretch.
fn short_tour(shapes: &[Shape], bounds: &[Bounds], stretch: &mut [usize]) {
    let m = stretch.len();
    let b: Vec<Bounds> = stretch.iter().map(|&i| bounds[i]).collect();
    let exact = m <= TOUR_MAX;
    // The gap between each two, and whether they must keep their order.
    let mut gap = vec![vec![0.0; m]; m];
    let mut over = vec![vec![false; m]; m];
    for x in 0..m {
        for y in x + 1..m {
            let boxes = box_gap(&b[x], &b[y]);
            let g = if !exact {
                boxes
            } else if boxes > NEAR.max(LAYERED) {
                boxes
            } else {
                object_gap(&shapes[stretch[x]].geometry, &shapes[stretch[y]].geometry)
            };
            gap[x][y] = g;
            gap[y][x] = g;
            let o = if exact {
                g <= LAYERED
            } else {
                boxes <= LAYERED
            };
            over[x][y] = o;
            over[y][x] = o;
        }
    }
    // Nearest neighbour, among those whose layered earlier ones are sewn.
    let mut done = vec![false; m];
    let mut tour = vec![0];
    done[0] = true;
    while tour.len() < m {
        let here = *tour.last().unwrap();
        let next = (0..m)
            .filter(|&j| !done[j] && (0..j).all(|i| !over[i][j] || done[i]))
            .min_by(|&x, &y| gap[here][x].total_cmp(&gap[here][y]).then(x.cmp(&y)))
            .expect("layering only holds back later objects, so one is free");
        done[next] = true;
        tour.push(next);
    }
    // 2-opt: reverse tour[i..=k] where that's shorter, and where no two of
    // those are layered (reversing would swap them).
    if exact {
        for _ in 0..m {
            let mut better = false;
            for i in 1..m - 1 {
                for k in i + 1..m {
                    let after_k = |x: usize| if k + 1 < m { gap[x][tour[k + 1]] } else { 0.0 };
                    let before = gap[tour[i - 1]][tour[i]] + after_k(tour[k]);
                    let after = gap[tour[i - 1]][tour[k]] + after_k(tour[i]);
                    if after < before - 1e-6
                        && !(i..=k).any(|x| (x + 1..=k).any(|y| over[tour[x]][tour[y]]))
                    {
                        tour[i..=k].reverse();
                        better = true;
                    }
                }
            }
            if !better {
                break;
            }
        }
    }
    let was: Vec<usize> = stretch.to_vec();
    for (slot, &t) in stretch.iter_mut().zip(&tour) {
        *slot = was[t];
    }
}

/// Geometry each shape is actually stitched with, after overlap removal.
pub fn effective_geometry(shapes: &[Shape], order: &[usize], opt: &PlanOptions) -> Vec<Geometry> {
    effective_geometry_cached(shapes, order, opt, &mut HashMap::new(), &Kept::default())
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
    kept: &Kept,
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
                kept.coverage(&shapes[j]).map(|(r, f)| {
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
    /// Satins' columns and travel's hiding places (see [`Kept`]).
    kept: Kept,
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
    // A hidden object isn't sewn: planned as one that's turned off (so
    // nothing under it is left out for it, and nothing travels under it).
    if shapes.iter().any(|s| s.params.hidden()) {
        let shown: Vec<Shape> = shapes
            .iter()
            .map(|s| {
                let mut s = s.clone();
                if s.params.hidden() {
                    s.params.kind = Some(Kind::Skip);
                }
                s
            })
            .collect();
        return plan_shown(name, &shown, opt, cache);
    }
    plan_shown(name, shapes, opt, cache)
}

fn plan_shown(name: &str, shapes: &[Shape], opt: &PlanOptions, cache: &mut PlanCache) -> Design {
    use std::hash::{Hash, Hasher};
    let order = sew_order(shapes, opt);
    let geo_keys: Vec<u64> = shapes.iter().map(|s| geometry_key(&s.geometry)).collect();
    let kept = &cache.kept;
    kept.begin(shapes);
    // (How narrow a satin may be is the checks' business: stitches kept
    // from last time are still right when only that changes.)
    let opt_key = format!(
        "{:?}",
        PlanOptions {
            narrow_satin: 0.0,
            ..opt.clone()
        }
    );
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
            let g = effective_geometry_cached(shapes, &order, opt, &mut cache.overlap_shapes, kept);
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
        bridges: Vec::new(),
        blends: Vec::new(),
        routes: Vec::new(),
        stitch_shapes: Vec::new(),
        travels: Vec::new(),
        satin_columns: Vec::new(),
    };
    // What each shape covers and where it is, worked out when travel first
    // looks for cover.
    let covers: RefCell<Vec<Option<Cover>>> = RefCell::new(vec![None; shapes.len()]);
    let shape_bounds: RefCell<Option<Vec<Bounds>>> = RefCell::new(None);
    let mut cursor: Option<P> = None;
    // Set by the last object: cut before the next run, or pause before
    // the next object.
    let mut force_trim = false;
    let mut stop_next = false;
    // The last run sewn belongs to the object being sewn, so its outline
    // can be used to route travel within the same shape.
    let mut same_shape = false;
    // Up to where satin columns sewn as one satin (see [`joined_satin`])
    // have been sewn with the first of them.
    let mut joined_until = 0;
    for (pos, &si) in order.iter().enumerate() {
        if pos < joined_until {
            continue;
        }
        let (members, joined) = match joined_satin(shapes, &order, pos) {
            Some((members, shape)) => (members, Some(shape)),
            None => (vec![si], None),
        };
        joined_until = pos + members.len();
        // (What's sewn after it is what's after all of them.)
        let pos = joined_until - 1;
        let shape = joined.as_ref().unwrap_or(&shapes[si]);
        let shape_geometry = joined.as_ref().map_or(&geometry[si], |s| &s.geometry);
        if shape.params.kind == Some(Kind::Skip) {
            continue;
        }
        // The fill's travel ring, worked out when a gap first needs it.
        let travel: std::cell::OnceCell<Rc<Vec<Polyline>>> = std::cell::OnceCell::new();
        // A tatami fill's rows sewn so far (when it's in several
        // sections): the way to the next section goes straight across the
        // fill only where that isn't over them.
        // (A satin whose shape couldn't all be columns has parts filled
        // the same way.)
        let is_tatami = matches!(
            shape.params.kind.unwrap_or(Kind::Fill),
            Kind::Fill | Kind::Satin
        );
        let fill_sewn: std::cell::RefCell<Vec<(P, P)>> = std::cell::RefCell::new(Vec::new());
        let mut fill_sewing: Option<Vec<(P, P)>> = None;
        // Likewise where travel is out of sight (and, for a satin, how
        // many of its columns had been sewn when that was worked out).
        // (By the satin's columns sewn and where the thread is: an order is
        // tried out before it's sewn, from other places with other columns
        // done.)
        let mut zones: HashMap<(Vec<usize>, [u64; 2]), Ground> = HashMap::new();
        // (And where it's under what's sewn later, whatever this shape
        // has sewn: for a fill's travel over its own finished rows.)
        let mut later_zone: Option<Option<Rc<Zone>>> = None;
        // Under a fill's own stitches, if they're close enough to hide it.
        let own_hides = shape.params.kind.unwrap_or(Kind::Fill) == Kind::Fill
            && shape.params.fill_spacing(&opt.fabric) <= HIDING_SPACING;
        // Or under a satin's own columns, which always are: the way to a
        // column and between its parts can run where it's about to sew
        // (worked out when a gap first needs it). Not where it has sewn:
        // travel through a column that's done would lie on top of it, a
        // line down the middle of the satin.
        let is_satin = shape.params.kind == Some(Kind::Satin);
        let satin_cols: std::cell::OnceCell<Rc<Vec<Vec<Polyline>>>> = std::cell::OnceCell::new();
        let satin_sewn: std::cell::RefCell<Vec<usize>> = std::cell::RefCell::new(Vec::new());
        // (The column of the piece in hand: sewn once the way to it has
        // been found, which may be under it.)
        let mut satin_sewing: Option<usize> = None;
        // Whether the piece just sewn was of a part of the satin that's
        // filled instead: the thread leaves it as it leaves a fill, with
        // no hop over its rows taken for free.
        let left_fill = std::cell::Cell::new(false);
        let start_at = shape
            .params
            .start
            .and_then(|t| outline_at(&shape.geometry, t));
        let end_at = shape
            .params
            .end
            .and_then(|t| outline_at(&shape.geometry, t));
        // What a straight stitch between two of the shape's stitches is
        // hidden under: the outline, grown by as far as a fill's rows
        // reach past it (widened for pull) and a little more, so a row's
        // end is inside it, not on its edge (judged on the edge, a line
        // from one goes in or out by where its samples happen to fall,
        // which changes when the shape is moved).
        let row_past = if shape.params.kind == Some(Kind::Fill) {
            mm(shape.params.fill_pull(&opt.fabric)).max(0.0)
        } else {
            0.0
        };
        let grown = |(rings, rule): &(Vec<Polyline>, FillRule)| -> (Vec<Polyline>, FillRule) {
            if row_past > 0.0 {
                (
                    boolean::offset(rings, *rule, row_past + 1.0),
                    FillRule::NonZero,
                )
            } else {
                (rings.clone(), *rule)
            }
        };
        let covered: std::cell::OnceCell<Option<(Vec<Polyline>, FillRule)>> =
            std::cell::OnceCell::new();
        // This shape's stitches: kept from last time when its outline, what's
        // left of it after overlaps, its settings and the options are the same.
        let key = {
            let mut h = hasher();
            joined
                .as_ref()
                .map_or(geo_keys[si], |s| geometry_key(&s.geometry))
                .hash(&mut h);
            shape.joined.hash(&mut h);
            shape.origin.x.to_bits().hash(&mut h);
            shape.origin.y.to_bits().hash(&mut h);
            geometry_key(shape_geometry).hash(&mut h);
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
                let sr = shape_runs_guarded(shape, shape_geometry, opt, &mut notes);
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
            design.stitch_shapes.push(Vec::new());
        }
        let last_group = sr.groups.iter().rposition(|g| !g.is_empty()).unwrap_or(0);
        let tops = sr.tops.clone();
        // The object sewn next in the same thread, if nothing says to cut
        // before it: what it will cover (travel can run under that), and
        // where a line of it can begin.
        let next_at =
            (pos + 1..order.len()).find(|&k| shapes[order[k]].params.kind != Some(Kind::Skip));
        let next_reach: Option<(Cover, Vec<P>)> = next_at
            .map(|k| order[k])
            .filter(|&j| {
                shapes[j].color == shape.color
                    && sr.colors.is_empty()
                    && sr.stops.is_empty()
                    && shape.params.trim != Some(true)
                    && shape.params.stop != Some(true)
            })
            .map(|j| {
                let cover = covers.borrow_mut()[j]
                    .get_or_insert_with(|| kept.coverage(&shapes[j]))
                    .clone();
                let ends = match &shapes[j].geometry {
                    Geometry::Stroke { lines, .. } => lines
                        .iter()
                        .filter(|(l, _)| !l.is_empty())
                        .flat_map(|(l, _)| [l[0], *l.last().unwrap()])
                        .collect(),
                    Geometry::Fill { .. } => Vec::new(),
                };
                (cover, ends)
            });
        // Whether the next object can be reached from `p` without a cut:
        // a short hop from where it can begin or from what it will cover,
        // or a way to it under what's sewn later.
        let reaches_next = |p: P| -> bool {
            let Some((cover, ends)) = &next_reach else {
                return false;
            };
            let near = opt.max_jump_no_trim;
            let beside = match cover {
                Some((rings, rule)) => {
                    crate::geom::point_inside(rings, *rule, p)
                        || rings.iter().any(|r| dist_to_ring(r, p) <= near)
                }
                None => ends.iter().any(|e| e.dist(p) <= near),
            };
            if beside || opt.travel_route <= 0.0 {
                return beside;
            }
            let Some(at) = next_at else {
                return false;
            };
            let next = &shapes[order[at]];
            // (Under the next object itself only where its stitches hide
            // what's beneath them, as for travel within it.)
            let hides = match next.params.kind.unwrap_or(Kind::Fill) {
                Kind::Satin => true,
                Kind::Fill => next.params.fill_spacing(&opt.fabric) <= HIDING_SPACING,
                _ => false,
            };
            // Where travel to it is out of sight: under it, and under
            // what's sewn after it.
            let zone = hidden_zone(
                shapes,
                &order,
                at,
                cover.as_ref().filter(|_| hides),
                &[],
                p,
                opt.travel_route,
                &mut covers.borrow_mut(),
                &mut shape_bounds.borrow_mut(),
                kept,
            );
            let Some(zone) = zone else {
                return false;
            };
            // Somewhere the next object can begin: on each of its
            // outlines, or at a line's ends.
            let begins: Vec<P> = match cover {
                Some((rings, _)) => rings.iter().filter_map(|r| r.first().copied()).collect(),
                None => ends.clone(),
            };
            begins.iter().any(|&q| zone.joined(p, near, q))
        };
        // The stitches from the last run (ending at `prev`) to a piece
        // starting at `start`, when the thread needn't be cut between
        // them; and whether they're a bridge in the open.
        // (Which way the last travel it found was.)
        let how = std::cell::Cell::new(TravelHow::Across);
        // What's been sewn before this shape, the last first: what a
        // thread sewn across the open here would lie on (see `blends`).
        let sewn_under: std::cell::OnceCell<Vec<(Bounds, Vec<Polyline>, FillRule, [u8; 3])>> =
            std::cell::OnceCell::new();
        // The way a gap can be sewn across because the thread blends into
        // what it crosses: straight if that fits, or else round by what
        // it's like in color (each stretch let go as far as its color
        // allows, and the whole way within that). Remembered: orders are
        // tried out from the same places over and over.
        let blend_ways: RefCell<HashMap<[i64; 5], Option<Vec<P>>>> = RefCell::new(HashMap::new());
        let blend_taken: RefCell<Option<Vec<P>>> = RefCell::new(None);
        let blends = |prev: P, start: P, same_shape: bool| -> Option<Vec<P>> {
            let gap = prev.dist(start);
            if opt.blend <= 0.0 || gap > opt.blend.max(opt.connect) {
                return None;
            }
            let key = [prev.x, prev.y, start.x, start.y, same_shape as u8 as f64]
                .map(|v| (v * 4.0).round() as i64);
            if let Some(hit) = blend_ways.borrow().get(&key) {
                return hit.clone();
            }
            let under = sewn_under.get_or_init(|| {
                order[..pos]
                    .iter()
                    .rev()
                    .filter(|&&j| shapes[j].params.kind != Some(Kind::Skip))
                    .filter_map(|&j| {
                        let cover = covers.borrow_mut()[j]
                            .get_or_insert_with(|| kept.coverage(&shapes[j]))
                            .clone();
                        let (rings, rule) = cover?;
                        let mut b = Bounds::empty();
                        rings.iter().flatten().for_each(|&q| b.add(q));
                        Some((b, rings, rule, shapes[j].color))
                    })
                    .collect()
            });
            let ground = Backdrop {
                under,
                // (Coming to it from another object, none of it is sewn
                // yet: there's nothing of its own to sew over.)
                own: sr.area.as_ref().filter(|_| same_shape),
                cloth: opt.cloth,
                thread: shape.color,
            };
            let way = ground
                .least_seen(
                    prev,
                    start,
                    &|w| blend_reach(opt, w),
                    opt.blend.max(opt.connect),
                    1.0,
                )
                .map(|(line, _)| line);
            blend_ways.borrow_mut().insert(key, way.clone());
            way
        };
        let mut connect =
            |prev: P, start: P, force_trim: bool, same_shape: bool| -> (Option<Vec<P>>, bool) {
                let gap = prev.dist(start);
                how.set(TravelHow::Across);
                let inside = covered
                    .get_or_init(|| sr.area.as_ref().map(grown))
                    .as_ref()
                    .is_some_and(|(rings, rule)| {
                        gap <= opt.travel_inside
                            && segment_inside(rings, *rule, prev, start, 2.0)
                            && (opt.over_sewn
                                || crossings(&[prev, start], &fill_sewn.borrow()) <= FEW_ROWS)
                    });
                let straight = || -> Vec<P> {
                    let n = (gap / 30.0).ceil() as usize;
                    (1..n)
                        .map(|i| prev.lerp(start, i as f64 / n as f64))
                        .collect()
                };
                if force_trim {
                    // Cut here, whatever's near.
                    return (None, false);
                }
                if gap <= opt.max_jump_no_trim || inside {
                    // Too short to matter, or straight across the fill.
                    return (Some(straight()), false);
                }
                let mut across: Option<Vec<P>> = None;
                // Within a fill: round the inside of its outline
                // (Ink/Stitch's underpath).
                if same_shape
                    && opt.travel_route > 0.0
                    && let Some((rings, rule)) = sr.area.as_ref()
                {
                    across = route_along(
                        travel.get_or_init(|| kept.travel_ring(rings, *rule)),
                        prev,
                        start,
                        opt.travel_route,
                    );
                    how.set(TravelHow::Edge);
                    // (Not over rows already sewn, there either: at the
                    // edge it shows less, but it shows.)
                    if is_tatami
                        && !opt.over_sewn
                        && !fill_sewn.borrow().is_empty()
                        && let Some(path) = &across
                        && over_sewn(
                            prev,
                            path,
                            start,
                            &fill_sewn.borrow(),
                            &mut later_zone,
                            || {
                                hidden_zone(
                                    shapes,
                                    &order,
                                    pos,
                                    None,
                                    &[],
                                    prev,
                                    opt.travel_route,
                                    &mut covers.borrow_mut(),
                                    &mut shape_bounds.borrow_mut(),
                                    kept,
                                )
                            },
                            0.0,
                        ) > FEW_ROWS
                    {
                        across = None;
                    }
                }
                // Or any way that's out of sight: through the fill (round its
                // holes, through its necks), under a satin's own columns and
                // under what's sewn later on top.
                if across.is_none() && opt.travel_route > 0.0 {
                    how.set(TravelHow::Under);
                    let mut sewn = satin_sewn.borrow().clone();
                    sewn.sort_unstable();
                    let key = (sewn, [prev.x.to_bits(), prev.y.to_bits()]);
                    if !zones.contains_key(&key) {
                        // The satin's columns still to sew; and its parts
                        // that are filled instead, whose rows hide what's
                        // under them as a fill's do (`parts`: with those,
                        // or the columns alone).
                        let filled = is_satin.then(|| kept.satin_filled(shape));
                        let any_filled = filled.as_ref().is_some_and(|f| f.contains(&true));
                        let fill_hides = shape.params.fill_spacing(&opt.fabric) <= HIDING_SPACING;
                        let still = |parts: bool| -> Option<(Vec<Polyline>, FillRule)> {
                            let (filled, done) = (filled.as_ref()?, satin_sewn.borrow());
                            Some((
                                satin_cols
                                    .get_or_init(|| kept.satin_columns(shape))
                                    .iter()
                                    .enumerate()
                                    .filter(|(k, _)| {
                                        if filled.get(*k).copied().unwrap_or(false) {
                                            parts && fill_hides
                                        } else {
                                            !done.contains(k)
                                        }
                                    })
                                    .flat_map(|(_, c)| c.iter().cloned())
                                    .collect(),
                                FillRule::NonZero,
                            ))
                        };
                        let to_sew = still(true);
                        let own = if is_satin {
                            to_sew.as_ref().filter(|(rings, _)| !rings.is_empty())
                        } else {
                            sr.area.as_ref().filter(|_| own_hides)
                        };
                        // (And its hairlines still to sew, wide enough to
                        // travel under.)
                        let thins = is_satin.then(|| kept.satin_thin(shape));
                        let thin: Vec<&Polyline> = thins
                            .iter()
                            .flat_map(|t| t.iter().enumerate())
                            .filter(|(k, _)| !satin_sewn.borrow().contains(k))
                            .filter_map(|(_, ring)| ring.as_ref())
                            .collect();
                        let ways = hidden_zone(
                            shapes,
                            &order,
                            pos,
                            own,
                            &thin,
                            prev,
                            opt.travel_route,
                            &mut covers.borrow_mut(),
                            &mut shape_bounds.borrow_mut(),
                            kept,
                        );
                        // What covers travel there once it's sewn: the
                        // same ground, but for the filled parts. Thread
                        // can run under one where its rows are still to
                        // come, but its rows already sewn aren't covered
                        // by anything.
                        let cover = if any_filled {
                            let columns = still(false);
                            hidden_zone(
                                shapes,
                                &order,
                                pos,
                                columns.as_ref().filter(|(rings, _)| !rings.is_empty()),
                                &thin,
                                prev,
                                opt.travel_route,
                                &mut covers.borrow_mut(),
                                &mut shape_bounds.borrow_mut(),
                                kept,
                            )
                        } else {
                            ways.clone()
                        };
                        zones.insert(key.clone(), Ground { ways, cover });
                    }
                    let zone = &zones[&key];
                    across = zone.ways.as_ref().and_then(|z| {
                        z.route_from(prev, start, opt.travel_route, opt.max_jump_no_trim)
                    });
                    // A fill's rows hide what's sewn before them, not
                    // after: a way over rows already sewn is in plain
                    // sight, unless something sewn later covers it there.
                    // (For a satin, its own columns still to sew are such
                    // cover: what the way crosses under them, the last
                    // stitches of a column that butts into one, they hide.)
                    let mut own_zone = is_satin.then(|| zone.cover.clone());
                    if is_tatami
                        && !opt.over_sewn
                        && !fill_sewn.borrow().is_empty()
                        && let Some(path) = &across
                        && over_sewn(
                            prev,
                            path,
                            start,
                            &fill_sewn.borrow(),
                            if is_satin {
                                &mut own_zone
                            } else {
                                &mut later_zone
                            },
                            || {
                                hidden_zone(
                                    shapes,
                                    &order,
                                    pos,
                                    None,
                                    &[],
                                    prev,
                                    opt.travel_route,
                                    &mut covers.borrow_mut(),
                                    &mut shape_bounds.borrow_mut(),
                                    kept,
                                )
                            },
                            if is_satin { opt.max_jump_no_trim } else { 0.0 },
                        ) > FEW_ROWS
                    {
                        across = None;
                    }
                    // (And that hop no more across finished stitching than
                    // the checks would say shows: a column's stitches
                    // fanning into its tip are many in a little way.)
                    if is_satin
                        && let Some(path) = &across
                        && over_sewn(
                            prev,
                            path,
                            start,
                            &fill_sewn.borrow(),
                            &mut own_zone,
                            || None,
                            0.0,
                        ) >= crate::checks::EXPOSED_ROWS
                    {
                        across = None;
                    }
                    // (Leaving a filled part, nothing still to sew is
                    // taken to cover the way out over its rows: the
                    // columns' outlines reach past their stitches there.)
                    if is_satin
                        && left_fill.get()
                        && let Some(path) = &across
                        && over_sewn(
                            prev,
                            path,
                            start,
                            &fill_sewn.borrow(),
                            &mut Some(None),
                            || None,
                            0.0,
                        ) >= LEFT_FILL_ROWS
                    {
                        across = None;
                    }
                }
                // Or in the open, where the document says gaps this short are
                // sewn across.
                if across.is_none() && gap <= opt.connect {
                    how.set(TravelHow::Open);
                    return (Some(straight()), true);
                }
                // Or where the thread doesn't show against what it crosses.
                if across.is_none()
                    && let Some(line) = blends(prev, start, same_shape)
                {
                    how.set(TravelHow::Blend);
                    let way = sewn_along(&line);
                    *blend_taken.borrow_mut() = Some(line);
                    return (Some(way), false);
                }
                (across, false)
            };
        // What sewing a piece of group `gi` leaves for the way to the
        // next, from its stitches as planned (`pts`) and the satin's column
        // it's of (`column`, by its place among those that are columns):
        // the satin's part that's no longer ground for travel, the rows a
        // way may cross (a fill's or a satin's), and whether the thread
        // leaves a fill. (Sewing, and trying out an order: the same.)
        //
        // A satin's column, once sewn, is no longer somewhere for travel
        // to hide: the one the piece is of (its place among the satin's
        // parts, the filled ones too). Not the one a stitch of it lies in:
        // columns that overlap both have their stitches there. (A part
        // that's filled instead is many pieces, and stays ground for
        // travel while any is to come: where its rows are sewn, they're
        // counted.) Every section's rows, whether its group has others or
        // it's a part of its own: the next may be reached across it. And a
        // satin's columns are its last group, if it has any: every other
        // group is of a part that's filled.
        let after = |pts: &Polyline, column: Option<usize>, gi: usize| {
            let part = column
                .filter(|_| is_satin && pts.len() >= 2)
                .and_then(|k| satin_part(&kept.satin_filled(shape), k))
                .filter(|i| !satin_sewn.borrow().contains(i));
            let rows =
                (is_tatami && (gi == last_group || tops.contains(&gi))).then(|| straight_runs(pts));
            let left = is_satin && (gi != last_group || tops.contains(&gi));
            (part, rows, left)
        };
        // A satin column there's no way to that's out of sight (column
        // `c`, by its place among the satin's parts), but which begins
        // straight across its own ground from where the thread is (the
        // thread at one corner of the end it starts from, or a hop off it;
        // the column starting from the middle of that end): run to it
        // under the column itself, which is about to cover the way.
        let under_own = |prev: P, start: P, c: usize| -> Option<Vec<P>> {
            if !is_satin || opt.travel_route <= 0.0 || prev.dist(start) > satin::AUTO_MAX_WIDTH {
                return None;
            }
            let cols = satin_cols.get_or_init(|| kept.satin_columns(shape));
            let gap = prev.dist(start);
            let steps = (gap / 2.0).ceil().max(1.0) as usize;
            let on_it = |k: usize| {
                let p = prev.lerp(start, k as f64 / steps as f64);
                crate::geom::point_inside(&cols[c], FillRule::EvenOdd, p)
                    || cols[c].iter().any(|r| dist_to_ring(r, p) <= OWN_GROUND)
            };
            let reached = (0..=steps).find(|&k| on_it(k))?;
            (gap * reached as f64 / steps as f64 <= opt.max_jump_no_trim
                && (reached..=steps).all(on_it)
                // (The hop onto it over no more finished stitching than
                // the checks would say shows.)
                && crossings(
                    &[prev, prev.lerp(start, reached as f64 / steps as f64)],
                    &fill_sewn.borrow(),
                ) < crate::checks::EXPOSED_ROWS)
                .then(|| {
                    let n = (gap / 30.0).ceil() as usize;
                    (1..n)
                        .map(|i| prev.lerp(start, i as f64 / n as f64))
                        .collect()
                })
        };
        // The groups in sewing order: one after another, or (a fill in
        // separate parts, with no start or end point to honour) part by
        // part, each part's groups together, nearest part next.
        let mut queue: std::collections::VecDeque<(usize, Vec<Piece>)> =
            std::collections::VecDeque::new();
        let mut parts: Vec<Vec<(usize, Vec<Piece>)>> = Vec::new();
        if !sr.islands.is_empty() && start_at.is_none() && end_at.is_none() {
            for (gi, (group, of)) in sr.groups.into_iter().zip(&sr.islands).enumerate() {
                for (piece, &k) in group.into_iter().zip(of) {
                    if parts.len() <= k {
                        parts.resize_with(k + 1, Vec::new);
                    }
                    match parts[k].last_mut() {
                        Some((g, pieces)) if *g == gi => pieces.push(piece),
                        _ => parts[k].push((gi, vec![piece])),
                    }
                }
            }
            parts.retain(|p| !p.is_empty());
        } else {
            queue.extend(sr.groups.into_iter().enumerate());
        }
        loop {
            if queue.is_empty() && !parts.is_empty() {
                let next = cursor.map_or(0, |here| {
                    let reach = |part: &Vec<(usize, Vec<Piece>)>| {
                        part[0]
                            .1
                            .iter()
                            .map(|piece| {
                                let d = piece.nearest_start(here).0;
                                piece.alt.as_ref().map_or(d, |alt| d.min(here.dist(alt[0])))
                            })
                            .fold(f64::INFINITY, f64::min)
                    };
                    (0..parts.len())
                        .min_by(|&a, &b| reach(&parts[a]).total_cmp(&reach(&parts[b])))
                        .unwrap_or(0)
                });
                queue.extend(parts.swap_remove(next));
            }
            let Some((gi, mut group)) = queue.pop_front() else {
                break;
            };
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
                design.blocks.push(Block {
                    color: group_color,
                    runs: Vec::new(),
                });
                design.block_shapes.push(Vec::new());
                design.stitch_shapes.push(Vec::new());
            }
            let bs = design.block_shapes.last_mut().unwrap();
            for &m in &members {
                if !bs.contains(&m) {
                    bs.push(m);
                }
            }
            let block_index = design.blocks.len() - 1;
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
            // A fill's sections, where going to the nearest each time
            // would take the thread over rows already sewn: the order
            // (and which way each is sewn) that doesn't.
            let mut planned = false;
            // A satin's columns, likewise: a column that ends against the
            // side of another is sewn before it, so the thread goes on
            // under the one still to come instead of being cut.
            //
            // The search counts cuts quickly, by which columns touch; the
            // sewing below finds its way by the travel it routes. So each
            // order it finds is tried out as it would be sewn, the search
            // told what each of its steps came to (cut or not, whatever it
            // had counted), and run again while it's learning something
            // and its count was wrong (lazy constraint generation).
            // Whichever of the orders found sews with fewest cuts is sewn,
            // going to the nearest piece each time among them (first, on a
            // tie): what's sewn is what was tried out.
            // (What the last piece of the group before left being sewn is
            // sewn by now: the orders tried out begin from that, as the
            // sewing below does.)
            if let Some(c) = satin_sewing.take() {
                satin_sewn.borrow_mut().push(c);
            }
            if let Some(rows) = fill_sewing.take() {
                fill_sewn.borrow_mut().extend(rows);
            }
            let columns = if is_satin
                && gi == last_group
                && sr.colors.is_empty()
                && start_at.is_none()
                && end_at.is_none()
                && group.len() >= 2
            {
                let cols = satin_cols.get_or_init(|| kept.satin_columns(shape));
                let from = cursor.filter(|_| !block.runs.is_empty() && !force_trim);
                let reach = next_reach
                    .is_some()
                    .then_some(&reaches_next as &dyn Fn(P) -> bool);
                let drawn = joined.is_some();
                // An order as it'd be sewn: its cuts, and which of its
                // steps are cut (by their places in it). As the loop below
                // sews it: each piece as planned and as sewn, and what
                // sewing it leaves (`after`) taken in where the loop takes
                // it in; all put back as it was after.
                let mut try_out = |order: &PieceOrder| -> (usize, Vec<usize>) {
                    let done = satin_sewn.borrow().clone();
                    let rows = fill_sewn.borrow().len();
                    let left = left_fill.get();
                    let mut prev = from;
                    let mut cut_at = Vec::new();
                    for (k, &(i, turned, leave)) in order.iter().enumerate() {
                        let piece = &group[i];
                        let (mut planned, mut sewn) = if turned {
                            (
                                piece.alt.clone().unwrap_or_else(|| piece.pts.clone()),
                                piece.alt_sewn.clone(),
                            )
                        } else {
                            (piece.pts.clone(), piece.sewn.clone())
                        };
                        if let Some(to) = leave {
                            planned.push(to);
                            if let Some(sewn) = &mut sewn {
                                sewn.push(to);
                            }
                        }
                        let (part, rows, left) = after(&planned, piece.column.map(|(c, _)| c), gi);
                        left_fill.set(left);
                        let mut pts = sewn.unwrap_or(planned);
                        dedupe(&mut pts, opt.min_stitch);
                        if !sewable(&pts, opt.min_stitch) {
                            pts.clear();
                        }
                        if let Some(p) = prev
                            && let Some(&start) = pts.first()
                            && connect(p, start, false, k > 0 || same_shape).0.is_none()
                            && part.is_none_or(|c| under_own(p, start, c).is_none())
                        {
                            cut_at.push(k);
                        }
                        satin_sewn.borrow_mut().extend(part);
                        fill_sewn.borrow_mut().extend(rows.into_iter().flatten());
                        prev = pts.last().copied().or(prev);
                    }
                    let missed = reach.is_some_and(|f| prev.is_some_and(|p| !f(p)));
                    *satin_sewn.borrow_mut() = done;
                    fill_sewn.borrow_mut().truncate(rows);
                    left_fill.set(left);
                    (cut_at.len() + missed as usize, cut_at)
                };
                let mut known: HashMap<Step, bool> = HashMap::new();
                let mut best: Option<(usize, PieceOrder)> = None;
                let (mut expected, mut searches) = (None, 0);
                // (The steps those searches took between them.)
                let mut searched = 0;
                for _ in 0..ORDER_SEARCHES {
                    let Some(plan) = column_order(
                        &group,
                        cols,
                        &kept.satin_filled(shape),
                        from,
                        reach,
                        opt,
                        drawn,
                        &known,
                    ) else {
                        break;
                    };
                    searches += 1;
                    expected.get_or_insert(plan.expected);
                    if best.is_none() && !drawn {
                        best = Some((try_out(&plan.first).0, plan.first.clone()));
                    }
                    let (cuts, cut_at) = try_out(&plan.order);
                    let mut learned = false;
                    for (k, &step) in plan.steps.iter().enumerate() {
                        learned |= known.insert(step, cut_at.contains(&k)).is_none();
                    }
                    if best.as_ref().is_none_or(|b| cuts < b.0) {
                        best = Some((cuts, plan.order));
                    }
                    searched += plan.searched;
                    if cuts == plan.expected || !learned || searched >= ORDER_SEARCH_STEPS {
                        break;
                    }
                }
                if trim_report()
                    && let Some((cuts, _)) = &best
                {
                    eprintln!(
                        "column order: {} columns, expected {} cuts; tried out, {cuts} ({searches} searches); taken",
                        group.len(),
                        expected.unwrap_or(0),
                    );
                }
                best.map(|b| b.1)
            } else {
                None
            };
            if let Some(order) = columns {
                let mut from: Vec<Option<Piece>> = group.drain(..).map(Some).collect();
                for (i, turned, leave) in order {
                    let Some(mut piece) = from[i].take() else {
                        continue;
                    };
                    if turned && let Some(alt) = piece.alt.take() {
                        piece.pts = alt;
                        piece.sewn = piece.alt_sewn.take();
                        if let Some((_, t)) = &mut piece.column {
                            *t = !*t;
                        }
                    }
                    piece.alt = None;
                    piece.alt_sewn = None;
                    if let Some(to) = leave {
                        piece.pts.push(to);
                        if let Some(sewn) = &mut piece.sewn {
                            sewn.push(to);
                        }
                    }
                    group.push(piece);
                }
                planned = true;
            } else if is_tatami
                && gi == last_group
                && start_at.is_none()
                && end_at.is_none()
                && let Some(area) = covered.get_or_init(|| sr.area.as_ref().map(grown)).as_ref()
                && let Some(order) = section_order(
                    &group,
                    cursor,
                    area,
                    travel.get_or_init(|| {
                        let (rings, rule) = sr.area.as_ref().unwrap();
                        kept.travel_ring(rings, *rule)
                    }),
                    opt,
                )
            {
                let mut from: Vec<Option<Piece>> = group.drain(..).map(Some).collect();
                for (i, turned) in order {
                    let Some(mut piece) = from[i].take() else {
                        continue;
                    };
                    if turned && let Some(alt) = piece.alt.take() {
                        piece.pts = alt;
                        piece.sewn = piece.alt_sewn.take();
                        if let Some((_, t)) = &mut piece.column {
                            *t = !*t;
                        }
                    }
                    piece.alt = None;
                    piece.alt_sewn = None;
                    group.push(piece);
                }
                planned = true;
            } else if joined.is_some() {
                // Columns sewn as one satin, too many to search: as
                // they're drawn.
                for piece in &mut group {
                    piece.alt = None;
                    piece.alt_sewn = None;
                }
                planned = true;
            }
            // Greedy nearest-neighbour ordering within the group.
            while !group.is_empty() {
                if let Some(c) = satin_sewing.take() {
                    satin_sewn.borrow_mut().push(c);
                }
                if let Some(rows) = fill_sewing.take() {
                    fill_sewn.borrow_mut().extend(rows);
                }
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
                    if planned {
                        // (In the order worked out: the first.)
                        break;
                    }
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
                let piece = if planned {
                    group.remove(0)
                } else {
                    group.swap_remove(best)
                };
                // (That moved the last piece into the gap: the one kept
                // for the end, if it was last.)
                if let Some((held, _)) = &mut ender
                    && *held == group.len()
                {
                    *held = best;
                }
                // The object's last piece, which can be sewn from either
                // end, with another object to follow: sewn toward the end
                // the next object can be reached from, when that saves a
                // cut (counting the one it may cost on the way in).
                if last_piece
                    && gi == last_group
                    && queue.is_empty()
                    && parts.is_empty()
                    && !piece.ring
                    && ender.is_none()
                    && !from_start
                    && next_reach.is_some()
                    && let Some(alt) = piece.alt.as_ref()
                    && let (Some(&a0), Some(&a1)) = (piece.pts.first(), piece.pts.last())
                    && let (Some(&b0), Some(&b1)) = (alt.first(), alt.last())
                {
                    let sewing = cursor.filter(|_| !block.runs.is_empty());
                    let mut cuts = |from: P, to: P| -> usize {
                        let cut_in = sewing.is_some_and(|prev| {
                            connect(prev, from, force_trim, same_shape).0.is_none()
                        });
                        cut_in as usize + !reaches_next(to) as usize
                    };
                    let (plain, turned) = (cuts(a0, a1), cuts(b0, b1));
                    if best_alt && plain < turned {
                        best_alt = false;
                    } else if !best_alt && turned < plain {
                        best_alt = true;
                    }
                }
                // (The object the piece is of: of columns sewn as one
                // satin, its column's.)
                let si = piece
                    .column
                    .and_then(|(c, _)| members.get(c).copied())
                    .unwrap_or(si);
                // (Which of the satin's columns this sews, among those
                // that are columns.)
                let own_column = piece.column.map(|(c, _)| c);
                // (Once: a satin of several threads sews each column
                // once a thread.)
                if let Some((c, turned)) = piece.column
                    && joined.is_none()
                    && !design
                        .satin_columns
                        .iter()
                        .rev()
                        .take_while(|e| e.0 == si)
                        .any(|e| e.1 == c)
                {
                    design.satin_columns.push((si, c, turned != best_alt));
                }
                let sewn = if best_alt { piece.alt_sewn } else { piece.sewn };
                let mut pts = if best_alt {
                    piece.alt.unwrap()
                } else if piece.ring && best_at > 0 {
                    rotate_ring(&piece.pts, best_at)
                } else {
                    piece.pts
                };
                let left;
                (satin_sewing, fill_sewing, left) = after(&pts, own_column, gi);
                left_fill.set(left);
                // (Planned as the column that ends where it meets the
                // next; sewn with its stitches run on under that one.)
                if let Some(sewn) = sewn {
                    pts = sewn;
                }
                dedupe(&mut pts, opt.min_stitch);
                if !sewable(&pts, opt.min_stitch) {
                    continue;
                }
                // Begin at the start point: a few stitches from it to the
                // first row, hidden under the fill (a satin or run just
                // starts at its nearest end).
                if from_start
                    && let Some(st) = start_at
                    && let Some((rings, rule)) =
                        covered.get_or_init(|| sr.area.as_ref().map(grown)).as_ref()
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
                let start = pts[0];
                let mut trim_before = false;
                let (travel_block, travel_run) = (block_index, block.runs.len().saturating_sub(1));
                if let (Some(prev), Some(run)) = (cursor, block.runs.last_mut()) {
                    let (mut across, mut bridged) = connect(prev, start, force_trim, same_shape);
                    // The way drawn for it by hand, from the object before:
                    // sewn as drawn, whatever the planner would have done
                    // (but a cut that's asked for is made).
                    if !same_shape
                        && !force_trim
                        && let Some(via) = &shape.params.via
                    {
                        let mut line = vec![prev];
                        line.extend(via.iter().map(|&(x, y)| P::new(x * 10.0, y * 10.0)));
                        line.push(start);
                        let way = sewn_along(&line);
                        design.routes.push((si, line));
                        across = Some(way);
                        bridged = false;
                        how.set(TravelHow::Routed);
                    }
                    if bridged {
                        design.bridges.push((prev, start));
                    } else if across.is_some() && how.get() == TravelHow::Blend {
                        design
                            .blends
                            .push(blend_taken.take().unwrap_or_else(|| vec![prev, start]));
                    }
                    if across.is_none()
                        && !force_trim
                        && !from_start
                        && let Some(c) = satin_sewing
                        && let Some(way) = under_own(prev, start, c)
                    {
                        across = Some(way);
                        how.set(TravelHow::Under);
                    }
                    if let Some(across) = across {
                        // No stitch shorter than the shortest allowed where
                        // the two meet: the way across gives up its last
                        // points to the piece's first, and the piece any of
                        // its own that are still too near.
                        let joined = run.pts.len();
                        if let Some(spans) =
                            design.stitch_shapes.last_mut().and_then(|b| b.last_mut())
                            && spans.last().is_none_or(|s| s.1 != si)
                        {
                            spans.push((joined, si));
                        }
                        extend_run(&mut run.pts, across, opt.min_stitch);
                        while run.pts.len() > joined
                            && run
                                .pts
                                .last()
                                .is_some_and(|q| q.dist(start) < opt.min_stitch)
                        {
                            run.pts.pop();
                        }
                        if prev.dist(start) > opt.max_jump_no_trim {
                            design.travels.push(Travel {
                                how: how.get(),
                                shape: si,
                                block: travel_block,
                                run: travel_run,
                                from: joined.saturating_sub(1),
                                to: run.pts.len(),
                            });
                        }
                        design.per_shape[si] += extend_run(&mut run.pts, pts, opt.min_stitch);
                        cursor = run.pts.last().copied();
                        force_trim = false;
                        same_shape = true;
                        continue;
                    }
                    trim_before = true;
                    if trim_report() {
                        eprintln!(
                            "trim before shape {si} ({}): {} -> {}, {:.1} mm (a hop is {:.1}){}",
                            shape.id,
                            at_mm(prev),
                            at_mm(start),
                            prev.dist(start) / 10.0,
                            opt.max_jump_no_trim / 10.0,
                            if force_trim { ", asked for" } else { "" },
                        );
                    }
                }
                force_trim = false;
                same_shape = true;
                cursor = pts.last().copied();
                design.per_shape[si] += pts.len();
                block.runs.push(Run { pts, trim_before });
                if let Some(b) = design.stitch_shapes.last_mut() {
                    b.push(vec![(0, si)]);
                }
            }
        }
        force_trim = shape.params.trim == Some(true);
        stop_next = shape.params.stop == Some(true);
        same_shape = false;
    }
    // Forget shapes no longer there (or no longer like that).
    cache.runs.retain(|k, _| used.contains(k));
    kept.end();
    let keep: Vec<bool> = design.blocks.iter().map(|b| !b.runs.is_empty()).collect();
    let mut k = keep.iter();
    design.block_shapes.retain(|_| *k.next().unwrap());
    let mut k = keep.iter();
    design.stitch_shapes.retain(|_| *k.next().unwrap());
    design.blocks.retain(|b| !b.runs.is_empty());
    design
}

/// A run's stitches as the straight lines they lie along: a fill's rows
/// (and the turns between them), far fewer than its stitches. For
/// counting what a travel path would cross.
fn straight_runs(pts: &[P]) -> Vec<(P, P)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < pts.len() {
        let a = pts[i];
        let d = pts[i + 1] - a;
        let mut j = i + 1;
        while j + 1 < pts.len() {
            let e = pts[j + 1] - pts[j];
            let (cross, dot) = (d.x * e.y - d.y * e.x, d.x * e.x + d.y * e.y);
            if dot > 0.0 && cross.abs() <= 1e-6 * dot {
                j += 1;
            } else {
                break;
            }
        }
        out.push((a, pts[j]));
        i = j;
    }
    out
}

/// How many of `rows` the path `path` crosses: as many rows' worth of
/// them as it does (see [`crate::checks::rows_worth`]: stitches crowded
/// together, round the inside of a bend or fanned into a corner, are no
/// more to travel across than the same distance of plain rows).
fn crossings(path: &[P], rows: &[(P, P)]) -> usize {
    let side = |a: P, b: P, c: P| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    // How far along the path each crossing is.
    let mut at: Vec<f64> = Vec::new();
    let mut gone = 0.0;
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (lo, hi) = (
            P::new(a.x.min(b.x), a.y.min(b.y)),
            P::new(a.x.max(b.x), a.y.max(b.y)),
        );
        let len = a.dist(b);
        for &(c, d) in rows {
            if c.x.max(d.x) < lo.x
                || c.x.min(d.x) > hi.x
                || c.y.max(d.y) < lo.y
                || c.y.min(d.y) > hi.y
            {
                continue;
            }
            let (sa, sb) = (side(c, d, a), side(c, d, b));
            if side(a, b, c) * side(a, b, d) < 0.0 && sa * sb < 0.0 {
                at.push(gone + len * sa / (sa - sb));
            }
        }
        gone += len;
    }
    crate::checks::rows_worth(at)
}

/// The way out of a satin's part that's filled instead may cross fewer
/// of its finished rows than this, nothing taken to cover them: one
/// under what the checks say shows. (Over the census's 2,423 letters
/// with a filled part, at 7 the letters with travel in sight went from 34
/// to 10 for 302 more trims; at 6, to 4 for 832 more.)
const LEFT_FILL_ROWS: usize = 7;

/// A travel path may cross this many rows already sewn and still count
/// as out of sight: a millimetre or so at a section's edge, as a short
/// hop is.
const FEW_ROWS: usize = 4;

/// How many of a fill's rows already sewn (`rows`) the way from `prev` by
/// `path` to `start` crosses in plain sight: not counting where it's
/// under what's sewn later (`later`: that zone, made by `zone` when first
/// needed).
///
/// Nor the first `free` units of the way: the hop off where the last piece
/// ended, as short as the gaps that are sewn straight across anyway.
fn over_sewn(
    prev: P,
    path: &[P],
    start: P,
    rows: &[(P, P)],
    later: &mut Option<Option<Rc<Zone>>>,
    zone: impl FnOnce() -> Option<Rc<Zone>>,
    free: f64,
) -> usize {
    let mut whole = vec![prev];
    whole.extend(path.iter().copied());
    whole.push(start);
    // (The way from where the hop is over.)
    let mut left = free;
    while left > 0.0 && whole.len() >= 2 {
        let len = whole[0].dist(whole[1]);
        if len > left {
            whole[0] = whole[0].lerp(whole[1], left / len);
            break;
        }
        left -= len;
        whole.remove(0);
    }
    let mut zone = Some(zone);
    let mut over = 0;
    for w in whole.windows(2) {
        let n = crossings(w, rows);
        if n == 0 {
            continue;
        }
        if later.is_none() {
            *later = Some(zone.take().and_then(|z| z()));
        }
        let covered = later
            .as_ref()
            .and_then(|z| z.as_ref())
            .is_some_and(|z| z.contains(w[0].lerp(w[1], 0.5)));
        if !covered {
            over += n;
        }
    }
    over
}

/// The most columns [`through`] cuts in two at their joins, and the
/// most it leaves after (each a column the order search tries).
const ORDER_COLUMNS: usize = 16;
/// How many steps [`column_order`]'s search takes before it settles for
/// the best order it has (a bound on its time).
const ORDER_BUDGET: usize = 30_000;
/// The most times a satin's columns are searched for an order, each time
/// told what the last one's steps came to, tried out (see `plan_shown`):
/// a bound on the time. (Most of the census's letters stop after one;
/// a few dozen go on, and one or two take all of these.)
const ORDER_SEARCHES: usize = 50;
/// The most steps those searches take between them: a bound on the time
/// for a satin of many letters (a line of type), where each search can use
/// all of [`ORDER_BUDGET`] and the next learns little (2026-11-21: a 38
/// column line searched 50 times over in 9 s, none better than the first).
/// Over the letter census no satin comes near it.
const ORDER_SEARCH_STEPS: usize = 5 * ORDER_BUDGET;

/// An order to sew a satin's pieces in: each piece, whether it's sewn
/// from its other end, and where it leaves the thread if not where its
/// last stitch ends (see [`end_stitch`]).
type PieceOrder = Vec<(usize, bool, Option<P>)>;

/// What [`column_order`] found: the order with fewest cuts as its search
/// counts them (`expected`), each of its steps (to tell the search what
/// each came to, tried out: see [`Step`]), and the order going to the
/// nearest piece each time would take (`first`), for comparison.
struct ColumnPlan {
    /// How many steps its search took (see [`ORDER_BUDGET`]).
    searched: usize,
    order: PieceOrder,
    steps: Vec<Step>,
    expected: usize,
    first: PieceOrder,
}

/// The order to sew a satin's columns in, and which way each (true: from
/// its other end), as its search counts the cuts (`None` if there are
/// too many columns to try): the caller tries it out (see
/// `plan_shown`), and tells it what the steps it's tried came to
/// (`known`: whether each was cut).
///
/// Thread gets from one column to the next without a cut by a short hop,
/// or under columns still to be sewn (never over one that's done). So a
/// column that ends against the side of another (a serif under a stem,
/// the bar of an e, an arm off a stem) has to be sewn first: sewn after,
/// it's cut off from the rest by the column it leans on. Nearest-next
/// sews whichever it comes to. `cols` is what each column covers (see
/// [`satin_columns`]); `from` where the thread is, if it needn't be cut
/// before this object; `reaches_next` whether the next object can be
/// reached from a point, if there is one to reach.
///
/// `drawn`: the pieces are columns drawn in the order they're to be sewn
/// ([`joined_satin`]), and only which side each ends on is chosen: always
/// an order, then (that one), unless there are too many to search.
#[allow(clippy::too_many_arguments)]
fn column_order(
    pieces: &[Piece],
    cols: &[Vec<Polyline>],
    filled: &[bool],
    from: Option<P>,
    reaches_next: Option<&dyn Fn(P) -> bool>,
    opt: &PlanOptions,
    drawn: bool,
    known: &HashMap<Step, bool>,
) -> Option<ColumnPlan> {
    let n = pieces.len();
    if n < 2 {
        return None;
    }
    // (What each piece covers: the column it sews, by its place among
    // the satin's parts, the filled ones too.)
    let col_of: Vec<usize> = pieces
        .iter()
        .map(|p| p.column.and_then(|(c, _)| satin_part(filled, c)))
        .collect::<Option<_>>()?;
    let outlines: Vec<&Vec<Polyline>> = col_of.iter().map(|&c| &cols[c]).collect();
    // Each way of sewing each piece: from either end, and with the thread
    // left where its last stitch ends or back along it (`leaves`: where,
    // if back; see [`end_stitch`]). (As it comes first.) Each as it's
    // sewn: with its stitches run on under the column it ends against.
    let mut ends: Vec<ColumnWay> = Vec::new();
    let mut leaves: Vec<Option<P>> = Vec::new();
    for (i, piece) in pieces.iter().enumerate() {
        let mut add = |pts: &Polyline, turned: bool| {
            if let (Some(&start), Some(&end)) = (pts.first(), pts.last()) {
                ends.push(ColumnWay {
                    piece: i,
                    turned,
                    start,
                    end,
                });
                leaves.push(None);
                for end in end_stitch(pts, opt.min_stitch) {
                    ends.push(ColumnWay {
                        piece: i,
                        turned,
                        start,
                        end,
                    });
                    leaves.push(Some(end));
                }
            }
        };
        add(piece.sewn.as_ref().unwrap_or(&piece.pts), false);
        if let Some(alt) = piece.alt.as_ref().filter(|_| !drawn) {
            add(piece.alt_sewn.as_ref().unwrap_or(alt), true);
        }
    }
    let found = fewest_cuts(
        &outlines,
        &ends,
        from,
        reaches_next,
        opt.max_jump_no_trim,
        drawn,
        known,
        usize::MAX,
    )?;
    let as_pieces = |ways: &[usize]| -> PieceOrder {
        ways.iter()
            .map(|&w| (ends[w].piece, ends[w].turned, leaves[w]))
            .collect()
    };
    let mut sewn = 0u64;
    let mut last = usize::MAX;
    let steps = found
        .order
        .iter()
        .map(|&w| {
            let step = (sewn, last, w);
            sewn |= 1u64.checked_shl(ends[w].piece as u32).unwrap_or(0);
            last = w;
            step
        })
        .collect();
    Some(ColumnPlan {
        searched: found.searched,
        order: as_pieces(&found.order),
        steps,
        expected: found.cuts,
        first: as_pieces(&found.first),
    })
}

/// Whether the thread gets from a point (how far from each column:
/// `here`, infinitely if more than a hop) to column `to` under columns
/// not yet sewn (`sewn`: a bit each), as the travel that's sewn will have
/// to ([`Zone::route_from`]): onto the ground the column is on, if that's
/// within a hop; or onto the nearest ground, and from that by one hop to
/// the column's. Not hop after hop: three serifs in a row, each a hop
/// from the next, are no way from the first to the third. (`joined`:
/// which columns are one piece of ground; `touch`: which are within a
/// hop. Each by both columns' places, row by row.)
#[cfg(test)]
fn hidden_way(here: &[f64], to: usize, sewn: u64, joined: &[bool], touch: &[bool]) -> bool {
    let n = here.len();
    hidden_way_from(
        &Reach::of(here),
        to,
        sewn,
        &bit_masks(joined, n),
        &bit_masks(touch, n),
    )
}

/// The columns a point is at (in, or a hop from): a bit each, and each of
/// them nearest first. (What [`hidden_way`] asks of the point, worked out
/// once for each column's end, as the order search asks at every step.)
struct Reach {
    at: u64,
    nearest: Vec<usize>,
}

impl Reach {
    /// From how far the point is from each column (infinitely, if more
    /// than a hop).
    fn of(here: &[f64]) -> Reach {
        let mut nearest: Vec<usize> = (0..here.len()).filter(|&j| here[j].is_finite()).collect();
        nearest.sort_by(|&a, &b| here[a].total_cmp(&here[b]));
        Reach {
            at: nearest.iter().fold(0u64, |m, &j| m | 1 << j),
            nearest,
        }
    }
}

/// [`hidden_way`], from a point's [`Reach`], with which columns are one
/// piece of ground with each (`joined`) and which are within a hop
/// (`touch`) as a bit each.
fn hidden_way_from(here: &Reach, to: usize, sewn: u64, joined: &[u64], touch: &[u64]) -> bool {
    let n = joined.len();
    let all = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };
    let unsewn = all & !sewn;
    // The columns still to sew that are one piece of ground with it.
    let goal = flood(1 << to, unsewn, joined);
    if here.at & goal != 0 {
        return true;
    }
    let Some(&nearest) = here.nearest.iter().find(|&&j| unsewn & (1 << j) != 0) else {
        return false;
    };
    let mut from = flood(1 << nearest, unsewn, joined);
    while from != 0 {
        let x = from.trailing_zeros() as usize;
        from &= from - 1;
        if touch[x] & goal != 0 {
            return true;
        }
    }
    false
}

/// Each of `n` columns' neighbours in `adj` (`n` by `n`), a bit each.
fn bit_masks(adj: &[bool], n: usize) -> Vec<u64> {
    (0..n)
        .map(|i| {
            (0..n)
                .filter(|&j| adj[i * n + j])
                .fold(0u64, |m, j| m | 1 << j)
        })
        .collect()
}

/// The columns reached from `start` by way of neighbours (`adj`, a bit
/// each) among `within`.
fn flood(start: u64, within: u64, adj: &[u64]) -> u64 {
    let (mut reached, mut edge) = (start, start);
    while edge != 0 {
        let mut grow = 0u64;
        let mut e = edge;
        while e != 0 {
            grow |= adj[e.trailing_zeros() as usize];
            e &= e - 1;
        }
        edge = grow & within & !reached;
        reached |= edge;
    }
    reached
}

/// Two columns no further apart than this (a cell of the ground travel is
/// routed over) are one piece of ground, with no hop between.
const ONE_GROUND: f64 = crate::route::CELL;

/// How near a column's outline (units) the thread is on the column's own
/// ground: its stitches reach a little past the outline they're drawn to.
const OWN_GROUND: f64 = 3.0;

/// Whether a piece's stitches (deduplicated: see [`dedupe`]) are any to
/// sew: not none, nor a sliver all within the shortest stitch (`min`) of
/// where it starts, which would be one stitch shorter than that. (A
/// single point is: a needle down, no stitch.)
fn sewable(pts: &Polyline, min: f64) -> bool {
    pts.first()
        .is_some_and(|&p| pts.len() == 1 || pts.iter().any(|q| q.dist(p) >= min))
}

/// Where else a satin column can leave the thread: back along its last
/// stitch, which hides the thread going there. Back to where it began
/// (the column's other side), or halfway (the middle of its end: where a
/// square end comes nearest what's beyond it, the dot over an i, which
/// either corner can miss by a hop). Not by a stitch shorter than
/// `shortest` (the shortest the machine is to sew).
fn end_stitch(pts: &Polyline, shortest: f64) -> Vec<P> {
    let n = pts.len();
    if n < 3 {
        return Vec::new();
    }
    // (The last stitch is to the last point, from the last before it that
    // isn't the same: a column's stitches can end with a needle down
    // twice in one place.)
    let last = pts[n - 1];
    let Some(&back) = pts[..n - 1].iter().rev().find(|q| q.dist(last) > 1e-9) else {
        return Vec::new();
    };
    [back, last.lerp(back, 0.5)]
        .into_iter()
        .filter(|q| q.dist(last) >= shortest)
        .collect()
}

/// Whether to say, on stderr, why the planner cuts the thread where it
/// does (`ZIGZAG_TRIMS` set): each trim, how far the jump was, and what
/// the searches that ordered and cut the columns expected. For finding
/// the cause of a trim in a letter without adding prints of one's own;
/// `zigzag convert` and the census pass it through.
fn trim_report() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ZIGZAG_TRIMS").is_some())
}

/// A point in mm, for [`trim_report`].
fn at_mm(p: P) -> String {
    format!("({:.1}, {:.1})", p.x / 10.0, p.y / 10.0)
}

/// A way of sewing one of a satin's columns: which (its place among the
/// columns), from which end (`turned`: its other one), and where that
/// starts and ends. (A column can end on either side, or between: see
/// [`end_stitch`].)
struct ColumnWay {
    piece: usize,
    turned: bool,
    start: P,
    end: P,
}

/// What [`fewest_cuts`] found: the cuts its order takes, the order (the
/// ways it was given, by their place among them), and the order going to
/// the nearest each time would take (`first`), likewise.
struct ColumnOrder {
    cuts: usize,
    /// How many steps the search took.
    searched: usize,
    order: Vec<usize>,
    first: Vec<usize>,
}

/// A step of an order of a satin's columns, as [`fewest_cuts`] can be told
/// what it came to tried out (`known`): the columns sewn before it (a bit
/// each), the way the last was sewn (`usize::MAX` for none: from where the
/// thread was), and the way to be sewn (each by its place among those it
/// was given).
type Step = (u64, usize, usize);

/// As many columns as [`letter_cuts`] searches at once (a bit each).
const SEARCH_COLUMNS: usize = 64;

/// [`column_order`]'s search, on what each column covers (`outlines`) and
/// the ways each can be sewn: the order with fewest cuts, a letter (the
/// columns that touch) at a time.
///
/// A word has more columns than are searched at once: its letters are
/// taken one after another, the nearest next, each from where the last
/// left the thread. (Each is searched as it would be alone: a word's
/// letters are cut no oftener than they are one at a time.) `None` if one
/// letter has too many, or a column no way to sew it.
///
/// `drawn`: the columns in the order given, a way of each chosen (see
/// [`in_turn`]). `below`: only an order of fewer cuts than that is wanted
/// (`None` if there's none): what [`through`] has already found, which
/// the search's bound then cuts it short at.
#[allow(clippy::too_many_arguments)]
fn fewest_cuts(
    outlines: &[&Vec<Polyline>],
    ends: &[ColumnWay],
    from: Option<P>,
    reaches_next: Option<&dyn Fn(P) -> bool>,
    near: f64,
    drawn: bool,
    known: &HashMap<Step, bool>,
    below: usize,
) -> Option<ColumnOrder> {
    let n = outlines.len();
    if n <= SEARCH_COLUMNS {
        return letter_cuts(
            outlines,
            ends,
            from,
            reaches_next,
            near,
            drawn,
            known,
            below,
        );
    }
    // The columns that touch (a point of one's outline in the other, or
    // a hop from it), a letter each.
    let boxes: Vec<Bounds> = outlines
        .iter()
        .map(|rings| {
            let mut b = Bounds::empty();
            rings.iter().flatten().for_each(|&p| b.add(p));
            Bounds {
                min: P::new(b.min.x - near, b.min.y - near),
                max: P::new(b.max.x + near, b.max.y + near),
            }
        })
        .collect();
    let at = |p: P, i: usize| {
        crate::geom::point_inside(outlines[i], FillRule::EvenOdd, p)
            || outlines[i].iter().any(|r| dist_to_ring(r, p) <= near)
    };
    let touch = |i: usize, j: usize| {
        overlaps(&boxes[i], &boxes[j])
            && (outlines[i].iter().flatten().any(|&p| at(p, j))
                || outlines[j].iter().flatten().any(|&p| at(p, i)))
    };
    let mut letters: Vec<Vec<usize>> = Vec::new();
    let mut placed = vec![false; n];
    for i in 0..n {
        if placed[i] {
            continue;
        }
        let mut letter = vec![i];
        placed[i] = true;
        let mut k = 0;
        while k < letter.len() {
            let from = letter[k];
            for (j, done) in placed.iter_mut().enumerate() {
                if !*done && touch(from, j) {
                    *done = true;
                    letter.push(j);
                }
            }
            k += 1;
        }
        letters.push(letter);
    }
    if drawn {
        letters = in_turn(&letters, n);
    }
    let mut here = from;
    let mut found = ColumnOrder {
        cuts: 0,
        searched: 0,
        order: Vec::with_capacity(n),
        first: Vec::with_capacity(n),
    };
    while !letters.is_empty() {
        let reach = |letter: &Vec<usize>| {
            here.map_or(0.0, |p| {
                ends.iter()
                    .filter(|w| letter.contains(&w.piece))
                    .map(|w| p.dist(w.start))
                    .fold(f64::INFINITY, f64::min)
            })
        };
        let k = (0..letters.len())
            .filter(|_| !drawn)
            .min_by(|&a, &b| reach(&letters[a]).total_cmp(&reach(&letters[b])))
            .unwrap_or(0);
        let letter = letters.remove(k);
        if letter.len() > SEARCH_COLUMNS {
            return None;
        }
        // The letter's columns and ways, by their places among its own.
        let its: Vec<&Vec<Polyline>> = letter.iter().map(|&i| outlines[i]).collect();
        let which: Vec<usize> = (0..ends.len())
            .filter(|&w| letter.contains(&ends[w].piece))
            .collect();
        let ways: Vec<ColumnWay> = which
            .iter()
            .map(|&w| ColumnWay {
                piece: letter.iter().position(|&i| i == ends[w].piece).unwrap_or(0),
                turned: ends[w].turned,
                start: ends[w].start,
                end: ends[w].end,
            })
            .collect();
        // (Only the last letter leads on to the next object.)
        let next = reaches_next.filter(|_| letters.is_empty());
        // (A word too long to search at once isn't told of steps cut.)
        let one = letter_cuts(
            &its,
            &ways,
            here,
            next,
            near,
            drawn,
            &HashMap::new(),
            usize::MAX,
        )?;
        found.cuts += one.cuts;
        found.searched += one.searched;
        here = one.order.last().map(|&w| ways[w].end).or(here);
        found.order.extend(one.order.iter().map(|&w| which[w]));
        found.first.extend(one.first.iter().map(|&w| which[w]));
    }
    Some(found)
}

/// [`fewest_cuts`], of no more columns than [`SEARCH_COLUMNS`].
#[allow(clippy::too_many_arguments)]
fn letter_cuts(
    outlines: &[&Vec<Polyline>],
    ends: &[ColumnWay],
    from: Option<P>,
    reaches_next: Option<&dyn Fn(P) -> bool>,
    near: f64,
    drawn: bool,
    known: &HashMap<Step, bool>,
    below: usize,
) -> Option<ColumnOrder> {
    let n = outlines.len();
    if n == 0 || n > SEARCH_COLUMNS {
        return None;
    }
    let boxes: Vec<Bounds> = outlines
        .iter()
        .map(|rings| {
            let mut b = Bounds::empty();
            rings.iter().flatten().for_each(|&p| b.add(p));
            Bounds {
                min: P::new(b.min.x - near, b.min.y - near),
                max: P::new(b.max.x + near, b.max.y + near),
            }
        })
        .collect();
    // A point is at a column when it's in it or a hop from its outline:
    // how far from it (nothing, in it), or infinitely far if further.
    let at = |p: P, i: usize| -> f64 {
        let b = &boxes[i];
        if p.x < b.min.x || p.x > b.max.x || p.y < b.min.y || p.y > b.max.y {
            return f64::INFINITY;
        }
        let rings = outlines[i];
        if crate::geom::point_inside(rings, FillRule::EvenOdd, p) {
            return 0.0;
        }
        let d = rings
            .iter()
            .map(|r| dist_to_ring(r, p))
            .fold(f64::INFINITY, f64::min);
        if d <= near { d } else { f64::INFINITY }
    };
    // Columns touch when a point of one's outline is at the other; and
    // they're one piece of ground when it's on the other, or all but.
    let mut touch = vec![false; n * n];
    let mut joined = vec![false; n * n];
    for i in 0..n {
        for j in i + 1..n {
            if !overlaps(&boxes[i], &boxes[j]) {
                continue;
            }
            let d = outlines[i]
                .iter()
                .flatten()
                .map(|&p| at(p, j))
                .chain(outlines[j].iter().flatten().map(|&p| at(p, i)))
                .fold(f64::INFINITY, f64::min);
            for k in [i * n + j, j * n + i] {
                touch[k] = d.is_finite();
                joined[k] = d <= ONE_GROUND;
            }
        }
    }
    // Each way of sewing each piece, and the columns its end is at (how
    // far from each).
    struct Way {
        piece: usize,
        start: P,
        end: P,
        end_at: Vec<f64>,
        reach: Reach,
    }
    let ways: Vec<Way> = ends
        .iter()
        .map(|w| {
            let end_at: Vec<f64> = (0..n).map(|j| at(w.end, j)).collect();
            Way {
                piece: w.piece,
                start: w.start,
                end: w.end,
                reach: Reach::of(&end_at),
                end_at,
            }
        })
        .collect();
    let (joined_bits, touch_bits) = (bit_masks(&joined, n), bit_masks(&touch, n));
    let hidden = |here: &Reach, to: usize, sewn: u64| {
        hidden_way_from(here, to, sewn, &joined_bits, &touch_bits)
    };
    // The columns that touch, a letter at a time: each is sewn whole, the
    // nearest next.
    let mut letters: Vec<Vec<usize>> = Vec::new();
    let mut placed = vec![false; n];
    for i in 0..n {
        if placed[i] {
            continue;
        }
        let mut letter = vec![i];
        placed[i] = true;
        let mut k = 0;
        while k < letter.len() {
            for j in 0..n {
                if touch[letter[k] * n + j] && !placed[j] {
                    placed[j] = true;
                    letter.push(j);
                }
            }
            k += 1;
        }
        letters.push(letter);
    }
    if drawn {
        letters = in_turn(&letters, n);
    }
    // Every order of a letter's columns, nearest next first (so the first
    // one finished is the order sewing would take anyway), keeping the
    // one with fewest cuts. (Drawn, only the next column's ways.)
    struct Search<'a> {
        ways: &'a [Way],
        /// The pieces to order (a bit each), and how many.
        letter: u64,
        count: usize,
        near: f64,
        /// The columns the thread is at as it comes to the letter.
        from_at: &'a Reach,
        hidden: &'a dyn Fn(&Reach, usize, u64) -> bool,
        /// Which columns each touches, a bit each.
        touch: &'a [u64],
        reaches_next: Option<&'a dyn Fn(P) -> bool>,
        drawn: bool,
        /// Steps tried out (see [`Step`]): whether each was cut, whatever
        /// the hop or the ground says; and the way sewn before the letter,
        /// for those.
        known: &'a HashMap<Step, bool>,
        before: usize,
        budget: usize,
        /// The steps taken.
        steps: usize,
        /// Only orders of fewer cuts than this are wanted.
        below: usize,
        first: Option<(usize, Vec<usize>)>,
        best: Option<(usize, Vec<usize>)>,
        path: Vec<usize>,
        /// The fewest cuts each state has been come to with: the columns
        /// sewn, and the way the last was.
        seen: std::collections::HashMap<(u64, usize), usize>,
    }
    impl Search<'_> {
        /// The fewest cuts sewing what's left takes: thread goes on
        /// unseen only under columns still to sew (or a hop, to one that
        /// touches), so every group of them that touches no other, but
        /// the one it goes on to, is come to by a cut (branch and bound's
        /// lower bound).
        fn least(&self, sewn: u64) -> usize {
            let mut left = self.letter & !sewn;
            let mut groups: usize = 0;
            while left != 0 {
                left &= !flood(1 << left.trailing_zeros(), left, self.touch);
                groups += 1;
            }
            groups.saturating_sub(1)
        }
        fn go(&mut self, here: Option<P>, sewn: u64, cuts: usize) {
            if self
                .best
                .as_ref()
                .map_or(self.below, |b| b.0.min(self.below))
                <= cuts + self.least(sewn)
                || (self.budget == 0 && (self.first.is_some() || self.below != usize::MAX))
            {
                return;
            }
            // (What's left to sew and where the thread is are all that
            // what follows depends on: come to again with as many cuts
            // made, there's nothing better to find from here.)
            let state = (sewn, self.path.last().copied().unwrap_or(usize::MAX));
            match self.seen.get_mut(&state) {
                Some(least) if *least <= cuts => return,
                Some(least) => *least = cuts,
                None => {
                    self.seen.insert(state, cuts);
                }
            }
            self.budget = self.budget.saturating_sub(1);
            self.steps += 1;
            if self.path.len() == self.count {
                let last = &self.ways[*self.path.last().unwrap()];
                let cuts = cuts + self.reaches_next.is_some_and(|f| !f(last.end)) as usize;
                if self.first.is_none() {
                    self.first = Some((cuts, self.path.clone()));
                }
                if self.best.as_ref().is_none_or(|b| cuts < b.0) {
                    self.best = Some((cuts, self.path.clone()));
                }
                return;
            }
            let first_left = (self.letter & !sewn).trailing_zeros() as usize;
            let mut next: Vec<(f64, usize)> = (0..self.ways.len())
                .filter(|&w| {
                    let bit = 1 << self.ways[w].piece;
                    self.letter & bit != 0
                        && sewn & bit == 0
                        && (!self.drawn || self.ways[w].piece == first_left)
                })
                // (Squared distances: they sort the same, and need no root.)
                .map(|w| {
                    let d = here.map_or(0.0, |p| {
                        let v = p - self.ways[w].start;
                        v.x * v.x + v.y * v.y
                    });
                    (d, w)
                })
                .collect();
            next.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (d, w) in next {
                let way = &self.ways[w];
                let last = self.path.last().copied().unwrap_or(self.before);
                let cut = here.is_some()
                    && match self.known.get(&(sewn, last, w)) {
                        Some(&cut) => cut,
                        None => {
                            d > self.near * self.near && {
                                let here_at: &Reach = match self.path.last() {
                                    Some(&last) => &self.ways[last].reach,
                                    None => self.from_at,
                                };
                                !(self.hidden)(here_at, way.piece, sewn)
                            }
                        }
                    };
                self.path.push(w);
                self.go(Some(way.end), sewn | 1 << way.piece, cuts + cut as usize);
                self.path.pop();
            }
        }
    }
    let mut here: Option<P> = from;
    let mut here_at = Reach::of(
        &(0..n)
            .map(|j| from.map_or(f64::INFINITY, |p| at(p, j)))
            .collect::<Vec<f64>>(),
    );
    let mut sewn: u64 = 0;
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut first_order: Vec<usize> = Vec::with_capacity(n);
    let mut total = 0;
    let mut searched = 0;
    while !letters.is_empty() {
        let reach = |letter: &Vec<usize>| {
            here.map_or(0.0, |p| {
                ways.iter()
                    .filter(|w| letter.contains(&w.piece))
                    .map(|w| p.dist(w.start))
                    .fold(f64::INFINITY, f64::min)
            })
        };
        let k = (0..letters.len())
            .filter(|_| !drawn)
            .min_by(|&a, &b| reach(&letters[a]).total_cmp(&reach(&letters[b])))
            .unwrap_or(0);
        let letter = letters.remove(k);
        let mut search = Search {
            ways: &ways,
            letter: letter.iter().fold(0, |m, &i| m | 1 << i),
            count: letter.len(),
            near,
            from_at: &here_at,
            hidden: &hidden,
            touch: &touch_bits,
            // (Only the last letter leads on to the next object.)
            reaches_next: reaches_next.filter(|_| letters.is_empty()),
            drawn,
            known,
            before: order.last().copied().unwrap_or(usize::MAX),
            // (A letter of many columns as long as one of few: the best
            // order it finds in that time. In the order drawn, there are
            // only a few ways to each.)
            budget: if drawn { usize::MAX } else { ORDER_BUDGET },
            steps: 0,
            below,
            first: None,
            best: None,
            path: Vec::new(),
            seen: std::collections::HashMap::new(),
        };
        search.go(here, sewn, 0);
        searched += search.steps;
        let (first, (cuts, path)) = (search.first?, search.best?);
        total += cuts;
        first_order.extend(first.1);
        let last = &ways[*path.last()?];
        let letter = search.letter;
        here = Some(last.end);
        here_at = Reach::of(&last.end_at);
        sewn |= letter;
        order.extend(path);
    }
    Some(ColumnOrder {
        cuts: total,
        searched,
        order,
        first: first_order,
    })
}

/// Columns that touch (`letters`, of `n` columns) taken in the order
/// they're drawn: each run of them one after another in the same letter,
/// in turn. (A column of one letter sewn between two of another's parts
/// it in two, as sewing does.)
fn in_turn(letters: &[Vec<usize>], n: usize) -> Vec<Vec<usize>> {
    let mut of = vec![0; n];
    for (k, letter) in letters.iter().enumerate() {
        for &i in letter {
            of[i] = k;
        }
    }
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for i in 0..n {
        match runs.last_mut() {
            Some(run) if run.last().is_some_and(|&j| of[j] == of[i]) => run.push(i),
            _ => runs.push(vec![i]),
        }
    }
    runs
}

/// How far the stitches of a column that ends against the side of another
/// are carried on under it, units.
const BUTT_LAP: f64 = 4.0;

/// Where the stitches of each of a part's columns run on past its ends:
/// one that ends against the side of another (a stem at its serif, a bar
/// at a stem, a bowl at its stem) is sewn a little way under it.
///
/// Ended at the cut between them, the stem's last stitch lies along the
/// cut, and the serif's come down to it end on: a row of points, with the
/// fabric showing between them. A row of the stem's stitches
/// [`BUTT_LAP`] past the cut lies across those points.
///
/// Not an end that meets the end of another column (the halves
/// [`through`] makes, a stroke cut across): their stitches lie side by
/// side there, as they do within a column. And the columns are left as
/// they are, ending where they meet: it's only stitches that lap.
fn laps(columns: &[Rails], inside: &dyn Fn(P) -> bool) -> Vec<satin::Laps> {
    // Each open column's ends: the column, whether it's the end it starts
    // at, and where its rails end there.
    let ends: Vec<(usize, bool, P, P)> = columns
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.closed && r.a.len() > 1 && r.b.len() > 1)
        .flat_map(|(k, r)| {
            [
                (k, true, r.a[0], r.b[0]),
                (k, false, r.a[r.a.len() - 1], r.b[r.b.len() - 1]),
            ]
        })
        .collect();
    // The way a rail runs out of its end (from a few units back along it).
    let out = |rail: &[P], start: bool| -> P {
        let tip = if start { rail[0] } else { rail[rail.len() - 1] };
        let mut back = tip;
        for k in 1..rail.len() {
            back = if start {
                rail[k]
            } else {
                rail[rail.len() - 1 - k]
            };
            if back.dist(tip) >= 3.0 {
                break;
            }
        }
        tip - back
    };
    let mut laps = vec![satin::NO_LAPS; columns.len()];
    for (i, &(k, start, pa, pb)) in ends.iter().enumerate() {
        let met = ends.iter().enumerate().any(|(j, &(_, _, qa, qb))| {
            j != i
                && ((pa.dist(qa) < 1.0 && pb.dist(qb) < 1.0)
                    || (pa.dist(qb) < 1.0 && pb.dist(qa) < 1.0))
        });
        let r = &columns[k];
        let u = out(&r.a, start).norm() + out(&r.b, start).norm();
        if pa.dist(pb) < 1.0 || met || u.len() < 1e-6 {
            continue;
        }
        let u = u.norm();
        // An end on the shape's own outline has nothing beyond it.
        let mid = pa.lerp(pb, 0.5);
        if !inside(mid) || !inside(mid + u) {
            continue;
        }
        // (Less far under a thin column: no further than halfway
        // through it, where its own stitches hide the row.)
        laps[k][if start { 0 } else { 1 }] = [BUTT_LAP, BUTT_LAP * 0.5]
            .into_iter()
            .find(|&lap| {
                [pa, pb, mid]
                    .iter()
                    .all(|&p| inside(p + u * lap) && inside(p + u * (2.0 * lap)))
            })
            .map(|lap| (pa + u * lap, pb + u * lap));
    }
    laps
}

/// The shortest part [`through`] cuts a column into, and the hop it takes
/// the thread to make without a cut, units: the planner's usual one
/// (`PlanOptions::max_jump_no_trim`, 10) less the most a stitch reaches
/// past its rails (pull compensation, 0.3 mm at most). `through` works
/// from the outline alone, before there are stitches or settings, from
/// where the rails end; an order that needs a whole hop there needs more
/// than one from where the stitches end, and is cut after all. (Its
/// guess is the one thing in a satin's order not tried out: see
/// `plan_shown`. Measured on the letter census, this margin saves CAT's
/// letters 23 cuts and costs Cut's 3.)
const THROUGH_PART: f64 = 12.0;
const THROUGH_HOP: f64 = 7.0;
/// The most choices of joins [`through`] tries (a bound on its time:
/// every choice of up to 6 joins). Capitals with a serif on every stroke
/// need several cut: every choice of up to 4 (15 tries) cost the letter
/// census over a hundred cuts.
const THROUGH_TRIES: usize = 63;

/// A part's columns, with those cut in two that the thread couldn't
/// otherwise get past without a cut of its own.
///
/// A column that another ends against the side of (the serif under a
/// stem) is a dead end: sewn from one tip to the other, the thread comes
/// out at a tip, with the stem it has to go on from back in the middle
/// and the serif just sewn in the way. A letter can end on one of those,
/// but not on two (the feet of an n, the serifs of a v). Cut in two where
/// the stem meets it, each half is sewn from its tip in to the stem, and
/// the thread is where it needs to be. (The halves' stitches lie the same
/// way, side by side, so the join doesn't show as one.) Only where that
/// saves a cut, and at as few joins as do.
fn through(columns: Vec<Rails>, touching: &[P]) -> Vec<Rails> {
    let n = columns.len();
    if !(2..=ORDER_COLUMNS).contains(&n) || columns.iter().any(|r| r.closed) {
        if trim_report() && n >= 2 {
            eprintln!(
                "through: {n} columns, not cut ({})",
                if n > ORDER_COLUMNS {
                    "too many"
                } else {
                    "a ring among them"
                }
            );
        }
        return columns;
    }
    let tips = |r: &Rails| -> Option<(P, P)> {
        Some((
            r.a.first()?.lerp(*r.b.first()?, 0.5),
            r.a.last()?.lerp(*r.b.last()?, 0.5),
        ))
    };
    let outline = Rails::outline;
    // (The fewest cuts an order of them takes, if fewer than `below`.)
    let cuts = |columns: &[Rails], below: usize| -> Option<usize> {
        let outlines: Vec<Vec<Polyline>> = columns.iter().map(outline).collect();
        let mut ends = Vec::with_capacity(columns.len() * 2);
        for (piece, r) in columns.iter().enumerate() {
            let (start, end) = tips(r)?;
            // (It can end on either side: see `end_stitch`.)
            for (turned, start, sides) in [
                (false, start, [r.a.last()?, r.b.last()?]),
                (true, end, [r.a.first()?, r.b.first()?]),
            ] {
                for &end in sides {
                    ends.push(ColumnWay {
                        piece,
                        turned,
                        start,
                        end,
                    });
                }
            }
        }
        let outlines: Vec<&Vec<Polyline>> = outlines.iter().collect();
        fewest_cuts(
            &outlines,
            &ends,
            None,
            None,
            THROUGH_HOP,
            false,
            &HashMap::new(),
            below,
        )
        .map(|found| found.cuts)
    };
    let Some(whole) = cuts(&columns, usize::MAX) else {
        return columns;
    };
    if whole == 0 {
        return columns;
    }
    // Where a column's end is against another's side, far enough from
    // that one's ends to cut it there: the column to cut, and where.
    let mut joins: Vec<(usize, P)> = Vec::new();
    for (i, r) in columns.iter().enumerate() {
        let Some((start, end)) = tips(r) else {
            continue;
        };
        for tip in [start, end] {
            for (j, bar) in columns.iter().enumerate() {
                let ring = &outline(bar)[0];
                if j != i
                    && (crate::geom::point_inside(
                        std::slice::from_ref(ring),
                        FillRule::EvenOdd,
                        tip,
                    ) || dist_to_ring(ring, tip) <= THROUGH_HOP)
                    && satin::split_at(bar, tip, THROUGH_PART).is_some()
                {
                    joins.push((j, tip));
                }
            }
        }
    }
    // And where two parts of the letter touch at a point (`touching`),
    // each column that runs past it: the thread goes from one part to the
    // other there, and nowhere else.
    for &at in touching {
        for (j, bar) in columns.iter().enumerate() {
            if dist_to_ring(&outline(bar)[0], at) <= THROUGH_HOP
                && satin::split_at(bar, at, THROUGH_PART).is_some()
            {
                joins.push((j, at));
            }
        }
    }
    if joins.is_empty() {
        return columns;
    }
    // The columns with some of the joins cut (a bit each).
    let cut_at = |chosen: u32| -> Vec<Rails> {
        let mut out = Vec::with_capacity(n + joins.len());
        for (j, bar) in columns.iter().enumerate() {
            let mut parts = vec![bar.clone()];
            for (k, &(at, tip)) in joins.iter().enumerate() {
                if at != j || chosen & (1 << k) == 0 {
                    continue;
                }
                // (A column two others end against is cut at each: the
                // part this one is on is the one that can be.)
                if let Some((i, (head, tail))) = parts
                    .iter()
                    .enumerate()
                    .find_map(|(i, part)| Some((i, satin::split_at(part, tip, THROUGH_PART)?)))
                {
                    parts.splice(i..=i, [head, tail]);
                }
            }
            out.extend(parts);
        }
        out
    };
    // Every choice of them, fewest joins first (at most 31: a bit each),
    // for as many tries as `THROUGH_TRIES`: the first that saves most.
    let m = joins.len().min(31) as u32;
    let mut best: Option<(usize, Vec<Rails>)> = None;
    let mut tries = 0;
    'sizes: for k in 1..=m {
        // (The choices of k of them in turn: Gosper's hack.)
        let mut chosen: u32 = (1 << k) - 1;
        while chosen < 1 << m {
            if tries == THROUGH_TRIES {
                break 'sizes;
            }
            tries += 1;
            let cut = cut_at(chosen);
            if cut.len() <= ORDER_COLUMNS
                && let Some(c) = cuts(&cut, best.as_ref().map_or(whole, |b| b.0))
            {
                best = Some((c, cut));
                if c == 0 {
                    break 'sizes;
                }
            }
            let low = chosen & chosen.wrapping_neg();
            let up = chosen + low;
            chosen = up | (((chosen ^ up) >> 2) / low);
        }
    }
    if trim_report() {
        eprintln!(
            "through: {n} columns, expected {whole} cuts whole; {}",
            match &best {
                Some((c, cut)) => format!("cut at {} joins, expecting {c}", cut.len() - n),
                None => "no join cut saves one".to_string(),
            }
        );
    }
    best.map_or(columns, |b| b.1)
}

/// The order to sew a fill's sections in, and which way each (true: from
/// its other end), when that keeps the thread's way between them off
/// rows already sewn: under rows still to come instead, where it's
/// covered. `None` to go on as usual (nearest next): when that's as
/// good, or there are too many sections to try every order.
///
/// Going to whichever section is nearest can sew the big one first and
/// leave a small one beside where it began; the way to that is then
/// across the big one, a line of running stitch on top of the fill.
fn section_order(
    pieces: &[Piece],
    from: Option<P>,
    area: &(Vec<Polyline>, FillRule),
    ring: &[Polyline],
    opt: &PlanOptions,
) -> Option<Vec<(usize, bool)>> {
    let n = pieces.len();
    if !(2..=6).contains(&n)
        || pieces
            .iter()
            .any(|p| p.ring || p.alt.is_none() || p.pts.len() < 2)
    {
        return None;
    }
    let rows: Vec<Vec<(P, P)>> = pieces.iter().map(|p| straight_runs(&p.pts)).collect();
    // A section sewn one way or the other: where it starts and ends.
    let ends = |i: usize, turned: bool| -> (P, P) {
        let pts = if turned {
            pieces[i].alt.as_ref().unwrap()
        } else {
            &pieces[i].pts
        };
        (pts[0], *pts.last().unwrap())
    };
    // The ways from the end of one to the start of another: each as how
    // long it is, how many rows of each section it crosses, and whether
    // it's round the edge (where a line over rows shows least, and is how
    // it's always been done) or straight across. (None at all: the
    // thread is cut.) A short hop crosses nothing that matters.
    type Way = (f64, Vec<usize>, bool);
    let ways = |a: P, b: P| -> Vec<Way> {
        let gap = a.dist(b);
        if gap <= opt.max_jump_no_trim {
            return vec![(gap, vec![0; n], false)];
        }
        let mut out: Vec<Way> = Vec::new();
        let mut add = |path: Vec<P>, edge: bool| {
            let len = crate::geom::polyline_length(&path);
            out.push((
                len,
                rows.iter().map(|r| crossings(&path, r)).collect(),
                edge,
            ));
        };
        if gap <= opt.travel_inside && crate::geom::segment_inside(&area.0, area.1, a, b, 2.0) {
            add(vec![a, b], false);
        }
        if opt.travel_route > 0.0
            && let Some(mut path) = route_along(ring, a, b, opt.travel_route)
        {
            path.insert(0, a);
            add(path, true);
        }
        out
    };
    // (Cuts, rows crossed round the edge, length): less is better, in
    // that order. Straight across rows already sewn isn't a way at all.
    type Cost = (usize, usize, f64);
    let plus = |a: Cost, b: Cost| (a.0 + b.0, a.1 + b.1, a.2 + b.2);
    let less =
        |a: &Cost, b: &Cost| (a.0, a.1) < (b.0, b.1) || ((a.0, a.1) == (b.0, b.1) && a.2 < b.2);
    let mut between: Vec<Vec<Way>> = Vec::with_capacity(4 * n * n);
    for s in 0..2 * n {
        for t in 0..2 * n {
            between.push(if s / 2 == t / 2 {
                Vec::new()
            } else {
                ways(ends(s / 2, s % 2 == 1).1, ends(t / 2, t % 2 == 1).0)
            });
        }
    }
    // From state `s` (a section and which way it was sewn) to `t`, with
    // the sections in `sewn` done.
    let step = |sewn: usize, s: usize, t: usize| -> Cost {
        between[s * 2 * n + t]
            .iter()
            .filter_map(|(len, over, edge)| {
                let seen: usize = (0..n).filter(|k| sewn >> k & 1 == 1).map(|k| over[k]).sum();
                // Over rows already sewn it isn't a way, unless the
                // document says to sew over them: then round the edge is,
                // the fewer rows the better.
                match (seen > FEW_ROWS, opt.over_sewn && *edge) {
                    (false, _) => Some((0, 0, *len)),
                    (true, true) => Some((0, seen, *len)),
                    (true, false) => None,
                }
            })
            .reduce(|a, b| if less(&b, &a) { b } else { a })
            .unwrap_or((1, 0, 0.0))
    };
    let first = |t: usize| -> Cost {
        (
            0,
            0,
            from.map_or(0.0, |f| f.dist(ends(t / 2, t % 2 == 1).0)),
        )
    };

    // As usual: whichever start is nearest, each time.
    let usual: Cost = {
        let mut here = from.unwrap_or(pieces[0].pts[0]);
        let (mut sewn, mut at, mut cost): (usize, Option<usize>, Cost) = (0, None, (0, 0, 0.0));
        while sewn != (1 << n) - 1 {
            let t = (0..2 * n)
                .filter(|t| sewn >> (t / 2) & 1 == 0)
                .min_by(|a, b| {
                    here.dist(ends(a / 2, a % 2 == 1).0)
                        .total_cmp(&here.dist(ends(b / 2, b % 2 == 1).0))
                })
                .unwrap();
            cost = plus(cost, at.map_or_else(|| first(t), |s| step(sewn, s, t)));
            sewn |= 1 << (t / 2);
            here = ends(t / 2, t % 2 == 1).1;
            at = Some(t);
        }
        cost
    };
    if (usual.0, usual.1) == (0, 0) {
        return None;
    }
    // Every order: the best way to have sewn each set of sections,
    // ending on each.
    let full = (1usize << n) - 1;
    let mut best: Vec<Option<(Cost, usize)>> = vec![None; (full + 1) * 2 * n];
    for t in 0..2 * n {
        best[(1 << (t / 2)) * 2 * n + t] = Some((first(t), usize::MAX));
    }
    for sewn in 1..=full {
        for s in 0..2 * n {
            let Some((cost, _)) = best[sewn * 2 * n + s] else {
                continue;
            };
            for t in (0..2 * n).filter(|t| sewn >> (t / 2) & 1 == 0) {
                let next = plus(cost, step(sewn, s, t));
                let slot = &mut best[(sewn | 1 << (t / 2)) * 2 * n + t];
                if slot.as_ref().is_none_or(|(c, _)| less(&next, c)) {
                    *slot = Some((next, s));
                }
            }
        }
    }
    let (mut at, cost) = (0..2 * n)
        .filter_map(|t| best[full * 2 * n + t].map(|(c, _)| (t, c)))
        .reduce(|a, b| if less(&b.1, &a.1) { b } else { a })?;
    if (cost.0, cost.1) >= (usual.0, usual.1) {
        return None;
    }
    let mut order = Vec::with_capacity(n);
    let mut sewn = full;
    loop {
        order.push((at / 2, at % 2 == 1));
        let (_, came) = best[sewn * 2 * n + at]?;
        sewn &= !(1 << (at / 2));
        if came == usize::MAX {
            break;
        }
        at = came;
    }
    order.reverse();
    Some(order)
}

/// How far `p` is from a closed ring's outline.
fn dist_to_ring(ring: &[P], p: P) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let ab = b - a;
            let len2 = ab.x * ab.x + ab.y * ab.y;
            let t = if len2 > 0.0 {
                (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            a.lerp(b, t).dist(p)
        })
        .fold(f64::INFINITY, f64::min)
}

/// Add `pts` to a run, leaving out any nearer than `min` to the point
/// before it (so no stitch is shorter than that). Returns how many went in.
fn extend_run(run: &mut Polyline, pts: impl IntoIterator<Item = P>, min: f64) -> usize {
    let before = run.len();
    for q in pts {
        if run.last().is_none_or(|last| last.dist(q) >= min) {
            run.push(q);
        }
    }
    run.len() - before
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
        for (a, b) in &mut self.bridges {
            (*a, *b) = (*a - c, *b - c);
        }
        for q in self.blends.iter_mut().flatten() {
            *q = *q - c;
        }
        for q in self.routes.iter_mut().flat_map(|r| &mut r.1) {
            *q = *q - c;
        }
    }

    pub fn stitch_count(&self) -> usize {
        self.blocks
            .iter()
            .flat_map(|b| &b.runs)
            .map(|r| r.pts.len())
            .sum()
    }

    /// Put the stitches of the input shapes `picked` (see
    /// [`Design::stitch_shapes`]) in blocks of their own: a block with
    /// their stitches and others' becomes two in the same thread, the
    /// others' and then theirs, each run cut where stitching went to the
    /// other. So that they can be shown apart from the rest (moving with a
    /// drag, or not at all): it isn't a plan to sew, with its extra
    /// blocks. True if any block was split.
    pub fn split_shapes(&mut self, picked: &[usize]) -> bool {
        if picked.is_empty()
            || self.stitch_shapes.len() != self.blocks.len()
            || self.block_shapes.len() != self.blocks.len()
        {
            return false;
        }
        // The stretches of `run` that are (or aren't) the picked shapes'.
        fn part(
            run: &Run,
            spans: &[(usize, usize)],
            of: impl Fn(usize) -> bool,
        ) -> Vec<(Run, Vec<(usize, usize)>)> {
            let mut out = Vec::new();
            let mut piece: Option<(Run, Vec<(usize, usize)>)> = None;
            let mut cut = run.trim_before;
            for (k, &(from, shape)) in spans.iter().enumerate() {
                let to = spans.get(k + 1).map_or(run.pts.len(), |s| s.0);
                let (from, to) = (from.min(run.pts.len()), to.min(run.pts.len()));
                if !of(shape) || from >= to {
                    if let Some(done) = piece.take() {
                        out.push(done);
                    }
                    cut |= !of(shape);
                    continue;
                }
                let (r, owners) = piece.get_or_insert_with(|| {
                    (
                        Run {
                            pts: Vec::new(),
                            trim_before: cut,
                        },
                        Vec::new(),
                    )
                });
                owners.push((r.pts.len(), shape));
                r.pts.extend_from_slice(&run.pts[from..to]);
            }
            out.extend(piece);
            out
        }
        let mut any = false;
        let mut blocks = Vec::with_capacity(self.blocks.len() + 1);
        let mut block_shapes = Vec::with_capacity(self.blocks.len() + 1);
        let mut stitch_shapes = Vec::with_capacity(self.blocks.len() + 1);
        let old = (
            std::mem::take(&mut self.blocks),
            std::mem::take(&mut self.block_shapes),
            std::mem::take(&mut self.stitch_shapes),
        );
        for ((blk, shapes), owners) in old.0.into_iter().zip(old.1).zip(old.2) {
            let theirs = |s: usize| picked.contains(&s);
            let mixed = owners.len() == blk.runs.len()
                && owners.iter().flatten().any(|(_, s)| theirs(*s))
                && owners.iter().flatten().any(|(_, s)| !theirs(*s));
            if !mixed {
                blocks.push(blk);
                block_shapes.push(shapes);
                stitch_shapes.push(owners);
                continue;
            }
            any = true;
            for pick in [false, true] {
                let (runs, spans): (Vec<Run>, Vec<Vec<(usize, usize)>>) = blk
                    .runs
                    .iter()
                    .zip(&owners)
                    .flat_map(|(run, spans)| part(run, spans, |s| theirs(s) == pick))
                    .unzip();
                if runs.is_empty() {
                    continue;
                }
                blocks.push(Block {
                    color: blk.color,
                    runs,
                });
                block_shapes.push(
                    shapes
                        .iter()
                        .copied()
                        .filter(|s| theirs(*s) == pick)
                        .collect(),
                );
                stitch_shapes.push(spans);
            }
        }
        self.blocks = blocks;
        self.block_shapes = block_shapes;
        self.stitch_shapes = stitch_shapes;
        any
    }

    /// Take the stitches of the input shapes `gone` out of the plan (see
    /// [`Design::stitch_shapes`]): what's left is the others' stitches as
    /// they were, with a cut where stitching was taken out of the middle
    /// of a run. For showing a plan without objects that have just gone
    /// (or are hidden) until it's planned again: it isn't a plan to sew,
    /// since the others aren't planned afresh without them. True if
    /// anything was taken out.
    pub fn drop_shapes(&mut self, gone: &[usize]) -> bool {
        if gone.is_empty()
            || self.stitch_shapes.len() != self.blocks.len()
            || self.block_shapes.len() != self.blocks.len()
        {
            return false;
        }
        self.split_shapes(gone);
        // Now every block is theirs or the others'.
        let keep: Vec<bool> = self
            .stitch_shapes
            .iter()
            .map(|runs| !runs.iter().flatten().any(|(_, s)| gone.contains(s)))
            .collect();
        if keep.iter().all(|k| *k) {
            return false;
        }
        let mut k = keep.iter();
        self.blocks.retain(|_| *k.next().unwrap());
        let mut k = keep.iter();
        self.block_shapes.retain(|_| *k.next().unwrap());
        let mut k = keep.iter();
        self.stitch_shapes.retain(|_| *k.next().unwrap());
        for shapes in &mut self.block_shapes {
            shapes.retain(|s| !gone.contains(s));
        }
        for &s in gone {
            if let Some(n) = self.per_shape.get_mut(s) {
                *n = 0;
            }
        }
        true
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
    use super::measure::{Lettered, layers, lettered, seams};
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
            outline: false,
            origin: P::default(),
            joined: Vec::new(),
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
    fn dropping_a_shapes_stitches_leaves_the_others_as_they_were() {
        // Three squares in one thread: one block, sewn one after another.
        let sq = |x: f64| Shape {
            id: format!("s{x}"),
            part: crate::svg::Part::Fill,
            label: String::new(),
            color: [0, 0, 0],
            paint: [0, 0, 0],
            geometry: Geometry::Fill {
                rings: vec![vec![
                    P::new(x, 0.0),
                    P::new(x + 100.0, 0.0),
                    P::new(x + 100.0, 100.0),
                    P::new(x, 100.0),
                ]],
                rule: FillRule::NonZero,
            },
            params: Params {
                kind: Some(Kind::Fill),
                ..Default::default()
            },
            uid: x as usize,
            groups: Vec::new(),
            outline: false,
            origin: P::default(),
            joined: Vec::new(),
        };
        let shapes = [sq(0.0), sq(105.0), sq(210.0)];
        let whole = plan("t", &shapes, &PlanOptions::default());
        assert_eq!(whole.blocks.len(), 1);
        let inside = |d: &Design, x: f64| {
            d.blocks
                .iter()
                .flat_map(|b| &b.runs)
                .flat_map(|r| &r.pts)
                .filter(|p| p.x > x + 4.0 && p.x < x + 96.0)
                .count()
        };
        let (a, b, c) = (
            inside(&whole, 0.0),
            inside(&whole, 105.0),
            inside(&whole, 210.0),
        );
        assert!(a > 50 && b > 50 && c > 50);
        // The middle one out: nothing of it left, the other two whole.
        let mut d = whole.clone();
        assert!(d.drop_shapes(&[1]));
        assert_eq!(inside(&d, 105.0), 0);
        assert_eq!((inside(&d, 0.0), inside(&d, 210.0)), (a, c));
        assert_eq!(d.per_shape[1], 0);
        assert_eq!(d.block_shapes, vec![vec![0, 2]]);
        assert_eq!(d.stitch_shapes.len(), 1);
        assert_eq!(d.stitch_shapes[0].len(), d.blocks[0].runs.len());
        assert!(
            d.blocks[0].runs.iter().skip(1).any(|r| r.trim_before),
            "a cut where it was"
        );
        // Or in a block of its own, for showing it apart from the others.
        let mut e = whole.clone();
        assert!(e.split_shapes(&[1]));
        assert_eq!(e.block_shapes, vec![vec![0, 2], vec![1]]);
        assert_eq!(e.stitch_count(), whole.stitch_count());
        assert_eq!(e.blocks[0].color, e.blocks[1].color);
        let of = |blk: &Block, x: f64| {
            blk.runs
                .iter()
                .flat_map(|r| &r.pts)
                .filter(|p| p.x > x + 4.0 && p.x < x + 96.0)
                .count()
        };
        assert_eq!((of(&e.blocks[1], 105.0), of(&e.blocks[0], 105.0)), (b, 0));
        assert_eq!((of(&e.blocks[0], 0.0), of(&e.blocks[0], 210.0)), (a, c));
        assert!(!e.split_shapes(&[1]), "already apart");
        // All of them out: nothing left at all.
        assert!(d.drop_shapes(&[0, 2]));
        assert!(d.blocks.is_empty() && d.block_shapes.is_empty());
        // Nothing to take out: nothing changes.
        let mut e = whole.clone();
        assert!(!e.drop_shapes(&[7]));
        assert_eq!(e.stitch_count(), whole.stitch_count());
    }

    #[test]
    fn a_caught_panic_is_known_to_be_one_while_its_caught() {
        assert!(!catching());
        // (Quiet: this panic is the point.)
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let inside = caught(|| {
            let seen = catching();
            let nested = caught(|| -> bool { panic!("inner") });
            assert!(nested.is_err() && catching(), "still inside the outer one");
            seen
        });
        let failed = caught(|| -> u8 { panic!("odd geometry") });
        std::panic::set_hook(hook);
        assert_eq!(inside.ok(), Some(true));
        assert!(failed.is_err());
        assert!(!catching(), "and not afterwards");
    }

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

    /// What one plan keeps for the next (satins' columns, the zones travel
    /// hides in, fills' travel rings) never changes a plan: with any one
    /// object moved, or gone, the plan from a warm cache is the plan from
    /// nothing, stitch for stitch.
    #[test]
    fn a_plan_from_a_warm_cache_is_the_plan_from_nothing() {
        for file in ["badge.svg", "groups.svg", "lettering.svg"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("examples")
                .join(file);
            let base = crate::document::Document::open(&path).unwrap();
            let opt = PlanOptions {
                remove_overlaps: true,
                ..base.plan_options()
            };
            let mut cache = PlanCache::default();
            let whole = plan("t", &base.shapes, &opt);
            assert!(same_stitches(
                &plan_cached("t", &base.shapes, &opt, &mut cache),
                &whole
            ));
            for i in 0..base.shapes.len() {
                let mut doc = base.clone();
                doc.preview_transform_from(&base, &[i], [1.0, 0.0, 0.0, 1.0, 37.0, -12.0]);
                assert!(
                    same_stitches(
                        &plan_cached("t", &doc.shapes, &opt, &mut cache),
                        &plan("t", &doc.shapes, &opt)
                    ),
                    "{file}: object {i} moved"
                );
                let rest: Vec<Shape> = (0..base.shapes.len())
                    .filter(|&j| j != i)
                    .map(|j| base.shapes[j].clone())
                    .collect();
                assert!(
                    same_stitches(
                        &plan_cached("t", &rest, &opt, &mut cache),
                        &plan("t", &rest, &opt)
                    ),
                    "{file}: object {i} gone"
                );
                // And back as it was: what was kept is still right.
                assert!(
                    same_stitches(&plan_cached("t", &base.shapes, &opt, &mut cache), &whole),
                    "{file}: object {i} back"
                );
            }
            // A setting changed: a satin's columns are its settings' too.
            for i in 0..base.shapes.len() {
                let mut shapes = base.shapes.clone();
                shapes[i].params.kind = Some(match shapes[i].params.kind {
                    Some(Kind::Satin) => Kind::Fill,
                    _ => Kind::Satin,
                });
                assert!(
                    same_stitches(
                        &plan_cached("t", &shapes, &opt, &mut cache),
                        &plan("t", &shapes, &opt)
                    ),
                    "{file}: object {i} sewn another way"
                );
            }
        }
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
    fn a_blanket_outline_keeps_corners_and_points_into_the_shape() {
        let all = |d: &Design| -> Vec<P> {
            d.blocks
                .iter()
                .flat_map(|b| &b.runs)
                .flat_map(|r| r.pts.iter().copied())
                .collect()
        };
        // A square's outline, drawn either way round: the line goes
        // through all four corners, and no prong leaves the square.
        for reversed in [false, true] {
            let mut ring = sq(0.0, 0.0, 200.0);
            if reversed {
                ring.reverse();
            }
            let mut s = shape(
                Geometry::Stroke {
                    lines: vec![(ring.clone(), true)],
                    width: 20.0,
                },
                Kind::Blanket,
                [0, 0, 0],
            );
            s.outline = true;
            let pts = all(&plan("t", &[s], &PlanOptions::default()));
            for c in &ring {
                assert!(
                    pts.iter().any(|p| p.dist(*c) < 0.5),
                    "through the corner {c:?} (reversed: {reversed})"
                );
            }
            assert!(
                pts.iter()
                    .all(|p| p.x > -1.0 && p.y > -1.0 && p.x < 201.0 && p.y < 201.0),
                "none point outward (reversed: {reversed})"
            );
            let inner = pts
                .iter()
                .filter(|p| p.x > 10.0 && p.x < 190.0 && p.y > 10.0 && p.y < 190.0)
                .count();
            assert!(inner > 20, "prongs reach in: {inner}");
        }
        // A square with a square hole: the hole's prongs point away from
        // the hole, into what's filled.
        let mut s = shape(
            Geometry::Stroke {
                lines: vec![(sq(0.0, 0.0, 300.0), true), (sq(100.0, 100.0, 100.0), true)],
                width: 20.0,
            },
            Kind::Blanket,
            [0, 0, 0],
        );
        s.outline = true;
        let pts = all(&plan("t", &[s], &PlanOptions::default()));
        assert!(
            pts.iter()
                .all(|p| !(p.x > 105.0 && p.x < 195.0 && p.y > 105.0 && p.y < 195.0)),
            "none in the hole"
        );
        assert!(
            pts.iter()
                .all(|p| p.x > -1.0 && p.y > -1.0 && p.x < 301.0 && p.y < 301.0)
        );
        // Tight little curves (a dot under a millimetre across, the corners
        // of a rounded box) aren't a prong at every point: no closer
        // together than half the spacing asked for.
        let dot: Polyline = (0..10)
            .map(|k| {
                let a = std::f64::consts::TAU * k as f64 / 10.0;
                P::new(4.0 * a.cos(), 4.0 * a.sin())
            })
            .collect();
        let around = 2.0 * std::f64::consts::PI * 4.0;
        let b = blanket(&dot, true, 30.0, 25.0, 1.0);
        let prongs = b.iter().filter(|p| p.len() > 6.0).count();
        assert!(
            prongs as f64 <= around / 15.0 + 2.0,
            "{prongs} prongs round a {around:.0}-unit dot"
        );
        // A line (not a shape's outline) keeps its prongs on its left, and
        // its corner.
        let bend = vec![P::new(0.0, 0.0), P::new(100.0, 0.0), P::new(100.0, 100.0)];
        let b = blanket(&bend, false, 30.0, 25.0, 1.0);
        assert!(b.iter().any(|p| p.dist(P::new(100.0, 0.0)) < 1e-6));
        let on_line = |p: &P| (p.y.abs() < 1e-6 && p.x <= 100.0) || ((p.x - 100.0).abs() < 1e-6);
        assert!(b.iter().filter(|p| !on_line(p)).count() >= 6, "{b:?}");
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
    fn a_gap_is_sewn_across_where_the_thread_blends_in() {
        let red = [200, 0, 0];
        // Two red squares 2 mm apart, on a backdrop sewn first. (The
        // thread leaves the first at its far side: 10 mm back over its own
        // red, then 2 mm of backdrop.)
        let on = |backdrop: [u8; 3], opt: &PlanOptions| {
            let under = shape(fill(sq(-50.0, -50.0, 400.0)), Kind::Fill, backdrop);
            let a = shape(fill(sq(0.0, 0.0, 100.0)), Kind::Fill, red);
            let b = shape(fill(sq(120.0, 0.0, 100.0)), Kind::Fill, red);
            plan("p", &[under, a, b], opt)
        };
        let blend = PlanOptions {
            blend: 150.0,
            ..PlanOptions::default()
        };
        // Light red under it: the red thread goes across, and nothing
        // says it shows.
        let d = on([255, 120, 120], &blend);
        let trims_light = d.trim_count();
        assert_eq!(d.blends.len(), 1, "sewn across where it blends");
        assert!(d.bridges.is_empty(), "not a bridge in plain view");
        assert_eq!(crate::checks::exposed_travel(&d), []);
        // Black under it: cut, as without.
        let d = on([0, 0, 0], &blend);
        assert!(d.blends.is_empty());
        assert_eq!(d.trim_count(), trims_light + 1);
        // And off, it's cut on anything.
        let d = on([255, 120, 120], &PlanOptions::default());
        assert!(d.blends.is_empty());
        assert_eq!(d.trim_count(), trims_light + 1);
        // How far it reaches falls off with how plainly it would show.
        let reach = |a, b| blend_reach(&blend, contrast(a, b));
        assert_eq!(reach(red, red), 150.0);
        assert_eq!(reach([255; 3], [0; 3]), 0.0);
        let (light, navy) = (
            reach([255, 120, 120], red),
            reach([20, 30, 90], [30, 80, 200]),
        );
        assert!((75.0..105.0).contains(&light), "{light}");
        assert!(
            navy > light,
            "a darker shade of the same blue is nearer still"
        );
        // On bare cloth it's the cloth's color that counts.
        let apart = |cloth: [u8; 3], thread: [u8; 3]| {
            let a = shape(fill(sq(0.0, 0.0, 100.0)), Kind::Fill, thread);
            let b = shape(fill(sq(120.0, 0.0, 100.0)), Kind::Fill, thread);
            plan(
                "p",
                &[a, b],
                &PlanOptions {
                    cloth,
                    ..blend.clone()
                },
            )
            .blends
            .len()
        };
        assert_eq!(
            apart([250, 250, 250], [255, 255, 255]),
            1,
            "white on a white shirt"
        );
        assert_eq!(apart([250, 250, 250], [0, 0, 0]), 0, "black on white");
    }

    #[test]
    fn a_gap_is_sewn_round_by_what_the_thread_is_like() {
        let red = [200, 0, 0];
        // Two small red squares 6 mm apart on a backdrop of nearly the
        // same red, with a black bar lying between them, past both
        // their tops and bottoms.
        let bar = |x: f64, y: f64, w: f64, h: f64| {
            fill(vec![
                P::new(x, y),
                P::new(x + w, y),
                P::new(x + w, y + h),
                P::new(x, y + h),
            ])
        };
        let shapes = vec![
            shape(fill(sq(-100.0, -100.0, 300.0)), Kind::Fill, [210, 10, 10]),
            shape(bar(55.0, -30.0, 30.0, 90.0), Kind::Fill, [0, 0, 0]),
            shape(fill(sq(0.0, 0.0, 40.0)), Kind::Fill, red),
            shape(fill(sq(100.0, 0.0, 40.0)), Kind::Fill, red),
        ];
        let off = plan("p", &shapes, &PlanOptions::default());
        let blend = PlanOptions {
            blend: 150.0,
            ..PlanOptions::default()
        };
        let d = plan("p", &shapes, &blend);
        assert_eq!(d.trim_count() + 1, off.trim_count(), "one cut saved");
        assert_eq!(d.blends.len(), 1);
        let way = &d.blends[0];
        assert!(way.len() > 2, "not straight across: {way:?}");
        // Round the bar, not over it: no stitch of the way on the black.
        let on_bar = |q: P| q.x > 56.0 && q.x < 84.0 && q.y > -29.0 && q.y < 59.0;
        for w in way.windows(2) {
            for k in 0..=10 {
                let q = w[0].lerp(w[1], k as f64 / 10.0);
                assert!(!on_bar(q), "{q:?} of {way:?}");
            }
        }
        let len: f64 = way.windows(2).map(|w| w[0].dist(w[1])).sum();
        assert!(len <= 150.0 + 1e-6, "{len}");
        assert_eq!(crate::checks::exposed_travel(&d), []);
        // With the backdrop black too there's no way round: cut.
        let mut dark = shapes.clone();
        dark[0].color = [0, 0, 0];
        let d = plan("p", &dark, &blend);
        assert!(
            !d.blends
                .iter()
                .any(|w| w.last().is_some_and(|q| (q.x - 100.0).abs() < 1.0)),
            "{:?}",
            d.blends
        );
    }

    #[test]
    fn the_thread_goes_the_way_its_drawn_by_hand() {
        let a = shape(fill(sq(0.0, 0.0, 100.0)), Kind::Fill, [0; 3]);
        let mut b = shape(fill(sq(400.0, 0.0, 100.0)), Kind::Fill, [0; 3]);
        // 30 mm apart: cut.
        let d = plan("p", &[a.clone(), b.clone()], &PlanOptions::default());
        assert_eq!(d.trim_count(), 1);
        // Drawn a way round, through two points: sewn through them, no cut.
        b.params.via = Some(vec![(20.0, 30.0), (40.0, 30.0)]);
        let d = plan("p", &[a.clone(), b.clone()], &PlanOptions::default());
        assert_eq!(d.trim_count(), 0);
        assert_eq!(d.routes.len(), 1);
        assert_eq!(d.routes[0].0, 1);
        let pts: Vec<P> = d.blocks[0]
            .runs
            .iter()
            .flat_map(|r| r.pts.clone())
            .collect();
        for q in [P::new(200.0, 300.0), P::new(400.0, 300.0)] {
            assert!(pts.iter().any(|p| p.dist(q) < 1.0), "through {q:?}");
        }
        // No stitch of it longer than a running stitch.
        let i = pts
            .iter()
            .position(|p| p.dist(P::new(200.0, 300.0)) < 1.0)
            .unwrap();
        assert!(pts[i].dist(pts[i + 1]) <= 30.0 + 1e-6);
        assert_eq!(
            crate::checks::exposed_travel(&d),
            [],
            "the person's own way"
        );
        // None given: straight there. And a trim asked for is still made.
        b.params.via = Some(Vec::new());
        assert_eq!(
            plan("p", &[a.clone(), b.clone()], &PlanOptions::default()).trim_count(),
            0
        );
        let mut cut = a.clone();
        cut.params.trim = Some(true);
        assert_eq!(
            plan("p", &[cut, b.clone()], &PlanOptions::default()).trim_count(),
            1
        );
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

    /// One object in two separate 10 mm squares, `gap` apart.
    fn two_squares(gap: f64) -> Shape {
        shape(
            Geometry::Fill {
                rings: vec![sq(0.0, 0.0, 100.0), sq(100.0 + gap, 0.0, 100.0)],
                rule: FillRule::NonZero,
            },
            Kind::Fill,
            [0; 3],
        )
    }

    /// Every point along the thread of `run` (each stitch, and between).
    fn along(run: &Run) -> Vec<P> {
        run.pts
            .windows(2)
            .flat_map(|w| {
                let n = w[0].dist(w[1]).ceil().max(1.0) as usize;
                (0..=n).map(move |i| w[0].lerp(w[1], i as f64 / n as f64))
            })
            .collect()
    }

    #[test]
    fn each_part_of_a_fill_is_sewn_whole_before_the_next() {
        let mut s = two_squares(200.0);
        s.params.underlay = Some(true);
        let d = plan("p", &[s.clone()], &PlanOptions::default());
        // One cut, between the parts: not one after the first's underlay
        // and another back for its top.
        assert_eq!(d.trim_count(), 1);
        let runs = &d.blocks[0].runs;
        assert_eq!(runs.len(), 2);
        let left = |r: &Run| r.pts.iter().all(|p| p.x < 150.0);
        let right = |r: &Run| r.pts.iter().all(|p| p.x > 150.0);
        assert!(
            (left(&runs[0]) && right(&runs[1])) || (right(&runs[0]) && left(&runs[1])),
            "a run to each square"
        );
        // A start point to honour: sewn group by group, as it was.
        s.params.start = Some(0.0);
        assert!(plan("p", &[s], &PlanOptions::default()).trim_count() > 1);
    }

    #[test]
    fn travel_between_parts_goes_under_what_is_sewn_on_top() {
        // Two squares 4 mm apart, and a later object across the gap.
        let under = two_squares(40.0);
        let over = shape(
            fill(vec![
                P::new(80.0, 20.0),
                P::new(160.0, 20.0),
                P::new(160.0, 80.0),
                P::new(80.0, 80.0),
            ]),
            Kind::Fill,
            [255, 0, 0],
        );
        let d = plan("p", &[under.clone(), over.clone()], &PlanOptions::default());
        assert_eq!(d.blocks[0].runs.len(), 1, "no cut between the squares");
        assert!(d.bridges.is_empty(), "and nothing in the open");
        // What crosses the gap is under the object on top, off its edges.
        let crossing: Vec<P> = along(&d.blocks[0].runs[0])
            .into_iter()
            .filter(|p| p.x > 105.0 && p.x < 135.0)
            .collect();
        assert!(!crossing.is_empty());
        assert!(
            crossing.iter().all(|p| p.y > 24.0 && p.y < 76.0),
            "{crossing:?}"
        );
        // Nothing on top: the gap is bare cloth, and the thread is cut.
        let d = plan("p", std::slice::from_ref(&under), &PlanOptions::default());
        assert_eq!(d.trim_count(), 1);
        // Sewn the other way up, the cover is underneath: no hiding there.
        let d = plan("p", &[over.clone(), under.clone()], &PlanOptions::default());
        assert_eq!(d.blocks[1].runs.len(), 2);
        // And no routing asked for: cut.
        let off = PlanOptions {
            travel_route: 0.0,
            ..PlanOptions::default()
        };
        let d = plan("p", &[under, over], &off);
        assert!(d.blocks[0].runs.len() >= 2);
        assert!(
            d.blocks[0]
                .runs
                .iter()
                .all(|r| along(r).iter().all(|p| p.x < 105.0 || p.x > 135.0)),
            "nothing sewn across the gap"
        );
    }

    fn stroke(pts: &[(f64, f64)], width: f64, kind: Kind) -> Shape {
        shape(
            Geometry::Stroke {
                lines: vec![(pts.iter().map(|&(x, y)| P::new(x, y)).collect(), false)],
                width,
            },
            kind,
            [0, 0, 0],
        )
    }

    #[test]
    fn travel_to_a_satin_goes_under_the_satin_itself() {
        // A T in two 1.6 mm satin strokes. Stem first: where it ends, at
        // the top, is under the bar to come, and so is the way along to
        // where the bar begins. One run, no cut.
        let stem = stroke(&[(50.0, 8.0), (50.0, 140.0)], 16.0, Kind::Satin);
        let bar = stroke(&[(0.0, 8.0), (100.0, 8.0)], 16.0, Kind::Satin);
        let d = plan("t", &[stem.clone(), bar.clone()], &PlanOptions::default());
        assert_eq!(d.trim_count(), 0);
        assert!(d.bridges.is_empty());
        // The stitches between the two are inside the bar, off its ends.
        let run = &d.blocks[0].runs[0];
        let between = &run.pts[d.per_shape[0]..run.pts.len() - d.per_shape[1]];
        assert!(!between.is_empty());
        assert!(
            between
                .iter()
                .all(|q| q.y > 0.0 && q.y < 16.0 && q.x > 0.0 && q.x < 100.0),
            "{between:?}"
        );
        // Bar first: it's sewn, so the way from its end to the stem would
        // lie on top of it. That's a cut.
        let d = plan("t", &[bar.clone(), stem.clone()], &PlanOptions::default());
        assert_eq!(d.trim_count(), 1);
        // A stroke that begins a short hop from where the last one ended
        // (a corner of its column), the hop over what it will cover.
        let bowl = stroke(
            &[(50.0, 16.0), (90.0, 16.0), (90.0, 70.0)],
            16.0,
            Kind::Satin,
        );
        let d = plan("p", &[stem.clone(), bowl], &PlanOptions::default());
        assert_eq!(d.trim_count(), 0);
        // Nothing of the kind when routing is off.
        let off = PlanOptions {
            travel_route: 0.0,
            ..PlanOptions::default()
        };
        assert_eq!(plan("t", &[stem, bar], &off).trim_count(), 1);
    }

    #[test]
    fn a_fills_sections_are_sewn_so_the_thread_stays_off_finished_rows() {
        // A body with a tower at each end of its top: three sections (the
        // towers, and the body under both). Nearest first, it's a tower,
        // the body under it, and then the other tower, reached round the
        // edge of the body already sewn: running stitch on top of the
        // fill. Sewn tower, tower, body, the way between the towers is
        // along the top of the body, under rows still to come.
        let castle = fill(vec![
            P::new(0.0, 0.0),
            P::new(60.0, 0.0),
            P::new(60.0, 60.0),
            P::new(180.0, 60.0),
            P::new(180.0, 0.0),
            P::new(240.0, 0.0),
            P::new(240.0, 260.0),
            P::new(0.0, 260.0),
        ]);
        let d = plan(
            "c",
            &[shape(castle.clone(), Kind::Fill, [0, 0, 0])],
            &PlanOptions::default(),
        );
        assert_eq!(d.trim_count(), 0);
        let pts: Vec<P> = d.blocks[0]
            .runs
            .iter()
            .flat_map(|r| r.pts.iter().copied())
            .collect();
        // Rows are the stitches that run level; the rest, when longer
        // than a turn between rows, are the thread getting somewhere.
        let mut rows: Vec<(P, P)> = Vec::new();
        let mut over = 0;
        for w in pts.windows(2) {
            if (w[0].y - w[1].y).abs() < 1e-6 {
                rows.push((w[0], w[1]));
            } else if w[0].dist(w[1]) > 8.0 {
                over += crossings(w, &rows);
            }
        }
        assert!(rows.len() > 500);
        assert!(over <= FEW_ROWS, "travel crosses {over} rows already sewn");
        // All three parts are filled.
        for (x, y) in [(30.0, 20.0), (210.0, 20.0), (120.0, 200.0)] {
            assert!(
                rows.iter()
                    .any(|(a, b)| (a.y - y).abs() < 3.0 && a.x.min(b.x) <= x && x <= a.x.max(b.x)),
                "nothing sewn at {x}, {y}"
            );
        }
        // And the check that reads the stitches back agrees: no travel
        // over finished stitching.
        assert_eq!(crate::checks::exposed_travel(&d), []);

        // Made to begin at the top of the left tower (a start point), it's
        // sewn nearest first: tower, body, and the other tower with the
        // body in the way. Out of sight there's no way to it, so the
        // thread is cut...
        let mut from_tower = shape(castle.clone(), Kind::Fill, [0, 0, 0]);
        from_tower.params.start = Some(0.0);
        let cut = plan(
            "c",
            std::slice::from_ref(&from_tower),
            &PlanOptions::default(),
        );
        assert_eq!(cut.trim_count(), 1);
        assert_eq!(crate::checks::exposed_travel(&cut), []);
        // ...unless the document says to sew over the fill instead: no
        // cut, and running stitch up the edge of the body, which the
        // check finds.
        let over = PlanOptions {
            over_sewn: true,
            ..PlanOptions::default()
        };
        let sewn = plan("c", &[from_tower], &over);
        assert_eq!(sewn.trim_count(), 0);
        let shows = crate::checks::exposed_travel(&sewn);
        assert_eq!(shows.len(), 1, "{shows:?}");
        assert!(shows[0].mm > 10.0 && shows[0].rows > 40, "{shows:?}");
        // Either way the sections are sewn in the order that needs least
        // of it: with no start point, sewing over changes nothing.
        let free = plan("c", &[shape(castle, Kind::Fill, [0, 0, 0])], &over);
        assert_eq!(free.trim_count(), 0);
        assert_eq!(crate::checks::exposed_travel(&free), []);
    }

    #[test]
    fn travel_between_a_satins_columns_keeps_off_the_ones_already_sewn() {
        // One satin object in two bars, and a patch sewn later over their
        // right ends. The first bar is sewn from the left and back to it
        // (underlay out, satin back); the hidden way from there to the
        // second bar used to be along the first again (taken for cover,
        // though it was sewn by then), under the patch and back along the
        // second: a line of running stitch down the middle of the
        // finished satin. Now it's a cut.
        let bar = |y: f64| {
            vec![
                P::new(0.0, y),
                P::new(120.0, y),
                P::new(120.0, y + 30.0),
                P::new(0.0, y + 30.0),
            ]
        };
        let bars = shape(
            Geometry::Fill {
                rings: vec![bar(0.0), bar(100.0)],
                rule: FillRule::NonZero,
            },
            Kind::Satin,
            [0, 0, 0],
        );
        let patch = shape(
            fill(vec![
                P::new(80.0, -30.0),
                P::new(140.0, -30.0),
                P::new(140.0, 160.0),
                P::new(80.0, 160.0),
            ]),
            Kind::Fill,
            [0, 0, 0],
        );
        let d = plan("b", &[bars, patch], &PlanOptions::default());
        assert!(d.notes.is_empty(), "{:?}", d.notes);
        // The bars' stitches, in order, up to the first in the second bar.
        let pts: Vec<P> = d.blocks[0]
            .runs
            .iter()
            .flat_map(|r| r.pts.iter().copied())
            .take(d.per_shape[0])
            .collect();
        let first_bar: Vec<P> = pts.iter().copied().take_while(|q| q.y < 60.0).collect();
        assert!(
            first_bar.len() > 40 && first_bar.len() < pts.len(),
            "{} of {}",
            first_bar.len(),
            pts.len()
        );
        // Its zigzag reaches the rails (15 from the middle); once that's
        // begun, nothing more runs down its middle.
        let zigzag = first_bar
            .iter()
            .position(|q| (q.y - 15.0).abs() > 10.0)
            .unwrap();
        let down_the_middle: Vec<&P> = first_bar[zigzag..]
            .iter()
            .filter(|q| (q.y - 15.0).abs() < 5.0 && q.x > 10.0 && q.x < 110.0)
            .collect();
        assert!(down_the_middle.is_empty(), "{down_the_middle:?}");
        // The second bar is all there too.
        assert!(pts.iter().filter(|q| q.y > 125.0).count() > 15);
    }

    #[test]
    fn a_line_is_sewn_toward_the_object_that_comes_next() {
        // A run from left to right, then one that begins by its left end:
        // sewn right to left, the two join; as drawn, they'd be cut apart.
        let first = stroke(&[(0.0, 0.0), (200.0, 0.0)], 3.0, Kind::Run);
        let second = stroke(&[(0.0, 6.0), (0.0, 120.0)], 3.0, Kind::Run);
        let d = plan(
            "l",
            &[first.clone(), second.clone()],
            &PlanOptions::default(),
        );
        assert_eq!(d.trim_count(), 0);
        let run = along(&d.blocks[0].runs[0]);
        assert_eq!(
            (run[0], *run.last().unwrap()),
            (P::new(200.0, 0.0), P::new(0.0, 120.0))
        );
        // With nothing after it, or a cut asked for, it's sewn as drawn.
        let d = plan("l", std::slice::from_ref(&first), &PlanOptions::default());
        assert_eq!(along(&d.blocks[0].runs[0])[0], P::new(0.0, 0.0));
        let mut cut = first.clone();
        cut.params.trim = Some(true);
        let d = plan("l", &[cut, second.clone()], &PlanOptions::default());
        assert_eq!(along(&d.blocks[0].runs[0])[0], P::new(0.0, 0.0));
        assert_eq!(d.trim_count(), 1);
        // Nor is it turned round when the next object is out of reach of
        // both ends, or in another thread.
        let far = stroke(&[(100.0, 60.0), (100.0, 120.0)], 3.0, Kind::Run);
        let d = plan("l", &[first.clone(), far], &PlanOptions::default());
        assert_eq!(along(&d.blocks[0].runs[0])[0], P::new(0.0, 0.0));
        let mut red = second;
        red.color = [255, 0, 0];
        let d = plan("l", &[first, red], &PlanOptions::default());
        assert_eq!(along(&d.blocks[0].runs[0])[0], P::new(0.0, 0.0));
    }

    #[test]
    fn travel_round_an_outline_begins_and_ends_beside_it() {
        // A 10 mm square: its travel ring is half a millimetre inside it.
        let rings = vec![sq(0.0, 0.0, 100.0)];
        let ring = travel_ring(&rings, FillRule::NonZero);
        // From one edge round to another: along the ring.
        let way = route_along(&ring, P::new(0.0, 50.0), P::new(100.0, 50.0), 500.0).unwrap();
        assert!(
            way.iter()
                .all(|q| q.x >= 0.0 && q.x <= 100.0 && q.y >= 0.0 && q.y <= 100.0)
        );
        // From a point 3 mm outside it (another part of the shape, too
        // thin for a ring of its own): no way by this ring, which the
        // needle would have to cross open cloth to reach.
        assert!(route_along(&ring, P::new(-30.0, 50.0), P::new(100.0, 50.0), 500.0).is_none());
        assert!(route_along(&ring, P::new(0.0, 50.0), P::new(130.0, 50.0), 500.0).is_none());
    }

    #[test]
    fn travel_goes_through_a_neck_of_a_close_fill_but_not_an_open_one() {
        // Two blocks joined by a neck 0.6 mm wide: one outline, but the
        // walk round its inside (0.5 mm in) can't get through.
        let necked: Polyline = vec![
            P::new(0.0, 0.0),
            P::new(100.0, 0.0),
            P::new(100.0, 47.0),
            P::new(120.0, 47.0),
            P::new(120.0, 0.0),
            P::new(220.0, 0.0),
            P::new(220.0, 100.0),
            P::new(120.0, 100.0),
            P::new(120.0, 53.0),
            P::new(100.0, 53.0),
            P::new(100.0, 100.0),
            P::new(0.0, 100.0),
        ];
        let mut sh = shape(fill(necked.clone()), Kind::Fill, [0; 3]);
        sh.params.angle = Some(90.0);
        let d = plan("p", &[sh.clone()], &PlanOptions::default());
        assert_eq!(d.trim_count(), 0, "through the neck");
        // All of it inside the outline (a row's end may pull a little past).
        let grown = boolean::offset(&[necked], FillRule::NonZero, 6.0);
        for run in &d.blocks[0].runs {
            assert!(along(run).iter().all(|&p| crate::geom::point_inside(
                &grown,
                FillRule::NonZero,
                p
            )));
        }
        // Rows 2 mm apart hide nothing under them: cut instead.
        sh.params.spacing = Some(2.0);
        let d = plan("p", &[sh], &PlanOptions::default());
        assert!(d.trim_count() > 0);
    }

    #[test]
    fn connect_bridges_the_parts_of_one_object_and_says_where() {
        let s = two_squares(30.0);
        let d = plan("p", std::slice::from_ref(&s), &PlanOptions::default());
        assert_eq!(d.trim_count(), 1);
        assert!(d.bridges.is_empty());
        let opt = PlanOptions {
            connect: 300.0,
            ..PlanOptions::default()
        };
        let d = plan("p", &[s], &opt);
        assert_eq!(d.trim_count(), 0, "sewn across the gap");
        assert_eq!(d.bridges.len(), 1);
        let (a, b) = d.bridges[0];
        assert!(a.dist(b) <= 300.0);
        assert!(a.x.min(b.x) < 105.0 && a.x.max(b.x) > 125.0, "{a:?} {b:?}");
        // Further apart than that: still cut.
        let d = plan("p", &[two_squares(500.0)], &opt);
        assert_eq!(d.trim_count(), 1);
        assert!(d.bridges.is_empty());
        // Centered, the bridges move with the stitches.
        let mut d = plan("p", &[two_squares(30.0)], &opt);
        let before = (d.bridges[0].0, d.blocks[0].runs[0].pts[0]);
        d.center();
        let after = (d.bridges[0].0, d.blocks[0].runs[0].pts[0]);
        assert!((before.0 - after.0).dist(before.1 - after.1) < 1e-9);
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
        // (Each row widened for pull, its ends that far past the edge.)
        let pull = mm(Params::default().fill_pull(&PlanOptions::default().fabric));
        let edge_hits = pts
            .iter()
            .filter(|p| (p.x + pull).abs() < 1e-6 || (p.x - 100.0 - pull).abs() < 1e-6)
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
        // ...but the base still reaches 0.5 mm under its edge (and its
        // rows a pull's width more), so no gap shows.
        let pull = mm(Params::default().fill_pull(&PlanOptions::default().fabric));
        let near_edge = with.blocks[0]
            .runs
            .iter()
            .flat_map(|r| &r.pts)
            .any(|p| p.x > 52.0 && p.x < 56.0 + pull && p.y > 80.0 && p.y < 120.0);
        assert!(near_edge);
    }

    /// Two clusters of small same-thread pieces 20 mm apart, listed
    /// alternately (as a tracer lists them): three 3 mm runs 1.5 mm apart
    /// and a small fill in each.
    fn two_clusters() -> Vec<Shape> {
        let piece = |cluster: f64, k: usize| -> Shape {
            if k == 3 {
                return shape(fill(sq(cluster + 50.0, 0.0, 30.0)), Kind::Fill, [0, 0, 0]);
            }
            let x = cluster + 15.0 * k as f64;
            stroke(&[(x, 0.0), (x, 30.0)], 1.0, Kind::Run)
        };
        (0..4)
            .flat_map(|k| [piece(0.0, k), piece(200.0, k)])
            .collect()
    }

    #[test]
    fn small_pieces_of_one_thread_sew_cluster_by_cluster() {
        let shapes = two_clusters();
        let opt = PlanOptions {
            connect: 30.0,
            ..PlanOptions::default()
        };
        let listed = PlanOptions {
            short_travel: false,
            ..opt.clone()
        };
        let before = plan("t", &shapes, &listed);
        let after = plan("t", &shapes, &opt);
        assert_eq!(before.sew_order, (0..8).collect::<Vec<_>>());
        assert!(before.trim_count() >= 6, "{} trims", before.trim_count());
        assert_eq!(after.sew_order, vec![0, 2, 4, 6, 1, 3, 5, 7]);
        assert_eq!(after.trim_count(), 1, "one jump between the clusters");
    }

    #[test]
    fn same_thread_objects_over_one_another_keep_their_order() {
        // Each cluster's pieces overlap: fills stacked a little apart, the
        // second cluster 20 mm away. Every overlapping pair keeps its
        // order; only which cluster comes when changes.
        let shapes: Vec<Shape> = (0..3)
            .flat_map(|k| {
                let o = 10.0 * k as f64;
                [0.0, 200.0].map(|x| shape(fill(sq(x + o, o, 40.0)), Kind::Fill, [0, 0, 0]))
            })
            .collect();
        let d = plan("t", &shapes, &PlanOptions::default());
        assert_eq!(d.sew_order, vec![0, 2, 4, 1, 3, 5]);
        let pos = |i: usize| d.sew_order.iter().position(|&x| x == i).unwrap();
        assert!(pos(0) < pos(2) && pos(2) < pos(4), "{:?}", d.sew_order);
        assert!(pos(1) < pos(3) && pos(3) < pos(5), "{:?}", d.sew_order);
        // All over one another: as listed, stitch for stitch.
        let stack: Vec<Shape> = (0..4)
            .map(|k| shape(fill(sq(10.0 * k as f64, 0.0, 40.0)), Kind::Fill, [0, 0, 0]))
            .collect();
        let off = PlanOptions {
            short_travel: false,
            ..PlanOptions::default()
        };
        let a = plan("t", &stack, &PlanOptions::default());
        assert_eq!(a.sew_order, vec![0, 1, 2, 3]);
        assert!(same_stitches(&a, &plan("t", &stack, &off)));
    }

    #[test]
    fn an_object_that_cuts_or_stops_after_it_stays_where_it_is() {
        let mut shapes = two_clusters();
        shapes[3].params.trim = Some(true);
        let d = plan("t", &shapes, &PlanOptions::default());
        assert_eq!(d.sew_order[3], 3, "{:?}", d.sew_order);
        // Each side of it is a stretch of its own, shortened.
        assert_eq!(&d.sew_order[..3], &[0, 2, 1]);
        assert_eq!(&d.sew_order[4..], &[4, 6, 5, 7]);
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

    /// An axis-aligned box: x0, y0, x1, y1.
    type Box4 = (f64, f64, f64, f64);

    /// The outline of the stems and arms given (each a box), joined into
    /// one shape.
    fn boxes(parts: &[Box4]) -> Geometry {
        let rings: Vec<(Vec<Polyline>, FillRule)> = parts
            .iter()
            .map(|&(x0, y0, x1, y1)| {
                (
                    vec![vec![
                        P::new(x0, y0),
                        P::new(x1, y0),
                        P::new(x1, y1),
                        P::new(x0, y1),
                    ]],
                    FillRule::NonZero,
                )
            })
            .collect();
        Geometry::Fill {
            rings: boolean::union(&rings),
            rule: FillRule::NonZero,
        }
    }

    /// A geometric e: a ring 35 thick, a bar 20 deep across it whose
    /// lower edge runs out to the right, the ring cut off level below it.
    fn round_e() -> Shape {
        let arc = |r: f64, from: f64, to: f64| -> Vec<P> {
            let n = ((to - from).abs() / 0.08).ceil() as usize;
            (0..=n)
                .map(|k| {
                    let t = from + (to - from) * k as f64 / n as f64;
                    P::new(r * t.cos(), r * t.sin())
                })
                .collect()
        };
        // (y down: angles run clockwise on the page from 3 o'clock.)
        let (out, inn) = (70.0f64, 35.0f64);
        let at = |r: f64, y: f64| (y / r).asin();
        let tau = std::f64::consts::TAU;
        // From the terminal round under, up the left and over to the arm's
        // foot…
        let mut outer: Vec<P> = arc(out, at(out, 24.0), at(out, 12.0) + tau);
        // …in along the bar's lower edge, round the inside under the bar,
        // and out along the cut.
        let mut inside = arc(inn, at(inn, 24.0), std::f64::consts::PI - at(inn, 12.0));
        inside.reverse();
        outer.extend(inside);
        // The counter over the bar.
        let counter = arc(inn, std::f64::consts::PI + at(inn, 8.0), tau - at(inn, 8.0));
        shape(
            Geometry::Fill {
                rings: vec![outer, counter],
                rule: FillRule::EvenOdd,
            },
            Kind::Satin,
            [0, 0, 0],
        )
    }

    #[test]
    fn a_shape_that_touches_itself_at_a_point_is_the_shapes_either_side() {
        // Two bars that meet corner to corner, drawn as one outline that
        // goes through the corner twice (as the arm and leg of a k meet
        // its stem in some faces): two shapes, touching.
        let ring = vec![
            P::new(0.0, 0.0),
            P::new(100.0, 0.0),
            P::new(100.0, 20.0),
            P::new(200.0, 20.0),
            P::new(200.0, 40.0),
            P::new(100.0, 40.0),
            P::new(100.0, 20.0),
            P::new(0.0, 20.0),
        ];
        let (rings, at) = unpinched(std::slice::from_ref(&ring)).expect("it touches itself");
        assert_eq!(rings.len(), 2);
        assert_eq!(at, vec![P::new(100.0, 20.0)]);
        assert!(rings.iter().all(|r| r.len() == 4));
        assert!(
            (rings
                .iter()
                .map(|r| boolean::ring_area(r).abs())
                .sum::<f64>()
                - 4000.0)
                .abs()
                < 1e-6
        );
        // (Not a ring that doesn't, or one with a point doubled.)
        assert!(unpinched(&[ring[..4].to_vec()]).is_none());
        let doubled = vec![ring[0], ring[1], ring[1], ring[2], ring[7]];
        assert!(unpinched(&[doubled]).is_none());
        // A column each, and wherever it's put: no cut across a neck of
        // no width, placed by the last bit of where the letter lies.
        for by in [P::new(0.0, 0.0), P::new(0.37, 913.11), P::new(-77.7, 3.3)] {
            let bars = shape(
                Geometry::Fill {
                    rings: vec![ring.iter().map(|p| *p + by).collect()],
                    rule: FillRule::EvenOdd,
                },
                Kind::Satin,
                [0, 0, 0],
            );
            let columns = satin_rails(&bars);
            assert_eq!(columns.len(), 2, "moved {by:?}");
            assert!(columns.iter().all(|c| c.is_ok()));
        }
    }

    #[test]
    fn a_column_can_leave_the_thread_back_along_its_last_stitch() {
        // A column's last stitches across its end: from (0, 90) to
        // (20, 100), then over to (0, 100).
        let pts = vec![P::new(0.0, 90.0), P::new(20.0, 100.0), P::new(0.0, 100.0)];
        // Back where that stitch began, or halfway: the middle of the end.
        assert_eq!(
            end_stitch(&pts, 4.0),
            vec![P::new(20.0, 100.0), P::new(10.0, 100.0)]
        );
        // Not by a stitch shorter than the shortest to be sewn, nor with no
        // stitch across to go back along.
        assert_eq!(end_stitch(&pts, 15.0), vec![P::new(20.0, 100.0)]);
        assert!(end_stitch(&pts, 25.0).is_empty());
        assert!(end_stitch(&pts[..2].to_vec(), 4.0).is_empty());
        // Ending with the needle down twice in the last place, the same.
        let mut twice = pts.clone();
        twice.push(P::new(0.0, 100.0));
        assert_eq!(end_stitch(&twice, 4.0), end_stitch(&pts, 4.0));
    }

    #[test]
    fn a_words_letters_are_ordered_as_each_would_be_alone() {
        // A T: a bar, and a stem that ends against the middle of it.
        // `count` of them in a row, well apart.
        let word = |count: usize| -> (Vec<Vec<Polyline>>, Vec<ColumnWay>) {
            let (mut outlines, mut ends) = (Vec::new(), Vec::new());
            for k in 0..count {
                let x = k as f64 * 160.0;
                let rect = |x0: f64, y0: f64, x1: f64, y1: f64| {
                    vec![vec![
                        P::new(x0, y0),
                        P::new(x1, y0),
                        P::new(x1, y1),
                        P::new(x0, y1),
                    ]]
                };
                let bar = (P::new(x, 5.0), P::new(x + 60.0, 5.0));
                let stem = (P::new(x + 30.0, 80.0), P::new(x + 30.0, 10.0));
                outlines.push(rect(x, 0.0, x + 60.0, 10.0));
                outlines.push(rect(x + 25.0, 10.0, x + 35.0, 80.0));
                for (piece, (a, b)) in [(2 * k, bar), (2 * k + 1, stem)] {
                    for (turned, start, end) in [(false, a, b), (true, b, a)] {
                        ends.push(ColumnWay {
                            piece,
                            turned,
                            start,
                            end,
                        });
                    }
                }
            }
            (outlines, ends)
        };
        let cuts = |count: usize| {
            let (outlines, ends) = word(count);
            let outlines: Vec<&Vec<Polyline>> = outlines.iter().collect();
            let found = fewest_cuts(
                &outlines,
                &ends,
                None,
                None,
                10.0,
                false,
                &HashMap::new(),
                usize::MAX,
            )
            .expect("an order");
            // Every column once, and each letter's two together.
            let pieces: Vec<usize> = found.order.iter().map(|&w| ends[w].piece).collect();
            let mut sorted = pieces.clone();
            sorted.sort();
            assert_eq!(sorted, (0..2 * count).collect::<Vec<_>>());
            assert!(pieces.chunks(2).all(|c| c[0] / 2 == c[1] / 2), "{pieces:?}");
            found.cuts
        };
        // One T takes no cut: its stem up to the bar, then the bar.
        assert_eq!(cuts(1), 0);
        // A word of them takes one between each two, and no more: thirty
        // (sixty columns, searched all at once), and forty (eighty: more
        // than are searched at once, so a letter at a time).
        assert_eq!(cuts(30), 29);
        assert_eq!(cuts(40), 39);
    }

    #[test]
    fn thread_gets_to_a_column_by_one_hop_not_hop_after_hop() {
        // Four serifs in a row, each within a hop of the next and none
        // joined to another; the first is sewn, and the thread is at the
        // second (3 units from it).
        let n = 4;
        let mut touch = vec![false; n * n];
        for (i, j) in [(0, 1), (1, 2), (2, 3)] {
            touch[i * n + j] = true;
            touch[j * n + i] = true;
        }
        let none = vec![false; n * n];
        let far = f64::INFINITY;
        let here = [0.0, 3.0, far, far];
        // To the second: it's on it. To the third: on to the second, and
        // a hop from that. To the fourth: two hops, which travel isn't
        // given.
        assert!(hidden_way(&here, 1, 1, &none, &touch));
        assert!(hidden_way(&here, 2, 1, &none, &touch));
        assert!(!hidden_way(&here, 3, 1, &none, &touch));
        // With the third and fourth one piece of ground (a stem and its
        // serif), the fourth is a hop from the second's ground.
        let mut joined = none.clone();
        joined[2 * n + 3] = true;
        joined[3 * n + 2] = true;
        assert!(hidden_way(&here, 3, 1, &joined, &touch));
        // Not through a column that's sewn: with the third done too, the
        // fourth is cut off.
        assert!(!hidden_way(&here, 3, 1 | 1 << 2, &joined, &touch));
        // And from nowhere near a column still to sew, there's no way.
        assert!(!hidden_way(&[0.0, far, far, far], 1, 1, &none, &touch));
    }

    #[test]
    fn a_hairline_is_wide_enough_to_travel_under() {
        // A hairline 0.2 mm wide, 18 mm long, on the slant: narrower than
        // the cells travel is routed over, so as it's drawn there's no way
        // along it.
        let line = [P::new(0.0, 0.0), P::new(150.0, 100.0)];
        let hair = satin::rails_from_centerline(&line, false, 2.0);
        let mut drawn = hair.a.clone();
        drawn.extend(hair.b.iter().rev());
        let mut bounds = Bounds::empty();
        drawn.iter().for_each(|&p| bounds.add(p));
        let bounds = Bounds {
            min: bounds.min - P::new(20.0, 20.0),
            max: bounds.max + P::new(20.0, 20.0),
        };
        let way = |ring: &Polyline| {
            Zone::new(&[(std::slice::from_ref(ring), FillRule::EvenOdd)], bounds)
                .and_then(|z| z.route(P::new(6.0, 4.0), P::new(144.0, 96.0), 500.0))
        };
        assert!(way(&drawn).is_none());
        // Taken as wide as travel needs, there is: down its middle, under
        // its stitches.
        let ground = thin_ground(&hair).expect("a hairline");
        let route = way(&ground).expect("a way along it");
        assert!(route.iter().all(|p| {
            let t = ((p.x * 150.0 + p.y * 100.0) / (150.0 * 150.0 + 100.0 * 100.0)).clamp(0.0, 1.0);
            p.dist(P::new(150.0 * t, 100.0 * t)) <= GROUND_WIDTH * 0.5 + 0.5
        }));
        // A column that's wide all along is its own ground; so is a ring.
        assert!(thin_ground(&satin::rails_from_centerline(&line, false, 30.0)).is_none());
    }

    #[test]
    fn columns_that_cross_cover_where_they_cross() {
        // A plus: two two-rail satins, one across the other, sewn as one
        // satin (grouped, the same settings). Where they cross is covered
        // (by both), not a hole between them: travel can hide there.
        let column = |a: Polyline, b: Polyline| {
            let mut s = shape(
                Geometry::Stroke {
                    lines: vec![(a, false), (b, false)],
                    width: 0.0,
                },
                Kind::Satin,
                [0, 0, 0],
            );
            s.groups = vec!["plus".into()];
            s.params.rails = Some((1, 2));
            s
        };
        let shapes = [
            column(
                vec![P::new(-20.0, -100.0), P::new(-20.0, 100.0)],
                vec![P::new(20.0, -100.0), P::new(20.0, 100.0)],
            ),
            column(
                vec![P::new(-100.0, -20.0), P::new(100.0, -20.0)],
                vec![P::new(-100.0, 20.0), P::new(100.0, 20.0)],
            ),
        ];
        let (_, plus) = joined_satin(&shapes, &[0, 1], 0).expect("one satin");
        for cover in [coverage(&plus), Kept::default().coverage(&plus)] {
            let (rings, rule) = cover.expect("a satin covers");
            assert!(
                crate::geom::point_inside(&rings, rule, P::new(0.0, 0.0)),
                "where they cross"
            );
            assert!(
                crate::geom::point_inside(&rings, rule, P::new(0.0, 60.0)),
                "one alone"
            );
            assert!(
                !crate::geom::point_inside(&rings, rule, P::new(60.0, 60.0)),
                "neither"
            );
        }
    }

    #[test]
    fn overlap_lays_a_letters_strokes_over_each_other() {
        // An H: stems 3 mm wide, a bar 2 mm.
        let h = vec![vec![
            P::new(100.0, 100.0),
            P::new(130.0, 100.0),
            P::new(130.0, 190.0),
            P::new(270.0, 190.0),
            P::new(270.0, 100.0),
            P::new(300.0, 100.0),
            P::new(300.0, 300.0),
            P::new(270.0, 300.0),
            P::new(270.0, 210.0),
            P::new(130.0, 210.0),
            P::new(130.0, 300.0),
            P::new(100.0, 300.0),
        ]];
        let mut shape = shape(
            Geometry::Fill {
                rings: h,
                rule: FillRule::NonZero,
            },
            Kind::Satin,
            [0, 0, 0],
        );
        let outline = |r: &Rails| r.outline().remove(0);
        // How far into another column each column's ends reach.
        let overlap = |cols: &[Rails]| -> f64 {
            let mut most: f64 = 0.0;
            for (i, r) in cols.iter().enumerate() {
                for (j, other) in cols.iter().enumerate() {
                    if i == j {
                        continue;
                    }
                    let ring = outline(other);
                    for p in [r.a[0], r.b[0], *r.a.last().unwrap(), *r.b.last().unwrap()] {
                        if crate::geom::point_inside(
                            std::slice::from_ref(&ring),
                            FillRule::EvenOdd,
                            p,
                        ) {
                            most = most.max(dist_to_ring(&ring, p));
                        }
                    }
                }
            }
            most
        };
        let cut: Vec<Rails> = satin_rails(&shape)
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        assert!(overlap(&cut) < 0.1, "cut: the columns meet edge to edge");
        shape.params.overlap = Some(true);
        let laid: Vec<Rails> = satin_rails(&shape)
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        // (A square join has no bracket to fill: the bar ends just inside
        // each stem, as wide as it is, and its stitches lap on under.)
        assert!(
            overlap(&laid) > 0.1,
            "overlapping: the bar runs on into the stems"
        );
        assert!(
            overlap(&laid) < 5.0,
            "no wider into the join than the bar is"
        );
        // Either way it sews in one piece.
        let d = plan("t", &[shape], &PlanOptions::default());
        assert_eq!(d.trim_count(), 0);
    }

    #[test]
    fn a_letter_has_the_same_columns_wherever_it_lies() {
        // Its columns are found for its shape, read from its own first
        // point: the same ones, moved, wherever it's put on the design
        // (and so found once for a line of text that's moved about).
        let e = round_e();
        let was: Vec<Rails> = satin_rails(&e).into_iter().map(|r| r.unwrap()).collect();
        for by in [P::new(1234.5678, -987.6543), P::new(-0.3, 77_777.7)] {
            let mut moved = e.clone();
            if let Geometry::Fill { rings, .. } = &mut moved.geometry {
                rings.iter_mut().flatten().for_each(|p| *p = *p + by);
            }
            let now: Vec<Rails> = satin_rails(&moved)
                .into_iter()
                .map(|r| r.unwrap())
                .collect();
            assert_eq!(was.len(), now.len());
            for (w, n) in was.iter().zip(&now) {
                assert_eq!((w.a.len(), w.b.len()), (n.a.len(), n.b.len()));
                assert_eq!(w.sync, n.sync);
                for (p, q) in w.a.iter().zip(&n.a).chain(w.b.iter().zip(&n.b)) {
                    assert!((*p + by).dist(*q) < 1e-6, "{p:?} + {by:?} is not {q:?}");
                }
            }
        }
        // A rail's point that's a point of the outline is that point, to
        // the bit.
        let Geometry::Fill { rings, .. } = &e.geometry else {
            unreachable!()
        };
        let near = |p: P| rings.iter().flatten().find(|q| q.dist(p) < 1e-6);
        let on: Vec<(P, &P)> = was
            .iter()
            .flat_map(|r| r.a.iter().chain(&r.b))
            .filter_map(|p| near(*p).map(|q| (*p, q)))
            .collect();
        assert!(on.len() > 4);
        assert!(on.iter().all(|(p, q)| p == *q));
    }

    #[test]
    fn the_bar_of_an_e_ends_square_and_its_bowl_on_its_cuts() {
        // The bar butts into the end of the bowl where the counter's curve
        // meets it at a slant: cut along that curve, the bar's end slants
        // and its stitches fan to meet it. And the bowl's foot is cut
        // level where the stroke is still climbing: its end is the cut,
        // not the sharp corner of it. (As cut into columns, corners
        // squared: the `cut` setting. By the chordal axis the bar and bowl
        // meet on a mitre instead.)
        let mut e = round_e();
        e.params.cut = Some(true);
        let columns: Vec<Rails> = satin_rails(&e).into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(columns.len(), 2, "a bar and a bowl");
        let bar = columns
            .iter()
            .min_by(|x, y| {
                crate::geom::polyline_length(&x.a).total_cmp(&crate::geom::polyline_length(&y.a))
            })
            .unwrap();
        let bowl = columns.iter().find(|c| !std::ptr::eq(*c, bar)).unwrap();
        // Every stitch of the bar stands straight up.
        for (a, b) in satin::pairs(bar, 3.0) {
            assert!((a.x - b.x).abs() < 1.5, "a bar stitch leans: {a:?} {b:?}");
        }
        // The bowl's two ends lie level: along the bar's lower edge (y 12)
        // and along the cut across its foot (y 24), each a stroke wide.
        let ends = [
            (bowl.a[0], bowl.b[0]),
            (*bowl.a.last().unwrap(), *bowl.b.last().unwrap()),
        ];
        for y in [12.0, 24.0] {
            assert!(
                ends.iter().any(|(a, b)| {
                    (a.y - y).abs() < 1.0 && (b.y - y).abs() < 1.0 && a.dist(*b) > 25.0
                }),
                "no end along y {y}: {ends:?}"
            );
        }
    }

    /// How a letter's satin columns came out: what a change to how
    /// columns are found, ended or ordered must not make worse.
    /// How many degrees further a letter's worst column may lean than it
    /// did before [`letters_in_faces_of_your_own`] says it's worse.
    const LEAN_MORE: usize = 5;
    /// And how many more of its stitches may be skewed.
    const SKEWED_MORE: usize = 3;
    /// And how much more of it may be bare (hundredths of a square mm).
    const BARE_MORE: usize = 30;

    fn bar(a: (f64, f64), b: (f64, f64), across: f64, lean: f64) -> Rails {
        // A straight column from `a` to `b`, `across` wide, its stitches
        // leaning `lean` units along it (rail b's points moved on).
        let (a, b) = (P::new(a.0, a.1), P::new(b.0, b.1));
        let n = (b - a).norm().perp() * across;
        let along = (b - a).norm() * lean;
        Rails {
            a: vec![a, b],
            b: vec![a + n + along, b + n + along],
            closed: false,
            sync: Vec::new(),
        }
    }

    #[test]
    fn the_seam_measure_sees_a_stroke_sewn_in_pieces_whose_stitches_jump() {
        let outline = vec![vec![
            P::new(0.0, 0.0),
            P::new(200.0, 0.0),
            P::new(200.0, 30.0),
            P::new(0.0, 30.0),
        ]];
        // One bar in two pieces end to end, stitches square in both: no
        // seam (a column cut in two for the thread is sewn so).
        let square = [
            bar((0.0, 0.0), (100.0, 0.0), 30.0, 0.0),
            bar((100.0, 0.0), (200.0, 0.0), 30.0, 0.0),
        ];
        assert_eq!(seams(&outline, FillRule::NonZero, &square), 0);
        // The second piece's stitches leaning 40°: a seam where they meet.
        let lean = 30.0 * 40f64.to_radians().tan();
        let jumps = [
            bar((0.0, 0.0), (100.0, 0.0), 30.0, 0.0),
            bar((100.0, 0.0), (200.0, 0.0), 30.0, lean),
        ];
        assert!(seams(&outline, FillRule::NonZero, &jumps) > 0);
        // A bar meeting a stem across it (an L): a join, not a seam.
        let l = vec![vec![
            P::new(0.0, 0.0),
            P::new(30.0, 0.0),
            P::new(30.0, 170.0),
            P::new(200.0, 170.0),
            P::new(200.0, 200.0),
            P::new(0.0, 200.0),
        ]];
        let join = [
            bar((0.0, 0.0), (0.0, 200.0), -30.0, 0.0),
            bar((30.0, 170.0), (200.0, 170.0), 30.0, 0.0),
        ];
        assert_eq!(seams(&l, FillRule::NonZero, &join), 0);
    }

    #[test]
    fn the_layer_measure_counts_three_columns_over_each_other() {
        let two = [
            bar((0.0, 0.0), (100.0, 0.0), 30.0, 0.0),
            bar((50.0, -50.0), (50.0, 80.0), -30.0, 0.0),
        ];
        assert_eq!(layers(&two), 0, "two crossing is how a crossing is sewn");
        let three = [
            two[0].clone(),
            two[1].clone(),
            bar((0.0, 10.0), (100.0, 10.0), 10.0, 0.0),
        ];
        assert!(layers(&three) > 30);
    }

    #[test]
    fn overlapping_strokes_that_cross_are_two_layers_at_most() {
        // A t without its curl: a stem 3 mm wide, a bar 2 mm across it.
        let t = vec![vec![
            P::new(100.0, 0.0),
            P::new(130.0, 0.0),
            P::new(130.0, 60.0),
            P::new(180.0, 60.0),
            P::new(180.0, 80.0),
            P::new(130.0, 80.0),
            P::new(130.0, 250.0),
            P::new(100.0, 250.0),
            P::new(100.0, 80.0),
            P::new(50.0, 80.0),
            P::new(50.0, 60.0),
            P::new(100.0, 60.0),
        ]];
        let mut s = shape(
            Geometry::Fill {
                rings: t,
                rule: FillRule::NonZero,
            },
            Kind::Satin,
            [0, 0, 0],
        );
        s.params.overlap = Some(true);
        let l = lettered(&[s]);
        assert_eq!(l.layers, 0, "{l:?}");
        assert_eq!(l.trims, 0, "{l:?}");
    }

    /// The font families installed, one per line as `FAMILY\t<name>`:
    /// what `tools/census/census.sh` sweeps.
    #[test]
    #[ignore]
    fn installed_font_families() {
        for family in crate::document::font_families() {
            println!("FAMILY\t{family}");
        }
    }

    /// Every letter and digit as a satin, in typefaces of your own: the
    /// faces `ZIGZAG_FACES` names (`Futura:bold;Georgia;Times New
    /// Roman:italic`), and every `.svg` in the folder `ZIGZAG_DESIGNS`
    /// names, if it does. With `ZIGZAG_LETTERS` naming a file, what each
    /// came to is written there the first time, and held to after: a
    /// letter with more of it filled, more trims, more stitches off the
    /// shape, more travel in plain sight or stitches leaning clearly
    /// further than they did fails. (Delete the file to take what there is
    /// now as the measure.) With `ZIGZAG_OVERLAP` set, the faces' letters
    /// are sewn with the `overlap` setting; `ZIGZAG_FONT_SIZE` sets how
    /// big they're set, in mm (21, letters about 15 mm tall, unless it
    /// says; 11 is about the 8 mm new text starts at).
    /// `cargo test --lib -- --ignored letters_in_faces_of_your_own --nocapture`
    #[test]
    #[ignore]
    fn letters_in_faces_of_your_own() {
        let mut found: Vec<(String, Lettered)> = Vec::new();
        // (The letters as one setting or another sews them: `ZIGZAG_AUTO`,
        // mitred where that's clean, else cut; `ZIGZAG_MITRED`, every corner
        // mitred; `ZIGZAG_OVERLAP`, strokes laid over each other. Cut, the
        // default, otherwise; `ZIGZAG_CUT` says so too.)
        let id = if std::env::var_os("ZIGZAG_OVERLAP").is_some() {
            "satin_overlap"
        } else if std::env::var_os("ZIGZAG_CUT").is_some() {
            "satin_cut"
        } else if std::env::var_os("ZIGZAG_MITRED").is_some() {
            "satin_mitred"
        } else if std::env::var_os("ZIGZAG_AUTO").is_some() {
            "satin_cut=0"
        } else {
            "satin"
        };
        let size: f64 = std::env::var("ZIGZAG_FONT_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|v: &f64| *v > 0.0)
            .unwrap_or(21.0);
        for face in std::env::var("ZIGZAG_FACES")
            .unwrap_or_default()
            .split(';')
            .map(str::trim)
            .filter(|f| !f.is_empty())
        {
            let mut parts = face.split(':');
            let family = parts.next().unwrap();
            let rest: Vec<&str> = parts.collect();
            let weight = if rest.contains(&"bold") {
                "bold"
            } else {
                "normal"
            };
            let style = if rest.contains(&"italic") {
                "italic"
            } else {
                "normal"
            };
            for ch in ('a'..='z').chain('A'..='Z').chain('0'..='9') {
                let svg = format!(
                    r##"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="40mm" viewBox="0 0 40 40"><text id="{id}" x="6" y="28" font-family="{family}" font-weight="{weight}" font-style="{style}" font-size="{size}" fill="#000000">{ch}</text></svg>"##
                );
                let shapes = crate::svg::load(svg.as_bytes()).unwrap_or_default();
                // (For tools/census/trims.py: whose trims the report's
                // lines that follow are.)
                if trim_report() {
                    eprintln!("LETTER {face} {ch}");
                }
                found.push((format!("{face} {ch}"), lettered(&shapes)));
            }
        }
        if let Ok(dir) = std::env::var("ZIGZAG_DESIGNS") {
            let mut paths: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| p.extension().is_some_and(|e| e == "svg"))
                .collect();
            paths.sort();
            for path in paths {
                let shapes = crate::svg::load_file(&path).unwrap_or_default();
                let name = path.file_stem().unwrap().to_string_lossy().into_owned();
                found.push((name, lettered(&shapes)));
            }
        }
        assert!(
            !found.is_empty(),
            "ZIGZAG_FACES or ZIGZAG_DESIGNS: something to sew"
        );
        let sum = |f: &dyn Fn(&Lettered) -> usize| found.iter().map(|(_, l)| f(l)).sum::<usize>();
        println!(
            "{} letters: {} columns, {} filled, {} pointed ends, {} stitches astray, {} trims",
            found.len(),
            sum(&|l| l.columns),
            sum(&|l| l.fills),
            sum(&|l| l.pointed),
            sum(&|l| l.astray),
            sum(&|l| l.trims),
        );
        // A letter in one piece is one piece of thread: the trims over
        // what its separate parts take.
        let broken: Vec<&(String, Lettered)> =
            found.iter().filter(|(_, l)| l.trims > l.owed).collect();
        println!(
            "{} letters break the thread ({} trims more than their parts take)",
            broken.len(),
            broken.iter().map(|(_, l)| l.trims - l.owed).sum::<usize>(),
        );
        println!(
            "{} letters with travel in plain sight ({} stretches)",
            found.iter().filter(|(_, l)| l.exposed > 0).count(),
            sum(&|l| l.exposed),
        );
        println!(
            "{:.1} sq mm bare inside satin letters, {} letters with over 1 sq mm",
            found
                .iter()
                .filter(|(_, l)| l.fills == 0)
                .map(|(_, l)| l.bare)
                .sum::<usize>() as f64
                / 100.0,
            found
                .iter()
                .filter(|(_, l)| l.fills == 0 && l.bare > 100)
                .count(),
        );
        if std::env::var("ZIGZAG_BROKEN").is_ok() {
            for (name, l) in &broken {
                println!(
                    "  {name}: {} columns, {} filled, {} trims, {} owed",
                    l.columns, l.fills, l.trims, l.owed
                );
            }
            for (name, l) in found.iter().filter(|(_, l)| l.exposed > 0) {
                println!("  {name}: {} stretches of travel in plain sight", l.exposed);
            }
        }
        let Ok(file) = std::env::var("ZIGZAG_LETTERS") else {
            return;
        };
        let line = |name: &str, l: &Lettered| {
            format!(
                "{name}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                l.columns,
                l.fills,
                l.pointed,
                l.astray,
                l.trims,
                l.exposed,
                l.lean,
                l.skewed,
                l.bare,
                l.off_edge,
                l.seams,
                l.layers,
                l.outside,
                l.crowded
            )
        };
        let Ok(was) = std::fs::read_to_string(&file) else {
            let all: Vec<String> = found.iter().map(|(n, l)| line(n, l)).collect();
            std::fs::write(&file, all.join("\n") + "\n").unwrap();
            println!("written to {file}");
            return;
        };
        let was: std::collections::HashMap<&str, Vec<usize>> = was
            .lines()
            .filter_map(|l| {
                let mut f = l.split('\t');
                let name = f.next()?;
                Some((name, f.filter_map(|v| v.parse().ok()).collect()))
            })
            .collect();
        let mut worse = Vec::new();
        for (name, l) in &found {
            let Some(w) = was.get(name.as_str()).filter(|w| w.len() >= 5) else {
                continue;
            };
            // (A file from before travel in plain sight was counted: none
            // to hold it to.)
            let exposed = w.get(5).copied().unwrap_or(usize::MAX);
            // (A letter that was filled had no stitches to go astray:
            // made into columns, it's held to what it comes to next time.)
            let astray = l.fills >= w[1] && l.astray > w[3] + 1;
            // (Nor to how its stitches leaned: and a file from before
            // that was measured has none to hold it to.)
            let leans = l.fills >= w[1]
                && (w.get(6).is_some_and(|was| l.lean > was + LEAN_MORE)
                    || w.get(7).is_some_and(|was| l.skewed > was + SKEWED_MORE));
            // (Nor to how much of it was bare.)
            let barer = l.fills >= w[1] && w.get(8).is_some_and(|was| l.bare > was + BARE_MORE);
            if l.fills > w[1] || astray || leans || barer || l.trims > w[4] || l.exposed > exposed {
                worse.push(format!(
                    "{name}: filled {} → {}, astray {} → {}, trims {} → {}, travel in sight {} → {}, lean {} → {}, skewed {} → {}, bare {} → {}",
                    w[1],
                    l.fills,
                    w[3],
                    l.astray,
                    w[4],
                    l.trims,
                    w.get(5).map_or("?".to_string(), |e| e.to_string()),
                    l.exposed,
                    w.get(6).map_or("?".to_string(), |e| e.to_string()),
                    l.lean,
                    w.get(7).map_or("?".to_string(), |e| e.to_string()),
                    l.skewed,
                    w.get(8).map_or("?".to_string(), |e| e.to_string()),
                    l.bare
                ));
            }
        }
        assert!(
            worse.is_empty(),
            "{} worse:\n{}",
            worse.len(),
            worse.join("\n")
        );
    }

    #[test]
    fn a_letter_drawn_in_overlapping_pieces_is_split_as_one_shape() {
        // A T as a variable font draws it: a bar, and a stem laid over
        // it from inside the bar down, the two outlines crossing. Filled
        // by the nonzero rule that's one shape, and it's sewn as that
        // shape is: a bar and a stem butted under it, nothing filled, the
        // same as if its outline had been drawn in one piece.
        let rect = |x0: f64, y0: f64, x1: f64, y1: f64| -> Vec<P> {
            vec![
                P::new(x0, y0),
                P::new(x1, y0),
                P::new(x1, y1),
                P::new(x0, y1),
            ]
        };
        let (bar, stem) = (rect(0.0, 0.0, 120.0, 14.0), rect(53.0, 5.0, 67.0, 140.0));
        let pieces = shape(
            Geometry::Fill {
                rings: vec![bar.clone(), stem.clone()],
                rule: FillRule::NonZero,
            },
            Kind::Satin,
            [0, 0, 0],
        );
        let columns = satin_rails(&pieces);
        assert!(
            columns.iter().all(Result::is_ok),
            "filled: {:?}",
            columns
                .iter()
                .filter_map(|c| c.as_ref().err().map(|e| e.1.clone()))
                .collect::<Vec<_>>()
        );
        assert_eq!(columns.len(), 2, "a bar and a stem");
        // (No stitch of the stem's inside the bar: it stops under it.)
        for r in columns.iter().flatten() {
            let tall = r.a.iter().chain(&r.b).any(|q| q.y > 100.0);
            if tall {
                assert!(r.a.iter().chain(&r.b).all(|q| q.y > 13.0), "{r:?}");
            }
        }
        let d = plan("t", std::slice::from_ref(&pieces), &PlanOptions::default());
        assert_eq!(d.trim_count(), 0, "one piece of thread");
        // Rings that don't cross are handed back as they were: the
        // letters either side of such a one are none the worse.
        let apart = vec![bar.clone(), rect(200.0, 0.0, 214.0, 140.0)];
        let none = untangled(&apart, FillRule::NonZero);
        assert!(none.tangles.is_empty());
        assert!(matches!(none.plain, std::borrow::Cow::Borrowed(_)));
        // And with a tangle after them, they still come first, in order.
        let mut mixed = apart.clone();
        mixed.push(rect(400.0, 0.0, 520.0, 14.0));
        mixed.push(rect(453.0, 5.0, 467.0, 140.0));
        let out = untangled(&mixed, FillRule::NonZero);
        assert_eq!(out.plain[..], apart[..]);
        assert_eq!(out.tangles.len(), 1);
        assert_eq!(out.tangles[0].0.len(), 2, "the two that crossed");
        assert_eq!(out.tangles[0].1.len(), 1, "are one outline together");
        // A contour that crosses itself inside another that does (a B's
        // two counters drawn as one, in an outline drawn as one) is that
        // one's: its counters, not shapes of their own.
        let knot = |x: f64, y: f64, w: f64, h: f64| -> Vec<P> {
            vec![
                P::new(x, y),
                P::new(x + w, y + h),
                P::new(x + w, y),
                P::new(x, y + h),
            ]
        };
        let nested = vec![knot(0.0, 0.0, 200.0, 200.0), knot(20.0, 80.0, 20.0, 40.0)];
        assert_eq!(untangled(&nested, FillRule::NonZero).tangles.len(), 1);
    }

    #[test]
    fn an_eight_is_two_rings_one_over_the_other() {
        // Two rings that run into each other at the waist, heavy enough
        // that there's one bar of it between the two counters. Each ring
        // is sewn round its own counter, the waist cut along between the
        // nips in the outline: not a ring round both with a bar across
        // it, whose stitches would lean all the way round, nor a fill.
        let circle = |cy: f64, r: f64, from: f64, to: f64| -> Vec<P> {
            let n = ((to - from).abs() / 0.1).ceil() as usize;
            (0..=n)
                .map(|k| {
                    let t = from + (to - from) * k as f64 / n as f64;
                    P::new(r * t.cos(), cy + r * t.sin())
                })
                .collect()
        };
        // (Each outer circle from one nip round to the other: they cross
        // where y is 0.)
        let (r, off) = (40.0f64, 30.0f64);
        let nip = (off / r).asin();
        let tau = std::f64::consts::TAU;
        let pi = std::f64::consts::PI;
        let mut outer = circle(off, r, -nip, pi + nip);
        outer.extend(circle(-off, r, pi - nip, tau + nip));
        let mut eight = shape(
            Geometry::Fill {
                rings: vec![
                    outer.clone(),
                    circle(off, 16.0, 0.0, tau - 0.1),
                    circle(-off, 16.0, 0.0, tau - 0.1),
                ],
                rule: FillRule::EvenOdd,
            },
            Kind::Satin,
            [0, 0, 0],
        );
        // By its chordal axis (the default), one stroke round both counters
        // and across the waist, as a pen draws an 8: all satin, its
        // stitches square to the stroke, in one piece of thread.
        let axis: Vec<Rails> = satin_rails(&eight)
            .into_iter()
            .map(|r| r.expect("a column, not a fill"))
            .collect();
        for c in &axis {
            let lean = satin::lean(std::slice::from_ref(c));
            assert!(lean < 5.0, "stitches lean {lean:.1}°");
        }
        let d = plan("8", std::slice::from_ref(&eight), &PlanOptions::default());
        assert_eq!(d.trim_count(), 0, "one piece of thread");
        // Cut into columns (the `cut` setting): two rings, as below.
        eight.params.cut = Some(true);
        let columns: Vec<Rails> = satin_rails(&eight)
            .into_iter()
            .map(|r| r.expect("a column, not a fill"))
            .collect();
        assert_eq!(columns.len(), 2);
        assert!(columns.iter().all(|c| c.closed), "two rings");
        // (Their stitches square to the stroke all the way round.)
        for c in &columns {
            let lean = satin::lean(std::slice::from_ref(c));
            assert!(lean < 5.0, "stitches lean {lean:.1}°");
        }
        let d = plan("8", std::slice::from_ref(&eight), &PlanOptions::default());
        assert_eq!(d.trim_count(), 0, "one piece of thread");
    }

    #[test]
    fn a_letter_with_two_serifs_is_still_one_piece_of_thread() {
        // A serif across the end of a stem is a dead end: sewn tip to
        // tip, the thread comes out at a tip, with the stem back in the
        // middle. A letter can end on one; with two, one of them is cut
        // in two at its stem and each half sewn in to it, so the thread
        // goes on from there.
        let with = |mut s: Shape, center: bool| {
            s.params.underlay = None;
            s.params.satin_underlay = Some(if center { underlay::CENTER } else { 0 });
            s
        };
        let letters: [(&str, Vec<Box4>, usize); 4] = [
            // A stem with a bar across its top: nothing to cut.
            (
                "T",
                vec![(0.0, 0.0, 120.0, 14.0), (53.0, 14.0, 67.0, 140.0)],
                2,
            ),
            // A stem between two serifs: one of them in two halves.
            (
                "I",
                vec![
                    (0.0, 0.0, 120.0, 14.0),
                    (53.0, 14.0, 67.0, 126.0),
                    (0.0, 126.0, 120.0, 140.0),
                ],
                4,
            ),
            // Two stems, a foot under each, a bar across their tops.
            (
                "n",
                vec![
                    (20.0, 0.0, 34.0, 126.0),
                    (0.0, 126.0, 54.0, 140.0),
                    (106.0, 0.0, 120.0, 126.0),
                    (86.0, 126.0, 140.0, 140.0),
                    (34.0, 0.0, 106.0, 14.0),
                ],
                6,
            ),
            // Two stems with a serif across each end, and a bar between
            // them: more joins than every choice of can be tried, and no
            // one of them cut that saves a trim alone.
            (
                "H",
                vec![
                    (20.0, 14.0, 34.0, 126.0),
                    (0.0, 0.0, 54.0, 14.0),
                    (0.0, 126.0, 54.0, 140.0),
                    (106.0, 14.0, 120.0, 126.0),
                    (86.0, 0.0, 140.0, 14.0),
                    (86.0, 126.0, 140.0, 140.0),
                    (34.0, 63.0, 106.0, 77.0),
                ],
                0,
            ),
        ];
        for (name, parts, columns) in &letters {
            for center in [false, true] {
                let s = with(shape(boxes(parts), Kind::Satin, [0, 0, 0]), center);
                // (However many the H is cut into: as few as do.)
                if *columns > 0 {
                    assert_eq!(satin_rails(&s).len(), *columns, "{name}: columns");
                }
                let d = plan("t", std::slice::from_ref(&s), &PlanOptions::default());
                assert!(d.notes.is_empty(), "{name}: {:?}", d.notes);
                assert_eq!(
                    d.trim_count(),
                    0,
                    "{name}, center walk {center}: one piece of thread"
                );
                let shows = crate::checks::exposed_travel(&d);
                assert!(shows.is_empty(), "{name}, center walk {center}: {shows:?}");
            }
        }
    }

    #[test]
    fn a_satins_part_thats_filled_instead_isnt_travelled_across_once_sewn() {
        // A ring 14 mm deep (too wide for satin: it's filled instead),
        // with a bar 3 mm wide beside it that is sewn as a column. The
        // ring's rows are sewn in several sections, round its hole.
        let circle = |r: f64| -> Polyline {
            (0..72)
                .map(|k| {
                    let t = k as f64 / 72.0 * std::f64::consts::TAU;
                    P::new(r * t.cos(), r * t.sin())
                })
                .collect()
        };
        let mut rings = boolean::union(&[(vec![circle(200.0), circle(60.0)], FillRule::EvenOdd)]);
        rings.extend(boolean::union(&[(
            vec![vec![
                P::new(230.0, -150.0),
                P::new(260.0, -150.0),
                P::new(260.0, 150.0),
                P::new(230.0, 150.0),
            ]],
            FillRule::NonZero,
        )]));
        let mixed = shape(
            Geometry::Fill {
                rings,
                rule: FillRule::EvenOdd,
            },
            Kind::Satin,
            [0, 0, 0],
        );
        let parts = satin_rails(&mixed);
        assert_eq!(parts.iter().filter(|r| r.is_err()).count(), 1);
        assert_eq!(parts.iter().filter(|r| r.is_ok()).count(), 1);
        // No travel lies on top of the ring's finished rows: the way from
        // one section of it to the next, and on to the bar, is under rows
        // still to come, or the thread is cut. (The rows of a part that's
        // filled are top stitching, as a fill's are, though the satin's
        // columns are sewn after them.)
        let d = plan("t", std::slice::from_ref(&mixed), &PlanOptions::default());
        let shows: Vec<_> = crate::checks::exposed_travel(&d)
            .into_iter()
            .filter(|e| e.rows >= crate::checks::EXPOSED_ROWS)
            .collect();
        assert!(shows.is_empty(), "{shows:?}");
        // And it's cut no oftener than the same ring is as a plain fill.
        let mut fill = mixed.clone();
        fill.params.kind = Some(Kind::Fill);
        let as_fill = plan("t", std::slice::from_ref(&fill), &PlanOptions::default());
        assert!(
            d.trim_count() <= as_fill.trim_count() + 1,
            "{} {}",
            d.trim_count(),
            as_fill.trim_count()
        );
    }

    #[test]
    fn an_arm_that_meets_a_stem_at_a_sharp_angle_is_sewn_square_to_itself() {
        // A stem 3 mm wide, and an arm nearly as wide rising from its
        // side at 35° to it (a k's, a Y's), its far end square: where the
        // arm ends against the stem it's cut off 55° from square.
        let (sin, cos) = 35f64.to_radians().sin_cos();
        let (arm, across) = (P::new(sin, -cos), P::new(cos, sin));
        let top = P::new(15.0, 65.0) + arm * 110.0;
        let rings = boolean::union(&[
            (
                vec![vec![
                    P::new(0.0, -30.0),
                    P::new(30.0, -30.0),
                    P::new(30.0, 140.0),
                    P::new(0.0, 140.0),
                ]],
                FillRule::NonZero,
            ),
            (
                vec![vec![
                    P::new(15.0, 115.0),
                    P::new(15.0, 65.0),
                    top,
                    top + across * (50.0 * sin),
                ]],
                FillRule::NonZero,
            ),
        ]);
        // (As cut into columns, corners squared: the `cut` setting. By the
        // chordal axis the arm and stem meet on a mitre instead.)
        let mut k = shape(
            Geometry::Fill {
                rings,
                rule: FillRule::NonZero,
            },
            Kind::Satin,
            [0, 0, 0],
        );
        k.params.cut = Some(true);
        let columns: Vec<Rails> = satin_rails(&k).into_iter().map(|r| r.unwrap()).collect();
        assert!(columns.len() >= 2, "{}", columns.len());
        // Every stitch of the arm is within 30° of square to it, right
        // down to the stem: none lies along the cut, with the ones before
        // it fanned round. And the stem's are straight across it.
        let (mut seen, mut leaning) = (0, 0);
        for r in &columns {
            for (a, b) in satin::pairs(r, 3.0) {
                let mid = a.lerp(b, 0.5);
                if a.dist(b) < 10.0 {
                    continue;
                }
                let stroke = if mid.x > 31.0 { arm } else { P::new(0.0, 1.0) };
                seen += 1;
                leaning += usize::from((b - a).norm().dot(stroke).abs() > 0.5);
            }
        }
        assert!(seen > 60, "{seen}");
        assert_eq!(leaning, 0, "of {seen}");
        // And it's all one piece of thread.
        let d = plan("t", std::slice::from_ref(&k), &PlanOptions::default());
        assert_eq!(d.trim_count(), 0);
    }

    #[test]
    fn a_column_that_ends_against_the_side_of_another_is_sewn_on_under_it() {
        // A T, the bar 3 mm deep: the stem ends at the underside of the
        // bar, where the bar's stitches come down end on. Sewn to there
        // and no further, fabric shows between the points of them.
        let mut t = shape(
            boxes(&[(0.0, 0.0, 120.0, 30.0), (53.0, 30.0, 67.0, 140.0)]),
            Kind::Satin,
            [0, 0, 0],
        );
        t.params.underlay = None;
        t.params.satin_underlay = Some(0);
        // The stem's stitches run on under the bar, and nothing else's
        // anywhere: a row across it, the lap past the cut.
        let sewn: Vec<Sewn> = satin_rails_sewn(&t)
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let rows: Vec<(P, P)> = sewn
            .iter()
            .flat_map(|(_, laps)| laps)
            .flatten()
            .copied()
            .collect();
        assert_eq!(rows.len(), 1, "{rows:?}");
        let (a, b) = rows[0];
        assert!((a.y - (30.0 - BUTT_LAP)).abs() < 1e-6 && (b.y - a.y).abs() < 1e-6);
        assert!((a.x.min(b.x) - 53.0).abs() < 1e-6 && (a.x.max(b.x) - 67.0).abs() < 1e-6);
        // The columns themselves end where they meet: what's shown,
        // measured and planned with is as it was.
        let stem = sewn
            .iter()
            .find(|(_, laps)| *laps != satin::NO_LAPS)
            .unwrap();
        assert!(stem.0.a.iter().chain(&stem.0.b).all(|p| p.y >= 30.0 - 1e-6));
        // And so is the thread: one piece of it, with stitches of the
        // stem's under the bar that the bar's own never make (theirs run
        // from its top edge to its bottom one).
        let d = plan("t", std::slice::from_ref(&t), &PlanOptions::default());
        assert_eq!(d.trim_count(), 0);
        assert!(crate::checks::exposed_travel(&d).is_empty());
        let under: Vec<P> = d
            .blocks
            .iter()
            .flat_map(|b| &b.runs)
            .flat_map(|r| &r.pts)
            .copied()
            .filter(|p| (p.y - (30.0 - BUTT_LAP)).abs() < 1e-6 && p.x > 45.0 && p.x < 75.0)
            .collect();
        assert_eq!(under.len(), 2, "{under:?}");
        assert!((under[0].x - under[1].x).abs() > 14.0, "across the stem");
        // Not a stem that ends on the shape's own outline, or a column
        // cut across (the halves of a serif cut in two at its stem).
        let i = shape(
            boxes(&[
                (0.0, 0.0, 120.0, 14.0),
                (53.0, 14.0, 67.0, 126.0),
                (0.0, 126.0, 120.0, 140.0),
            ]),
            Kind::Satin,
            [0, 0, 0],
        );
        let laps = satin_rails_sewn(&i)
            .into_iter()
            .flat_map(|r| r.unwrap().1)
            .flatten()
            .count();
        assert_eq!(laps, 2, "the stem at each serif, and nothing else");
    }

    #[test]
    fn a_letters_columns_are_sewn_in_the_order_that_needs_no_cut() {
        // Letters whose columns end against the side of another: sewn
        // after the one they lean on, there's no way to them but across
        // it, and the thread is cut. Sewn before it, the thread goes on
        // under the column still to come.
        // (With a center walk under it a column ends where it began;
        // without, at its other end. As cut into columns, corners squared:
        // the `cut` setting, whose columns these are.)
        let with = |mut s: Shape, center: bool| {
            s.params.underlay = None;
            s.params.satin_underlay = Some(if center { underlay::CENTER } else { 0 });
            s.params.cut = Some(true);
            s
        };
        let letters: [(&str, Vec<Box4>); 4] = [
            // A stem with a bar across its top.
            (
                "T",
                vec![(0.0, 0.0, 120.0, 14.0), (53.0, 14.0, 67.0, 140.0)],
            ),
            // A stem and three arms.
            (
                "E",
                vec![
                    (0.0, 0.0, 14.0, 140.0),
                    (14.0, 0.0, 90.0, 14.0),
                    (14.0, 63.0, 70.0, 77.0),
                    (14.0, 126.0, 90.0, 140.0),
                ],
            ),
            // A stem with a foot and two arms to one side.
            (
                "F",
                vec![
                    (30.0, 0.0, 44.0, 126.0),
                    (0.0, 126.0, 74.0, 140.0),
                    (44.0, 0.0, 110.0, 14.0),
                    (44.0, 56.0, 96.0, 70.0),
                ],
            ),
            // A stem on a slab serif, with an arm.
            (
                "r",
                vec![
                    (30.0, 0.0, 44.0, 126.0),
                    (0.0, 126.0, 74.0, 140.0),
                    (44.0, 20.0, 100.0, 34.0),
                ],
            ),
        ];
        for (name, parts) in &letters {
            for center in [false, true] {
                let s = with(shape(boxes(parts), Kind::Satin, [0, 0, 0]), center);
                let d = plan("t", std::slice::from_ref(&s), &PlanOptions::default());
                assert!(d.notes.is_empty(), "{name}: {:?}", d.notes);
                assert_eq!(
                    d.trim_count(),
                    0,
                    "{name}, center walk {center}: one piece of thread"
                );
                // And none of the travel is over satin already sewn.
                let shows = crate::checks::exposed_travel(&d);
                assert!(shows.is_empty(), "{name}, center walk {center}: {shows:?}");
            }
        }
        // Two letters in one object: each sewn whole, one cut between.
        let two: Vec<Box4> = letters[0]
            .1
            .iter()
            .copied()
            .chain(
                letters[3]
                    .1
                    .iter()
                    .map(|&(a, b, c, d)| (a + 200.0, b, c + 200.0, d)),
            )
            .collect();
        let d = plan(
            "t",
            &[shape(boxes(&two), Kind::Satin, [0, 0, 0])],
            &PlanOptions::default(),
        );
        assert_eq!(d.trim_count(), 1);
    }

    #[test]
    fn a_satin_is_sewn_toward_the_next_when_the_way_is_under_what_comes_later() {
        // Three columns, each its own object, sewn in this order: a stem,
        // a second stem some way off, and last a bar that lies across the
        // tops of both. The way from the first stem to the second is under
        // the bar, from the first stem's top: so that's where it's left,
        // though its bottom is where sewing began (at the end of a line
        // that leads up to it).
        let lead = stroke(&[(0.0, 220.0), (0.0, 156.0)], 3.0, Kind::Run);
        // (Whether a column ends where it began, with a center walk
        // under it, or at its other end, without.)
        for center in [false, true] {
            let satin = |pts: &[(f64, f64)]| {
                let mut s = stroke(pts, 14.0, Kind::Satin);
                s.params.underlay = None;
                s.params.satin_underlay = Some(if center { underlay::CENTER } else { 0 });
                s
            };
            let stem = |x: f64| satin(&[(x, 150.0), (x, 0.0)]);
            let bar = satin(&[(-10.0, 5.0), (110.0, 5.0)]);
            let d = plan(
                "t",
                &[lead.clone(), stem(0.0), stem(100.0), bar],
                &PlanOptions::default(),
            );
            assert_eq!(d.trim_count(), 0, "center walk {center}");
            assert!(
                crate::checks::exposed_travel(&d).is_empty(),
                "center walk {center}"
            );
        }
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

    #[test]
    fn a_ring_with_corners_is_sewn_as_columns_and_a_round_one_as_a_ring() {
        // A Δ: round between its outlines every stitch would point at its
        // middle, leaning. A column a side instead, each sewn square across.
        let delta = shape(
            Geometry::Fill {
                rings: vec![
                    vec![P::new(0.0, 0.0), P::new(300.0, 0.0), P::new(150.0, -259.8)],
                    vec![
                        P::new(51.96, -30.0),
                        P::new(248.04, -30.0),
                        P::new(150.0, -199.8),
                    ],
                ],
                rule: FillRule::EvenOdd,
            },
            Kind::Satin,
            [0, 0, 0],
        );
        let rails: Vec<Rails> = satin_rails(&delta)
            .into_iter()
            .map(|r| r.expect("a column"))
            .collect();
        assert_eq!(rails.len(), 3);
        assert!(rails.iter().all(|r| !r.closed));
        assert!(satin::lean(&rails) < satin::RING_LEAN);

        // An O has no corners to get out of step at: one ring, as ever.
        let circle = |r: f64| -> Polyline {
            (0..64)
                .map(|i| {
                    let t = i as f64 / 64.0 * std::f64::consts::TAU;
                    P::new(r * t.cos(), r * t.sin())
                })
                .collect()
        };
        let o = shape(
            Geometry::Fill {
                rings: vec![circle(60.0), circle(40.0)],
                rule: FillRule::EvenOdd,
            },
            Kind::Satin,
            [0, 0, 0],
        );
        let rails = satin_rails(&o);
        assert_eq!(rails.len(), 1);
        assert!(rails[0].as_ref().is_ok_and(|r| r.closed));
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
                outline: false,
                origin: P::default(),
                joined: Vec::new(),
            };
            let d = plan("t", &[s], &PlanOptions::default());
            // (Rows widened for pull reach that far past the outline, a
            // notch's sides included: the notch sewn that much narrower
            // comes out its own width.)
            let pull = mm(Params::default().fill_pull(&PlanOptions::default().fabric));
            let m0 = 1.0 + pull;
            for r in d.blocks.iter().flat_map(|b| &b.runs) {
                for w in r.pts.windows(2) {
                    let m = w[0].lerp(w[1], 0.5);
                    let in_notch = m.x > 301.0 + m0
                        && m.x < 335.0 - m0
                        && (m.y < 106.0 - m0 || m.y > 132.0 + m0);
                    assert!(
                        !in_notch,
                        "angle {angle}: stitch {:?} -> {:?} crosses a notch",
                        w[0], w[1]
                    );
                }
            }
        }
    }

    #[test]
    fn a_panic_on_a_shared_core_is_the_callers_and_as_caught() {
        if std::thread::available_parallelism().map_or(1, |c| c.get()) < 2 {
            return;
        }
        // Work shared among the cores is inside the caller's `caught`...
        let inside = caught(|| in_parallel(vec![0; 8], |_| catching())).unwrap();
        assert_eq!(inside, vec![true; 8]);
        assert_eq!(in_parallel(vec![0; 8], |_| catching()), vec![false; 8]);
        // ...and what fails there comes back as what it was.
        let before = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = caught(|| {
            in_parallel((0..8).collect(), |k: usize| {
                if k == 3 {
                    panic!("odd outline");
                }
                k
            })
        });
        std::panic::set_hook(before);
        let why = r.expect_err("the panic is the caller's");
        assert_eq!(why.downcast_ref::<&str>().copied(), Some("odd outline"));
    }
}
