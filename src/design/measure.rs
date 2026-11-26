//! How cleanly satin stitching came out, in numbers: what the letter
//! census counts (`tools/census`), and what an agent reads before and
//! after it changes something to see whether that made it better
//! (`measure_stitching`). Every count is of something that shows in the
//! thread, and less of each is better. They're measures, not eyes: each
//! was checked against letters judged by eye (`tools/census/judged.txt`),
//! and a fan of stitches into a corner is the one they judge poorly.

use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct Lettered {
    pub columns: usize,
    /// Parts that couldn't be made into columns, and are filled.
    pub fills: usize,
    /// Column ends that come to a point.
    pub pointed: usize,
    /// Stitches that leave the shape.
    pub astray: usize,
    pub trims: usize,
    /// Trims the letter can't be sewn without: one fewer than its
    /// separate parts (the dot of an i).
    pub owed: usize,
    /// Stretches of travel left lying on top of finished stitching
    /// (see [`crate::checks::exposed_travel`]): what a trim saved
    /// mustn't cost.
    pub exposed: usize,
    /// How far the stitches of its worst column are from square
    /// across it, on average, degrees (see [`satin::lean`]): a bar
    /// whose stitches fan to meet a slanted end shows here and
    /// nowhere else.
    pub lean: usize,
    /// And how many of its stitches are more than 30° from square
    /// across their column: the fan itself, however few stitches it
    /// is of a long column.
    pub skewed: usize,
    /// How much of its inside no thread passes over, in hundredths of
    /// a square millimetre: fabric showing through, at a join the
    /// stitches either side of don't lap, or where they fan apart.
    /// (Thread taken as 0.4 mm wide; the 0.3 mm inside the outline
    /// not counted, where a satin's edge is scalloped anyway.)
    pub bare: usize,
    /// How many of its stitches are more than 30° from square to the
    /// letter's own edge where they end on it (to both edges, taken
    /// together, where both ends do). Unlike `skewed`, which is
    /// against the column's middle line: where two diagonals are cut
    /// apart on a mitre that line bends toward the cut, and stitches
    /// square to the stroke as it's drawn read as leaning.
    pub off_edge: usize,
    /// Seams: places (one every 0.3 mm) where one column carries on
    /// into another running the same way (within 45°), meeting or
    /// over it, and the stitches turn more than 25° from one to the
    /// other, with no notch in the outline nearby to make it a join
    /// (a V's crotch). A curve sewn in pieces, the stitch direction
    /// jumping along it. (Not a stroke meeting another across it, a
    /// serif under a stem.)
    pub seams: usize,
    /// How much of it (hundredths of a square mm) three or more
    /// columns cover: satin piled on satin. (Two, where one stroke
    /// crosses another, is how it's sewn.)
    pub layers: usize,
    /// Needle points more than [`OUTSIDE`] past the outline, in the
    /// stitches as planned: thread well outside the letter.
    pub outside: usize,
    /// Needle points with [`CROWD`] others or more within [`CROWDED`]
    /// of them, in the stitches as planned: the narrow end of a fan,
    /// where stitches radiate from one place, the needle goes down
    /// in the same few holes and the thread heaps up. Read off what's
    /// sewn, whatever the columns: a column fanned into a corner is
    /// square to itself, so `skewed` doesn't see it.
    pub crowded: usize,
}

/// How far past the outline a needle point is outside the letter
/// (units): beyond the most a satin's pull compensation takes it
/// (0.3 mm a side) and a little more.
pub const OUTSIDE: f64 = 6.0;
/// How near other needle points are to crowd one (units: 0.25 mm,
/// well under a satin's spacing, 0.4 mm).
pub const CROWDED: f64 = 2.5;
/// How many others that near crowd a needle point.
pub const CROWD: usize = 3;

/// [`Lettered::crowded`] of a design's stitches.
pub fn crowded(d: &Design) -> usize {
    let pts: Vec<P> = d
        .blocks
        .iter()
        .flat_map(|b| &b.runs)
        .flat_map(|r| r.pts.iter().copied())
        .collect();
    let cell = |p: P| {
        (
            (p.x / CROWDED).floor() as i64,
            (p.y / CROWDED).floor() as i64,
        )
    };
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, &p) in pts.iter().enumerate() {
        grid.entry(cell(p)).or_default().push(i);
    }
    pts.iter()
        .enumerate()
        .filter(|&(i, &p)| {
            let (x, y) = cell(p);
            let near = (-1..=1)
                .flat_map(|dx| (-1..=1).map(move |dy| (x + dx, y + dy)))
                .filter_map(|c| grid.get(&c))
                .flatten()
                .filter(|&&j| j != i && pts[j].dist(p) <= CROWDED)
                .count();
            near >= CROWD
        })
        .count()
}

