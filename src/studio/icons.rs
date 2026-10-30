//! Small line icons in the spirit of SF Symbols, painted with egui so they
//! stay crisp at any scale and take the current text color.

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, epaint::CubicBezierShape};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Select,
    /// Illustrator's white arrow: an outline arrow with a point square.
    Direct,
    Hand,
    Pen,
    Text,
    Angle,
    Measure,
    Play,
    Pause,
    /// A filled square: stop.
    Stop,
    /// An embroidery machine: arm, pillar, bed and needle.
    Machine,
    /// [`Icon::Machine`] without the needle, for drawing it moving.
    MachineFrame,
    /// A plug: agents connected over MCP.
    Mcp,
    Share,
    Send,
    ViewOptions,
    Plus,
    Minus,
    ArrowUp,
    ArrowDown,
    Warning,
    Info,
    ChevronDown,
    ChevronRight,
    ChevronLeft,
    Link,
    FlipH,
    FlipV,
    RotateLeft,
    RotateRight,
    Target,
    LeftPanel,
    RightPanel,
    ZoomFit,
    Duplicate,
    /// Three lines: a panel's menu.
    Menu,
    /// An ×, to close.
    Close,
    /// Panels, for the icon strip: stacked layers, a zigzag, a transform
    /// box, swatches, a page.
    Layers,
    Stitch,
    Transform,
    Swatches,
    Page,
    /// » and «: collapse or expand a dock.
    ChevronsRight,
    ChevronsLeft,
    /// Pathfinder: two squares merged, the back one cut, the overlap, the rest.
    PathUnite,
    PathMinusFront,
    PathIntersect,
    PathExclude,
    /// A paintbrush.
    Brush,
    /// Illustrator's Anchor Point tool: a caret over a point.
    AnchorPoint,
    /// Three dots: more.
    More,
    /// A four-point sparkle: the agent.
    Sparkle,
    /// The Pen with a + or a −: Add and Delete Anchor Point.
    PenAdd,
    PenDelete,
    /// A check mark: all clear.
    Check,
    /// A rounded corner.
    RoundCorner,
    /// The Brush group's clean-up tools: a brush over a smooth wave, an
    /// eraser cutting a line, two lines meeting at a point.
    Smooth,
    PathEraser,
    /// A smooth curve through three points: Simplify.
    Simplify,
    Join,
    /// The Pen's modes: a curve between two anchor points (a shape or
    /// line), and needle points joined by stitches (manual stitches).
    PenPath,
    NeedlePoints,
    /// Paragraph alignment: lines of text, left, centered, right.
    AlignLeft,
    AlignCenter,
    AlignRight,
    /// The Shape tools.
    ShapeRect,
    ShapeEllipse,
    ShapePolygon,
    ShapeStar,
    ShapeLine,
}

