//! Decorative fills, after Ink/Stitch's: meander (a wandering line),
//! ripple (concentric running-stitch rows, far apart), cross-stitch (a grid
//! of X's) and tartan (stripes). Each returns polylines in design units
//! (0.1 mm); the planner joins and routes them.

use crate::geom::{FillRule, P, Polyline, point_inside, segment_inside};

/// Keep the parts of `pts` (a path) that lie inside the shape: a list of
/// runs, each at least two points.
pub fn clip_inside(pts: &[P], rings: &[Polyline], rule: FillRule) -> Vec<Polyline> {
    let mut out = Vec::new();
    let mut cur: Polyline = Vec::new();
    for w in pts.windows(2) {
        if point_inside(rings, rule, w[0])
            && point_inside(rings, rule, w[1])
            && segment_inside(rings, rule, w[0], w[1], 2.0)
        {
            if cur.is_empty() {
                cur.push(w[0]);
            }
            cur.push(w[1]);
        } else if cur.len() >= 2 {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.clear();
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

fn bounds(rings: &[Polyline]) -> (P, P) {
    let mut lo = P::new(f64::INFINITY, f64::INFINITY);
    let mut hi = P::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for q in rings.iter().flatten() {
        lo = P::new(lo.x.min(q.x), lo.y.min(q.y));
        hi = P::new(hi.x.max(q.x), hi.y.max(q.y));
    }
    (lo, hi)
}

/// A small deterministic random source (xorshift), so a design plans the
/// same way every time.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.max(1) ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Meander: one wandering line that visits every `spacing`-sized cell of
/// the shape (a random walk with backtracking, so it's a single path),
/// its corners rounded, stitched every `len`. Light and quilting-like.
pub fn meander(
    rings: &[Polyline],
    rule: FillRule,
    spacing: f64,
    len: f64,
    seed: u64,
) -> Vec<Polyline> {
    let (lo, hi) = bounds(rings);
    if !lo.x.is_finite() {
        return Vec::new();
    }
    let cols = ((hi.x - lo.x) / spacing).ceil() as usize + 1;
    let rows = ((hi.y - lo.y) / spacing).ceil() as usize + 1;
    if cols * rows > 400_000 {
        return Vec::new();
    }
    let at = |c: usize, r: usize| P::new(lo.x + c as f64 * spacing, lo.y + r as f64 * spacing);
    // Cells whose center is inside, and whose links to neighbours stay inside.
    let inside: Vec<bool> = (0..cols * rows)
        .map(|k| point_inside(rings, rule, at(k % cols, k / cols)))
        .collect();
    let mut visited = vec![false; cols * rows];
    let mut rng = Rng::new(seed);
    let mut out: Vec<Polyline> = Vec::new();
    let link_ok = |a: usize, b: usize| {
        segment_inside(
            rings,
            rule,
            at(a % cols, a / cols),
            at(b % cols, b / cols),
            2.0,
        )
    };
    for start in 0..cols * rows {
        if !inside[start] || visited[start] {
            continue;
        }
        // A depth-first walk from here: every step goes to an unvisited
        // neighbour (random order); when stuck it walks back along its own
        // path to the last cell with a free neighbour, which is still one
        // continuous line.
        let mut path: Vec<usize> = vec![start];
        let mut stack: Vec<usize> = vec![start];
        visited[start] = true;
        while let Some(&here) = stack.last() {
            let (c, r) = (here % cols, here / cols);
            let mut opts: Vec<usize> = Vec::with_capacity(4);
            if c > 0 {
                opts.push(here - 1);
            }
            if c + 1 < cols {
                opts.push(here + 1);
            }
            if r > 0 {
                opts.push(here - cols);
            }
            if r + 1 < rows {
                opts.push(here + cols);
            }
            // Random order.
            for i in (1..opts.len()).rev() {
                let j = rng.below(i + 1);
                opts.swap(i, j);
            }
            let next = opts
                .into_iter()
                .find(|&n| inside[n] && !visited[n] && link_ok(here, n));
            match next {
                Some(n) => {
                    visited[n] = true;
                    stack.push(n);
                    path.push(n);
                }
                None => {
                    stack.pop();
                    if let Some(&back) = stack.last() {
                        path.push(back);
                    }
                }
            }
        }
        if path.len() < 2 {
            continue;
        }
        // Round the corners: a point a third of the way along each step,
        // a Chaikin pass, then stitches every `len`.
        let pts: Polyline = path.iter().map(|&k| at(k % cols, k / cols)).collect();
        let mut smooth: Polyline = vec![pts[0]];
        for w in pts.windows(2) {
            smooth.push(w[0].lerp(w[1], 0.3));
            smooth.push(w[0].lerp(w[1], 0.7));
        }
        smooth.push(*pts.last().unwrap());
        let dense = crate::geom::resample_keep_corners(&smooth, len.max(5.0));
        out.extend(clip_inside(&dense, rings, rule));
    }
    out
}

/// Ripple: running-stitch rows `spacing` apart that follow the outline
/// inward, like contour fill but open (`spacing` is 1 mm and up) and
/// joined into one continuous spiral when `spiral` is set. Returns the
/// rows outside in.
pub fn ripple(
    rings: &[Polyline],
    rule: FillRule,
    spacing: f64,
    len: f64,
    spiral: bool,
) -> Vec<Polyline> {
    let mut levels: Vec<Vec<Polyline>> = Vec::new();
    let mut d = spacing * 0.5;
    for _ in 0..500 {
        let level: Vec<Polyline> = crate::boolean::offset(rings, rule, -d)
            .into_iter()
            .filter(|r| r.len() >= 3 && crate::boolean::ring_area(r) > spacing * spacing)
            .map(|mut r| {
                if r.first() != r.last() {
                    r.push(r[0]);
                }
                crate::geom::resample_keep_corners(&r, len)
            })
            .collect();
        if level.is_empty() {
            break;
        }
        levels.push(level);
        d += spacing;
    }
    if !spiral {
        return levels.into_iter().flatten().collect();
    }
    // One spiral: each ring blended into the next so the line never
    // closes. Only when every level is a single ring; otherwise the rings
    // stay separate and the planner joins them.
    if levels.iter().any(|l| l.len() != 1) {
        return levels.into_iter().flatten().collect();
    }
    let mut out: Polyline = Vec::new();
    for pair in levels.windows(2) {
        let (a, b) = (&pair[0][0], &pair[1][0]);
        let n = a.len().max(2);
        for (i, p) in a.iter().enumerate() {
            let t = i as f64 / (n - 1) as f64;
            // The matching point on the inner ring, by fraction of its length.
            let q = b[((t * (b.len() - 1) as f64).round() as usize).min(b.len() - 1)];
            out.push(p.lerp(q, t));
        }
    }
    if let Some(last) = levels.last() {
        out.extend(last[0].iter().copied());
    }
    if out.len() < 2 {
        return Vec::new();
    }
    vec![out]
}

/// Cross-stitch: an X in every `cell`-sized square whose center is inside
/// the shape, sewn as rows of half-crosses out and back (as hand
/// cross-stitch is), one row of cells at a time.
pub fn cross_stitch(rings: &[Polyline], rule: FillRule, cell: f64) -> Vec<Polyline> {
    let (lo, hi) = bounds(rings);
    if !lo.x.is_finite() {
        return Vec::new();
    }
    // Cells on a global grid, so neighbouring shapes line up.
    let c0 = (lo.x / cell).floor() as i64;
    let c1 = (hi.x / cell).ceil() as i64;
    let r0 = (lo.y / cell).floor() as i64;
    let r1 = (hi.y / cell).ceil() as i64;
    let mut out = Vec::new();
    for r in r0..r1 {
        let y0 = r as f64 * cell;
        let y1 = y0 + cell;
        let cells: Vec<i64> = (c0..c1)
            .filter(|&c| {
                let x = c as f64 * cell;
                point_inside(rings, rule, P::new(x + cell * 0.5, y0 + cell * 0.5))
            })
            .collect();
        // Runs of adjacent cells share one out-and-back row.
        let mut i = 0;
        while i < cells.len() {
            let mut j = i;
            while j + 1 < cells.len() && cells[j + 1] == cells[j] + 1 {
                j += 1;
            }
            let (ca, cb) = (cells[i], cells[j]);
            let mut row: Polyline = Vec::new();
            // Out: bottom-left to top-right of each cell.
            for c in ca..=cb {
                let x = c as f64 * cell;
                row.push(P::new(x, y1));
                row.push(P::new(x + cell, y0));
            }
            // Back: top-left to bottom-right, right to left.
            for c in (ca..=cb).rev() {
                let x = c as f64 * cell;
                row.push(P::new(x + cell, y1));
                row.push(P::new(x, y0));
            }
            out.push(row);
            i = j + 1;
        }
    }
    out
}

/// One tartan stripe: its colour and width (units).
#[derive(Clone, Debug, PartialEq)]
pub struct Stripe {
    pub color: [u8; 3],
    pub width: f64,
}

/// Tartan: the shape filled with horizontal and vertical stripes of
/// `sett` (repeated, mirrored as a symmetric sett), each stripe tatami
/// rows `spacing` apart; the vertical stripes are sewn over the horizontal
/// ones at 45° the other way, as woven tartan reads. Returns, per colour,
/// the rows to sew (one entry per distinct colour, in sett order).
pub fn tartan(
    rings: &[Polyline],
    rule: FillRule,
    sett: &[Stripe],
    spacing: f64,
    len: f64,
    angle: f64,
) -> Vec<([u8; 3], Vec<Polyline>)> {
    use crate::stitch::fill::{FillParams, tatami};
    if sett.is_empty() {
        return Vec::new();
    }
    let total: f64 = sett.iter().map(|s| s.width).sum::<f64>() * 2.0;
    if total <= 0.0 {
        return Vec::new();
    }
    // The sett out and back (mirrored).
    let mut bands: Vec<(f64, f64, [u8; 3])> = Vec::new();
    let mut at = 0.0;
    for s in sett.iter().chain(sett.iter().rev()) {
        bands.push((at, at + s.width, s.color));
        at += s.width;
    }
    let (lo, _) = bounds(rings);
    if !lo.x.is_finite() {
        return Vec::new();
    }
    let (c, s) = (angle.cos(), angle.sin());
    let rot = |p: P| P::new(p.x * c - p.y * s, p.x * s + p.y * c);
    let unrot = |p: P| P::new(p.x * c + p.y * s, -p.x * s + p.y * c);
    // Work in the rotated frame: stripes along x and along y.
    let rr: Vec<Polyline> = rings
        .iter()
        .map(|r| r.iter().map(|&p| unrot(p)).collect())
        .collect();
    let (rlo, rhi) = bounds(&rr);
    let reach = (rhi.x - rlo.x).max(rhi.y - rlo.y) + total;
    let mut per_color: Vec<([u8; 3], Vec<Polyline>)> = Vec::new();
    let mut push = |color: [u8; 3], rows: Vec<Polyline>| {
        if rows.is_empty() {
            return;
        }
        match per_color.iter_mut().find(|(cc, _)| *cc == color) {
            Some((_, v)) => v.extend(rows),
            None => per_color.push((color, rows)),
        }
    };
    for horizontal in [true, false] {
        let start = if horizontal { rlo.y } else { rlo.x };
        let first = ((start / total).floor()) * total;
        let mut k = 0.0;
        while first + k <= start + reach {
            for &(b0, b1, color) in &bands {
                let (p0, p1) = (first + k + b0, first + k + b1);
                // The band as a rectangle across the shape, intersected with it.
                let band: Polyline = if horizontal {
                    vec![
                        P::new(rlo.x - total, p0),
                        P::new(rhi.x + total, p0),
                        P::new(rhi.x + total, p1),
                        P::new(rlo.x - total, p1),
                    ]
                } else {
                    vec![
                        P::new(p0, rlo.y - total),
                        P::new(p1, rlo.y - total),
                        P::new(p1, rhi.y + total),
                        P::new(p0, rhi.y + total),
                    ]
                };
                let clipped = crate::boolean::pathfinder(
                    &[(rr.clone(), rule), (vec![band], FillRule::NonZero)],
                    crate::boolean::PathOp::Intersect,
                );
                if clipped.is_empty() {
                    continue;
                }
                let fp = FillParams {
                    // Rows along the stripe: a woven look.
                    angle: if horizontal {
                        0.0
                    } else {
                        std::f64::consts::FRAC_PI_2
                    },
                    spacing,
                    len,
                    inset: 0.0,
                    pull: 0.0,
                    stagger: 4,
                    bottom_up: false,
                };
                let rows: Vec<Polyline> = tatami(&clipped, FillRule::EvenOdd, &fp)
                    .into_iter()
                    .map(|row| row.into_iter().map(rot).collect())
                    .collect();
                push(color, rows);
            }
            k += total;
        }
    }
    per_color
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sq(w: f64) -> Vec<Polyline> {
        vec![vec![
            P::new(0.0, 0.0),
            P::new(w, 0.0),
            P::new(w, w),
            P::new(0.0, w),
        ]]
    }

    fn all_inside(runs: &[Polyline], w: f64) -> bool {
        runs.iter()
            .flatten()
            .all(|p| p.x >= -0.5 && p.x <= w + 0.5 && p.y >= -0.5 && p.y <= w + 0.5)
    }

    #[test]
    fn meander_is_one_line_covering_the_shape() {
        let rings = sq(200.0);
        let runs = meander(&rings, FillRule::NonZero, 30.0, 20.0, 7);
        assert!(all_inside(&runs, 200.0));
        let n: usize = runs.iter().map(|r| r.len()).sum();
        assert!(n > 80, "{n}");
        // The same seed gives the same walk.
        let again = meander(&rings, FillRule::NonZero, 30.0, 20.0, 7);
        assert_eq!(runs, again);
        // It visits all four corners of the square.
        let near = |x: f64, y: f64| runs.iter().flatten().any(|p| p.dist(P::new(x, y)) < 45.0);
        assert!(near(0.0, 0.0) && near(200.0, 0.0) && near(0.0, 200.0) && near(200.0, 200.0));
    }

    #[test]
    fn ripple_rows_are_spaced_and_a_spiral_is_one_line() {
        let rings = sq(200.0);
        let rows = ripple(&rings, FillRule::NonZero, 20.0, 25.0, false);
        assert!(rows.len() >= 4 && rows.len() <= 6, "{}", rows.len());
        assert!(all_inside(&rows, 200.0));
        let sp = ripple(&rings, FillRule::NonZero, 20.0, 25.0, true);
        assert_eq!(sp.len(), 1, "one continuous line");
        assert!(all_inside(&sp, 200.0));
        assert!(sp[0].len() > 40);
    }

    #[test]
    fn cross_stitch_puts_an_x_in_every_cell() {
        let rings = sq(100.0);
        let rows = cross_stitch(&rings, FillRule::NonZero, 25.0);
        // 4 rows of 4 cells, each row one out-and-back run.
        assert_eq!(rows.len(), 4);
        for r in &rows {
            assert_eq!(r.len(), 16);
        }
        assert!(all_inside(&rows, 100.0));
    }

    #[test]
    fn tartan_makes_one_block_per_color_and_stays_inside() {
        let rings = sq(300.0);
        let sett = vec![
            Stripe {
                color: [200, 0, 0],
                width: 40.0,
            },
            Stripe {
                color: [0, 80, 0],
                width: 20.0,
            },
        ];
        let out = tartan(&rings, FillRule::NonZero, &sett, 4.0, 30.0, 0.0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, [200, 0, 0]);
        for (_, rows) in &out {
            assert!(!rows.is_empty());
            assert!(all_inside(rows, 300.0));
        }
        let red: usize = out[0].1.iter().map(|r| r.len()).sum();
        let green: usize = out[1].1.iter().map(|r| r.len()).sum();
        assert!(red > green, "wider stripes sew more: {red} vs {green}");
    }
}
