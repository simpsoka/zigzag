//! Design checks: the size and width rules experienced digitizers apply,
//! reported per object so the editor (and agents) can point at the problem.

use crate::design::{Design, PlanOptions, satin_rails};
use crate::geom::{Bounds, Polyline};
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

fn ring_bounds(rings: &[Polyline]) -> Bounds {
    let mut b = Bounds::empty();
    rings.iter().flatten().for_each(|&p| b.add(p));
    b
}

/// Run all checks. `hoop_mm` is the hoop's (width, height). (`_opt`: the
/// options the design was planned with; what overlaps cover comes from the
/// plan itself.)
pub fn check(
    shapes: &[Shape],
    design: &Design,
    _opt: &PlanOptions,
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
        if kind == Kind::Skip {
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
                let comps = satin::components(rings, *rule);
                let small_parts = comps
                    .iter()
                    .filter(|c| ring_bounds(c).height() < 40.0)
                    .count();
                if comps.len() >= 3 && small_parts * 2 > comps.len() {
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
                for hole in comps.iter().flat_map(|c| c.iter().skip(1)) {
                    let hb = ring_bounds(std::slice::from_ref(hole));
                    if hb.width().min(hb.height()) < 10.0 {
                        add(
                            Some(i),
                            Level::Warning,
                            "Has a gap under 1 mm wide; thread will close it up.".into(),
                        );
                        break;
                    }
                }
            }
            (_, Kind::Satin) => {
                let widths: Vec<f64> = satin_rails(s)
                    .iter()
                    .filter_map(|r| r.as_ref().ok())
                    .flat_map(|r| satin::pairs(r, 10.0).into_iter().map(|(a, b)| a.dist(b)))
                    .collect();
                if widths.is_empty() {
                    continue;
                }
                let max = widths.iter().copied().fold(0.0, f64::max) / 10.0;
                let mean = widths.iter().sum::<f64>() / widths.len() as f64 / 10.0;
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
                if mean < 1.0 {
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
