//! The Shape tools, like Illustrator's: Rectangle (M), Ellipse (L), Polygon,
//! Star and Line (\). Drag to draw from a corner; Option draws from the
//! center; Shift makes a square or circle (or keeps a polygon upright, a line
//! at 45°). While dragging a polygon or star, ↑/↓ add or remove sides or
//! points. Click without dragging to type an exact size. Right-click the tool
//! (or use the bar at the top of the canvas) to pick the shape.

use crate::app::Studio;
use crate::icons::Icon;
use crate::theme::{icon_button, space};
use eframe::egui::{self, Color32, Key, Pos2, Rect, Stroke, Vec2};
use zigzag::bezier::{self, BezPath};
use zigzag::geom::P;
use zigzag::params::Kind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ShapeKind {
    #[default]
    Rect,
    Ellipse,
    Polygon,
    Star,
    Line,
}

pub const KINDS: [ShapeKind; 5] = [
    ShapeKind::Rect,
    ShapeKind::Ellipse,
    ShapeKind::Polygon,
    ShapeKind::Star,
    ShapeKind::Line,
];

impl ShapeKind {
    pub fn name(self) -> &'static str {
        match self {
            ShapeKind::Rect => "Rectangle",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Polygon => "Polygon",
            ShapeKind::Star => "Star",
            ShapeKind::Line => "Line",
        }
    }
    pub fn icon(self) -> Icon {
        match self {
            ShapeKind::Rect => Icon::ShapeRect,
            ShapeKind::Ellipse => Icon::ShapeEllipse,
            ShapeKind::Polygon => Icon::ShapePolygon,
            ShapeKind::Star => Icon::ShapeStar,
            ShapeKind::Line => Icon::ShapeLine,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            ShapeKind::Rect => "M",
            ShapeKind::Ellipse => "L",
            ShapeKind::Line => "\\",
            _ => "",
        }
    }
}

/// Exact sizes typed in after a click (mm): width/height, or radius.
#[derive(Clone, Copy, Debug)]
pub struct ShapeDialog {
    pub kind: ShapeKind,
    pub center: P,
    pub a: f64,
    pub b: f64,
}

pub struct ShapeState {
    pub kind: ShapeKind,
    /// Rectangle corner radius (mm).
    pub corner_mm: f64,
    pub sides: usize,
    pub points: usize,
    /// A star's notches, as a share of its radius.
    pub inner: f64,
    drag: Option<P>,
    /// Shift and Option during the drag.
    mods: (bool, bool),
    pub dialog: Option<ShapeDialog>,
    /// The last typed size, offered next time (mm).
    last: (f64, f64),
}

impl Default for ShapeState {
    fn default() -> Self {
        ShapeState {
            kind: ShapeKind::Rect,
            corner_mm: 0.0,
            sides: 6,
            points: 5,
            inner: 0.5,
            drag: None,
            mods: (false, false),
            dialog: None,
            last: (20.0, 20.0),
        }
    }
}

impl ShapeState {
    /// The shape for a drag from `a` to `b` (design units).
    pub fn build(&self, a: P, b: P, shift: bool, alt: bool) -> BezPath {
        let up = -std::f64::consts::FRAC_PI_2;
        match self.kind {
            ShapeKind::Rect | ShapeKind::Ellipse => {
                let mut d = b - a;
                if shift {
                    let s = d.x.abs().max(d.y.abs());
                    d = P::new(s * d.x.signum(), s * d.y.signum());
                }
                let (p0, p1) = if alt { (a - d, a + d) } else { (a, a + d) };
                if self.kind == ShapeKind::Rect {
                    bezier::rect(p0, p1, self.corner_mm * 10.0)
                } else {
                    bezier::ellipse(p0, p1)
                }
            }
            ShapeKind::Polygon | ShapeKind::Star => {
                let r = (b - a).len();
                let rot = if shift {
                    up
                } else {
                    (b - a).y.atan2((b - a).x)
                };
                if self.kind == ShapeKind::Polygon {
                    bezier::polygon(a, r, self.sides, rot)
                } else {
                    bezier::star(a, r, r * self.inner, self.points, rot)
                }
            }
            ShapeKind::Line => {
                let b = if shift { bezier::constrain_45(a, b) } else { b };
                if alt {
                    bezier::line(a * 2.0 - b, b)
                } else {
                    bezier::line(a, b)
                }
            }
        }
    }

