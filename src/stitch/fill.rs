//! Tatami fill: parallel rows at an angle, split into monotone sections that
//! can each be sewn back-and-forth without leaving the shape.
//!
//! The approach follows the classic scanline method also used by Ink/Stitch:
//! rotate the shape so rows are horizontal, intersect each row with the
//! outline (honoring the fill rule), group row intervals into sections, then
//! stitch each section boustrophedon with staggered needle penetrations.

use crate::geom::{FillRule, P, Polyline};

#[derive(Clone, Debug)]
pub struct FillParams {
    /// Row direction, radians.
    pub angle: f64,
    /// Distance between rows, units.
    pub spacing: f64,
    /// Max stitch length along a row, units.
    pub len: f64,
    /// Shrink each row end by this much (for underlay), units.
    pub inset: f64,
    /// Extend each row end by this much (pull compensation), units.
    pub pull: f64,
    /// Number of rows in the stagger cycle (tatami pattern).
    pub stagger: u32,
    /// Sew rows from the bottom up (for caps: push puckering away from the brim seam).
    pub bottom_up: bool,
    /// No stitch shorter than this, units. Rows closer together than that
    /// can't be sewn one after the next (the turn at each row's end would
    /// be too short a stitch), so they're sewn every second (or third…)
    /// row one way and the rows between on the way back: the same needle
    /// points, with turns two (or three…) rows long.
    pub min_stitch: f64,
}

struct Edge {
    y0: f64,
    y1: f64,
    x0: f64,
    dxdy: f64,
    dir: i32,
}

struct Row {
    k: i64,
    y: f64,
    xa: f64,
    xb: f64,
    /// A stitch straight from the row before's end to this one's start
    /// would leave the shape (the ends are far apart along a slanting or
    /// wavy edge): the way between them is along the rows instead.
    dogleg: bool,
}

