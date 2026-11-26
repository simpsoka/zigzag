//! Running stitch along a line (satin columns live in `satin.rs`).

use crate::geom::{P, Polyline, resample_keep_corners};

/// Running stitch along `line`, `passes` times back and forth (odd = ends at the far end).
pub fn running(line: &[P], closed: bool, len: f64, passes: u32) -> Polyline {
    let mut base: Polyline = line.to_vec();
    if closed && base.len() > 2 {
        base.push(base[0]);
    }
    let one = resample_keep_corners(&base, len);
    let mut out = one.clone();
    for pass in 1..passes.max(1) {
        if pass % 2 == 1 {
            out.extend(one.iter().rev().skip(1));
        } else {
            out.extend(one.iter().skip(1));
        }
    }
    out
}

/// Lines that meet, each group of them as one line to sew: what a letter
/// of a single-line font is (an H is three strokes, touching), sewn in
/// one piece of thread the way hand embroiderers and digitizers sew
/// redwork. Where the strokes branch it goes down one, comes back along
/// it over its own stitches, and carries on; strokes are joined where one
/// ends on another or they cross (within `tol`). The groups come out in
/// the order their first strokes are in.
pub fn joined(lines: &[Polyline], tol: f64) -> Vec<Polyline> {
    // Each line's points, with the places others meet it added.
    let mut pts: Vec<Polyline> = lines.iter().filter(|l| l.len() >= 2).cloned().collect();
    let bounds: Vec<(P, P)> = pts
        .iter()
        .map(|l| {
            let mut lo = P::new(f64::MAX, f64::MAX);
            let mut hi = P::new(f64::MIN, f64::MIN);
            for q in l {
                lo = P::new(lo.x.min(q.x), lo.y.min(q.y));
                hi = P::new(hi.x.max(q.x), hi.y.max(q.y));
            }
            (lo - P::new(tol, tol), hi + P::new(tol, tol))
        })
        .collect();
    let near = |a: usize, b: usize| {
        let ((alo, ahi), (blo, bhi)) = (bounds[a], bounds[b]);
        alo.x <= bhi.x && blo.x <= ahi.x && alo.y <= bhi.y && blo.y <= ahi.y
    };
    // (line, segment, where along it) for every meeting found.
    let mut cuts: Vec<(usize, usize, P)> = Vec::new();
    for a in 0..pts.len() {
        for b in 0..pts.len() {
            if !near(a, b) {
                continue;
            }
            // An end of `a` on a segment of `b`.
            if a != b {
                for end in [pts[a][0], *pts[a].last().unwrap()] {
                    for (k, w) in pts[b].windows(2).enumerate() {
                        let q = closest_on(w[0], w[1], end);
                        if q.dist(end) <= tol {
                            cuts.push((b, k, q));
                        }
                    }
                }
            }
            // Crossings (each pair once; a line may cross itself).
            if b < a {
                continue;
            }
            for (i, u) in pts[a].windows(2).enumerate() {
                for (k, v) in pts[b].windows(2).enumerate() {
                    if a == b && k <= i + 1 {
                        continue;
                    }
                    if let Some(x) = crossing(u[0], u[1], v[0], v[1]) {
                        cuts.push((a, i, x));
                        cuts.push((b, k, x));
                    }
                }
            }
        }
    }
    // Put the cuts into the lines, the last segment first so the indices
    // of those before stay put, in order along each segment.
    cuts.sort_by(|x, y| {
        (x.0, x.1)
            .cmp(&(y.0, y.1))
            .then_with(|| {
                let s = pts[x.0][x.1];
                s.dist(x.2).total_cmp(&s.dist(y.2))
            })
    });
    let mut cut_at: Vec<Vec<usize>> = vec![Vec::new(); pts.len()];
    for (line, seg, q) in cuts.into_iter().rev() {
        let l = &mut pts[line];
        let (s, e) = (l[seg], l[seg + 1]);
        let k = if q.dist(s) <= tol * 0.25 {
            seg
        } else if q.dist(e) <= tol * 0.25 {
            seg + 1
        } else {
            l.insert(seg + 1, q);
            for c in cut_at[line].iter_mut() {
                if *c > seg {
                    *c += 1;
                }
            }
            seg + 1
        };
        cut_at[line].push(k);
    }
    // Nodes: the lines' ends and cuts, those within `tol` one node.
    let mut nodes: Vec<P> = Vec::new();
    let mut node_of = |q: P| -> usize {
        match nodes.iter().position(|n| n.dist(q) <= tol) {
            Some(i) => i,
            None => {
                nodes.push(q);
                nodes.len() - 1
            }
        }
    };
    // Edges: the pieces of line between nodes.
    let mut edges: Vec<(usize, usize, Polyline)> = Vec::new();
    for (li, l) in pts.iter().enumerate() {
        let mut at: Vec<usize> = cut_at[li].clone();
        at.push(0);
        at.push(l.len() - 1);
        at.sort_unstable();
        at.dedup();
        // (A whole stroke, short as it may be, is something drawn: a full
        // stop, the dot of an i. Only what a cut leaves over is dropped.)
        let whole = at.len() == 2;
        for w in at.windows(2) {
            let piece = l[w[0]..=w[1]].to_vec();
            let short = crate::geom::polyline_length(&piece) <= tol && !whole;
            if short && w[0] != 0 {
                continue;
            }
            let (a, b) = (node_of(piece[0]), node_of(*piece.last().unwrap()));
            if a == b && short {
                continue;
            }
            edges.push((a, b, piece));
        }
    }
    let n = nodes.len();
    let mut at_node: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (k, (a, b, _)) in edges.iter().enumerate() {
        at_node[*a].push(k);
        if b != a {
            at_node[*b].push(k);
        }
    }
    let len_of: Vec<f64> = edges
        .iter()
        .map(|(_, _, l)| crate::geom::polyline_length(l))
        .collect();
    let other = |k: usize, from: usize| {
        if edges[k].0 == from {
            edges[k].1
        } else {
            edges[k].0
        }
    };
    // The edge from `from`, in its direction.
    let walk = |k: usize, from: usize| -> Polyline {
        let l = &edges[k].2;
        if edges[k].0 == from {
            l.clone()
        } else {
            l.iter().rev().copied().collect()
        }
    };
    let mut sewn = vec![false; edges.len()];
    let mut out = Vec::new();
    for first in 0..edges.len() {
        if sewn[first] {
            continue;
        }
        // Start at an end (a node with an odd number of strokes) of this
        // group if there is one: the leftmost, else the first stroke's.
        let mut group = vec![false; n];
        let mut stack = vec![edges[first].0];
        while let Some(v) = stack.pop() {
            if std::mem::replace(&mut group[v], true) {
                continue;
            }
            stack.extend(at_node[v].iter().map(|&k| other(k, v)));
        }
        let mut cur = (0..n)
            .filter(|&v| group[v] && at_node[v].len() % 2 == 1)
            .min_by(|&a, &b| nodes[a].x.total_cmp(&nodes[b].x))
            .unwrap_or(edges[first].0);
        let mut path: Polyline = vec![nodes[cur]];
        loop {
            let free: Vec<usize> = at_node[cur].iter().copied().filter(|&k| !sewn[k]).collect();
            if !free.is_empty() {
                // A dead end first (shortest), to come back from at once.
                let dead = |k: usize| {
                    let v = other(k, cur);
                    v != cur && at_node[v].iter().all(|&j| j == k || sewn[j])
                };
                let k = free
                    .iter()
                    .copied()
                    .filter(|&k| dead(k))
                    .min_by(|&a, &b| len_of[a].total_cmp(&len_of[b]))
                    .unwrap_or(free[0]);
                sewn[k] = true;
                path.extend(walk(k, cur).into_iter().skip(1));
                cur = other(k, cur);
                continue;
            }
            // Stuck: back along what's sewn to the nearest node with a
            // stroke still to sew (shortest way, along the strokes).
            let mut dist = vec![f64::INFINITY; n];
            let mut came: Vec<Option<(usize, usize)>> = vec![None; n];
            let mut done = vec![false; n];
            dist[cur] = 0.0;
            let mut target = None;
            while let Some(v) = (0..n)
                .filter(|&v| !done[v] && dist[v].is_finite())
                .min_by(|&a, &b| dist[a].total_cmp(&dist[b]))
            {
                done[v] = true;
                if at_node[v].iter().any(|&k| !sewn[k]) {
                    target = Some(v);
                    break;
                }
                for &k in &at_node[v] {
                    let u = other(k, v);
                    if dist[v] + len_of[k] < dist[u] {
                        dist[u] = dist[v] + len_of[k];
                        came[u] = Some((k, v));
                    }
                }
            }
            let Some(t) = target else { break };
            let mut back = Vec::new();
            let mut v = t;
            while let Some((k, from)) = came[v] {
                back.push((k, from));
                v = from;
            }
            for (k, from) in back.into_iter().rev() {
                path.extend(walk(k, from).into_iter().skip(1));
            }
            cur = t;
        }
        out.push(path);
    }
    out
}

