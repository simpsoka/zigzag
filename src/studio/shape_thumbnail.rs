//! Shared painter-only artwork previews, with bounded solid interior coverage.
use eframe::egui::{self, Color32, Painter, Pos2, Rect, Stroke};
use zigzag::{
    geom::{P, point_inside},
    svg::{Geometry, Shape},
};

const MAX_POINTS: usize = 2048;
const PATH_POINTS: usize = 128;
// No ring decimation for interiors: omit an entire fill if its exact geometry
// exceeds the shared budget, rather than accidentally closing holes/islands.
const FILL_POINTS: usize = 16_384;
const GRID: usize = 22;
const FILL_SPANS: usize = 2048;

fn paths(shape: &Shape) -> impl Iterator<Item = (&[P], bool)> {
    let (rings, lines) = match &shape.geometry {
        Geometry::Fill { rings, .. } => (Some(rings), None),
        Geometry::Stroke { lines, .. } => (None, Some(lines)),
    };
    rings
        .into_iter()
        .flatten()
        .map(|p| (p.as_slice(), true))
        .chain(
            lines
                .into_iter()
                .flatten()
                .map(|(p, closed)| (p.as_slice(), *closed)),
        )
}

/// Pure projection shared by individual and composite previews. Hidden filtering is
/// explicit: a hidden object still has an individual preview in Layers.
pub(crate) fn project(shapes: &[&Shape], rect: Rect, skip_hidden: bool) -> Vec<egui::Shape> {
    if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return Vec::new();
    }
    let visible = || {
        shapes
            .iter()
            .copied()
            .filter(|s| !skip_hidden || !s.params.hidden())
    };
    let points = || {
        visible()
            .flat_map(paths)
            .flat_map(|(p, _)| p.iter())
            .filter(|p| p.x.is_finite() && p.y.is_finite())
    };
    // Normalize before subtracting: even opposite f64::MAX coordinates fit.
    let magnitude = points().fold(1.0_f64, |m, p| m.max(p.x.abs()).max(p.y.abs()));
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in points() {
        for (axis, value) in [p.x / magnitude, p.y / magnitude].into_iter().enumerate() {
            lo[axis] = lo[axis].min(value);
            hi[axis] = hi[axis].max(value);
        }
    }
    if !lo[0].is_finite() {
        return Vec::new();
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let size = f64::from(rect.width().min(rect.height()).max(0.0));
    let map = |p: &P| {
        let axis = |v: f64, a: usize| {
            if span > 0.0 {
                ((v / magnitude - lo[a]) - (hi[a] - lo[a]) / 2.0) / span * size
            } else {
                0.0
            }
        };
        rect.center() + egui::vec2(axis(p.x, 0) as f32, axis(p.y, 1) as f32)
    };
    let mut result = Vec::new();
    let mut remaining = MAX_POINTS;
    let mut fill_points = FILL_POINTS;
    let mut fill_spans = FILL_SPANS;
    for shape in visible() {
        let color = Color32::from_rgb(shape.color[0], shape.color[1], shape.color[2]);
        if let Geometry::Fill { rings, rule } = &shape.geometry {
            let count = rings.iter().try_fold(0usize, |n, r| n.checked_add(r.len()));
            if span > 0.0 && count.is_some_and(|n| n <= fill_points) {
                fill_points -= count.unwrap();
                // Normalize in f64 before winding calculations; neither subtraction
                // nor cross products can overflow even for opposite f64::MAXs.
                // Reject a malformed ring whole, never bridge across invalid points.
                let normalized: Vec<_> = rings
                    .iter()
                    .filter(|r| {
                        r.len() >= 3 && r.iter().all(|p| p.x.is_finite() && p.y.is_finite())
                    })
                    .map(|r| {
                        r.iter()
                            .map(|p| {
                                let axis = |v: f64, a: usize| {
                                    ((v / magnitude - lo[a]) - (hi[a] - lo[a]) / 2.0) / span
                                };
                                P::new(axis(p.x, 0), axis(p.y, 1))
                            })
                            .collect()
                    })
                    .collect();
                let cells = (size.ceil() as usize).clamp(1, GRID);
                let step = size / cells as f64;
                let origin = rect.center() - egui::Vec2::splat(size as f32 / 2.0);
                // Center-sampled coverage, grouped into opaque horizontal spans.
                // At most 22² winding queries per shape; no textures or tessellator.
                let mut spans = Vec::new();
                for y in 0..cells {
                    let mut start = None;
                    for x in 0..=cells {
                        let inside = x < cells
                            && point_inside(
                                &normalized,
                                *rule,
                                P::new(
                                    (x as f64 + 0.5) / cells as f64 - 0.5,
                                    (y as f64 + 0.5) / cells as f64 - 0.5,
                                ),
                            );
                        if inside && start.is_none() {
                            start = Some(x);
                        }
                        if !inside {
                            if let Some(left) = start.take() {
                                let min = origin
                                    + egui::vec2(
                                        (left as f64 * step) as f32,
                                        (y as f64 * step) as f32,
                                    );
                                let max = origin
                                    + egui::vec2(
                                        (x as f64 * step) as f32,
                                        ((y + 1) as f64 * step) as f32,
                                    );
                                spans.push(egui::Shape::rect_filled(
                                    Rect::from_min_max(min, max),
                                    0.0,
                                    color,
                                ));
                            }
                        }
                    }
                }
                // Keep a fill atomic if the composite primitive budget runs out.
                if spans.len() <= fill_spans {
                    fill_spans -= spans.len();
                    result.extend(spans);
                }
            }
        }
        for (path, closed) in paths(shape) {
            if remaining == 0 {
                return result;
            }
            if path.is_empty() {
                continue;
            }
            let n = path.len().min(PATH_POINTS).min(remaining);
            let pts: Vec<Pos2> = (0..n)
                .map(|i| {
                    &path[if n == 1 {
                        0
                    } else {
                        i * (path.len() - 1) / (n - 1)
                    }]
                })
                .filter(|p| p.x.is_finite() && p.y.is_finite())
                .map(map)
                .collect();
            remaining -= n;
            let stroke = Stroke::new(1.25, color);
            match pts.len() {
                0 => {}
                1 => result.push(egui::Shape::circle_filled(pts[0], 1.0, color)),
                _ if closed => result.push(egui::Shape::closed_line(pts, stroke)),
                _ => result.push(egui::Shape::line(pts, stroke)),
            }
        }
    }
    result
}

