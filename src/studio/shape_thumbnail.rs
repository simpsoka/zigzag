//! Painter-only artwork outlines. Fill rings stay outlines so holes are never filled in.
use eframe::egui::{self, Color32, Painter, Pos2, Rect, Stroke};
use zigzag::{
    geom::P,
    svg::{Geometry, Shape},
};

const MAX_POINTS: usize = 2048;
const PATH_POINTS: usize = 128;

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
fn project(shapes: &[&Shape], rect: Rect, skip_hidden: bool) -> Vec<egui::Shape> {
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
    for shape in visible() {
        let color = Color32::from_rgb(shape.color[0], shape.color[1], shape.color[2]);
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
        assert_eq!(out.len(), 2);
        let egui::Shape::Path(a) = &out[0] else {
            panic!()
        };
        let egui::Shape::Path(b) = &out[1] else {
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
        assert_eq!(project(&[&s[0], &s[0]], rect, true).len(), 2);
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
