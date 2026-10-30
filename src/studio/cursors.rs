//! Canvas cursors drawn by the studio, after VectorCraft's `cursors.rs`
//! (MIT OR Apache-2.0): black ink with a white halo, so they read on any
//! thread or fabric. macOS sizes cursor bitmaps in points and ignores
//! custom ones in places, so like VectorCraft we hide the system cursor over
//! the canvas and paint the tool's own on top, at the pointer.
//!
//! - Pen: a nib, with what a click would do (* new, + add, − delete, o close,
//!   / continue, ^ convert, & join).
//! - Shape, Measure and Fill Angle tools: a precise crosshair (with the shape).
//! - Direct Selection: the hollow arrow (a square beside it over a point).
//! - Rotate handles: a curved double arrow facing out from the corner.

use crate::app::Studio;
use crate::icons::Icon;
use eframe::egui::{self, Color32, Pos2, Shape, Stroke, Vec2};
use std::cell::RefCell;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Glyph {
    Pen(&'static str),
    Cross(Option<Icon>),
    /// Direct Selection's arrow; `true` over a point or handle.
    Hollow(bool),
    /// Rotate, the arrow facing `angle` (radians, screen y down).
    Rotate(f32),
    /// The Brush: a circle as wide as the line it paints (radius, points).
    Brush(f32),
    /// The Anchor Point tool: a caret, its tip on the hotspot.
    Convert,
    /// Reshaping a segment: the caret with a small curve beside it.
    Reshape,
}

#[derive(Default)]
pub struct CursorState {
    want: RefCell<Option<Glyph>>,
}

const INK: Color32 = Color32::BLACK;
const HALO: Color32 = Color32::WHITE;

/// Draw a polyline twice: a wide white halo, then the black line.
fn ink_line(p: &egui::Painter, pts: Vec<Pos2>, w: f32) {
    p.add(Shape::line(pts.clone(), Stroke::new(w + 2.0, HALO)));
    p.add(Shape::line(pts, Stroke::new(w, INK)));
}

fn ink_closed(p: &egui::Painter, pts: Vec<Pos2>, fill: Color32) {
    p.add(Shape::closed_line(pts.clone(), Stroke::new(3.0, HALO)));
    p.add(Shape::convex_polygon(pts, fill, Stroke::new(1.0, INK)));
}

fn rotate(v: Vec2, a: f32) -> Vec2 {
    let (s, c) = a.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Paint `g` with its hotspot at `at`.
pub fn paint(p: &egui::Painter, at: Pos2, g: Glyph) {
    match g {
        Glyph::Pen(hint) => {
            // A nib pointing down-left, its tip on the hotspot.
            let a = std::f32::consts::FRAC_PI_4;
            let pt = |x: f32, y: f32| at + rotate(Vec2::new(x, y), a);
            let nib = vec![
                pt(0.0, 0.0),
                pt(-4.5, -9.0),
                pt(-4.5, -15.0),
                pt(4.5, -15.0),
                pt(4.5, -9.0),
            ];
            ink_closed(p, nib, Color32::WHITE);
            ink_line(p, vec![pt(0.0, -2.0), pt(0.0, -8.0)], 1.0);
            p.circle_filled(pt(0.0, -9.5), 1.4, INK);
            if !hint.is_empty() {
                let text_at = at + Vec2::new(13.0, 2.0);
                for d in [
                    Vec2::new(-1.0, 0.0),
                    Vec2::new(1.0, 0.0),
                    Vec2::new(0.0, -1.0),
                    Vec2::new(0.0, 1.0),
                ] {
                    p.text(
                        text_at + d,
                        egui::Align2::LEFT_TOP,
                        hint,
                        egui::FontId::monospace(12.0),
                        HALO,
                    );
                }
                p.text(
                    text_at,
                    egui::Align2::LEFT_TOP,
                    hint,
                    egui::FontId::monospace(12.0),
                    INK,
                );
            }
        }
        Glyph::Cross(icon) => {
            for (a, b) in [
                (Vec2::new(-9.0, 0.0), Vec2::new(-2.5, 0.0)),
                (Vec2::new(2.5, 0.0), Vec2::new(9.0, 0.0)),
                (Vec2::new(0.0, -9.0), Vec2::new(0.0, -2.5)),
                (Vec2::new(0.0, 2.5), Vec2::new(0.0, 9.0)),
            ] {
                ink_line(p, vec![at + a, at + b], 1.0);
            }
            if let Some(icon) = icon {
                let r = egui::Rect::from_min_size(at + Vec2::new(7.0, 7.0), Vec2::splat(11.0));
                p.rect_filled(r.expand(1.5), 2.0, Color32::from_white_alpha(220));
                crate::icons::paint(p, r, icon, INK);
            }
        }
        Glyph::Hollow(on_point) => {
            let arrow: Vec<Pos2> = [
                (0.0, 0.0),
                (0.0, 16.0),
                (4.0, 12.5),
                (7.0, 18.5),
                (9.5, 17.5),
                (6.5, 11.5),
                (11.5, 11.5),
            ]
            .iter()
            .map(|&(x, y)| at + Vec2::new(x, y))
            .collect();
            // Not convex: outline only, filled white by a halo pass.
            p.add(Shape::closed_line(arrow.clone(), Stroke::new(3.5, HALO)));
            p.add(Shape::convex_polygon(
                [arrow[0], arrow[1], arrow[2], arrow[5], arrow[6]].to_vec(),
                HALO,
                Stroke::NONE,
            ));
            p.add(Shape::convex_polygon(
                [arrow[2], arrow[3], arrow[4], arrow[5]].to_vec(),
                HALO,
                Stroke::NONE,
            ));
            p.add(Shape::closed_line(arrow, Stroke::new(1.0, INK)));
            if on_point {
                let r = egui::Rect::from_min_size(at + Vec2::new(12.0, 13.0), Vec2::splat(5.0));
                p.rect_filled(r.expand(1.0), 0.0, HALO);
                p.rect_stroke(r, 0.0, Stroke::new(1.0, INK), egui::StrokeKind::Inside);
            }
        }
        Glyph::Convert => {
            ink_line(
                p,
                vec![at + Vec2::new(-6.0, 12.0), at, at + Vec2::new(6.0, 12.0)],
                1.4,
            );
        }
        Glyph::Reshape => {
            ink_line(
                p,
                vec![at + Vec2::new(-6.0, 12.0), at, at + Vec2::new(6.0, 12.0)],
                1.4,
            );
            let curve: Vec<Pos2> = (0..=10)
                .map(|k| {
                    let t = k as f32 / 10.0;
                    at + Vec2::new(9.0 + 9.0 * t, 14.0 - 8.0 * (t * std::f32::consts::PI).sin())
                })
                .collect();
            ink_line(p, curve, 1.2);
        }
        Glyph::Brush(r) => {
            p.circle_stroke(at, r + 1.0, Stroke::new(1.0, HALO));
            p.circle_stroke(at, r, Stroke::new(1.0, INK));
            p.circle_filled(at, 1.8, HALO);
            p.circle_filled(at, 1.0, INK);
        }
        Glyph::Rotate(angle) => {
            // An arc around the hotspot's outside, arrowheads at both ends.
            let n = 12;
            let (a0, a1) = (-0.9f32, 0.9f32);
            let r = 9.0;
            let pts: Vec<Pos2> = (0..=n)
                .map(|k| {
                    let t = a0 + (a1 - a0) * k as f32 / n as f32;
                    at + rotate(
                        Vec2::new(t.cos(), t.sin()) * r - Vec2::new(r - 3.0, 0.0),
                        angle,
                    )
                })
                .collect();
            ink_line(p, pts.clone(), 1.4);
            for (end, dir) in [(pts[0], -1.0f32), (pts[n], 1.0f32)] {
                let t = if dir < 0.0 { a0 } else { a1 };
                let tangent = rotate(Vec2::new(-t.sin(), t.cos()) * dir, angle);
                let side = Vec2::new(-tangent.y, tangent.x);
                let head = vec![end + tangent * 4.5, end + side * 3.5, end - side * 3.5];
                ink_closed(p, head, INK);
            }
        }
    }
}

impl Studio {
    /// Ask for a canvas cursor this frame (the last asked wins).
    pub fn want_cursor(&self, g: Glyph) {
        *self.cursor.want.borrow_mut() = Some(g);
    }

    /// Paint this frame's canvas cursor at the pointer, hiding the system
    /// one. Only over the canvas itself (not its floating bars).
    pub fn paint_cursor(&self, ctx: &egui::Context, canvas: &egui::Response) {
        let Some(g) = self.cursor.want.borrow_mut().take() else {
            return;
        };
        let Some(at) = ctx.pointer_hover_pos() else {
            return;
        };
        let over = canvas.hovered() || canvas.dragged() || canvas.is_pointer_button_down_on();
        if !over || !canvas.rect.contains(at) {
            return;
        }
        ctx.set_cursor_icon(egui::CursorIcon::None);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("canvas-cursor"),
        ));
        paint(&painter, at, g);
    }
}
