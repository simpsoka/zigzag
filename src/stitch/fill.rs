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
    for k in k_lo..=k_hi {
        let y = (k as f64 + 0.5) * p.spacing;
        while next < edges.len() && edges[next].y0 <= y {
            active.push(next);
            next += 1;
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
                    ivs.push(Row { k, y, xa, xb });
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
        let eps = 1.0f64
            .min((last.xb - last.xa) * 0.25)
            .min((next.xb - next.xa) * 0.25);
        let a = P::new(x0 + inward * eps, last.y);
        let b = P::new(x1 + inward * eps, next.y);
        (1..4).all(|k| {
            let q = a.lerp(b, k as f64 / 4.0).rotate(cb0, sb0);
            crate::geom::point_inside(rings, rule, q)
        })
    };

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
        for (iv, c) in ivs.into_iter().zip(cands) {
            if c.len() == 1 && hits[c[0]] == 1 && connects_inside(&sections[open[c[0]]], &iv) {
                sections[open[c[0]]].push(iv);
                new_open.push(open[c[0]]);
            } else {
                sections.push(vec![iv]);
                new_open.push(sections.len() - 1);
            }
        }
        open = new_open;
    }

    let (cb, sb) = (p.angle.cos(), p.angle.sin());
    let stagger = p.stagger.max(1) as i64;
    let min_end = (p.len * 0.25).min(5.0);
    sections
        .into_iter()
        .map(|rows| {
            let mut pts: Polyline = Vec::with_capacity(rows.len() * 4);
            for (ri, r) in rows.iter().enumerate() {
                let forward = ri % 2 == 0;
                let off = (r.k.rem_euclid(stagger)) as f64 * p.len / stagger as f64;
                let m0 = ((r.xa + min_end - off) / p.len).ceil() as i64;
                let m1 = ((r.xb - min_end - off) / p.len).floor() as i64;
                let (xs, xe) = if forward { (r.xa, r.xb) } else { (r.xb, r.xa) };
                let start = P::new(xs, r.y);
                if let Some(&prev) = pts.last() {
                    // Travel from the previous row's end along the outline.
                    let d = prev.dist(start);
                    let n = (d / p.len).ceil() as usize;
                    for i in 1..n {
                        pts.push(prev.lerp(start, i as f64 / n as f64));
                    }
                }
                pts.push(start);
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
        }
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
        };
        let secs = tatami(&[ring], FillRule::NonZero, &p);
        assert_eq!(secs.len(), 1);
        assert!(secs[0].iter().all(|q| q.len() <= 56.5));
        assert!(secs[0].windows(2).all(|w| w[0].dist(w[1]) <= 30.0 + 1e-6));
    }
}
