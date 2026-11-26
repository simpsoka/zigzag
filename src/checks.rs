//! Design checks: the size and width rules experienced digitizers apply,
//! reported per object so the editor (and agents) can point at the problem.

use crate::design::{Design, PlanOptions, satin_rails};
use crate::geom::{Bounds, P, Polyline, polyline_length};
use crate::params::Kind;
use crate::stitch::satin;
use crate::svg::{Geometry, Shape};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    /// Index into the shape list, or `None` for whole-design issues.
    pub shape: Option<usize>,
    pub level: Level,
    pub message: String,
}

/// Travel that lies over finished stitching in plain sight: found by
/// [`exposed_travel`].
#[derive(Clone, Debug, PartialEq)]
pub struct Exposed {
    /// The input shape the travel is for.
    pub shape: usize,
    /// Which of the planner's ways it took.
    pub how: crate::design::TravelHow,
    /// How much of the travel shows, mm.
    pub mm: f64,
    /// How many stitches already sewn it crosses.
    pub rows: usize,
    /// Where (the middle of its first stretch that shows), units.
    pub at: P,
}

/// Travel that crosses this many stitches already sewn, in plain sight,
/// is said in the checks: a couple of millimetres across a fill's rows.
pub const EXPOSED_ROWS: usize = 8;

/// Stitches a travel path crosses closer together than this (units: a
/// satin's stitches at 0.4 mm, over and back, are this far apart along
/// it) count by the distance they take up, not one each.
const ROW_WORTH: f64 = 2.0;

/// How many rows' worth of stitches a path crosses, from how far along it
/// each crossing is. Rows as far apart as rows are sewn count one each.
/// Stitches crowded together (round the inside of a bend, or fanned into
/// a corner) are no more for travel to show across than the same
/// distance of plain rows: a run of crossings counts no more than fit,
/// [`ROW_WORTH`] apart, between its first and its last, and one over (a
/// zigzag crossed near its edge, where the stitch over and the stitch
/// back all but meet, is two threads at each place).
pub fn rows_worth(mut at: Vec<f64>) -> usize {
    at.sort_by(f64::total_cmp);
    let mut worth = 0.0;
    let mut i = 0;
    while i < at.len() {
        let mut j = i;
        while j + 1 < at.len() && at[j + 1] - at[j] <= 3.0 * ROW_WORTH {
            j += 1;
        }
        let count = (j - i + 1) as f64;
        worth += count.min(2.0 + (at[j] - at[i]) / ROW_WORTH);
        i = j + 1;
    }
    (worth + 0.5).floor() as usize
}

/// A stitch sewn across an earlier one at more than 60° (the sine of
/// that) lies over it there, and hides it: a fill's top rows over its
/// underlay. (Not rows side by side, which run the same way.)
const BURIES: f64 = 0.87;

/// The sine of the angle between two directions.
fn cross_sin(u: P, v: P) -> f64 {
    let (lu, lv) = (u.len(), v.len());
    if lu < 1e-9 || lv < 1e-9 {
        return 0.0;
    }
    ((u.x * v.y - u.y * v.x) / (lu * lv)).abs()
}

/// Travel that crosses stitches no oftener than this (units between
/// crossings) is running along them, not across.
const ALONG: f64 = 7.0;

/// How near (units) a later stitch must pass for it to cover a point of
/// travel: thread is about 0.4 mm wide, and a fill's rows or a satin's
/// stitches are no further apart than that.
const COVER: f64 = 3.0;