    /// The shape from a typed size, centered on `c`.
    pub fn build_exact(&self, d: &ShapeDialog) -> BezPath {
        let (a, b) = (d.a * 10.0, d.b * 10.0);
        let c = d.center;
        let up = -std::f64::consts::FRAC_PI_2;
        match d.kind {
            ShapeKind::Rect => bezier::rect(
                c - P::new(a / 2.0, b / 2.0),
                c + P::new(a / 2.0, b / 2.0),
                self.corner_mm * 10.0,
            ),
            ShapeKind::Ellipse => {
                bezier::ellipse(c - P::new(a / 2.0, b / 2.0), c + P::new(a / 2.0, b / 2.0))
            }
            ShapeKind::Polygon => bezier::polygon(c, a, self.sides, up),
            ShapeKind::Star => bezier::star(c, a, a * self.inner, self.points, up),
            ShapeKind::Line => {
                let t = b.to_radians();
                let half = P::new(t.cos(), -t.sin()) * (a / 2.0);
                bezier::line(c - half, c + half)
            }
        }
    }
}

fn describe(kind: ShapeKind, path: &BezPath, s: &ShapeState) -> String {
    let b = bezier::bounds(path);
    match kind {
        ShapeKind::Rect | ShapeKind::Ellipse => crate::units::fmt_size(b.width(), b.height()),
        ShapeKind::Polygon => format!(
            "{} across · {} sides",
            crate::units::fmt(b.width()),
            s.sides
        ),
        ShapeKind::Star => format!(
            "{} across · {} points",
            crate::units::fmt(b.width()),
            s.points
        ),
        ShapeKind::Line => {
            let a = &path[0].anchors;
            let d = a[1].p - a[0].p;
            format!(
                "{} · {:.0}°",
                crate::units::fmt(d.len()),
                (-d.y.atan2(d.x).to_degrees()).round() + 0.0
            )
        }
    }
}

const ACCENT: Color32 = Color32::from_rgb(40, 140, 255);

impl Studio {
    /// Canvas input and drawing for the Shape tools.
    pub fn shape_canvas(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        self.want_cursor(crate::cursors::Glyph::Cross(Some(self.shapes.kind.icon())));
        let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
        self.snap_targets(None, false);
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(at) = ui.input(|i| i.pointer.press_origin())
        {
            self.shapes.drag =
                Some(self.snap_point(self.view.to_design(rect, at), None, false, &[]));
        }
        // Before pressing: where the shape would start.
        if self.shapes.drag.is_none()
            && let Some(h) = resp.hover_pos()
        {
            self.snap_point(self.view.to_design(rect, h), None, false, &[]);
        }
        if let Some(a) = self.shapes.drag
            && let Some(pos) = resp.interact_pointer_pos().or(resp.hover_pos())
        {
            // Shift and Option as they were while the button was down: they
            // are often let go in the same instant as the mouse.
            let (shift, alt) = if resp.drag_stopped() {
                self.shapes.mods
            } else {
                self.shapes.mods = (shift, alt);
                (shift, alt)
            };
            // Lines, polygons and stars leave their start at an angle: rays
            // from it guide them; boxes just line up.
            let from = matches!(
                self.shapes.kind,
                ShapeKind::Line | ShapeKind::Polygon | ShapeKind::Star
            )
            .then_some(a);
            let b = self.snap_point(self.view.to_design(rect, pos), from, false, &[a]);
            let path = self.shapes.build(a, b, shift, alt);
            let to = |p: P| self.view.to_screen(rect, p);
            for sp in &path {
                for (i, j) in sp.segments() {
                    let pts: Vec<Pos2> = bezier::sample(&sp.cubic(i, j), 24)
                        .into_iter()
                        .map(to)
                        .collect();
                    painter.add(egui::Shape::line(pts, Stroke::new(1.5, ACCENT)));
                }
            }
            crate::view_tools::label(
                painter,
                pos + Vec2::new(14.0, 14.0),
                describe(self.shapes.kind, &path, &self.shapes),
            );
            if resp.drag_stopped() {
                self.shapes.drag = None;
                let big = (to(b) - to(a)).length() > 4.0;
                if big {
                    self.add_shape(&path);
                }
            }
            ui.ctx().request_repaint();
        }
        if resp.clicked()
            && let Some(at) = resp.interact_pointer_pos()
        {
            // A click: type the size.
            let (a, b) = match self.shapes.kind {
                ShapeKind::Polygon | ShapeKind::Star => (self.shapes.last.0 / 2.0, 0.0),
                ShapeKind::Line => (self.shapes.last.0, 0.0),
                _ => self.shapes.last,
            };
            self.shapes.dialog = Some(ShapeDialog {
                kind: self.shapes.kind,
                center: self.snap_point(self.view.to_design(rect, at), None, false, &[]),
                a,
                b,
            });
        }
        self.hud(
            painter,
            rect,
            "Drag to draw · Option: from the center · Shift: square, circle, upright or 45° · arrow keys while dragging: sides or points · click to type a size",
        );
        self.shape_options(ui.ctx(), ui.make_persistent_id("shape-options"), rect);
    }