/// The point of segment `a b` nearest `q`.
fn closest_on(a: P, b: P, q: P) -> P {
    let d = b - a;
    let l2 = d.dot(d);
    if l2 <= 1e-12 {
        return a;
    }
    a + d * ((q - a).dot(d) / l2).clamp(0.0, 1.0)
}

/// Where segments `a b` and `c d` cross, strictly inside both.
fn crossing(a: P, b: P, c: P, d: P) -> Option<P> {
    let r = b - a;
    let s = d - c;
    let den = crate::geom::cross(r, s);
    if den.abs() < 1e-12 {
        return None;
    }
    let t = crate::geom::cross(c - a, s) / den;
    let u = crate::geom::cross(c - a, r) / den;
    let eps = 1e-6;
    (t > eps && t < 1.0 - eps && u > eps && u < 1.0 - eps).then(|| a + r * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(p: &[(f64, f64)]) -> Polyline {
        p.iter().map(|&(x, y)| P::new(x, y)).collect()
    }

    #[test]
    fn an_h_is_sewn_in_one_piece_going_back_along_a_stroke_once() {
        // Two stems and a bar between their middles.
        let h = [
            line(&[(0.0, 0.0), (0.0, 100.0)]),
            line(&[(60.0, 0.0), (60.0, 100.0)]),
            line(&[(0.0, 50.0), (60.0, 50.0)]),
        ];
        let paths = joined(&h, 2.0);
        assert_eq!(paths.len(), 1);
        let len = crate::geom::polyline_length(&paths[0]);
        // Every stroke once (260), and back along half a stem each side
        // of the bar at most.
        assert!((260.0..=360.0).contains(&len), "{len}");
        // Everything is sewn: each stroke's middle is on the path.
        for q in [(0.0, 25.0), (0.0, 75.0), (60.0, 25.0), (60.0, 75.0), (30.0, 50.0)] {
            let q = P::new(q.0, q.1);
            assert!(
                paths[0]
                    .windows(2)
                    .any(|w| closest_on(w[0], w[1], q).dist(q) < 0.01)
            );
        }
    }

    #[test]
    fn crossing_strokes_join_and_apart_ones_do_not() {
        let x = [
            line(&[(0.0, 0.0), (100.0, 100.0)]),
            line(&[(0.0, 100.0), (100.0, 0.0)]),
            // An i's dot, apart.
            line(&[(200.0, 0.0), (200.0, 5.0)]),
        ];
        let paths = joined(&x, 2.0);
        assert_eq!(paths.len(), 2);
        let len = crate::geom::polyline_length(&paths[0]);
        // Both diagonals (283), and back along two of the four arms to
        // reach the last (141): four ends, so two of them have to be
        // joined by stitches sewn twice. No shorter way exists.
        assert!(len > 282.0 && len < 425.0, "{len}");
    }

    #[test]
    fn a_stroke_shorter_than_the_tolerance_is_still_sewn() {
        // A full stop beside a stem: 0.3 units long, tolerance 5.
        let dot = line(&[(40.0, 100.0), (40.3, 100.0)]);
        let paths = joined(&[line(&[(0.0, 0.0), (0.0, 100.0)]), dot.clone()], 5.0);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[1], dot);
        // (What a crossing leaves over, as short, is still dropped: the
        // bar of a T overshooting its stem by a hair.)
        let t = [
            line(&[(0.0, 0.0), (0.0, 100.0)]),
            line(&[(-1.0, 50.0), (60.0, 50.0)]),
        ];
        let paths = joined(&t, 5.0);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].iter().all(|p| p.x > -0.5), "{:?}", paths[0]);
    }

    #[test]
    fn a_closed_stroke_is_sewn_round_once() {
        let o = [line(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)])];
        let paths = joined(&o, 1.0);
        assert_eq!(paths.len(), 1);
        assert!((crate::geom::polyline_length(&paths[0]) - 40.0).abs() < 1e-9);
    }

    #[test]
    fn running_respects_length_and_passes() {
        let line = vec![P::new(0.0, 0.0), P::new(100.0, 0.0)];
        let r = running(&line, false, 25.0, 1);
        assert_eq!(r.len(), 5);
        let bean = running(&line, false, 25.0, 3);
        assert_eq!(bean.len(), 13);
        assert_eq!(bean.last().unwrap().x, 100.0);
    }
}