/// A column's outline, to test points against (even-odd).
fn column_rings(r: &Rails) -> Vec<Polyline> {
    r.outline()
}

/// Where the outline turns in on the letter (more than 35° at one
/// point, the chord across it outside): the corners of a join.
fn notches(rings: &[Polyline], rule: FillRule) -> Vec<P> {
    let mut out = Vec::new();
    for ring in rings {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        for k in 0..n {
            let (a, p, b) = (ring[(k + n - 1) % n], ring[k], ring[(k + 1) % n]);
            let (u, v) = ((p - a).norm(), (b - p).norm());
            if u.len() < 0.5 || v.len() < 0.5 {
                continue;
            }
            let turn = crate::geom::cross(u, v).atan2(u.dot(v)).abs();
            if turn > 35f64.to_radians() && !crate::geom::point_inside(rings, rule, a.lerp(b, 0.5))
            {
                out.push(p);
            }
        }
    }
    out
}

/// [`Lettered::seams`] of one shape's columns.
pub fn seams(rings: &[Polyline], rule: FillRule, cols: &[Rails]) -> usize {
    let notch = notches(rings, rule);
    let outlines: Vec<Vec<Polyline>> = cols.iter().map(column_rings).collect();
    let pairs: Vec<Vec<(P, P)>> = cols.iter().map(|r| satin::pairs(r, 3.0)).collect();
    // Column `j` where it's nearest `p`: the stitch across it there,
    // and the way it runs.
    let at = |j: usize, k: usize| -> (P, P) {
        let ps = &pairs[j];
        let mid = |k: usize| ps[k].0.lerp(ps[k].1, 0.5);
        let (lo, hi) = (k.saturating_sub(1), (k + 1).min(ps.len() - 1));
        (ps[k].1 - ps[k].0, mid(hi) - mid(lo))
    };
    let nearest = |j: usize, p: P| -> Option<usize> {
        (0..pairs[j].len()).min_by(|&x, &y| {
            let m = |k: usize| pairs[j][k].0.lerp(pairs[j][k].1, 0.5);
            m(x).dist(p).total_cmp(&m(y).dist(p))
        })
    };
    let alike = |u: P, v: P, deg: f64| {
        u.len() > 1e-6 && v.len() > 1e-6 && u.norm().dot(v.norm()).abs() >= deg.to_radians().cos()
    };
    let mut count = 0;
    for i in 0..cols.len() {
        for k in 0..pairs[i].len() {
            let (a, b) = &pairs[i][k];
            let (stitch, runs) = at(i, k);
            if stitch.len() < 1.0 {
                continue;
            }
            // Along the stitch, where it meets or lies over another
            // column.
            let seam = (0..=4).any(|t| {
                let p = a.lerp(*b, t as f64 / 4.0);
                (0..cols.len()).filter(|&j| j != i).any(|j| {
                    let meets = crate::geom::point_inside(&outlines[j], FillRule::EvenOdd, p)
                        || outlines[j].iter().any(|r| dist_to_ring(r, p) <= 1.5);
                    let width = stitch.len().max(10.0);
                    // (One stroke carrying on into another column, the
                    // two running the same way there: not one stroke
                    // meeting another across it, a stem a serif.)
                    meets
                        && nearest(j, p).is_some_and(|q| {
                            let (other, other_runs) = at(j, q);
                            other.len() >= 1.0
                                && alike(runs, other_runs, 45.0)
                                && !alike(stitch, other, 25.0)
                        })
                        && !notch.iter().any(|c| c.dist(p) <= width)
                })
            });
            count += usize::from(seam);
        }
    }
    count
}

/// [`Lettered::layers`] of one shape's columns: unit cells (0.1 mm)
/// inside three or more.
pub fn layers(cols: &[Rails]) -> usize {
    if cols.len() < 3 {
        return 0;
    }
    let outlines: Vec<Vec<Polyline>> = cols.iter().map(column_rings).collect();
    let mut b = Bounds::empty();
    outlines.iter().flatten().flatten().for_each(|&p| b.add(p));
    let mut count = 0;
    let (x0, y0) = (b.min.x.floor() as i64, b.min.y.floor() as i64);
    let (x1, y1) = (b.max.x.ceil() as i64, b.max.y.ceil() as i64);
    for y in y0..y1 {
        for x in x0..x1 {
            let p = P::new(x as f64 + 0.5, y as f64 + 0.5);
            let over = outlines
                .iter()
                .filter(|o| crate::geom::point_inside(o, FillRule::EvenOdd, p))
                .count();
            count += usize::from(over >= 3);
        }
    }
    count
}