/// The design's travel (stitches that only take the thread from one piece
/// of stitching to the next) that crosses stitches sewn before it and
/// isn't covered by stitches sewn after: running stitch lying on top of
/// finished work. Travel under what's sewn later, or along the line of a
/// row still to come, doesn't count; nor does a short hop.
///
/// Worked out from the stitches themselves, whatever the planner meant:
/// the check on its travel rules.
pub fn exposed_travel(design: &Design) -> Vec<Exposed> {
    // Every stitch as the straight lines it lies along (a fill's rows, a
    // satin's stitches), with where each is in the sewing order.
    struct Seg {
        a: P,
        b: P,
        first: usize,
        last: usize,
    }
    let mut segs: Vec<Seg> = Vec::new();
    let mut bases: Vec<Vec<usize>> = Vec::new();
    let mut base = 0;
    for block in &design.blocks {
        let mut of_block = Vec::new();
        for run in &block.runs {
            of_block.push(base);
            let pts = &run.pts;
            let mut i = 0;
            while i + 1 < pts.len() {
                let d = pts[i + 1] - pts[i];
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
                segs.push(Seg {
                    a: pts[i],
                    b: pts[j],
                    first: base + i,
                    last: base + j,
                });
                i = j;
            }
            base += pts.len();
        }
        bases.push(of_block);
    }
    let side = |a: P, b: P, c: P| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    // (How far along a to b it's crossed, if it is.)
    let crosses = |a: P, b: P, s: &Seg| {
        let (sa, sb) = (side(s.a, s.b, a), side(s.a, s.b, b));
        (side(a, b, s.a) * side(a, b, s.b) < 0.0 && sa * sb < 0.0)
            .then(|| a.dist(b) * sa / (sa - sb))
    };
    let near = |q: P, s: &Seg| {
        let ab = s.b - s.a;
        let len2 = ab.x * ab.x + ab.y * ab.y;
        let t = if len2 > 0.0 {
            (((q.x - s.a.x) * ab.x + (q.y - s.a.y) * ab.y) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        s.a.lerp(s.b, t).dist(q) <= COVER
    };
    let mut out = Vec::new();
    for t in &design.travels {
        // (Sewn across what its thread blends into, by the document's
        // leave: chosen for not showing, as the colors go.)
        // (Or as drawn by hand: the person's own way for it.)
        if matches!(
            t.how,
            crate::design::TravelHow::Blend | crate::design::TravelHow::Routed
        ) {
            continue;
        }
        let Some(pts) = design
            .blocks
            .get(t.block)
            .and_then(|b| b.runs.get(t.run))
            .map(|r| &r.pts)
        else {
            continue;
        };
        let Some(&base) = bases.get(t.block).and_then(|b| b.get(t.run)) else {
            continue;
        };
        let (from, to) = (t.from.min(pts.len() - 1), t.to.min(pts.len() - 1));
        if to <= from {
            continue;
        }
        let way = &pts[from..=to];
        let mut b = Bounds::empty();
        way.iter().for_each(|&q| b.add(q));
        let (lo, hi) = (b.min - P::new(COVER, COVER), b.max + P::new(COVER, COVER));
        let by: Vec<&Seg> = segs
            .iter()
            .filter(|s| {
                s.a.x.max(s.b.x) >= lo.x
                    && s.a.x.min(s.b.x) <= hi.x
                    && s.a.y.max(s.b.y) >= lo.y
                    && s.a.y.min(s.b.y) <= hi.y
            })
            .collect();
        let (g0, g1) = (base + from, base + to);
        // Where the way from a to b crosses stitch `s`, if it shows there:
        // `s` was sewn before it, and no layer of stitching sewn since
        // (two stitches or more) lies across `s` at that place. (A fill's
        // underlay under its top rows, a satin's under its stitches, is
        // hidden: the travel lies on what's over it, and is counted
        // crossing that. One stitch across it, a jump, hides nothing.)
        let seen = |a: P, b: P, s: &Seg| -> Option<f64> {
            if s.last > g0 {
                return None;
            }
            let d = crosses(a, b, s)?;
            let q = a + (b - a).norm() * d;
            let along = (s.b - s.a).norm();
            let buried = by
                .iter()
                .filter(|t| {
                    t.first >= s.last
                        && t.last <= g0
                        && near(q, t)
                        && cross_sin(along, t.b - t.a) > BURIES
                })
                .take(2)
                .count()
                >= 2;
            (!buried).then_some(d)
        };
        let (mut mm, mut rows, mut at) = (0.0, 0, None);
        for w in way.windows(2) {
            let over = rows_worth(by.iter().filter_map(|s| seen(w[0], w[1], s)).collect());
            // (Travel along the rows, or nearly, crosses one now and
            // then and lies among them like another row: it's across
            // them, a row or more every 0.7 mm, that it shows.)
            if (over as f64) < w[0].dist(w[1]) / ALONG {
                continue;
            }
            // (Along its length, not at its ends: those are on the edge
            // of what was sewn last and what's sewn next.)
            let covered = (1..=3).all(|k| {
                let q = w[0].lerp(w[1], k as f64 / 4.0);
                by.iter().any(|s| s.first >= g1 && near(q, s))
            });
            // Or it lies along a row already sewn, like another stitch
            // of it: what it crosses there is underneath that row.
            let along = !covered && {
                let d = w[1] - w[0];
                let len = w[0].dist(w[1]);
                (1..=3).all(|k| {
                    let q = w[0].lerp(w[1], k as f64 / 4.0);
                    by.iter().any(|s| {
                        let e = s.b - s.a;
                        s.last <= g0
                            && near(q, s)
                            && (d.x * e.y - d.y * e.x).abs() <= 0.26 * len * s.a.dist(s.b)
                    })
                })
            };
            if !covered && !along {
                // What it crosses where nothing sewn later lies over it:
                // a long stitch that starts across finished stitching and
                // goes on under the column sewn next shows for its first
                // part alone.
                let (len, dir) = (w[0].dist(w[1]), (w[1] - w[0]).norm());
                let crossed: Vec<f64> = by.iter().filter_map(|s| seen(w[0], w[1], s)).collect();
                let bare: Vec<f64> = crossed
                    .iter()
                    .copied()
                    .filter(|&d| {
                        let q = w[0] + dir * d;
                        !by.iter().any(|s| s.first >= g1 && near(q, s))
                    })
                    .collect();
                if bare.is_empty() {
                    continue;
                }
                mm += len / 10.0 * bare.len() as f64 / crossed.len() as f64;
                let middle = bare.iter().sum::<f64>() / bare.len() as f64;
                at.get_or_insert(w[0] + dir * middle);
                rows += rows_worth(bare);
            }
        }
        if let Some(at) = at {
            out.push(Exposed {
                shape: t.shape,
                how: t.how,
                mm,
                rows,
                at,
            });
        }
    }
    out
}

/// A stitch longer than this (units) is more than a machine sews as one:
/// the longest a Tajima file holds, 12.1 mm.
const LONG_STITCH: f64 = 121.0;
/// A piece of thread between two cuts with fewer stitches than this, or
/// shorter than this (units), is one a trimmer can miss.
const SHORT_PIECE_STITCHES: usize = 4;
const SHORT_PIECE: f64 = 20.0;
/// This many needle points within [`HOLE_SPOT`] units of each other (a
/// square that size) cut the cloth rather than sew it.
const HOLES: usize = 8;
const HOLE_SPOT: f64 = 2.0;
/// More thread than this over a square millimetre (mm of it) is piling
/// up: about four layers of satin. (One layer at 0.4 mm is 5 mm of
/// thread a square millimetre; a fill at 0.25 mm, 4. A border over a
/// patch over a ground, each with its underlay, is about 16, and sews.)
const DENSE: f64 = 20.0;
/// The squares thread is weighed over, units.
const DENSE_CELL: f64 = 10.0;

/// What a machine would make of the stitches themselves, whatever they're
/// for: the faults a shop's pre-flight check looks for before a file goes
/// on the machine. Stitches shorter than the document's shortest (a
/// machine drops them), stitches too long to sew, pieces between cuts too
/// short to trim reliably, needle points piled in one spot, and thread
/// piled too thick. Each is said once for each object it's found in
/// (`None`: a design that doesn't say whose its stitches are). The first
/// three are what a machine does; the last two are a judgement of how
/// much is too much, and are notes.
pub fn machine_issues(design: &Design, opt: &PlanOptions) -> Vec<Issue> {
    let mut out = Vec::new();
    let b = design.bounds();
    if b.is_empty() {
        return out;
    }
    // Whose stitch point `k` of run `ri` of block `bi` is.
    let owner = |bi: usize, ri: usize, k: usize| -> Option<usize> {
        let spans = design.stitch_shapes.get(bi)?.get(ri)?;
        spans.iter().rev().find(|s| s.0 <= k).map(|s| s.1)
    };
    let at = |p: P| format!("{:.1}, {:.1} mm", p.x / 10.0, p.y / 10.0);
    // Counted per object, with where the first (or worst) one is.
    #[derive(Default)]
    struct Tally {
        by: Vec<(Option<usize>, usize, P, f64)>,
    }
    impl Tally {
        /// One more for `shape`, at `p`, this bad (the worst is kept).
        fn add(&mut self, shape: Option<usize>, p: P, how: f64) {
            match self.by.iter_mut().find(|t| t.0 == shape) {
                Some(t) => {
                    t.1 += 1;
                    if how > t.3 {
                        (t.2, t.3) = (p, how);
                    }
                }
                None => self.by.push((shape, 1, p, how)),
            }
        }
    }
    let (mut short, mut long, mut pieces) = (Tally::default(), Tally::default(), Tally::default());
    // (A hair under the shortest: one that's exactly it is fine.)
    let shortest = opt.min_stitch - 1e-3;
    // (A stitch point: its block, run and place in the run.)
    type Point = (usize, usize, usize);
    // Needle points counted by the spot they're in: a grid of spots over
    // the design (a map of them for one too big for that, a stray stitch
    // far off; a map for every design took many times as long on a big
    // one). `piled`: the spots that fill, as they do, with the point that
    // filled each.
    let spot_of = |q: P| {
        (
            (q.x / HOLE_SPOT).floor() as i64,
            (q.y / HOLE_SPOT).floor() as i64,
        )
    };
    let (first, last) = (spot_of(b.min), spot_of(b.max));
    let (gw, gh) = (
        (last.0 - first.0 + 1).max(1) as usize,
        (last.1 - first.1 + 1).max(1) as usize,
    );
    let dense = gw.saturating_mul(gh) <= 16_000_000;
    let mut grid = vec![0u16; if dense { gw * gh } else { 0 }];
    let mut sparse: std::collections::HashMap<(i64, i64), u16> = std::collections::HashMap::new();
    let in_grid = |cell: (i64, i64)| -> Option<usize> {
        let (x, y) = (cell.0 - first.0, cell.1 - first.1);
        (x >= 0 && y >= 0 && (x as usize) < gw && (y as usize) < gh)
            .then(|| y as usize * gw + x as usize)
    };
    let mut piled: Vec<((i64, i64), Point)> = Vec::new();
    let (w, h) = (
        (b.width() / DENSE_CELL).ceil() as usize + 1,
        (b.height() / DENSE_CELL).ceil() as usize + 1,
    );
    let mut thread = vec![0f32; w * h];
    let mut thread_at: Vec<Option<Point>> = vec![None; w * h];
    for (bi, block) in design.blocks.iter().enumerate() {
        // A piece: the runs from one cut to the next.
        let mut piece: Option<(usize, f64, (usize, usize))> = None;
        let close = |piece: &mut Option<(usize, f64, (usize, usize))>, pieces: &mut Tally| {
            if let Some((stitches, len, (ri, k))) = piece.take()
                && (stitches < SHORT_PIECE_STITCHES || len < SHORT_PIECE)
            {
                pieces.add(owner(bi, ri, k), block.runs[ri].pts[k], -len);
            }
        };
        for (ri, run) in block.runs.iter().enumerate() {
            if run.pts.is_empty() {
                continue;
            }
            if run.trim_before || ri == 0 {
                close(&mut piece, &mut pieces);
            }
            let p = piece.get_or_insert((0, 0.0, (ri, 0)));
            p.0 += run.pts.len().saturating_sub(1);
            p.1 += crate::geom::polyline_length(&run.pts);
            for (k, &q) in run.pts.iter().enumerate() {
                let cell = spot_of(q);
                let count = match in_grid(cell).filter(|_| dense) {
                    Some(c) => &mut grid[c],
                    None => sparse.entry(cell).or_insert(0),
                };
                *count = count.saturating_add(1);
                if *count as usize == HOLES {
                    piled.push((cell, (bi, ri, k)));
                }
            }
            for (k, s) in run.pts.windows(2).enumerate() {
                let d = s[0].dist(s[1]);
                if d < shortest {
                    short.add(owner(bi, ri, k + 1), s[1], -d);
                }
                if d > LONG_STITCH {
                    long.add(owner(bi, ri, k + 1), s[1], d);
                }
                // The thread of this stitch, shared among the squares it
                // lies over.
                let n = (d / (DENSE_CELL * 0.25)).ceil().max(1.0) as usize;
                for j in 0..n {
                    let q = s[0].lerp(s[1], (j as f64 + 0.5) / n as f64);
                    let c = ((q.y - b.min.y) / DENSE_CELL) as usize * w
                        + ((q.x - b.min.x) / DENSE_CELL) as usize;
                    if let Some(t) = thread.get_mut(c) {
                        *t += (d / n as f64) as f32;
                        thread_at[c].get_or_insert((bi, ri, k + 1));
                    }
                }
            }
        }
        close(&mut piece, &mut pieces);
    }
    let mut say = |tally: Tally, level: Level, text: &dyn Fn(usize, P, f64) -> String| {
        for (shape, count, p, how) in tally.by {
            out.push(Issue {
                shape,
                level,
                message: text(count, p, how),
            });
        }
    };
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    say(short, Level::Error, &|n, p, how| {
        format!(
            "{} shorter than the shortest the document allows ({:.1} mm; the shortest is {:.2} mm, at {}): a machine that drops short stitches would drop {}.",
            plural(n, "stitch", "stitches"),
            opt.min_stitch / 10.0,
            -how / 10.0,
            at(p),
            if n == 1 { "it" } else { "them" }
        )
    });
    say(long, Level::Warning, &|n, p, how| {
        format!(
            "{} longer than {:.1} mm (the longest is {:.1} mm, at {}): more than a machine sews as one stitch. Shorten the stitch length, or turn on a satin's auto split.",
            plural(n, "stitch", "stitches"),
            LONG_STITCH / 10.0,
            how / 10.0,
            at(p)
        )
    });
    say(pieces, Level::Warning, &|n, p, how| {
        format!(
            "{} of thread between cuts with under {SHORT_PIECE_STITCHES} stitches or {:.0} mm of sewing (the shortest is {:.1} mm, at {}): a trimmer can miss a piece that short. Join it to what's beside it (Connect, in the Document panel) or make it longer.",
            plural(n, "piece", "pieces"),
            SHORT_PIECE / 10.0,
            -how / 10.0,
            at(p)
        )
    });
    // Needle points piled in one spot, each spot once.
    let mut spots = Tally::default();
    // (In the order they filled: the same spots said the same way each
    // time. Whose a spot is: the point's that filled it.)
    for (cell, (bi, ri, k)) in piled {
        let count = match in_grid(cell).filter(|_| dense) {
            Some(c) => grid[c],
            None => sparse.get(&cell).copied().unwrap_or(0),
        };
        let p = P::new(
            (cell.0 as f64 + 0.5) * HOLE_SPOT,
            (cell.1 as f64 + 0.5) * HOLE_SPOT,
        );
        spots.add(owner(bi, ri, k), p, count as f64);
    }
    say(spots, Level::Info, &|n, p, how| {
        format!(
            "{} where {HOLES} or more needle points fall within {:.1} mm of each other (up to {how:.0}, at {}): that many holes in one place can cut the cloth or break the thread. Short stitches (on a satin's tight curves) and a wider tip spread them.",
            plural(n, "spot", "spots"),
            HOLE_SPOT / 10.0,
            at(p)
        )
    });
    // Thread piled thick, each square millimetre once.
    let mut thick = Tally::default();
    let square = DENSE_CELL * DENSE_CELL / 100.0;
    for (c, &t) in thread.iter().enumerate() {
        let per = t as f64 / 10.0 / square;
        if per > DENSE
            && let Some((bi, ri, k)) = thread_at[c]
        {
            let p = P::new(
                b.min.x + ((c % w) as f64 + 0.5) * DENSE_CELL,
                b.min.y + ((c / w) as f64 + 0.5) * DENSE_CELL,
            );
            thick.add(owner(bi, ri, k), p, per);
        }
    }
    say(thick, Level::Info, &|n, p, how| {
        format!(
            "Thread piles up over {} mm² ({how:.0} mm of it in one square millimetre at {}; a single layer of satin is about 5): stitching that thick is stiff and breaks needles. Open the spacing, or keep objects from lying on top of each other there.",
            n,
            at(p)
        )
    });
    out
}

fn ring_bounds(rings: &[Polyline]) -> Bounds {
    let mut b = Bounds::empty();
    rings.iter().flatten().for_each(|&p| b.add(p));
    b
}

/// What the checks measure of one shape, kept between runs: checking runs
/// after every replan, and these walk every point of the shape.
#[derive(Clone, Copy)]
enum Facts {
    Fill {
        parts: usize,
        small_parts: usize,
        /// A hole under 1 mm across.
        narrow_gap: bool,
    },
    /// Widest and mean width, mm (the mean by length: a stem counts for
    /// more than a serif's tip), and the share of its length narrower
    /// than the document's narrow width.
    Satin(Option<(f64, f64, f64)>),
}

/// `measure`'s result for a shape's geometry (and `settings`, when they
/// matter), from the last time it was asked if it was.
fn facts(geometry: &Geometry, settings: Option<&str>, measure: impl FnOnce() -> Facts) -> Facts {
    use std::hash::{Hash, Hasher};
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<u64, Facts>>> =
        std::sync::OnceLock::new();
    let key = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        crate::design::geometry_key(geometry).hash(&mut h);
        settings.hash(&mut h);
        h.finish()
    };
    let memo = MEMO.get_or_init(Default::default);
    if let Some(f) = memo.lock().ok().and_then(|m| m.get(&key).copied()) {
        return f;
    }
    let f = measure();
    if let Ok(mut m) = memo.lock() {
        if m.len() > 4096 {
            m.clear();
        }
        m.insert(key, f);
    }
    f
}

/// Run all checks. `hoop_mm` is the hoop's (width, height). (`opt`: the
/// options the design was planned with, for how narrow a satin may be; what
/// overlaps cover comes from the plan itself.)
pub fn check(
    shapes: &[Shape],
    design: &Design,
    opt: &PlanOptions,
    hoop_mm: Option<(f64, f64)>,
) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut add = |shape: Option<usize>, level: Level, message: String| {
        out.push(Issue {
            shape,
            level,
            message,
        })
    };

    // Travel that shows: running stitch over stitches already sewn, with
    // nothing sewn later to cover it. (The planner keeps travel out of
    // sight; this is what it couldn't, measured from the stitches.)
    let mut shows: Vec<(usize, usize)> = Vec::new();
    for e in exposed_travel(design) {
        if std::env::var_os("ZIGZAG_TRAVEL_DEBUG").is_some() {
            eprintln!(
                "exposed: shape {} {:?} {:.1} mm over {} rows at ({:.1}, {:.1})",
                e.shape,
                e.how,
                e.mm,
                e.rows,
                e.at.x / 10.0,
                e.at.y / 10.0
            );
        }
        // (A few rows at an edge is a hop, and travel along the rows
        // crosses hardly any: neither is worth a line.)
        if e.rows < EXPOSED_ROWS {
            continue;
        }
        // (Nor what a trim setting sews in sight by choice: a gap sewn
        // across by Connect, or anything with Sew over fill on. Those
        // lines are the person's trade for fewer cuts, not a fault.)
        if opt.over_sewn || e.how == crate::design::TravelHow::Open {
            continue;
        }
        match shows.iter_mut().find(|(s, _)| *s == e.shape) {
            Some((_, rows)) => *rows += e.rows,
            None => shows.push((e.shape, e.rows)),
        }
    }
    for (shape, rows) in shows {
        add(
            Some(shape),
            Level::Info,
            format!(
                "The thread crosses {rows} rows of stitches already sewn on its way from one part of this object to the next, and shows on top of them."
            ),
        );
    }

    if let Some((w, h)) = hoop_mm {
        let b = design.bounds();
        if !b.is_empty() && (b.width() > w * 10.0 || b.height() > h * 10.0) {
            add(
                None,
                Level::Error,
                format!(
                    "Design is {:.1} × {:.1} mm; the hoop sews {:.0} × {:.0} mm.",
                    b.width() / 10.0,
                    b.height() / 10.0,
                    w,
                    h
                ),
            );
        } else if !b.is_empty()
            && (b.min.x < -w * 5.0 || b.max.x > w * 5.0 || b.min.y < -h * 5.0 || b.max.y > h * 5.0)
        {
            add(
                None,
                Level::Warning,
                "Design fits the hoop but isn't centered in it.".into(),
            );
        }
    }

    for (i, s) in shapes.iter().enumerate() {
        let kind = s.params.kind.unwrap_or(Kind::Fill);
        if kind == Kind::Skip || s.params.hidden() {
            continue;
        }
        // Planner notes say what it did instead (e.g. filled part of a satin): informational.
        for (si, note) in &design.notes {
            if *si == i {
                // An object the planner had to leave out is a warning.
                let level = if note.starts_with("Couldn't") {
                    Level::Warning
                } else {
                    Level::Info
                };
                add(Some(i), level, note.clone());
            }
        }
        match (&s.geometry, kind) {
            (Geometry::Fill { rings, rule }, Kind::Fill) => {
                if design.covered.get(i).copied().unwrap_or(false) {
                    add(
                        Some(i),
                        Level::Info,
                        "Completely covered by objects sewn on top; it won't be stitched.".into(),
                    );
                    continue;
                }
                let b = ring_bounds(rings);
                let (w, h) = (b.width() / 10.0, b.height() / 10.0);
                if w.max(h) < 1.5 {
                    add(
                        Some(i),
                        Level::Warning,
                        format!(
                            "Only {:.1} mm across: too small to fill. Make it bigger or use Run.",
                            w.max(h)
                        ),
                    );
                    continue;
                }
                let Facts::Fill {
                    parts,
                    small_parts,
                    narrow_gap,
                } = facts(&s.geometry, None, || {
                    let comps = satin::components(rings, *rule);
                    Facts::Fill {
                        parts: comps.len(),
                        small_parts: comps
                            .iter()
                            .filter(|c| ring_bounds(c).height() < 40.0)
                            .count(),
                        narrow_gap: comps.iter().flat_map(|c| c.iter().skip(1)).any(|hole| {
                            let hb = ring_bounds(std::slice::from_ref(hole));
                            hb.width().min(hb.height()) < 10.0
                        }),
                    }
                })
                else {
                    continue;
                };
                if parts >= 3 && small_parts * 2 > parts {
                    add(
                        Some(i),
                        Level::Warning,
                        "Looks like small lettering (under 4 mm tall). Fill won't read at this size: use Satin and keep letters at least 5 mm tall."
                            .into(),
                    );
                } else if w.min(h) < 2.5 {
                    add(
                        Some(i),
                        Level::Warning,
                        format!(
                            "Only {:.1} mm wide: Satin looks cleaner than Fill on narrow shapes.",
                            w.min(h)
                        ),
                    );
                }
                if narrow_gap {
                    add(
                        Some(i),
                        Level::Warning,
                        "Has a gap under 1 mm wide; thread will close it up.".into(),
                    );
                }
            }
            (_, Kind::Satin) => {
                let Facts::Satin(Some((max, mean, share))) = facts(
                    &s.geometry,
                    Some(&format!("{:?} {}", s.params, opt.narrow_satin)),
                    || {
                        // Each width across a column with the length of
                        // column it stands for (mm, mm): a long stem
                        // counts for more than a serif's short tip.
                        let widths: Vec<(f64, f64)> = satin_rails(s)
                            .iter()
                            .filter_map(|r| r.as_ref().ok())
                            .flat_map(|r| {
                                let pairs = satin::pairs(r, 10.0);
                                let len = (polyline_length(&r.a) + polyline_length(&r.b)) / 20.0;
                                let each = len / pairs.len().max(1) as f64;
                                pairs.into_iter().map(move |(a, b)| (a.dist(b) / 10.0, each))
                            })
                            .collect();
                        let total: f64 = widths.iter().map(|w| w.1).sum();
                        Facts::Satin((!widths.is_empty() && total > 0.0).then(|| {
                            let narrow: f64 = widths
                                .iter()
                                .filter(|w| w.0 < opt.narrow_satin)
                                .map(|w| w.1)
                                .sum();
                            (
                                widths.iter().map(|w| w.0).fold(0.0, f64::max),
                                widths.iter().map(|w| w.0 * w.1).sum::<f64>() / total,
                                narrow / total,
                            )
                        }))
                    },
                ) else {
                    continue;
                };
                match s.params.satin_split() {
                    Some(split) if max > split => add(
                        Some(i),
                        Level::Info,
                        format!(
                            "Satin up to {max:.1} mm wide: stitches longer than {split:.1} mm are split so they don't snag."
                        ),
                    ),
                    None if max > 7.0 => add(
                        Some(i),
                        Level::Warning,
                        format!(
                            "Satin up to {max:.1} mm wide: stitches over ~7 mm get loose and snag. Turn on auto split or use Fill."
                        ),
                    ),
                    _ => {}
                }
                if share >= NARROW_ALL {
                    add(
                        Some(i),
                        Level::Warning,
                        format!(
                            "Satin only {mean:.1} mm wide on average: too narrow to look smooth. Use Run."
                        ),
                    );
                } else if share >= NARROW_PART {
                    // Wide enough in places, the thick strokes say: Run
                    // would lose them. What's thin is the drawing's.
                    let what = if crate::document::has_type_tokens(&s.id) {
                        "Make the letters taller, or pick a font with heavier strokes."
                    } else {
                        "Make it larger, or sew its thin parts as Run objects of their own."
                    };
                    add(
                        Some(i),
                        Level::Warning,
                        format!(
                            "{:.0}% of this satin is under {:.1} mm wide (thin strokes and points): it sews thin and ragged there. {what}",
                            share * 100.0,
                            opt.narrow_satin
                        ),
                    );
                }
            }
            _ => {}
        }
    }
    out.extend(machine_issues(design, opt));
    out.sort_by_key(|i| std::cmp::Reverse(i.level));
    out
}