    /// Up/Down while dragging a polygon or star: more or fewer sides or
    /// points. Runs before the nudge shortcuts, which would otherwise move
    /// the selection.
    pub fn shape_keys(&mut self, ctx: &egui::Context) -> bool {
        if self.tool != crate::app::Tool::Shape || self.shapes.drag.is_none() {
            return false;
        }
        let step = ctx.input_mut(|i| {
            let mut n = 0i32;
            for m in [
                egui::Modifiers::NONE,
                egui::Modifiers::SHIFT,
                egui::Modifiers::ALT,
            ] {
                n += i.count_and_consume_key(m, Key::ArrowUp) as i32;
                n -= i.count_and_consume_key(m, Key::ArrowDown) as i32;
            }
            n
        });
        let s = &mut self.shapes;
        match s.kind {
            ShapeKind::Polygon => s.sides = (s.sides as i32 + step).clamp(3, 24) as usize,
            ShapeKind::Star => s.points = (s.points as i32 + step).clamp(3, 24) as usize,
            _ => {}
        }
        step != 0
    }

    /// Add a drawn shape as a new object and select it.
    pub fn add_shape(&mut self, path: &BezPath) {
        let closed = path.first().is_some_and(|sp| sp.closed);
        let kind = if closed { Kind::Fill } else { Kind::Run };
        let color = self.new_shape_color();
        let what = describe(self.shapes.kind, path, &self.shapes);
        let name = self.shapes.kind.name();
        let mut idx = 0;
        match self.structural(|d| {
            idx = d.add_drawing_path(path, color, kind)?;
            Ok(())
        }) {
            Ok(()) => {
                self.set_selection(&[idx]);
                let name = name.to_lowercase();
                let a = if name.starts_with(['a', 'e', 'i', 'o', 'u']) {
                    "an"
                } else {
                    "a"
                };
                self.status = format!("Drew {a} {name} ({what}).");
            }
            Err(e) => self.status = format!("Couldn't add the shape: {e:#}"),
        }
    }