pub fn paint(painter: &Painter, rect: Rect, shapes: &[&Shape], skip_hidden: bool) {
    if !painter.clip_rect().intersects(rect) {
        return;
    }
    painter
        .with_clip_rect(rect.intersect(painter.clip_rect()))
        .extend(project(shapes, rect.shrink(1.5), skip_hidden));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<Shape> {
        zigzag::document::Document::from_svg(r##"<svg xmlns="http://www.w3.org/2000/svg"><g id="pair"><path d="M0 0L10 0L10 10Z" fill="#e01020"/><path d="M20 0L30 10" fill="none" stroke="#1020e0"/></g></svg>"##, "test", None).unwrap().shapes
    }
    #[test]
    fn union_preserves_positions_colors_and_aspect() {
        let s = fixture();
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(22.0, 22.0));
        let out = project(&s.iter().collect::<Vec<_>>(), rect, true);
        assert!(
            out.iter().any(
                |s| matches!(s, egui::Shape::Rect(r) if r.fill == Color32::from_rgb(224,16,32))
            )
        );
        let out: Vec<_> = out
            .iter()
            .filter(|s| matches!(s, egui::Shape::Path(_)))
            .collect();
        assert_eq!(out.len(), 2);
        let egui::Shape::Path(a) = out[0] else {
            panic!()
        };
        let egui::Shape::Path(b) = out[1] else {
            panic!()
        };
        assert!(a.closed && !b.closed);
        assert_eq!(
            a.stroke.color,
            egui::epaint::ColorMode::Solid(Color32::from_rgb(224, 16, 32))
        );
        assert_eq!(
            b.stroke.color,
            egui::epaint::ColorMode::Solid(Color32::from_rgb(16, 32, 224))
        );
        assert!(a.points.last().unwrap().x < b.points[0].x);
        assert!((a.points[1].x - a.points[0].x - (a.points[2].y - a.points[1].y)).abs() < 0.001);
        for path in [a, b] {
            assert!(path.points.iter().all(|p| rect.contains(*p)));
        }
        assert_eq!(
            project(&[&s[0], &s[0]], rect, true).len(),
            2 * project(&[&s[0]], rect, true).len()
        );
    }
    fn preview(svg: &str) -> Vec<egui::Shape> {
        let doc = zigzag::document::Document::from_svg(svg, "fill test", None).unwrap();
        project(
            &doc.shapes.iter().collect::<Vec<_>>(),
            Rect::from_min_size(Pos2::ZERO, egui::Vec2::splat(22.0)),
            true,
        )
    }

    fn color_at(out: &[egui::Shape], x: f32, y: f32) -> Option<Color32> {
        out.iter()
            .filter_map(|s| match s {
                egui::Shape::Rect(r) if r.rect.contains(Pos2::new(x, y)) => Some(r.fill),
                _ => None,
            })
            .last()
    }

    #[test]
    fn solid_fill_and_winding_holes_islands() {
        for (rule, inner, hole) in [
            ("evenodd", "M3 3H7V7H3Z", true),
            ("nonzero", "M3 3V7H7V3Z", true),
            ("nonzero", "M3 3H7V7H3Z", false),
        ] {
            let svg = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg"><path fill="#e01020" fill-rule="{rule}" d="M0 0H10V10H0Z {inner}"/></svg>"##
            );
            let out = preview(&svg);
            assert_eq!(
                color_at(&out, 2.5, 11.5),
                Some(Color32::from_rgb(224, 16, 32))
            );
            assert_eq!(color_at(&out, 11.5, 11.5).is_none(), hole);
            assert!(
                out.iter()
                    .any(|s| matches!(s, egui::Shape::Path(p) if p.closed && p.stroke.width > 0.0))
            );
        }
        let out = preview(
            r##"<svg xmlns="http://www.w3.org/2000/svg"><path fill="red" fill-rule="evenodd" d="M0 0H10V10H0Z M2 2H8V8H2Z M4 4H6V6H4Z"/></svg>"##,
        );
        assert!(color_at(&out, 6.5, 11.5).is_none());
        assert_eq!(color_at(&out, 11.5, 11.5), Some(Color32::RED));
    }

    #[test]
    fn concavity_disjoint_subpaths_and_strokes() {
        let out = preview(
            r##"<svg xmlns="http://www.w3.org/2000/svg"><path fill="red" d="M0 0H4V6H10V10H0Z M8 0H10V2H8Z"/></svg>"##,
        );
        assert_eq!(color_at(&out, 2.5, 2.5), Some(Color32::RED));
        assert!(color_at(&out, 13.5, 5.5).is_none());
        assert_eq!(color_at(&out, 20.5, 2.5), Some(Color32::RED));
        for close in ["", "Z"] {
            let out = preview(&format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg"><path fill="none" stroke="blue" d="M0 0H10V10H0{close}"/></svg>"##
            ));
            assert!(color_at(&out, 11.5, 11.5).is_none());
            assert!(
                out.iter()
                    .any(|s| matches!(s, egui::Shape::Path(p) if p.closed == !close.is_empty()))
            );
        }
    }

    #[test]
    fn composite_color_order_and_hidden_members() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg"><g id="outer"><path fill="red" d="M0 0H10V10H0Z"/><g id="inner"><path fill="blue" d="M3 3H7V7H3Z"/></g></g></svg>"##;
        let mut doc = zigzag::document::Document::from_svg(svg, "group", None).unwrap();
        let rect = Rect::from_min_size(Pos2::ZERO, egui::Vec2::splat(22.0));
        let out = project(&doc.shapes.iter().collect::<Vec<_>>(), rect, true);
        assert_eq!(color_at(&out, 11.5, 11.5), Some(Color32::BLUE));
        assert_eq!(color_at(&out, 2.5, 11.5), Some(Color32::RED));
        let reversed = project(&doc.shapes.iter().rev().collect::<Vec<_>>(), rect, true);
        assert_eq!(color_at(&reversed, 11.5, 11.5), Some(Color32::RED));
        doc.shapes[1].params.hide = Some(true);
        assert_eq!(
            color_at(
                &project(&doc.shapes.iter().collect::<Vec<_>>(), rect, true),
                11.5,
                11.5
            ),
            Some(Color32::RED)
        );
        assert_eq!(
            color_at(
                &project(&doc.shapes.iter().collect::<Vec<_>>(), rect, false),
                11.5,
                11.5
            ),
            Some(Color32::BLUE)
        );
    }

    #[test]
    fn fill_extremes_invalid_rings_and_atomic_budget() {
        use zigzag::geom::FillRule;
        let mut shape = fixture().remove(0);
        let rect = Rect::from_min_size(Pos2::ZERO, egui::Vec2::splat(22.0));
        for scale in [f64::MAX, 1.0, f64::MIN_POSITIVE] {
            shape.geometry = Geometry::Fill {
                rings: vec![vec![
                    P::new(-scale, -scale),
                    P::new(scale, -scale),
                    P::new(scale, scale),
                    P::new(-scale, scale),
                ]],
                rule: FillRule::NonZero,
            };
            let out = project(&[&shape], rect, false);
            assert_eq!(
                color_at(&out, 11.5, 11.5),
                Some(Color32::from_rgb(224, 16, 32))
            );
            assert!(out.iter().all(|s| s.visual_bounding_rect().is_finite()));
        }
        for ring in [
            vec![],
            vec![P::new(1.0, 1.0); 3],
            vec![P::new(f64::NAN, 0.0), P::new(0.0, 1.0), P::new(1.0, 0.0)],
            vec![P::new(0.0, 0.0); FILL_POINTS + 1],
        ] {
            shape.geometry = Geometry::Fill {
                rings: vec![ring],
                rule: FillRule::EvenOdd,
            };
            let out = project(&[&shape], rect, false);
            assert!(out.iter().all(|s| !matches!(s, egui::Shape::Rect(_))));
        }
        let shapes = fixture();
        let many = vec![&shapes[0]; 1000];
        let out = project(&many, rect, false);
        assert!(out.len() <= FILL_SPANS + MAX_POINTS);
    }

    #[test]
    fn extreme_empty_point_and_budget_are_safe() {
        let mut s = fixture().remove(0);
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(22.0, 22.0));
        for points in [
            vec![],
            vec![P::new(1.0, 1.0)],
            vec![P::new(-f64::MAX, 0.0), P::new(f64::MAX, 0.0)],
            vec![P::new(0.0, 0.0); 10000],
        ] {
            s.geometry = Geometry::Stroke {
                lines: vec![(points, false)],
                width: 1.0,
            };
            for out in project(&[&s], rect, false) {
                assert!(rect.contains_rect(out.visual_bounding_rect().shrink(1.0)));
            }
        }
        s.geometry = Geometry::Stroke {
            lines: vec![(vec![P::new(0.0, 0.0); 1000], false); 100],
            width: 1.0,
        };
        let dense = project(&[&s], rect, false);
        let count: usize = dense
            .iter()
            .map(|shape| match shape {
                egui::Shape::Path(path) => path.points.len(),
                _ => 1,
            })
            .sum();
        assert_eq!(count, MAX_POINTS);
        s.params.hide = Some(true);
        assert!(project(&[&s], rect, true).is_empty());
        assert!(!project(&[&s], rect, false).is_empty());
    }
}
