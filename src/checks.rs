//! Design checks: the size and width rules experienced digitizers apply,
//! reported per object so the editor (and agents) can point at the problem.

use crate::design::{Design, PlanOptions, satin_rails};
use crate::geom::{Bounds, P, Polyline};
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
const EXPOSED_ROWS: usize = 8;

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
    let crosses = |a: P, b: P, s: &Seg| {
        side(a, b, s.a) * side(a, b, s.b) < 0.0 && side(s.a, s.b, a) * side(s.a, s.b, b) < 0.0
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
        let (mut mm, mut rows, mut at) = (0.0, 0, None);
        for w in way.windows(2) {
            let over = by
                .iter()
                .filter(|s| s.last <= g0 && crosses(w[0], w[1], s))
                .count();
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
                rows += over;
                mm += w[0].dist(w[1]) / 10.0;
                at.get_or_insert(w[0].lerp(w[1], 0.5));
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
    /// Widest and mean width, mm.
    Satin(Option<(f64, f64)>),
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
                "The thread crosses {rows} rows of stitches already sewn on its way from one part of this object to the next, and shows on top of them.{}",
                if opt.over_sewn {
                    " Turn off Sew over fill (Document panel) to cut it there instead."
                } else {
                    ""
                }
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
                let Facts::Satin(Some((max, mean))) =
                    facts(&s.geometry, Some(&format!("{:?}", s.params)), || {
                        let widths: Vec<f64> = satin_rails(s)
                            .iter()
                            .filter_map(|r| r.as_ref().ok())
                            .flat_map(|r| satin::pairs(r, 10.0).into_iter().map(|(a, b)| a.dist(b)))
                            .collect();
                        Facts::Satin((!widths.is_empty()).then(|| {
                            (
                                widths.iter().copied().fold(0.0, f64::max) / 10.0,
                                widths.iter().sum::<f64>() / widths.len() as f64 / 10.0,
                            )
                        }))
                    })
                else {
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
                if mean < opt.narrow_satin {
                    add(
                        Some(i),
                        Level::Warning,
                        format!(
                            "Satin only {mean:.1} mm wide on average: too narrow to look smooth. Use Run."
                        ),
                    );
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.level));
    out
}

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