    /// The bar at the top of the canvas: which shape, and its settings.
    fn shape_options(&mut self, ctx: &egui::Context, id: egui::Id, rect: Rect) {
        egui::Area::new(id)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(rect.center_top() + Vec2::new(0.0, space::M))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .corner_radius(9)
                    .inner_margin(egui::Margin::same(space::XS as i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = space::XS;
                            for k in KINDS {
                                let tip = if k.key().is_empty() {
                                    k.name().to_string()
                                } else {
                                    format!("{} ({})", k.name(), k.key())
                                };
                                if icon_button(ui, k.icon(), k.name(), self.shapes.kind == k, 26.0)
                                    .on_hover_text(tip)
                                    .clicked()
                                {
                                    self.shapes.kind = k;
                                }
                            }
                            ui.add_space(space::S);
                            let s = &mut self.shapes;
                            let field = |ui: &mut egui::Ui, label: &str, w: egui::DragValue| {
                                ui.label(
                                    egui::RichText::new(label)
                                        .size(12.0)
                                        .color(crate::theme::pal_ui(ui).secondary),
                                );
                                ui.add(w);
                            };
                            match s.kind {
                                ShapeKind::Rect => field(
                                    ui,
                                    "Corners",
                                    crate::units::drag_mm(&mut s.corner_mm, 0.0..=50.0, 0.1),
                                ),
                                ShapeKind::Polygon => field(
                                    ui,
                                    "Sides",
                                    egui::DragValue::new(&mut s.sides).range(3..=24),
                                ),
                                ShapeKind::Star => {
                                    field(
                                        ui,
                                        "Points",
                                        egui::DragValue::new(&mut s.points).range(3..=24),
                                    );
                                    let mut pct = s.inner * 100.0;
                                    field(
                                        ui,
                                        "Inner",
                                        egui::DragValue::new(&mut pct)
                                            .range(10.0..=95.0)
                                            .speed(1.0)
                                            .fixed_decimals(0)
                                            .suffix("%"),
                                    );
                                    s.inner = pct / 100.0;
                                }
                                _ => {}
                            }
                        });
                    });
            });
    }

    /// The window for typing a shape's exact size.
    pub fn shape_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.shapes.dialog else {
            return;
        };
        let mut open = true;
        let (mut ok, mut cancel) = (false, false);
        egui::Window::new(d.kind.name())
            .title_frame(crate::theme::title_frame(ctx))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::new(0.0, -60.0))
            .show(ctx, |ui| {
                let num = |ui: &mut egui::Ui, v: &mut f64, label: &str, suffix: &str| {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [72.0, 20.0],
                            egui::Label::new(
                                egui::RichText::new(label)
                                    .color(crate::theme::pal_ui(ui).secondary),
                            ),
                        );
                        let r = ui.add(if suffix == "°" {
                            egui::DragValue::new(v)
                                .range(0.0..=400.0)
                                .speed(0.1)
                                .fixed_decimals(1)
                                .suffix(suffix)
                        } else {
                            crate::units::drag_mm(v, 0.0..=400.0, 0.1)
                        });
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::SpinButton, true, label)
                        });
                    });
                };
                match d.kind {
                    ShapeKind::Rect | ShapeKind::Ellipse => {
                        num(ui, &mut d.a, "Width", " mm");
                        num(ui, &mut d.b, "Height", " mm");
                    }
                    ShapeKind::Polygon | ShapeKind::Star => num(ui, &mut d.a, "Radius", " mm"),
                    ShapeKind::Line => {
                        num(ui, &mut d.a, "Length", " mm");
                        num(ui, &mut d.b, "Angle", "°");
                    }
                }
                ui.add_space(space::S);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 24.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let accent = crate::theme::pal_ui(ui).accent;
                        ok = ui
                            .add(
                                egui::Button::new(egui::RichText::new("OK").color(Color32::WHITE))
                                    .fill(accent),
                            )
                            .clicked()
                            || ui.input(|i| i.key_pressed(Key::Enter));
                        cancel = ui.button("Cancel").clicked()
                            || ui.input(|i| i.key_pressed(Key::Escape));
                    },
                );
            });
        self.shapes.dialog = Some(d);
        if ok {
            self.shapes.dialog = None;
            if d.a > 0.05 && (d.b > 0.05 || !matches!(d.kind, ShapeKind::Rect | ShapeKind::Ellipse))
            {
                if matches!(d.kind, ShapeKind::Rect | ShapeKind::Ellipse) {
                    self.shapes.last = (d.a, d.b);
                } else if matches!(d.kind, ShapeKind::Polygon | ShapeKind::Star) {
                    self.shapes.last = (d.a * 2.0, d.a * 2.0);
                }
                let kind = self.shapes.kind;
                self.shapes.kind = d.kind;
                let path = self.shapes.build_exact(&d);
                self.add_shape(&path);
                self.shapes.kind = kind;
            }
        } else if cancel || !open {
            self.shapes.dialog = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Tool, shape_bounds};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    fn studio() -> Harness<'static, Studio> {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        h
    }

    fn drag(h: &mut Harness<'_, Studio>, from: Pos2, to: Pos2) {
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        for k in 1..=4 {
            h.hover_at(from + (to - from) * (k as f32 / 4.0));
            h.run_steps(1);
        }
        h.drop_at(to);
        h.run_steps(3);
    }

    #[test]
    fn m_draws_rectangles_and_l_ellipses() {
        let mut h = studio();
        // Exact sizes: no smart guides pulling the corners.
        h.state_mut().view_tools.smart_guides = false;
        h.key_press(Key::M);
        h.run_steps(1);
        assert_eq!(h.state().tool, Tool::Shape);
        assert_eq!(h.state().shapes.kind, ShapeKind::Rect);
        let c = h.state().canvas_rect.center();
        let s = h.state().view.scale; // px per mm
        drag(&mut h, c, c + Vec2::new(30.0 * s, 20.0 * s));
        assert_eq!(h.state().doc.shapes.len(), 1);
        let b = shape_bounds(&h.state().doc.shapes[0]);
        assert!(
            (b.width() - 300.0).abs() < 2.0 && (b.height() - 200.0).abs() < 2.0,
            "{b:?}"
        );
        assert_eq!(h.state().doc.shapes[0].params.kind, Some(Kind::Fill));

        // Ellipse from the center with Option, made round with Shift.
        h.key_press(Key::L);
        h.run_steps(1);
        assert_eq!(h.state().shapes.kind, ShapeKind::Ellipse);
        h.event(egui::Event::ModifiersChanged(
            egui::Modifiers::ALT | egui::Modifiers::SHIFT,
        ));
        drag(&mut h, c, c + Vec2::new(10.0 * s, 4.0 * s));
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        h.run_steps(1);
        assert_eq!(h.state().doc.shapes.len(), 2);
        let b = shape_bounds(&h.state().doc.shapes[1]);
        assert!(
            (b.width() - 200.0).abs() < 2.0 && (b.height() - 200.0).abs() < 2.0,
            "{b:?}"
        );
        let mid = h.state().view.to_design(h.state().canvas_rect, c);
        assert!(
            (b.center() - mid).len() < 1.0,
            "centered where the drag began"
        );
    }

    #[test]
    fn a_click_asks_for_the_size() {
        let mut h = studio();
        h.state_mut().set_tool(Tool::Shape);
        h.state_mut().shapes.kind = ShapeKind::Star;
        h.run_steps(1);
        let c = h.state().canvas_rect.center();
        h.hover_at(c);
        h.run_steps(1);
        h.drag_at(c);
        h.run_steps(1);
        h.drop_at(c);
        h.run_steps(2);
        assert!(h.state().shapes.dialog.is_some());
        h.get_by_label("OK").click();
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), 1);
        let b = shape_bounds(&h.state().doc.shapes[0]);
        assert!(b.height() > 150.0, "a 20 mm star: {b:?}");
    }

    #[test]
    fn arrows_while_dragging_add_points_and_release_keeps_shift() {
        let mut h = studio();
        h.state_mut().set_tool(Tool::Shape);
        h.state_mut().shapes.kind = ShapeKind::Star;
        h.run_steps(1);
        let c = h.state().canvas_rect.center();
        let s = h.state().view.scale;
        let mid = h.state().view.to_design(h.state().canvas_rect, c);
        h.hover_at(c);
        h.run_steps(1);
        h.drag_at(c);
        h.run_steps(1);
        h.hover_at(c + Vec2::new(10.0 * s, 3.0 * s));
        h.run_steps(1);
        h.key_press(Key::ArrowUp);
        h.run_steps(1);
        h.key_press(Key::ArrowUp);
        h.run_steps(1);
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
        h.hover_at(c + Vec2::new(10.0 * s, 4.0 * s));
        h.run_steps(1);
        // Shift let go in the same instant as the mouse.
        h.drop_at(c + Vec2::new(10.0 * s, 4.0 * s));
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        h.run_steps(3);
        assert_eq!(h.state().shapes.points, 7);
        assert_eq!(h.state().doc.shapes.len(), 1, "the star, nothing nudged");
        let b = shape_bounds(&h.state().doc.shapes[0]);
        // Upright: the top point straight above the center.
        assert!((mid.y - b.min.y - (10.77f64 * 10.0)).abs() < 3.0, "{b:?}");
        assert!(((b.min.x + b.max.x) / 2.0 - mid.x).abs() < 1.0, "{b:?}");
    }
}