pub fn paint(p: &Painter, r: Rect, icon: Icon, c: Color32) {
    let w = (r.width().min(r.height()) / 11.0).clamp(1.1, 1.8);
    let s = Stroke::new(w, c);
    // Unit coordinates: (0,0) top-left .. (1,1) bottom-right of a square box.
    let side = r.width().min(r.height());
    let o = r.center() - Vec2::splat(side / 2.0);
    let at = |x: f32, y: f32| Pos2::new(o.x + x * side, o.y + y * side);
    let line = |pts: &[(f32, f32)]| {
        p.add(Shape::line(pts.iter().map(|&(x, y)| at(x, y)).collect(), s));
    };
    let closed = |pts: &[(f32, f32)], fill: Option<Color32>| {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| at(x, y)).collect();
        match fill {
            Some(f) => p.add(Shape::convex_polygon(v, f, Stroke::NONE)),
            None => p.add(Shape::closed_line(v, s)),
        };
    };
    let arc = |cx: f32, cy: f32, rad: f32, a0: f32, a1: f32| {
        let n = 16;
        let pts: Vec<Pos2> = (0..=n)
            .map(|k| {
                let a = a0 + (a1 - a0) * k as f32 / n as f32;
                at(cx + rad * a.cos(), cy + rad * a.sin())
            })
            .collect();
        p.add(Shape::line(pts, s));
    };
    use std::f32::consts::PI;
    match icon {
        Icon::Select => {
            closed(
                &[
                    (0.25, 0.08),
                    (0.25, 0.82),
                    (0.43, 0.65),
                    (0.56, 0.95),
                    (0.68, 0.9),
                    (0.55, 0.6),
                    (0.8, 0.6),
                ],
                None,
            );
        }
        Icon::Direct => {
            closed(
                &[
                    (0.18, 0.06),
                    (0.18, 0.7),
                    (0.33, 0.56),
                    (0.44, 0.8),
                    (0.54, 0.76),
                    (0.44, 0.52),
                    (0.64, 0.52),
                ],
                None,
            );
            p.rect_stroke(
                Rect::from_min_max(at(0.66, 0.68), at(0.92, 0.94)),
                0.0,
                s,
                eframe::egui::StrokeKind::Middle,
            );
        }
        Icon::Hand => {
            // An open hand: four fingers and a thumb.
            for (x, top) in [(0.32, 0.22), (0.45, 0.1), (0.58, 0.14), (0.71, 0.26)] {
                line(&[(x, 0.56), (x, top)]);
            }
            line(&[
                (0.32, 0.56),
                (0.19, 0.44),
                (0.12, 0.5),
                (0.3, 0.86),
                (0.64, 0.93),
                (0.76, 0.72),
                (0.71, 0.26),
            ]);
        }
        Icon::Pen => {
            // Fountain-pen nib.
            closed(
                &[
                    (0.5, 0.05),
                    (0.8, 0.45),
                    (0.62, 0.92),
                    (0.38, 0.92),
                    (0.2, 0.45),
                ],
                None,
            );
            line(&[(0.5, 0.05), (0.5, 0.52)]);
            p.circle_stroke(at(0.5, 0.58), side * 0.06, s);
        }
        Icon::Text => {
            line(&[(0.18, 0.12), (0.82, 0.12)]);
            line(&[(0.18, 0.12), (0.18, 0.24)]);
            line(&[(0.82, 0.12), (0.82, 0.24)]);
            line(&[(0.5, 0.12), (0.5, 0.9)]);
            line(&[(0.36, 0.9), (0.64, 0.9)]);
        }
        Icon::Angle => {
            line(&[(0.1, 0.88), (0.92, 0.88)]);
            line(&[(0.1, 0.88), (0.78, 0.2)]);
            arc(0.1, 0.88, 0.45, -PI / 4.0, 0.0);
        }
        Icon::Measure => {
            closed(
                &[(0.05, 0.32), (0.95, 0.32), (0.95, 0.68), (0.05, 0.68)],
                None,
            );
            for (k, x) in [0.22, 0.38, 0.54, 0.7, 0.86].iter().enumerate() {
                line(&[(*x, 0.32), (*x, if k % 2 == 0 { 0.5 } else { 0.44 })]);
            }
        }
        Icon::AnchorPoint => {
            line(&[(0.14, 0.86), (0.5, 0.2), (0.86, 0.86)]);
            p.rect_stroke(
                Rect::from_center_size(at(0.5, 0.2), Vec2::splat(side * 0.2)),
                0.0,
                s,
                eframe::egui::StrokeKind::Middle,
            );
        }
        Icon::Brush => {
            // Handle up-right, ferrule, then the bristles' tip down-left.
            line(&[(0.92, 0.08), (0.52, 0.48)]);
            closed(
                &[(0.5, 0.42), (0.58, 0.5), (0.46, 0.62), (0.38, 0.54)],
                None,
            );
            closed(
                &[
                    (0.38, 0.54),
                    (0.46, 0.62),
                    (0.34, 0.8),
                    (0.12, 0.9),
                    (0.2, 0.68),
                ],
                Some(c),
            );
        }
        Icon::Check => line(&[(0.16, 0.54), (0.4, 0.78), (0.86, 0.24)]),
        Icon::PenAdd | Icon::PenDelete => {
            // The nib, a little smaller and to the right, the sign at top left.
            let nib = Rect::from_min_size(at(0.3, 0.12), Vec2::splat(side * 0.78));
            paint(p, nib, Icon::Pen, c);
            line(&[(0.04, 0.22), (0.36, 0.22)]);
            if icon == Icon::PenAdd {
                line(&[(0.2, 0.06), (0.2, 0.38)]);
            }
        }
        Icon::Sparkle => {
            // A four-point star (two thin diamonds) and a small one beside it.
            let star = |cx: f32, cy: f32, r: f32| {
                let t = r * 0.24;
                closed(
                    &[(cx, cy - r), (cx + t, cy), (cx, cy + r), (cx - t, cy)],
                    Some(c),
                );
                closed(
                    &[(cx - r, cy), (cx, cy - t), (cx + r, cy), (cx, cy + t)],
                    Some(c),
                );
            };
            star(0.42, 0.56, 0.4);
            star(0.8, 0.2, 0.17);
        }
        Icon::More => {
            for x in [0.2, 0.5, 0.8] {
                p.circle_filled(at(x, 0.5), side * 0.075, c);
            }
        }
        Icon::Smooth => {
            let brush = Rect::from_min_size(at(0.36, 0.0), Vec2::splat(side * 0.64));
            paint(p, brush, Icon::Brush, c);
            let wave: Vec<(f32, f32)> = (0..=12)
                .map(|k| {
                    let t = k as f32 / 12.0;
                    (0.06 + 0.6 * t, 0.84 - 0.1 * (t * 2.0 * PI).sin())
                })
                .collect();
            line(&wave);
        }
        Icon::Simplify => {
            arc(0.5, 0.95, 0.42, PI * 1.08, PI * 1.92);
            for (x, y) in [(0.09, 0.85), (0.5, 0.53), (0.91, 0.85)] {
                p.rect_filled(
                    Rect::from_center_size(at(x, y), Vec2::splat(side * 0.14)),
                    0.0,
                    c,
                );
            }
        }
        Icon::PathEraser => {
            // A line, broken where the eraser crosses it.
            line(&[(0.04, 0.86), (0.22, 0.86)]);
            line(&[(0.62, 0.86), (0.96, 0.86)]);
            closed(
                &[(0.62, 0.08), (0.9, 0.36), (0.56, 0.7), (0.28, 0.42)],
                None,
            );
            closed(
                &[(0.28, 0.42), (0.42, 0.56), (0.34, 0.64), (0.2, 0.5)],
                Some(c),
            );
            line(&[(0.42, 0.56), (0.76, 0.22)]);
        }
        Icon::Join => {
            line(&[(0.08, 0.86), (0.5, 0.3)]);
            line(&[(0.92, 0.86), (0.5, 0.3)]);
            p.circle_filled(at(0.5, 0.3), side * 0.09, c);
            p.circle_stroke(at(0.08, 0.86), side * 0.06, s);
            p.circle_stroke(at(0.92, 0.86), side * 0.06, s);
        }
        Icon::PenPath => {
            // A curve, square anchors at its ends, one with its handle.
            let (p0, c1, c2, p3) = ((0.12, 0.84), (0.2, 0.3), (0.62, 0.86), (0.86, 0.3));
            let curve: Vec<(f32, f32)> = (0..=20)
                .map(|k| {
                    let t = k as f32 / 20.0;
                    let u = 1.0 - t;
                    let b = |a: f32, b: f32, c: f32, d: f32| {
                        u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
                    };
                    (b(p0.0, c1.0, c2.0, p3.0), b(p0.1, c1.1, c2.1, p3.1))
                })
                .collect();
            line(&curve);
            line(&[(0.86, 0.3), (0.62, 0.08)]);
            p.circle_filled(at(0.62, 0.08), side * 0.07, c);
            for (x, y) in [p0, p3] {
                p.rect_filled(
                    Rect::from_center_size(at(x, y), Vec2::splat(side * 0.2)),
                    0.0,
                    c,
                );
            }
        }
        Icon::NeedlePoints => {
            let pts = [
                (0.06, 0.74),
                (0.3, 0.24),
                (0.52, 0.66),
                (0.76, 0.2),
                (0.94, 0.56),
            ];
            p.add(Shape::line(
                pts.iter().map(|&(x, y)| at(x, y)).collect(),
                Stroke::new(s.width * 0.7, c.gamma_multiply(0.75)),
            ));
            for (x, y) in pts {
                p.circle_filled(at(x, y), side * 0.1, c);
            }
        }
        Icon::AlignLeft | Icon::AlignCenter | Icon::AlignRight => {
            let widths = [0.9, 0.55, 0.75, 0.6];
            for (k, w) in widths.iter().enumerate() {
                let y = 0.2 + k as f32 * 0.2;
                let (x0, x1) = match icon {
                    Icon::AlignLeft => (0.05, 0.05 + w),
                    Icon::AlignCenter => (0.5 - w / 2.0, 0.5 + w / 2.0),
                    _ => (0.95 - w, 0.95),
                };
                line(&[(x0, y), (x1, y)]);
            }
        }
        Icon::RoundCorner => {
            line(&[(0.15, 0.88), (0.15, 0.55)]);
            arc(0.55, 0.55, 0.4, PI, 1.5 * PI);
            line(&[(0.55, 0.15), (0.88, 0.15)]);
        }
        Icon::ShapeRect => {
            p.rect_stroke(
                Rect::from_min_max(at(0.14, 0.22), at(0.86, 0.78)),
                side * 0.04,
                s,
                eframe::egui::StrokeKind::Middle,
            );
        }
        Icon::ShapeEllipse => {
            p.circle_stroke(at(0.5, 0.5), side * 0.37, s);
        }
        Icon::ShapePolygon => {
            let v: Vec<(f32, f32)> = (0..6)
                .map(|k| {
                    let a = -PI / 2.0 + k as f32 * PI / 3.0;
                    (0.5 + 0.4 * a.cos(), 0.52 + 0.4 * a.sin())
                })
                .collect();
            closed(&v, None);
        }
        Icon::ShapeStar => {
            let v: Vec<(f32, f32)> = (0..10)
                .map(|k| {
                    let a = -PI / 2.0 + k as f32 * PI / 5.0;
                    let r = if k % 2 == 0 { 0.44 } else { 0.19 };
                    (0.5 + r * a.cos(), 0.54 + r * a.sin())
                })
                .collect();
            closed(&v, None);
        }
        Icon::ShapeLine => line(&[(0.16, 0.84), (0.84, 0.16)]),
        Icon::Play => closed(&[(0.25, 0.12), (0.88, 0.5), (0.25, 0.88)], Some(c)),
        Icon::Machine => {
            // Arm across the top, pillar at the right, bed under, the
            // needle bar hanging from the arm's end.
            closed(
                &[
                    (0.1, 0.2),
                    (0.88, 0.2),
                    (0.88, 0.78),
                    (0.66, 0.78),
                    (0.66, 0.4),
                    (0.26, 0.4),
                    (0.26, 0.5),
                    (0.1, 0.5),
                ],
                None,
            );
            line(&[(0.04, 0.86), (0.96, 0.86)]);
            line(&[(0.18, 0.5), (0.18, 0.7)]);
        }
        Icon::Mcp => {
            // Two prongs, the plug body tapering to the cord.
            line(&[(0.38, 0.08), (0.38, 0.28)]);
            line(&[(0.62, 0.08), (0.62, 0.28)]);
            closed(
                &[
                    (0.24, 0.28),
                    (0.76, 0.28),
                    (0.76, 0.5),
                    (0.6, 0.68),
                    (0.4, 0.68),
                    (0.24, 0.5),
                ],
                None,
            );
            line(&[(0.5, 0.68), (0.5, 0.92)]);
        }
        Icon::MachineFrame => {
            // The machine without its needle (drawn moving by the caller).
            closed(
                &[
                    (0.1, 0.2),
                    (0.88, 0.2),
                    (0.88, 0.78),
                    (0.66, 0.78),
                    (0.66, 0.4),
                    (0.26, 0.4),
                    (0.26, 0.5),
                    (0.1, 0.5),
                ],
                None,
            );
            line(&[(0.04, 0.86), (0.96, 0.86)]);
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_min_max(at(0.2, 0.2), at(0.8, 0.8)), 2.0, c);
        }
        Icon::Pause => {
            p.rect_filled(Rect::from_min_max(at(0.22, 0.14), at(0.4, 0.86)), 1.5, c);
            p.rect_filled(Rect::from_min_max(at(0.6, 0.14), at(0.78, 0.86)), 1.5, c);
        }
        Icon::Share => {
            line(&[
                (0.32, 0.4),
                (0.18, 0.4),
                (0.18, 0.95),
                (0.82, 0.95),
                (0.82, 0.4),
                (0.68, 0.4),
            ]);
            line(&[(0.5, 0.05), (0.5, 0.65)]);
            line(&[(0.32, 0.22), (0.5, 0.05), (0.68, 0.22)]);
        }
        Icon::Send => {
            // Needle and thread loop.
            line(&[(0.15, 0.85), (0.78, 0.22)]);
            p.circle_stroke(at(0.83, 0.17), side * 0.07, s);
            let b = CubicBezierShape::from_points_stroke(
                [at(0.78, 0.22), at(0.35, 0.1), at(0.1, 0.45), at(0.45, 0.6)],
                false,
                Color32::TRANSPARENT,
                Stroke::new(w * 0.8, c),
            );
            p.add(b);
        }
        Icon::ViewOptions => {
            for (y, x) in [(0.22, 0.65), (0.5, 0.3), (0.78, 0.6)] {
                line(&[(0.08, y), (0.92, y)]);
                p.circle_filled(at(x, y), side * 0.1, c);
            }
        }
        Icon::Plus => {
            line(&[(0.5, 0.15), (0.5, 0.85)]);
            line(&[(0.15, 0.5), (0.85, 0.5)]);
        }
        Icon::Minus => line(&[(0.15, 0.5), (0.85, 0.5)]),
        Icon::ArrowUp => {
            line(&[(0.5, 0.88), (0.5, 0.15)]);
            line(&[(0.22, 0.42), (0.5, 0.14), (0.78, 0.42)]);
        }
        Icon::ArrowDown => {
            line(&[(0.5, 0.12), (0.5, 0.85)]);
            line(&[(0.22, 0.58), (0.5, 0.86), (0.78, 0.58)]);
        }
        Icon::Warning => {
            closed(&[(0.5, 0.06), (0.96, 0.9), (0.04, 0.9)], Some(c));
            let ink = if c.r() as u32 + c.g() as u32 + c.b() as u32 > 380 {
                Color32::BLACK
            } else {
                Color32::WHITE
            };
            p.line_segment([at(0.5, 0.36), at(0.5, 0.62)], Stroke::new(w * 1.1, ink));
            p.circle_filled(at(0.5, 0.76), side * 0.05, ink);
        }
        Icon::Info => {
            p.circle_stroke(at(0.5, 0.5), side * 0.44, s);
            line(&[(0.5, 0.45), (0.5, 0.75)]);
            p.circle_filled(at(0.5, 0.3), side * 0.05, c);
        }
        Icon::ChevronDown => line(&[(0.2, 0.38), (0.5, 0.66), (0.8, 0.38)]),
        Icon::ChevronRight => line(&[(0.38, 0.2), (0.66, 0.5), (0.38, 0.8)]),
        Icon::ChevronLeft => line(&[(0.62, 0.2), (0.34, 0.5), (0.62, 0.8)]),
        Icon::Link => {
            p.rect_stroke(
                Rect::from_min_max(at(0.08, 0.36), at(0.58, 0.64)),
                side * 0.14,
                s,
                eframe::egui::StrokeKind::Middle,
            );
            p.rect_stroke(
                Rect::from_min_max(at(0.42, 0.36), at(0.92, 0.64)),
                side * 0.14,
                s,
                eframe::egui::StrokeKind::Middle,
            );
        }
        Icon::FlipH => {
            line(&[(0.5, 0.05), (0.5, 0.95)]);
            closed(&[(0.4, 0.2), (0.4, 0.8), (0.08, 0.8)], None);
            closed(&[(0.6, 0.2), (0.6, 0.8), (0.92, 0.8)], Some(c));
        }
        Icon::FlipV => {
            line(&[(0.05, 0.5), (0.95, 0.5)]);
            closed(&[(0.2, 0.4), (0.8, 0.4), (0.8, 0.08)], None);
            closed(&[(0.2, 0.6), (0.8, 0.6), (0.8, 0.92)], Some(c));
        }
        Icon::RotateLeft => {
            arc(0.5, 0.55, 0.34, 0.3 * PI, -PI);
            line(&[(0.04, 0.44), (0.16, 0.6), (0.28, 0.44)]);
        }
        Icon::RotateRight => {
            arc(0.5, 0.55, 0.34, 0.7 * PI, 2.0 * PI);
            line(&[(0.72, 0.44), (0.84, 0.6), (0.96, 0.44)]);
        }
        Icon::Target => {
            p.circle_stroke(at(0.5, 0.5), side * 0.3, s);
            line(&[(0.5, 0.0), (0.5, 0.3)]);
            line(&[(0.5, 0.7), (0.5, 1.0)]);
            line(&[(0.0, 0.5), (0.3, 0.5)]);
            line(&[(0.7, 0.5), (1.0, 0.5)]);
        }
        Icon::LeftPanel | Icon::RightPanel => {
            p.rect_stroke(
                Rect::from_min_max(at(0.04, 0.16), at(0.96, 0.84)),
                side * 0.12,
                s,
                eframe::egui::StrokeKind::Middle,
            );
            let x = if icon == Icon::LeftPanel { 0.38 } else { 0.62 };
            line(&[(x, 0.16), (x, 0.84)]);
        }
        Icon::ZoomFit => {
            for (a, b, c2) in [
                ((0.08, 0.32), (0.08, 0.08), (0.32, 0.08)),
                ((0.68, 0.08), (0.92, 0.08), (0.92, 0.32)),
                ((0.92, 0.68), (0.92, 0.92), (0.68, 0.92)),
                ((0.32, 0.92), (0.08, 0.92), (0.08, 0.68)),
            ] {
                line(&[a, b, c2]);
            }
        }
        Icon::PathUnite | Icon::PathMinusFront | Icon::PathIntersect | Icon::PathExclude => {
            let rect = |x0: f32, y0: f32, x1: f32, y1: f32| {
                p.rect_filled(Rect::from_min_max(at(x0, y0), at(x1, y1)), 0.0, c);
            };
            let outline = |x0: f32, y0: f32, x1: f32, y1: f32| {
                p.rect_stroke(
                    Rect::from_min_max(at(x0, y0), at(x1, y1)),
                    0.0,
                    Stroke::new(1.0, c.gamma_multiply(0.6)),
                    eframe::egui::StrokeKind::Middle,
                );
            };
            // Back square (0.1..0.62), front square (0.38..0.9).
            let back = |rect: &dyn Fn(f32, f32, f32, f32)| {
                rect(0.1, 0.1, 0.62, 0.38);
                rect(0.1, 0.38, 0.38, 0.62);
            };
            let front = |rect: &dyn Fn(f32, f32, f32, f32)| {
                rect(0.62, 0.38, 0.9, 0.9);
                rect(0.38, 0.62, 0.62, 0.9);
            };
            match icon {
                Icon::PathUnite => {
                    back(&rect);
                    front(&rect);
                    rect(0.38, 0.38, 0.62, 0.62);
                }
                Icon::PathMinusFront => {
                    back(&rect);
                    outline(0.38, 0.38, 0.9, 0.9);
                }
                Icon::PathIntersect => {
                    outline(0.1, 0.1, 0.62, 0.62);
                    outline(0.38, 0.38, 0.9, 0.9);
                    rect(0.38, 0.38, 0.62, 0.62);
                }
                _ => {
                    back(&rect);
                    front(&rect);
                }
            }
        }
        Icon::Layers => {
            closed(&[(0.5, 0.15), (0.9, 0.35), (0.5, 0.55), (0.1, 0.35)], None);
            line(&[(0.1, 0.52), (0.5, 0.72), (0.9, 0.52)]);
            line(&[(0.1, 0.68), (0.5, 0.88), (0.9, 0.68)]);
        }
        Icon::Stitch => {
            line(&[
                (0.08, 0.7),
                (0.22, 0.3),
                (0.36, 0.7),
                (0.5, 0.3),
                (0.64, 0.7),
                (0.78, 0.3),
                (0.92, 0.7),
            ]);
        }
        Icon::Transform => {
            p.rect_stroke(
                Rect::from_min_max(at(0.2, 0.2), at(0.8, 0.8)),
                0.0,
                s,
                eframe::egui::StrokeKind::Middle,
            );
            for (x, y) in [(0.2, 0.2), (0.8, 0.2), (0.2, 0.8), (0.8, 0.8)] {
                p.rect_filled(
                    Rect::from_center_size(at(x, y), Vec2::splat(side * 0.16)),
                    0.0,
                    c,
                );
            }
        }
        Icon::Swatches => {
            for (x, y) in [(0.1, 0.1), (0.55, 0.1), (0.1, 0.55), (0.55, 0.55)] {
                p.rect_filled(
                    Rect::from_min_size(at(x, y), Vec2::splat(side * 0.35)),
                    side * 0.06,
                    c,
                );
            }
        }
        Icon::Page => {
            closed(
                &[
                    (0.22, 0.08),
                    (0.62, 0.08),
                    (0.8, 0.26),
                    (0.8, 0.92),
                    (0.22, 0.92),
                ],
                None,
            );
            line(&[(0.62, 0.08), (0.62, 0.26), (0.8, 0.26)]);
            line(&[(0.34, 0.5), (0.68, 0.5)]);
            line(&[(0.34, 0.68), (0.68, 0.68)]);
        }
        Icon::ChevronsRight => {
            line(&[(0.2, 0.25), (0.45, 0.5), (0.2, 0.75)]);
            line(&[(0.52, 0.25), (0.77, 0.5), (0.52, 0.75)]);
        }
        Icon::ChevronsLeft => {
            line(&[(0.8, 0.25), (0.55, 0.5), (0.8, 0.75)]);
            line(&[(0.48, 0.25), (0.23, 0.5), (0.48, 0.75)]);
        }
        Icon::Close => {
            line(&[(0.25, 0.25), (0.75, 0.75)]);
            line(&[(0.75, 0.25), (0.25, 0.75)]);
        }
        Icon::Menu => {
            for y in [0.25, 0.5, 0.75] {
                line(&[(0.15, y), (0.85, y)]);
            }
        }
        Icon::Duplicate => {
            p.rect_stroke(
                Rect::from_min_max(at(0.08, 0.3), at(0.7, 0.92)),
                2.0,
                s,
                eframe::egui::StrokeKind::Middle,
            );
            line(&[
                (0.3, 0.3),
                (0.3, 0.08),
                (0.92, 0.08),
                (0.92, 0.7),
                (0.7, 0.7),
            ]);
        }
    }
}