/// The most cells (0.1 mm squares over the outlines' bounds) bare cloth is
/// looked for over: past that it isn't counted (see [`counted`]).
pub const BARE_CELLS: usize = 4_000_000;

/// The outlines of `shapes` and `within`: what stitching is held to.
fn outlines<'a>(shapes: &'a [Shape], within: &'a [Shape]) -> Vec<(&'a Vec<Polyline>, FillRule)> {
    shapes
        .iter()
        .chain(within)
        .filter_map(|s| match &s.geometry {
            Geometry::Fill { rings, rule } => Some((rings, *rule)),
            _ => None,
        })
        .collect()
}

/// The bounds of those outlines.
fn outline_bounds(shapes: &[Shape], within: &[Shape]) -> Bounds {
    let mut bounds = Bounds::empty();
    for (rings, _) in outlines(shapes, within) {
        rings.iter().flatten().for_each(|&p| bounds.add(p));
    }
    bounds
}

/// Whether [`measured`] counts, for these, needle points outside the
/// outline (there's an outline to hold the stitching to) and bare cloth
/// (and it's not too big to go over cell by cell).
pub fn counted(shapes: &[Shape], within: &[Shape]) -> (bool, bool) {
    let b = outline_bounds(shapes, within);
    if b.is_empty() {
        return (false, false);
    }
    let cells = (b.width().ceil() as usize + 1).saturating_mul(b.height().ceil() as usize + 1);
    (true, cells <= BARE_CELLS)
}

/// A satin drawn as its two rails (what Break into Columns makes): its
/// column.
pub fn drawn_column(s: &Shape) -> Option<Rails> {
    let Geometry::Stroke { lines, .. } = &s.geometry else {
        return None;
    };
    (s.params.kind == Some(Kind::Satin) && !s.outline && !s.is_single_line() && lines.len() >= 2)
        .then(|| satin::rails_from_subpaths(lines, s.params.satin_rails()))
        .flatten()
}

/// Letters set as satins, each planned on its own as ZigZag plans it.
pub fn lettered(shapes: &[Shape]) -> Lettered {
    measured(shapes, &[], &PlanOptions::default())
}