/// Returns one stitched polyline per section, in original coordinates.
pub fn tatami(rings: &[Polyline], rule: FillRule, p: &FillParams) -> Vec<Polyline> {
    let (c, s) = ((-p.angle).cos(), (-p.angle).sin());
    let mut edges: Vec<Edge> = Vec::new();
    for ring in rings {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let a = ring[i].rotate(c, s);
            let b = ring[(i + 1) % n].rotate(c, s);
            if (a.y - b.y).abs() < 1e-9 {
                continue;
            }
            let (top, bot, dir) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
            edges.push(Edge {
                y0: top.y,
                y1: bot.y,
                x0: top.x,
                dxdy: (bot.x - top.x) / (bot.y - top.y),
                dir,
            });
        }
    }
    if edges.is_empty() {
        return Vec::new();
    }
    edges.sort_unstable_by(|a, b| a.y0.total_cmp(&b.y0));
    let ymin = edges[0].y0;
    let ymax = edges.iter().map(|e| e.y1).fold(f64::NEG_INFINITY, f64::max);

    // Rows sit on a global grid so neighbouring shapes with the same settings line up.
    let k_lo = (ymin / p.spacing - 0.5).ceil() as i64;
    let k_hi = (ymax / p.spacing - 0.5).floor() as i64;
    let shrink = p.inset - p.pull;
    let min_row = (p.spacing * 0.5).max(3.0);

    let mut active: Vec<usize> = Vec::new();
    let mut next = 0usize;
    let mut xs: Vec<(f64, i32)> = Vec::new();
    let mut rows: Vec<Vec<Row>> = Vec::new(); // grouped per k
    // The edges crossing the gap between row k and the next, per k: what
    // decides whether a stitch joining the two rows stays inside.
    let mut gaps: Vec<Vec<usize>> = Vec::new();
    for k in k_lo..=k_hi {
        let y = (k as f64 + 0.5) * p.spacing;
        while next < edges.len() && edges[next].y0 <= y {
            active.push(next);
            next += 1;
        }
        if k > k_lo {
            // Still active from the row before, or starting by this one.
            gaps.push(active.clone());
        }
        active.retain(|&e| edges[e].y1 > y);
        xs.clear();
        xs.extend(active.iter().map(|&e| {
            let e = &edges[e];
            (e.x0 + (y - e.y0) * e.dxdy, e.dir)
        }));
        xs.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut w = 0;
        let mut start = 0.0;
        let mut ivs = Vec::new();
        for &(x, d) in &xs {
            let was = rule.inside(w);
            w += d;
            let now = rule.inside(w);
            if !was && now {
                start = x;
            } else if was && !now {
                let (xa, xb) = (start + shrink, x - shrink);
                if xb - xa >= min_row {
                    ivs.push(Row {
                        k,
                        y,
                        xa,
                        xb,
                        dogleg: false,
                    });
                }
            }
        }
        rows.push(ivs);
    }

    if p.bottom_up {
        rows.reverse();
    }

    // Rows in a section are joined end to end with a straight stitch. Only
    // continue a section if that stitch stays inside the shape; otherwise a
    // big jump in the row ends (a stem meeting a crossbar) would cut a corner.
    let (cb0, sb0) = (p.angle.cos(), p.angle.sin());
    let connects_inside = |section: &[Row], next: &Row| -> bool {
        let last = section.last().unwrap();
        // Last row runs forward (ends at xb) when the section has an odd row count.
        let at_b = section.len() % 2 == 1;
        let (x0, x1, inward) = if at_b {
            (last.xb, next.xb, -1.0)
        } else {
            (last.xa, next.xa, 1.0)
        };
        // (From just inside the outline: rows widened for pull end past
        // it, where nothing is inside.)
        let eps = 1.0f64
            .min((last.xb - last.xa) * 0.25)
            .min((next.xb - next.xa) * 0.25)
            + (-shrink).max(0.0);
        let a = P::new(x0 + inward * eps, last.y);
        let b = P::new(x1 + inward * eps, next.y);
        let gap = (last.k - next.k)
            .abs()
            .eq(&1)
            .then(|| gaps.get((last.k.min(next.k) - k_lo) as usize))
            .flatten();
        (1..4).all(|k| {
            let q = a.lerp(b, k as f64 / 4.0);
            match gap {
                // The winding at q, from only the edges between the rows
                // (the outline can have thousands of points).
                Some(gap) => {
                    let mut w = 0;
                    for &e in gap {
                        let e = &edges[e];
                        if e.y0 <= q.y && q.y < e.y1 && e.x0 + (q.y - e.y0) * e.dxdy < q.x {
                            w += e.dir;
                        }
                    }
                    rule.inside(w)
                }
                None => crate::geom::point_inside(rings, rule, q.rotate(cb0, sb0)),
            }
        })
    };

    // The rows are sewn one after the next (not in passes: see `min_stitch`).
    let one_pass = !(p.min_stitch > p.spacing && p.spacing > 0.0);

    // Group intervals into sections: an interval continues a section when it is
    // the only overlap of that section's last interval (and vice versa).
    let mut sections: Vec<Vec<Row>> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for ivs in rows {
        if ivs.is_empty() {
            open.clear();
            continue;
        }
        let mut hits = vec![0u32; open.len()];
        let cands: Vec<Vec<usize>> = ivs
            .iter()
            .map(|iv| {
                let c: Vec<usize> = open
                    .iter()
                    .enumerate()
                    .filter(|(_, si)| {
                        let last = sections[**si].last().unwrap();
                        last.xa < iv.xb && iv.xa < last.xb
                    })
                    .map(|(i, _)| i)
                    .collect();
                for &i in &c {
                    hits[i] += 1;
                }
                c
            })
            .collect();
        let mut new_open = Vec::with_capacity(ivs.len());
        for (mut iv, c) in ivs.into_iter().zip(cands) {
            if c.len() == 1 && hits[c[0]] == 1 {
                // The one row over the one row before: the same section.
                // Where a stitch between their ends would cut a corner (a
                // stem meeting a crossbar, a row at a wavy edge), the two
                // are joined along the rows (see `dogleg`). A row split
                // off there would be a section of its own, a row or two
                // sewn after the rest, with the thread taken to it across
                // rows already sewn. (Rows sewn in passes go by the old
                // way: their turns skip rows.)
                let inside = connects_inside(&sections[open[c[0]]], &iv);
                if inside || one_pass {
                    iv.dogleg = !inside;
                    sections[open[c[0]]].push(iv);
                    new_open.push(open[c[0]]);
                    continue;
                }
            }
            {
                sections.push(vec![iv]);
                new_open.push(sections.len() - 1);
            }
        }
        open = new_open;
    }

    let (cb, sb) = (p.angle.cos(), p.angle.sin());
    let stagger = p.stagger.max(1) as i64;
    let min_end = (p.len * 0.25).min(5.0).max(p.min_stitch);
    sections
        .into_iter()
        .map(|rows| {
            let mut pts: Polyline = Vec::with_capacity(rows.len() * 4);
            // The order the rows are sewn in: as they come, or in `skip`
            // passes of every `skip`th row, there and back.
            let skip = if p.min_stitch > p.spacing && p.spacing > 0.0 {
                (p.min_stitch / p.spacing).ceil() as usize
            } else {
                1
            };
            let mut order: Vec<usize> = Vec::with_capacity(rows.len());
            for pass in 0..skip.min(rows.len()) {
                let at = order.len();
                order.extend((pass..rows.len()).step_by(skip));
                if pass % 2 == 1 {
                    order[at..].reverse();
                }
                // A pass begins beside the row the last one ended on: too
                // near. So it begins at its first row that's far enough,
                // comes back for the ones before, and goes on from there:
                // every turn is long enough.
                if pass > 0 {
                    let last = order[at - 1];
                    if let Some(j) = order[at..].iter().position(|k| k.abs_diff(last) >= skip) {
                        order[at..=at + j].reverse();
                    }
                }
            }
            // (The row sewn last: its place among the section's rows.)
            let mut last_row: Option<usize> = None;
            for (ri, &k) in order.iter().enumerate() {
                let r = &rows[k];
                let forward = ri % 2 == 0;
                let off = (r.k.rem_euclid(stagger)) as f64 * p.len / stagger as f64;
                let m0 = ((r.xa + min_end - off) / p.len).ceil() as i64;
                let m1 = ((r.xb - min_end - off) / p.len).floor() as i64;
                let (xs, xe) = if forward { (r.xa, r.xb) } else { (r.xb, r.xa) };
                let start = P::new(xs, r.y);
                match pts.last().copied() {
                    // The last row ended too near for a stitch between
                    // (a pass of one row, or rows that meet at a shape's
                    // tip): one needle point turns the corner, whichever
                    // of the two reaches further out, so neither row stops
                    // short of the outline.
                    Some(prev) if prev.dist(start) < p.min_stitch => {
                        let further = if forward {
                            start.x < prev.x
                        } else {
                            start.x > prev.x
                        };
                        if further {
                            *pts.last_mut().unwrap() = start;
                        }
                    }
                    Some(_) => {
                        // Travel from the previous row's end along the
                        // outline. To a row some way off (where a pass
                        // over the rows ends and the next begins), by the
                        // ends of the rows between, `skip` rows a stitch:
                        // straight there could leave the shape.
                        if let Some(from) = last_row {
                            let hops = from.abs_diff(k) / skip.max(1);
                            for h in 1..hops {
                                let m = if k > from {
                                    from + h * skip
                                } else {
                                    from - h * skip
                                };
                                let via = &rows[m];
                                pts.push(P::new(if forward { via.xa } else { via.xb }, via.y));
                            }
                        }
                        let prev = *pts.last().unwrap();
                        // Round a corner the straight way would cut: back
                        // along the row just sewn to where this one is
                        // over it, across, and along this row's line to
                        // its start. All of it on a row or under one.
                        let mut legs = vec![prev];
                        if r.dogleg && skip == 1 {
                            let x = prev.x.clamp(r.xa, r.xb);
                            for q in [P::new(x, prev.y), P::new(x, r.y)] {
                                if legs.last().is_some_and(|l| l.dist(q) > 1e-6) {
                                    legs.push(q);
                                }
                            }
                        }
                        legs.push(start);
                        for w in legs.windows(2) {
                            let d = w[0].dist(w[1]);
                            let n = (d / p.len).ceil() as usize;
                            for i in 1..n {
                                pts.push(w[0].lerp(w[1], i as f64 / n as f64));
                            }
                            if d > 1e-6 {
                                pts.push(w[1]);
                            }
                        }
                        if pts.last().is_some_and(|l| l.dist(start) > 1e-6) {
                            pts.push(start);
                        }
                    }
                    None => pts.push(start),
                }
                last_row = Some(k);
                let mut push = |q: P| {
                    // Split the (rare) over-long first/last stitch of a row.
                    let prev = *pts.last().unwrap();
                    let n = (prev.dist(q) / p.len).ceil() as usize;
                    for i in 1..n {
                        pts.push(prev.lerp(q, i as f64 / n as f64));
                    }
                    pts.push(q);
                };
                if m1 >= m0 {
                    if forward {
                        (m0..=m1).for_each(|m| push(P::new(off + m as f64 * p.len, r.y)));
                    } else {
                        (m0..=m1)
                            .rev()
                            .for_each(|m| push(P::new(off + m as f64 * p.len, r.y)));
                    }
                }
                push(P::new(xe, r.y));
            }
            for q in pts.iter_mut() {
                *q = q.rotate(cb, sb);
            }
            pts
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, w: f64) -> Polyline {
        vec![
            P::new(x0, y0),
            P::new(x0 + w, y0),
            P::new(x0 + w, y0 + w),
            P::new(x0, y0 + w),
        ]
    }

    fn params() -> FillParams {
        FillParams {
            angle: 0.0,
            spacing: 2.5,
            len: 30.0,
            inset: 0.0,
            pull: 0.0,
            stagger: 4,
            bottom_up: false,
            min_stitch: 0.0,
        }
    }

    #[test]
    fn a_stem_under_a_crossbar_is_one_section_joined_along_the_rows() {
        // A T: the crossbar's rows, then the stem's. A stitch from the
        // end of the last crossbar row to the stem would cut the corner
        // outside the shape, so the stem used to be a section of its own
        // (and a row or two split off like that at a wavy edge was sewn
        // after the rest, the thread taken to it across the fill). Now
        // it's the same section: back along the crossbar's last row to
        // over the stem, and on.
        let t = [vec![
            P::new(0.0, 0.0),
            P::new(300.0, 0.0),
            P::new(300.0, 50.0),
            P::new(180.0, 50.0),
            P::new(180.0, 200.0),
            P::new(120.0, 200.0),
            P::new(120.0, 50.0),
            P::new(0.0, 50.0),
        ]];
        let sections = tatami(&t, FillRule::NonZero, &params());
        assert_eq!(sections.len(), 1);
        let pts = &sections[0];
        // Every stitch stays in the T: no corner cut across the open
        // cloth beside the stem.
        for w in pts.windows(2) {
            for k in 0..=4 {
                let q = w[0].lerp(w[1], k as f64 / 4.0);
                let inside = if q.y <= 50.0 {
                    q.x > -0.01 && q.x < 300.01
                } else {
                    q.x > 119.99 && q.x < 180.01
                };
                assert!(inside && q.y > -0.01 && q.y < 200.01, "{q:?}");
            }
        }
        // And all of it is sewn: rows the crossbar's width, rows the
        // stem's, down to its foot.
        assert!(pts.iter().any(|q| q.x < 1.0) && pts.iter().any(|q| q.x > 299.0));
        assert!(pts.iter().any(|q| q.y > 195.0));
        // In passes (a shortest stitch longer than the rows are apart) the
        // turns skip rows, and the two stay separate, as before.
        let passes = FillParams {
            min_stitch: 4.0,
            ..params()
        };
        assert_eq!(tatami(&t, FillRule::NonZero, &passes).len(), 2);
    }

    #[test]
    fn rows_closer_than_the_shortest_stitch_are_sewn_in_passes() {
        let sq = [square(0.0, 0.0, 100.0)];
        let usual = tatami(&sq, FillRule::NonZero, &params()).remove(0);
        // The same needle points, whatever the order (and however often:
        // the way from one pass to the next goes by row ends it has sewn).
        let key = |q: &P| ((q.x * 1e3).round() as i64, (q.y * 1e3).round() as i64);
        let sorted = |pts: &[P]| {
            let mut k: Vec<_> = pts.iter().map(key).collect();
            k.sort();
            k.dedup();
            k
        };
        // 0.4 mm over 0.25 mm rows: every second row, then the rows
        // between. 0.4 mm over 0.15 mm rows: three passes.
        for (spacing, passes) in [(2.5, 2.0), (1.5, 3.0)] {
            let mut p = params();
            p.spacing = spacing;
            let plain = tatami(&sq, FillRule::NonZero, &p).remove(0);
            p.min_stitch = 4.0;
            let s = tatami(&sq, FillRule::NonZero, &p).remove(0);
            let shortest = s
                .windows(2)
                .map(|w| w[0].dist(w[1]))
                .fold(f64::INFINITY, f64::min);
            assert!(shortest >= 4.0 - 1e-9, "{spacing}: a {shortest} stitch");
            assert_eq!(
                sorted(&s),
                sorted(&plain),
                "{spacing}: the same needle points"
            );
            // Each turn is a stitch up the edge, `passes` rows long.
            let turns: Vec<f64> = s
                .windows(2)
                .filter(|w| (w[0].x - w[1].x).abs() < 1e-6)
                .map(|w| (w[0].y - w[1].y).abs())
                .collect();
            assert!(turns.len() > 30);
            assert!(
                turns.iter().all(|t| *t >= spacing * passes - 1e-6),
                "{spacing}"
            );
            assert!(s.windows(2).all(|w| w[0].dist(w[1]) <= 30.0 + 1e-6));
        }
        // Rows no closer than the shortest stitch are sewn as ever.
        let mut p = params();
        p.min_stitch = 2.5;
        assert_eq!(tatami(&sq, FillRule::NonZero, &p).remove(0), usual);
        // A section of one row, or two, has nowhere to go but the next row.
        let mut p = params();
        p.min_stitch = 4.0;
        let sliver = [vec![
            P::new(0.0, 0.0),
            P::new(100.0, 0.0),
            P::new(100.0, 4.0),
            P::new(0.0, 4.0),
        ]];
        let s = tatami(&sliver, FillRule::NonZero, &p).remove(0);
        assert!(s.len() >= 4 && s.iter().all(|q| q.y >= 0.0 && q.y <= 4.0));
    }

    #[test]
    fn rows_widened_for_pull_are_still_one_section() {
        // A stem 1.6 mm wide, its rows 0.3 mm apart and widened 0.3 mm a
        // side for pull (as on a knit): the rows end past the outline, and
        // are joined all the same. (Each used to be a section of its own.)
        let stem = [vec![
            P::new(0.0, 0.0),
            P::new(16.0, 0.0),
            P::new(16.0, 40.0),
            P::new(0.0, 40.0),
        ]];
        let mut p = params();
        p.spacing = 3.0;
        p.pull = 3.0;
        let plain = tatami(&stem, FillRule::NonZero, &p);
        assert_eq!(plain.len(), 1);
        // With a shortest stitch longer than the rows are apart, every row
        // is still sewn from one side to the other.
        p.min_stitch = 4.0;
        let s = tatami(&stem, FillRule::NonZero, &p).remove(0);
        for k in 0..13 {
            let y = (k as f64 + 0.5) * 3.0;
            for x in [-3.0, 19.0] {
                assert!(
                    s.iter()
                        .any(|q| (q.x - x).abs() < 1e-6 && (q.y - y).abs() < 1e-6),
                    "row {k} reaches x = {x}"
                );
            }
        }
        assert!(s.windows(2).all(|w| w[0].dist(w[1]) >= 4.0 - 1e-9));
        // And the way from one pass to the next keeps to the stem's side.
        assert!(s.iter().all(|q| q.x >= -3.0 - 1e-6 && q.x <= 19.0 + 1e-6));
        assert!(
            s.windows(2)
                .filter(|w| (w[0].x - w[1].x).abs() < 1e-6)
                .all(|w| (w[0].y - w[1].y).abs() <= 9.0 + 1e-6),
            "no stitch up the side longer than three rows"
        );
    }

    #[test]
    fn square_is_one_section_with_short_stitches() {
        let secs = tatami(&[square(0.0, 0.0, 100.0)], FillRule::NonZero, &params());
        assert_eq!(secs.len(), 1);
        let s = &secs[0];
        assert!(s.windows(2).all(|w| w[0].dist(w[1]) <= 30.0 + 1e-6));
        assert!(
            s.iter()
                .all(|p| p.x >= -1e-6 && p.x <= 100.0 + 1e-6 && p.y >= 0.0 && p.y <= 100.0)
        );
        // 40 rows at 0.25 mm in a 10 mm square.
        assert_eq!(
            s.iter()
                .filter(|p| p.x.abs() < 1e-6 || (p.x - 100.0).abs() < 1e-6)
                .count(),
            80
        );
    }

    #[test]
    fn hole_splits_into_sections_and_respects_evenodd() {
        let rings = vec![square(0.0, 0.0, 100.0), square(30.0, 30.0, 40.0)];
        let secs = tatami(&rings, FillRule::EvenOdd, &params());
        assert_eq!(secs.len(), 4, "top, left, right, bottom around the hole");
        for s in &secs {
            for p in s {
                assert!(
                    !(p.x > 31.0 && p.x < 69.0 && p.y > 31.0 && p.y < 69.0),
                    "stitch inside hole: {p:?}"
                );
            }
        }
    }

    #[test]
    fn angle_rotates_rows() {
        let mut p = params();
        p.angle = std::f64::consts::FRAC_PI_2;
        let secs = tatami(&[square(0.0, 0.0, 100.0)], FillRule::NonZero, &p);
        // Vertical rows: long runs keep x constant.
        let s = &secs[0];
        assert!(
            s.windows(2)
                .filter(|w| (w[0].x - w[1].x).abs() < 1e-6)
                .count()
                > s.len() / 2
        );
    }
}

#[cfg(test)]
mod circle_tests {
    use super::*;

    #[test]
    fn circle_fill_stays_inside_as_one_section() {
        let ring: Polyline = (0..200)
            .map(|i| {
                let a = i as f64 / 200.0 * std::f64::consts::TAU;
                P::new(56.4 * a.cos(), 56.4 * a.sin())
            })
            .collect();
        let p = FillParams {
            angle: (-30f64).to_radians(),
            spacing: 2.5,
            len: 30.0,
            inset: 0.0,
            pull: 0.0,
            stagger: 4,
            bottom_up: false,
            min_stitch: 0.0,
        };
        let secs = tatami(&[ring], FillRule::NonZero, &p);
        assert_eq!(secs.len(), 1);
        assert!(secs[0].iter().all(|q| q.len() <= 56.5));
        assert!(secs[0].windows(2).all(|w| w[0].dist(w[1]) <= 30.0 + 1e-6));
    }
}