/// Satin under the narrow width for this share of its length (or more):
/// it's a line, and Run sews it better.
const NARROW_ALL: f64 = 0.8;
/// For this share: wide in places, too thin to look right in others.
const NARROW_PART: f64 = 0.25;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::plan;
    use crate::geom::{FillRule, P};
    use crate::params::Params;
    use crate::svg::Part;

    fn shape(geometry: Geometry, kind: Kind) -> Shape {
        Shape {
            id: String::new(),
            part: Part::Fill,
            label: String::new(),
            color: [0, 0, 0],
            paint: [0, 0, 0],
            geometry,
            params: Params {
                kind: Some(kind),
                ..Params::default()
            },
            uid: 0,
            groups: Vec::new(),
            outline: false,
            origin: P::default(),
            joined: Vec::new(),
        }
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Polyline {
        vec![
            P::new(x, y),
            P::new(x + w, y),
            P::new(x + w, y + h),
            P::new(x, y + h),
        ]
    }

    fn issues(shapes: &[Shape], hoop: Option<(f64, f64)>) -> Vec<Issue> {
        let opt = PlanOptions::default();
        let d = plan("t", shapes, &opt);
        check(shapes, &d, &opt, hoop)
    }

    #[test]
    fn flags_narrow_fill_and_small_lettering() {
        let narrow = shape(
            Geometry::Fill {
                rings: vec![rect(0.0, 0.0, 200.0, 15.0)],
                rule: FillRule::NonZero,
            },
            Kind::Fill,
        );
        let i = issues(&[narrow], None);
        assert!(
            i.iter().any(|x| x.message.contains("Satin looks cleaner")),
            "{i:?}"
        );

        let letters: Vec<Polyline> = (0..5)
            .map(|k| rect(k as f64 * 30.0, 0.0, 20.0, 30.0))
            .collect();
        let text = shape(
            Geometry::Fill {
                rings: letters,
                rule: FillRule::NonZero,
            },
            Kind::Fill,
        );
        let i = issues(&[text], None);
        assert!(
            i.iter().any(|x| x.message.contains("small lettering")),
            "{i:?}"
        );
    }

    #[test]
    fn a_satin_thin_only_in_places_isnt_told_to_be_a_run() {
        // A two-rail satin widening from 0.4 to 2.4 mm: under 1 mm for its
        // first 30%, as a serif face's hairlines are beside its stems.
        let mut wedge = shape(
            Geometry::Stroke {
                lines: vec![
                    (vec![P::new(0.0, 0.0), P::new(300.0, 0.0)], false),
                    (vec![P::new(0.0, 4.0), P::new(300.0, 24.0)], false),
                ],
                width: 0.0,
            },
            Kind::Satin,
        );
        wedge.part = Part::Stroke;
        let opt = PlanOptions::default();
        let d = plan("t", std::slice::from_ref(&wedge), &opt);
        let issues = check(std::slice::from_ref(&wedge), &d, &opt, None);
        let narrow: Vec<&str> = issues
            .iter()
            .map(|x| x.message.as_str())
            .filter(|m| m.contains("mm wide") && !m.contains("up to"))
            .collect();
        assert_eq!(narrow.len(), 1, "{narrow:?}");
        assert!(!narrow[0].contains("Use Run"), "{}", narrow[0]);
        assert!(narrow[0].contains("% of this satin is under 1.0 mm"), "{}", narrow[0]);
    }

    #[test]
    fn how_narrow_a_satin_may_be_is_the_documents_to_say() {
        // A 0.9 mm column: under the usual 1 mm, so it's warned about.
        let thin = shape(
            Geometry::Stroke {
                lines: vec![(vec![P::new(0.0, 0.0), P::new(300.0, 0.0)], false)],
                width: 9.0,
            },
            Kind::Satin,
        );
        let warned = |narrow_satin: f64| {
            let opt = PlanOptions {
                narrow_satin,
                ..Default::default()
            };
            let d = plan("t", std::slice::from_ref(&thin), &opt);
            check(std::slice::from_ref(&thin), &d, &opt, None)
                .iter()
                .any(|x| x.message.contains("too narrow to look smooth"))
        };
        assert!(warned(1.0));
        // On a fabric where 0.9 mm sews cleanly, say so and it isn't.
        assert!(!warned(0.8));
        assert!(warned(1.2));
        assert!(!warned(0.0), "0 never warns");
    }

    #[test]
    fn flags_wide_satin_and_hoop_overflow() {
        let mut wide = shape(
            Geometry::Stroke {
                lines: vec![(vec![P::new(0.0, 0.0), P::new(300.0, 0.0)], false)],
                width: 90.0,
            },
            Kind::Satin,
        );
        let i = issues(std::slice::from_ref(&wide), Some((10.0, 10.0)));
        assert_eq!(i[0].level, Level::Error, "hoop overflow sorts first: {i:?}");
        assert!(i.iter().any(|x| x.message.contains("are split")));
        wide.params.split = Some(0.0);
        let i = issues(&[wide], None);
        assert!(i.iter().any(|x| x.message.contains("loose and snag")));
    }

    /// A design of one thread: each run's points, and whether the thread
    /// is cut before it; all of it object 0's.
    fn sewn(runs: Vec<(Vec<P>, bool)>) -> Design {
        let spans = runs.iter().map(|_| vec![(0, 0)]).collect();
        Design {
            blocks: vec![crate::design::Block {
                color: [0, 0, 0],
                runs: runs
                    .into_iter()
                    .map(|(pts, trim_before)| crate::design::Run { pts, trim_before })
                    .collect(),
            }],
            stitch_shapes: vec![spans],
            ..Default::default()
        }
    }

    /// A line of stitches `step` apart from `from` towards `to`.
    fn line(from: P, to: P, step: f64) -> Vec<P> {
        let n = (from.dist(to) / step).round().max(1.0) as usize;
        (0..=n)
            .map(|k| from.lerp(to, k as f64 / n as f64))
            .collect()
    }

    #[test]
    fn travel_shows_only_where_nothing_sewn_later_lies_over_it() {
        // A column's stitches (up and down across y 0 to 30, 0.2 mm
        // apart, from x = 0 to 60); then one long stitch of travel from
        // near its start out to x = 75; then a second column over
        // x 30 to 80, lying over the far half of the first.
        let column = |x0: f64, x1: f64| -> Vec<P> {
            let n = ((x1 - x0) / 2.0) as usize;
            (0..=n)
                .map(|k| P::new(x0 + k as f64 * 2.0, if k % 2 == 0 { 0.0 } else { 30.0 }))
                .collect()
        };
        let mut pts = column(0.0, 60.0);
        let from = pts.len();
        pts.extend([P::new(5.0, 15.0), P::new(75.0, 15.0)]);
        let to = pts.len() - 1;
        pts.extend(column(30.0, 80.0));
        let mut d = sewn(vec![(pts, false)]);
        d.travels = vec![crate::design::Travel {
            how: crate::design::TravelHow::Under,
            shape: 0,
            block: 0,
            run: 0,
            from,
            to,
        }];
        // It crosses 27 of the first column's stitches, but the second
        // column covers it from x = 27 on: what shows is the first two
        // millimetres, eleven stitches of it.
        let shows = exposed_travel(&d);
        assert_eq!(shows.len(), 1);
        assert!((9..=12).contains(&shows[0].rows), "{:?}", shows[0]);
        assert!(shows[0].mm < 3.5 && shows[0].at.x < 27.0, "{:?}", shows[0]);
        // With nothing sewn after it, all of it shows.
        d.blocks[0].runs[0].pts.truncate(to + 1);
        let shows = exposed_travel(&d);
        assert!(shows[0].rows >= 25 && shows[0].mm > 6.0, "{:?}", shows[0]);
    }

    #[test]
    fn rows_under_a_layer_sewn_since_dont_show() {
        // A square of fill 10 mm across: its underlay first, rows up and
        // down every 0.5 mm; then its top rows across, every 0.25 mm; then
        // travel on the slant over the middle of it, 5 mm each way (from
        // somewhere the last row didn't end: a stitch already there along
        // its line would be a row it lies along).
        let mut pts: Vec<P> = Vec::new();
        for k in 0..=20 {
            let x = k as f64 * 5.0;
            let (y0, y1) = if k % 2 == 0 {
                (0.0, 100.0)
            } else {
                (100.0, 0.0)
            };
            pts.extend([P::new(x, y0), P::new(x, y1)]);
        }
        for k in 0..=40 {
            let y = k as f64 * 2.5;
            let (x0, x1) = if k % 2 == 0 {
                (0.0, 100.0)
            } else {
                (100.0, 0.0)
            };
            pts.extend([P::new(x0, y), P::new(x1, y)]);
        }
        let from = pts.len();
        pts.extend([P::new(20.0, 30.0), P::new(70.0, 80.0)]);
        let to = pts.len() - 1;
        let mut d = sewn(vec![(pts, false)]);
        d.travels = vec![crate::design::Travel {
            how: crate::design::TravelHow::Under,
            shape: 0,
            block: 0,
            run: 0,
            from,
            to,
        }];
        // It crosses 20 of the top rows, and 10 rows of underlay under
        // them: only the top rows show.
        let shows = exposed_travel(&d);
        assert_eq!(shows.len(), 1);
        assert!((19..=21).contains(&shows[0].rows), "{:?}", shows[0]);
        // Rows up and down as close, with nothing over them: what it lies
        // on is those.
        let mut bare: Vec<P> = Vec::new();
        for k in 0..=40 {
            let x = k as f64 * 2.5;
            let (y0, y1) = if k % 2 == 0 {
                (0.0, 100.0)
            } else {
                (100.0, 0.0)
            };
            bare.extend([P::new(x, y0), P::new(x, y1)]);
        }
        let from = bare.len();
        bare.extend([P::new(20.0, 30.0), P::new(70.0, 80.0)]);
        let to = bare.len() - 1;
        let mut d = sewn(vec![(bare, false)]);
        d.travels = vec![crate::design::Travel {
            how: crate::design::TravelHow::Under,
            shape: 0,
            block: 0,
            run: 0,
            from,
            to,
        }];
        let shows = exposed_travel(&d);
        assert_eq!(shows.len(), 1);
        assert!((18..=21).contains(&shows[0].rows), "{:?}", shows[0]);
    }

    #[test]
    fn crowded_stitches_count_by_the_distance_they_take_up() {
        let every = |step: f64, n: usize| (0..n).map(|k| k as f64 * step).collect::<Vec<_>>();
        // Rows as far apart as rows are sewn count one each: a fill's at
        // 0.25 mm, a satin's stitches (0.4 mm, over and back), rows a
        // millimetre apart.
        assert_eq!(rows_worth(Vec::new()), 0);
        assert_eq!(rows_worth(every(2.5, 4)), 4);
        assert_eq!(rows_worth(every(2.0, 8)), 8);
        assert_eq!(rows_worth(every(10.0, 3)), 3);
        // (And a zigzag's crossed near its edge, where the stitch over
        // and the stitch back all but meet: as many as across its middle.)
        let near_edge: Vec<f64> = (0..8)
            .map(|k| (k / 2) as f64 * 4.0 + (k % 2) as f64 * 0.2)
            .collect();
        assert_eq!(rows_worth(near_edge), 8);
        // Ten stitches fanned into a corner, crossed within a tenth of a
        // millimetre, show no more than a couple of rows would.
        assert_eq!(rows_worth(every(0.1, 10)), 2);
        assert_eq!(rows_worth(every(0.25, 10)), 3);
        // Round the inside of a bend, twice as close as on a straight:
        // twelve stitches in 1.1 mm are worth the eight that 1.1 mm of
        // straight satin and one over would be.
        assert_eq!(rows_worth(every(1.0, 12)), 8);
        // Two such fans far apart are each their own: the plain cloth
        // between them isn't rows.
        let mut two = every(0.1, 10);
        two.extend(every(0.1, 10).iter().map(|d| d + 80.0));
        assert_eq!(rows_worth(two), 5);
    }

    #[test]
    fn the_stitches_themselves_are_checked_for_what_a_machine_cant_sew() {
        let opt = PlanOptions {
            min_stitch: 6.0,
            ..Default::default()
        };
        let found = |d: &Design, what: &str| {
            let all = machine_issues(d, &opt);
            let hit: Vec<&Issue> = all.iter().filter(|i| i.message.contains(what)).collect();
            assert!(hit.len() <= 1, "{what}: said once an object: {all:?}");
            hit.first().map(|i| (i.shape, i.level, i.message.clone()))
        };
        let whats = [
            "shorter than the shortest",
            "more than a machine sews",
            "trimmer can miss",
            "needle points fall within",
            "Thread piles up",
        ];
        // A plain line of 2.5 mm stitches has nothing wrong with it.
        let good = sewn(vec![(
            line(P::new(0.0, 0.0), P::new(300.0, 0.0), 25.0),
            false,
        )]);
        assert_eq!(machine_issues(&good, &opt), []);
        // Two stitches under the document's 0.6 mm, in a line of good ones:
        // an error, for the object they're in, saying how many and where.
        let mut pts = line(P::new(0.0, 0.0), P::new(300.0, 0.0), 25.0);
        pts.insert(4, P::new(78.0, 0.0));
        pts.insert(9, P::new(204.0, 0.0));
        let (whose, level, text) =
            found(&sewn(vec![(pts, false)]), whats[0]).expect("short stitches");
        assert_eq!((whose, level), (Some(0), Level::Error));
        assert!(
            text.starts_with("2 stitches") && text.contains("0.6 mm"),
            "{text}"
        );
        // One that's exactly the shortest is fine (as the plan leaves it).
        let exact = sewn(vec![(
            line(P::new(0.0, 0.0), P::new(60.0, 0.0), 6.0),
            false,
        )]);
        assert_eq!(found(&exact, whats[0]), None);
        // A stitch of 15 mm.
        let (_, level, text) = found(
            &sewn(vec![(
                vec![
                    P::new(0.0, 0.0),
                    P::new(30.0, 0.0),
                    P::new(180.0, 0.0),
                    P::new(210.0, 0.0),
                    P::new(240.0, 0.0),
                ],
                false,
            )]),
            whats[1],
        )
        .expect("a long stitch");
        assert_eq!(level, Level::Warning);
        assert!(
            text.starts_with("1 stitch ") && text.contains("15.0 mm"),
            "{text}"
        );
        // A piece of two stitches between cuts, after a long one.
        let short_piece = sewn(vec![
            (line(P::new(0.0, 0.0), P::new(300.0, 0.0), 25.0), false),
            (line(P::new(0.0, 100.0), P::new(50.0, 100.0), 25.0), true),
        ]);
        let (_, _, text) = found(&short_piece, whats[2]).expect("a short piece");
        assert!(text.starts_with("1 piece "), "{text}");
        // (The same stitches without the cut are part of the long piece.)
        let joined = sewn(vec![
            (line(P::new(0.0, 0.0), P::new(300.0, 0.0), 25.0), false),
            (line(P::new(300.0, 30.0), P::new(350.0, 30.0), 25.0), false),
        ]);
        assert_eq!(found(&joined, whats[2]), None);
        // A fan of stitches that all come back to one point.
        let mut fan = Vec::new();
        for k in 0..9 {
            fan.push(P::new(100.5, 100.5));
            fan.push(P::new(60.0 + k as f64 * 12.0, 160.0));
        }
        let (_, _, text) = found(&sewn(vec![(fan, false)]), whats[3]).expect("a pile of holes");
        assert!(
            text.starts_with("1 spot ") && text.contains("up to 9"),
            "{text}"
        );
        // Six layers of rows 0.25 mm apart over the same square: 24 mm of
        // thread a square millimetre.
        let mut rows = Vec::new();
        for _layer in 0..6 {
            for k in 0..=40 {
                let y = k as f64 * 2.5;
                let (a, b) = (P::new(0.0, y), P::new(100.0, y));
                rows.extend(if k % 2 == 0 { [a, b] } else { [b, a] });
            }
        }
        let (_, level, text) = found(&sewn(vec![(rows.clone(), false)]), whats[4]).expect("thick");
        assert_eq!(level, Level::Info);
        assert!(text.contains("mm²"), "{text}");
        // Three layers of them (a border on a patch on a ground) aren't.
        let two: Vec<P> = rows[..rows.len() / 2].to_vec();
        assert_eq!(found(&sewn(vec![(two, false)]), whats[4]), None);
        // And what the planner makes of ordinary shapes passes all five,
        // on a machine that wants stitches of 0.6 mm and more.
        let shapes = [
            shape(
                Geometry::Fill {
                    rings: vec![rect(0.0, 0.0, 300.0, 200.0)],
                    rule: FillRule::NonZero,
                },
                Kind::Fill,
            ),
            shape(
                Geometry::Stroke {
                    lines: vec![(vec![P::new(0.0, 260.0), P::new(300.0, 260.0)], false)],
                    width: 14.0,
                },
                Kind::Satin,
            ),
            shape(
                Geometry::Stroke {
                    lines: vec![(vec![P::new(0.0, 300.0), P::new(300.0, 320.0)], false)],
                    width: 3.0,
                },
                Kind::Run,
            ),
        ];
        let d = plan("t", &shapes, &opt);
        assert_eq!(machine_issues(&d, &opt), []);
    }

    #[test]
    fn reports_fully_covered_objects() {
        let under = shape(
            Geometry::Fill {
                rings: vec![rect(10.0, 10.0, 50.0, 50.0)],
                rule: FillRule::NonZero,
            },
            Kind::Fill,
        );
        let over = shape(
            Geometry::Fill {
                rings: vec![rect(0.0, 0.0, 100.0, 100.0)],
                rule: FillRule::NonZero,
            },
            Kind::Fill,
        );
        let i = issues(&[under, over], None);
        assert!(
            i.iter()
                .any(|x| x.shape == Some(0) && x.message.contains("Completely covered")),
            "{i:?}"
        );
    }
}