/// How `shapes` come out, sewn on their own with `opt`: filled shapes
/// sewn in satin (each held to its own outline), and satins drawn as two
/// rails. `within`: more outlines the stitching is held to, sewn or not:
/// the shape a set of broken-out columns came from, kept beside them, so
/// that needle points outside it and cloth left bare inside it are
/// counted for them too.
pub fn measured(shapes: &[Shape], within: &[Shape], opt: &PlanOptions) -> Lettered {
    let mut l = Lettered {
        columns: 0,
        fills: 0,
        pointed: 0,
        astray: 0,
        trims: 0,
        owed: 0,
        exposed: 0,
        lean: 0,
        skewed: 0,
        bare: 0,
        off_edge: 0,
        seams: 0,
        layers: 0,
        outside: 0,
        crowded: 0,
    };
    let mut parts = 0;
    // (A drawn column has no outline of its own: what's measured against
    // one is left out for it.)
    let no_outline: Vec<Polyline> = Vec::new();
    for s in shapes {
        let (rings, rule, columns) = match &s.geometry {
            Geometry::Fill { rings, rule } => {
                parts += satin::components(rings, *rule).len();
                (rings, rule, satin_rails(s))
            }
            Geometry::Stroke { .. } => {
                let Some(r) = drawn_column(s) else { continue };
                (&no_outline, &FillRule::NonZero, vec![Ok(r)])
            }
        };
        for r in columns {
            let Ok(r) = r else {
                l.fills += 1;
                continue;
            };
            l.columns += 1;
            l.lean = l
                .lean
                .max(satin::lean(std::slice::from_ref(&r)).round() as usize);
            let ps = satin::pairs(&r, 3.0);
            let mid = |k: usize| ps[k].0.lerp(ps[k].1, 0.5);
            // (Not where the stitches turn to lie along an end of the
            // letter's own, cut at a slant, as `satin::flat_ends` turns
            // them: over FACE_TURNING times its length.)
            let gone: Vec<f64> = std::iter::once(0.0)
                .chain((1..ps.len()).scan(0.0, |d, k| {
                    *d += mid(k).dist(mid(k - 1));
                    Some(*d)
                }))
                .collect();
            let total = gone.last().copied().unwrap_or(0.0);
            let turning = |(a, b): (P, P)| {
                let on_outline = rings
                    .iter()
                    .any(|ring| dist_to_ring(ring, a.lerp(b, 0.5)) <= 1.0);
                if on_outline && a.dist(b) > 1.0 {
                    satin::FACE_TURNING * a.dist(b)
                } else {
                    0.0
                }
            };
            let (near_start, near_end) = match (ps.first(), ps.last()) {
                (Some(&f), Some(&l)) if !r.closed => (turning(f), turning(l)),
                _ => (0.0, 0.0),
            };
            l.skewed += (1..ps.len().saturating_sub(1))
                .filter(|&k| {
                    let (along, across) = (mid(k + 1) - mid(k - 1), ps[k].1 - ps[k].0);
                    gone[k] >= near_start
                        && total - gone[k] >= near_end
                        && along.len() > 1e-6
                        && across.len() > 1.0
                        && along.norm().dot(across.norm()).abs() > 0.5
                })
                .count();
            if !rings.is_empty() {
                l.off_edge += crate::stitch::columns::off_square(rings, &r);
            }
            let mut widths: Vec<f64> = ps.iter().map(|(a, b)| a.dist(*b)).collect();
            widths.sort_by(f64::total_cmp);
            let median = widths[widths.len() / 2].max(1.0);
            if !rings.is_empty() {
                l.astray += crate::stitch::columns::astray_in(rings, *rule, &r);
            }
            if !r.closed {
                let ends = [
                    (r.a[0], r.b[0]),
                    (*r.a.last().unwrap(), *r.b.last().unwrap()),
                ];
                l.pointed += ends
                    .iter()
                    .filter(|(a, b)| a.dist(*b) < 0.25 * median)
                    .count();
            }
        }
    }
    for s in shapes {
        let Geometry::Fill { rings, rule } = &s.geometry else {
            continue;
        };
        let cols: Vec<Rails> = satin_rails(s).into_iter().flatten().collect();
        l.seams += seams(rings, *rule, &cols);
        l.layers += layers(&cols);
    }
    // (Drawn columns together: satin piled three deep where they lap.)
    let drawn: Vec<Rails> = shapes.iter().filter_map(drawn_column).collect();
    l.layers += layers(&drawn);
    let d = plan("t", shapes, opt);
    l.trims = d.trim_count();
    // (As the design's checks say it: a few stitches crossed at an
    // edge is a hop.)
    l.exposed = crate::checks::exposed_travel(&d)
        .iter()
        .filter(|e| e.rows >= crate::checks::EXPOSED_ROWS)
        .count();
    l.owed = parts.saturating_sub(1);
    // (Every needle point, against every letter's outline.)
    let held = outlines(shapes, within);
    let fills: Vec<&Vec<Polyline>> = held.iter().map(|(rings, _)| *rings).collect();
    l.outside = d
        .blocks
        .iter()
        .flat_map(|b| &b.runs)
        .flat_map(|r| &r.pts)
        .filter(|&&p| {
            fills.iter().all(|rings| {
                !crate::geom::point_inside(rings, FillRule::NonZero, p)
                    && rings.iter().all(|r| dist_to_ring(r, p) > OUTSIDE)
            })
        })
        .count();
    if fills.is_empty() {
        l.outside = 0;
    }
    l.crowded = crowded(&d);
    // The stitches stamped onto a grid of 0.1 mm cells, 0.2 mm either
    // side of each; and the cells well inside the shapes that none
    // reached.
    let bounds = outline_bounds(shapes, within);
    if counted(shapes, within).1 {
        let (w, h) = (
            bounds.width().ceil() as usize + 1,
            bounds.height().ceil() as usize + 1,
        );
        let mut sewn = vec![false; w * h];
        let mut stamp = |p: P| {
            let (cx, cy) = (p.x - bounds.min.x, p.y - bounds.min.y);
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let (x, y) = (cx.floor() as i32 + dx, cy.floor() as i32 + dy);
                    let near = (x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - cy) <= 2.0;
                    if near && x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                        sewn[y as usize * w + x as usize] = true;
                    }
                }
            }
        };
        for run in d.blocks.iter().flat_map(|b| &b.runs) {
            for seg in run.pts.windows(2) {
                let n = seg[0].dist(seg[1]).ceil().max(1.0) as usize;
                (0..=n).for_each(|k| stamp(seg[0].lerp(seg[1], k as f64 / n as f64)));
            }
        }
        for (rings, rule) in &held {
            let inside = |p: P| crate::geom::point_inside(rings, *rule, p);
            for y in 0..h {
                for x in 0..w {
                    if sewn[y * w + x] {
                        continue;
                    }
                    let p = P::new(bounds.min.x + x as f64 + 0.5, bounds.min.y + y as f64 + 0.5);
                    let deep = inside(p)
                        && [(3.0, 0.0), (-3.0, 0.0), (0.0, 3.0), (0.0, -3.0)]
                            .iter()
                            .all(|&(dx, dy)| inside(P::new(p.x + dx, p.y + dy)));
                    l.bare += usize::from(deep);
                }
            }
        }
    }
    l
}
