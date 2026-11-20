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
    /// The Rotate tool, as VectorCraft draws it (Lucide's rotate-ccw):
    /// most of a circle, round to an arrowhead at the top left.
    RotateTool,
    Target,
    LeftPanel,
    RightPanel,
    ZoomFit,
    Duplicate,
    /// Three lines: a panel's menu.
    Menu,
    /// An ×, to close.
    Close,
    /// An eye: shown (the Layers list's visibility column).
    Eye,
    /// A padlock: locked.
    Lock,
    /// Settings.
    Gear,
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
    /// A shape inside a heavy edge: Add Outline.
    AddOutline,
    /// An empty outline with a line struck through it: no outline
    /// (Remove Outline), as "none" is drawn for a stroke in Illustrator.
    NoOutline,
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
    /// A loop of rope with its tail: the Lasso.
    Lasso,
    /// The Pen's nib over a curve through round points: Curvature.
    Curvature,
    /// A pipette: the Eyedropper.
    Eyedropper,
    /// A blade on a handle: the Knife.
    Knife,
    /// Two rings overlapping, where they overlap filled: the Shape Builder.
    ShapeBuilder,
    /// A brush over the blob it has painted: the Blob Brush.
    BlobBrush,
    /// A speech bubble with one pointed corner: comment on the canvas for
    /// the agent (its marker there is the same shape).
    Comment,
    /// An eye struck through: hide.
    EyeOff,
    /// A frame split down the middle: side by side.
    Compare,
    /// A bin: delete.
    Trash,
    /// A frame with its left half filled: before; its right: after.
    BeforeHalf,
    AfterHalf,
}

/// An icon's line weight in a box `side` points across: a whole number of
/// screen pixels, so that every line of every icon is as heavy as the next
/// (a line a fraction of a pixel wide is drawn soft, and looks thinner).
pub fn line_width(side: f32, pixels_per_point: f32) -> f32 {
    ((side / 11.0).clamp(1.1, 1.8) * pixels_per_point)
        .round()
        .max(1.0)
        / pixels_per_point
}

/// A coordinate (in points) moved to where a level or upright line `width`
/// wide is drawn crisp: an odd number of pixels wide, centered on a pixel;
/// even, between two.
fn on_pixel(v: f32, width: f32, pixels_per_point: f32) -> f32 {
    if (width * pixels_per_point).round() as i64 % 2 == 1 {
        ((v * pixels_per_point - 0.5).round() + 0.5) / pixels_per_point
    } else {
        (v * pixels_per_point).round() / pixels_per_point
    }
}

/// Level and upright lines of a path moved onto the pixel grid, where a
/// line `width` wide is drawn crisp, as egui does for the rectangles icons
/// are also made of (it leaves paths where they are, and a line that falls
/// between pixels looks thinner than a rectangle's side next to it). Slanted
/// and curved parts stay where they are.
fn on_pixels(pts: &mut [Pos2], closed: bool, width: f32, pixels_per_point: f32) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    let snap = |v: f32| on_pixel(v, width, pixels_per_point);
    let was = pts.to_vec();
    for i in 0..n {
        let before = if i > 0 {
            Some(was[i - 1])
        } else {
            closed.then(|| was[n - 1])
        };
        let after = if i + 1 < n {
            Some(was[i + 1])
        } else {
            closed.then(|| was[0])
        };
        for q in [before, after].into_iter().flatten() {
            if q == was[i] {
                continue;
            }
            if (q.x - was[i].x).abs() < 1e-3 {
                pts[i].x = snap(was[i].x);
            }
            if (q.y - was[i].y).abs() < 1e-3 {
                pts[i].y = snap(was[i].y);
            }
        }
    }
}

pub fn paint(p: &Painter, r: Rect, icon: Icon, c: Color32) {
    // Unit coordinates: (0,0) top-left .. (1,1) bottom-right of a square box.
    let side = r.width().min(r.height());
    let ppp = p.pixels_per_point();
    let w = line_width(side, ppp);
    let s = Stroke::new(w, c);
    let o = r.center() - Vec2::splat(side / 2.0);
    let at = |x: f32, y: f32| Pos2::new(o.x + x * side, o.y + y * side);
    let line = |pts: &[(f32, f32)]| {
        let mut v: Vec<Pos2> = pts.iter().map(|&(x, y)| at(x, y)).collect();
        on_pixels(&mut v, false, w, ppp);
        p.add(Shape::line(v, s));
    };
    let closed = |pts: &[(f32, f32)], fill: Option<Color32>| {
        let mut v: Vec<Pos2> = pts.iter().map(|&(x, y)| at(x, y)).collect();
        match fill {
            Some(f) => p.add(Shape::convex_polygon(v, f, Stroke::NONE)),
            None => {
                on_pixels(&mut v, true, w, ppp);
                p.add(Shape::closed_line(v, s))
            }
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
            // An open hand in one outline: four round-tipped fingers side
            // by side (the middle one tallest), the palm rounding down to
            // the wrist, and a thumb out to the upper left. Short lines
            // part the fingers. It fills the box as the other tools do,
            // its lines' outer edges about 0.06 in from each side.
            //
            // The fingers' sides and the palm's foot are put on the pixel
            // grid before the curves are fitted to them (moving them after,
            // as `on_pixels` does, would leave each fingertip a sliver off
            // its sides, and the join a spike).
            let grid_x = |x: f32| (on_pixel(at(x, 0.0).x, w, ppp) - o.x) / side;
            let grid_y = |y: f32| (on_pixel(at(0.0, y).y, w, ppp) - o.y) / side;
            let edges = [0.29, 0.43, 0.57, 0.71, 0.85].map(grid_x);
            let foot = grid_y(0.9);
            let fingers: [(f32, f32, f32); 4] = std::array::from_fn(|i| {
                let tip = [0.3, 0.18, 0.215, 0.33][i];
                (
                    (edges[i] + edges[i + 1]) / 2.0,
                    tip,
                    (edges[i + 1] - edges[i]) / 2.0,
                )
            });
            let arc_pts =
                |cx: f32, cy: f32, r: f32, a0: f32, a1: f32, out: &mut Vec<(f32, f32)>| {
                    let n = 10;
                    for k in 0..=n {
                        let a = a0 + (a1 - a0) * k as f32 / n as f32;
                        out.push((cx + r * a.cos(), cy + r * a.sin()));
                    }
                };
            let cubic = |p: [(f32, f32); 4], out: &mut Vec<(f32, f32)>| {
                let n = 12;
                for k in 1..=n {
                    let t = k as f32 / n as f32;
                    let u = 1.0 - t;
                    let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
                    out.push((
                        (0..4).map(|i| w[i] * p[i].0).sum(),
                        (0..4).map(|i| w[i] * p[i].1).sum(),
                    ));
                }
            };
            // The thumb's axis, pointing up and left; its sides toward the
            // fingers (inner) and away (outer); its tip.
            let len = (0.42f32 * 0.42 + 0.91 * 0.91).sqrt();
            let d = (-0.42 / len, -0.91 / len);
            let inner = (-d.1, d.0);
            let outer = (d.1, -d.0);
            let (tip, r) = ((0.19, 0.49), 0.07);
            // Where the thumb's inner side meets the index finger.
            let index_left = edges[0];
            let from = (tip.0 + r * inner.0, tip.1 + r * inner.1);
            let join = (index_left, from.1 - d.1 * (index_left - from.0) / -d.0);
            // Where a short finger meets a taller one the outline would run
            // down one side and straight back up the other (a turn egui draws
            // as a spike), so it is drawn as strokes that end on the fingers'
            // shared sides instead. A finger's stroke starts where its left
            // side shows above the finger before it, rounds its tip, and runs
            // down its right side, which parts it from the next one.
            let separator = 0.45;
            let finger = |i: usize, out: &mut Vec<(f32, f32)>| {
                let (x, y, r) = fingers[i];
                if i > 0 && fingers[i - 1].1 > y {
                    out.push((edges[i], fingers[i - 1].1));
                }
                arc_pts(x, y, r, PI, 2.0 * PI, out);
            };
            let stroke = |pts: Vec<(f32, f32)>| {
                p.add(Shape::line(pts.iter().map(|&(x, y)| at(x, y)).collect(), s));
            };
            for i in 1..3 {
                let mut pts = Vec::new();
                finger(i, &mut pts);
                pts.push((edges[i + 1], separator));
                stroke(pts);
            }
            // The rest in one stroke: the little finger, down the palm's
            // side, round its foot, up the thumb, and over the index finger.
            let mut pts = Vec::new();
            finger(3, &mut pts);
            let right = edges[4];
            pts.push((right, 0.6));
            cubic(
                [(right, 0.6), (right, 0.8), (0.71, foot), (0.56, foot)],
                &mut pts,
            );
            pts.push((0.45, foot));
            let edge = (tip.0 + r * outer.0, tip.1 + r * outer.1);
            let wrist = (edge.0 - d.0 * 0.22, edge.1 - d.1 * 0.22);
            cubic(
                [
                    (0.45, foot),
                    (0.37, foot),
                    (wrist.0 - d.0 * 0.08, wrist.1 - d.1 * 0.08),
                    wrist,
                ],
                &mut pts,
            );
            let a = outer.1.atan2(outer.0);
            arc_pts(tip.0, tip.1, r, a, a + PI, &mut pts);
            pts.push(join);
            finger(0, &mut pts);
            pts.push((edges[1], separator));
            stroke(pts);
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
            // One line: up, round the corner, across.
            let mut v = vec![(0.15, 0.88)];
            v.extend((0..=16).map(|k| {
                let a = PI + 0.5 * PI * k as f32 / 16.0;
                (0.55 + 0.4 * a.cos(), 0.55 + 0.4 * a.sin())
            }));
            v.push((0.88, 0.15));
            line(&v);
        }
        Icon::AddOutline => {
            // The shape, and the outline round it (at the weight of every
            // other icon's lines).
            p.rect_filled(
                Rect::from_min_max(at(0.25, 0.25), at(0.75, 0.75)),
                side * 0.09,
                c,
            );
            p.rect_stroke(
                Rect::from_min_max(at(0.08, 0.08), at(0.92, 0.92)),
                side * 0.2,
                s,
                eframe::egui::StrokeKind::Middle,
            );
        }
        Icon::NoOutline => {
            p.rect_stroke(
                Rect::from_min_max(at(0.1, 0.1), at(0.9, 0.9)),
                side * 0.2,
                s,
                eframe::egui::StrokeKind::Middle,
            );
            // (Struck through from corner to corner, a little past each.)
            line(&[(0.0, 1.0), (1.0, 0.0)]);
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
        Icon::Lasso => {
            // The loop, a little flattened, and the rope's end hanging
            // from where it's tied.
            let n = 28;
            let ring: Vec<Pos2> = (0..n)
                .map(|k| {
                    let a = k as f32 / n as f32 * 2.0 * PI;
                    at(0.5 + 0.42 * a.cos(), 0.4 + 0.28 * a.sin())
                })
                .collect();
            p.add(Shape::closed_line(ring, s));
            p.circle_filled(at(0.3, 0.66), side * 0.07, c);
            line(&[(0.3, 0.66), (0.24, 0.8), (0.34, 0.94)]);
        }
        Icon::Curvature => {
            // The nib, smaller and to the right; under it a curve through
            // three round points.
            let nib = Rect::from_min_size(at(0.34, 0.0), Vec2::splat(side * 0.66));
            paint(p, nib, Icon::Pen, c);
            let curve: Vec<Pos2> = (0..=16)
                .map(|k| {
                    let t = k as f32 / 16.0;
                    at(0.08 + 0.84 * t, 0.84 - 0.1 * (t * 2.0 * PI).sin())
                })
                .collect();
            p.add(Shape::line(curve, s));
            for x in [0.08, 0.5, 0.92] {
                p.circle_filled(at(x, 0.84), side * 0.075, c);
            }
        }
        Icon::Eyedropper => {
            // From the top right: the bulb, the collar across, the glass
            // down to its tip at the bottom left.
            closed(
                &[(0.7, 0.06), (0.94, 0.3), (0.82, 0.42), (0.58, 0.18)],
                Some(c),
            );
            line(&[(0.46, 0.2), (0.8, 0.54)]);
            line(&[
                (0.56, 0.3),
                (0.2, 0.66),
                (0.14, 0.86),
                (0.34, 0.8),
                (0.7, 0.44),
            ]);
            line(&[(0.14, 0.86), (0.06, 0.94)]);
        }
        Icon::Knife => {
            // The handle at the top right, the blade down to its point.
            closed(
                &[(0.72, 0.06), (0.94, 0.28), (0.78, 0.44), (0.56, 0.22)],
                Some(c),
            );
            closed(&[(0.56, 0.22), (0.78, 0.44), (0.1, 0.9)], None);
        }
        Icon::ShapeBuilder => {
            // Two rings, and the lens where they overlap filled in.
            let ring = |cx: f32| {
                let n = 24;
                let v: Vec<Pos2> = (0..n)
                    .map(|k| {
                        let a = k as f32 / n as f32 * 2.0 * PI;
                        at(cx + 0.3 * a.cos(), 0.5 + 0.3 * a.sin())
                    })
                    .collect();
                p.add(Shape::closed_line(v, s));
            };
            ring(0.34);
            ring(0.66);
            // (The rings cross where cos a = 0.16 / 0.3.)
            let half = (0.16f32 / 0.3).acos();
            let n = 8;
            let lens: Vec<Pos2> = (0..=n)
                .map(|k| {
                    let a = -half + 2.0 * half * k as f32 / n as f32;
                    at(0.34 + 0.3 * a.cos(), 0.5 + 0.3 * a.sin())
                })
                .chain((0..=n).map(|k| {
                    let a = PI - half + 2.0 * half * k as f32 / n as f32;
                    at(0.66 + 0.3 * a.cos(), 0.5 + 0.3 * a.sin())
                }))
                .collect();
            p.add(Shape::convex_polygon(lens, c, Stroke::NONE));
        }
        Icon::BlobBrush => {
            // The brush, smaller and to the right; the blob it's painting
            // under its tip.
            let brush = Rect::from_min_size(at(0.3, 0.0), Vec2::splat(side * 0.7));
            paint(p, brush, Icon::Brush, c);
            let n = 20;
            let blob: Vec<Pos2> = (0..n)
                .map(|k| {
                    let a = k as f32 / n as f32 * 2.0 * PI;
                    at(0.36 + 0.3 * a.cos(), 0.78 + 0.16 * a.sin())
                })
                .collect();
            p.add(Shape::convex_polygon(blob, c, Stroke::NONE));
        }
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
        // (Share and Send sit between the same top and bottom as the
        // icons beside them in the toolbar, 0.12 to 0.88: Share's tray
        // hung lower than their feet, and the needle rode higher.)
        Icon::Share => {
            line(&[
                (0.32, 0.42),
                (0.18, 0.42),
                (0.18, 0.88),
                (0.82, 0.88),
                (0.82, 0.42),
                (0.68, 0.42),
            ]);
            line(&[(0.5, 0.1), (0.5, 0.64)]);
            line(&[(0.33, 0.26), (0.5, 0.1), (0.67, 0.26)]);
        }
        Icon::Send => {
            // Needle and thread loop.
            line(&[(0.15, 0.88), (0.78, 0.25)]);
            p.circle_stroke(at(0.83, 0.2), side * 0.07, s);
            let b = CubicBezierShape::from_points_stroke(
                [
                    at(0.78, 0.25),
                    at(0.35, 0.13),
                    at(0.1, 0.48),
                    at(0.45, 0.63),
                ],
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
        Icon::RotateTool => {
            // From the left, round the bottom, the right and the top, to
            // the corner of an arrowhead at the top left.
            let n = 28;
            let mut pts: Vec<Pos2> = (0..=n)
                .map(|k| {
                    let a = PI - 1.75 * PI * k as f32 / n as f32;
                    at(0.5 + 0.375 * a.cos(), 0.5 + 0.375 * a.sin())
                })
                .collect();
            pts.push(at(0.125, 0.333));
            p.add(Shape::line(pts, s));
            line(&[(0.125, 0.125), (0.125, 0.333), (0.333, 0.333)]);
        }
        Icon::Gear => {
            // Six broad teeth round a ring, and the hole for the shaft:
            // few enough, and with room enough round the hole, to read as
            // a gear at the size of a button (eight narrow ones crowded
            // it). Its teeth stop short of the box's edge, as the other
            // icons' lines do.
            let (teeth, outer, inner) = (6, 0.46, 0.33);
            let step = std::f32::consts::TAU / teeth as f32;
            let mut rim = Vec::with_capacity(teeth * 4);
            for k in 0..teeth {
                // (A tooth straight up, so it sits level in its box.)
                let a = k as f32 * step - std::f32::consts::FRAC_PI_2;
                for (da, rad) in [(-0.3, inner), (-0.19, outer), (0.19, outer), (0.3, inner)] {
                    let b = a + da * step;
                    rim.push(at(0.5 + rad * b.cos(), 0.5 + rad * b.sin()));
                }
            }
            p.add(Shape::closed_line(rim, s));
            p.circle_stroke(at(0.5, 0.5), side * 0.11, s);
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
        Icon::Eye => {
            // Two lids meeting at the corners, and the pupil.
            let lid = |up: f32| -> Vec<Pos2> {
                (0..=12)
                    .map(|k| {
                        let t = k as f32 / 12.0;
                        at(0.04 + 0.92 * t, 0.5 + up * 0.34 * (t * PI).sin())
                    })
                    .collect()
            };
            p.add(Shape::line(lid(-1.0), s));
            p.add(Shape::line(lid(1.0), s));
            p.circle_filled(at(0.5, 0.5), side * 0.13, c);
        }
        Icon::Lock => {
            p.rect_filled(
                Rect::from_min_max(at(0.14, 0.46), at(0.86, 0.94)),
                side * 0.1,
                c,
            );
            // The shackle.
            let arch: Vec<Pos2> = std::iter::once(at(0.3, 0.5))
                .chain((0..=10).map(|k| {
                    let a = PI + k as f32 / 10.0 * PI;
                    at(0.5 + 0.2 * a.cos(), 0.3 + 0.2 * a.sin())
                }))
                .chain(std::iter::once(at(0.7, 0.5)))
                .collect();
            p.add(Shape::line(arch, s));
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
        Icon::Comment => {
            // The canvas's note marker as an outline: round but for the
            // bottom-left corner, which comes to a point on the spot.
            let r = (side * 0.4) as u8;
            p.rect_stroke(
                Rect::from_min_max(at(0.08, 0.08), at(0.92, 0.92)),
                eframe::egui::CornerRadius {
                    nw: r,
                    ne: r,
                    sw: 0,
                    se: r,
                },
                Stroke::new(COMMENT_STROKE, c),
                eframe::egui::StrokeKind::Middle,
            );
        }
        Icon::EyeOff => {
            paint(p, r, Icon::Eye, c);
            line(&[(0.14, 0.12), (0.86, 0.88)]);
        }
        Icon::Compare => {
            p.rect_stroke(
                Rect::from_min_max(at(0.08, 0.16), at(0.92, 0.84)),
                side * 0.14,
                s,
                eframe::egui::StrokeKind::Middle,
            );
            line(&[(0.5, 0.06), (0.5, 0.94)]);
        }
        Icon::BeforeHalf | Icon::AfterHalf => {
            let frame = Rect::from_min_max(at(0.08, 0.16), at(0.92, 0.84));
            let x = if icon == Icon::BeforeHalf { 0.08 } else { 0.5 };
            p.rect_filled(
                Rect::from_min_max(at(x, 0.16), at(x + 0.42, 0.84)),
                side * 0.1,
                c,
            );
            p.rect_stroke(frame, side * 0.14, s, eframe::egui::StrokeKind::Middle);
        }
        Icon::Trash => {
            line(&[(0.1, 0.24), (0.9, 0.24)]);
            line(&[(0.37, 0.24), (0.37, 0.1), (0.63, 0.1), (0.63, 0.24)]);
            line(&[(0.2, 0.24), (0.26, 0.92), (0.74, 0.92), (0.8, 0.24)]);
            line(&[(0.42, 0.42), (0.42, 0.74)]);
            line(&[(0.58, 0.42), (0.58, 0.74)]);
        }
    }
}

/// The comment icon's line, and the prompt's Send arrow beside it: one
/// weight, so the two look like a pair.
pub const COMMENT_STROKE: f32 = 1.5;

/// The dots a floating bar is dragged by: two columns of three (`upright`)
/// or three columns of two, centered on `at`.
pub fn grip_dots(p: &Painter, at: Pos2, upright: bool, c: Color32) {
    let (cols, rows) = if upright { (2, 3) } else { (3, 2) };
    let gap = 5.0;
    for i in 0..cols {
        for j in 0..rows {
            let o = Vec2::new(
                (i as f32 - (cols - 1) as f32 / 2.0) * gap,
                (j as f32 - (rows - 1) as f32 / 2.0) * gap,
            );
            p.circle_filled(at + o, 1.4, c);
        }
    }
}
