//! The Brush (B): paint a line freehand and it comes out as a smooth path,
//! sewn as a satin column at the width you choose, or as a running stitch.
//! The hand's wobble is taken out (Smoothness), sharp turns stay sharp, and
//! ending the stroke where it began closes it. After VectorCraft's
//! Paintbrush (see `bezier::fit_stroke`).

use crate::app::Studio;
use crate::theme::{pal_ui, space};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use zigzag::bezier;
use zigzag::geom::P;
use zigzag::params::Kind;

pub struct BrushState {
    /// Satin or Run.
    pub kind: Kind,
    /// Satin column width (mm).
    pub width_mm: f64,
    /// 0–100: how much of the hand's wobble is smoothed away.
    pub smoothness: f64,
    /// 0–100: how far the Smooth tool may move a path to smooth it.
    pub smooth_tool: f64,
    /// The stroke being painted (design units).
    samples: Vec<P>,
}

impl Default for BrushState {
    fn default() -> Self {
        BrushState {
            kind: Kind::Satin,
            width_mm: 2.0,
            smoothness: 50.0,
            smooth_tool: 50.0,
            samples: Vec::new(),
        }
    }
}

impl BrushState {
    /// The fitting tolerance (screen points) and relaxation passes for the
    /// smoothness setting.
    fn fidelity(&self) -> (f64, usize) {
        let s = self.smoothness.clamp(0.0, 100.0) / 100.0;
        (0.6 + 7.4 * s, (s * 4.0).round() as usize)
    }
    /// How wide the line sews (mm): the satin width, or a run's thin line.
    fn sewn_width(&self) -> f64 {
        if self.kind == Kind::Satin {
            self.width_mm
        } else {
            0.5
        }
    }
}

impl Studio {
    /// Canvas input and drawing for the Brush.
    pub fn brush_canvas(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        let px = self.view.scale as f64 / 10.0; // screen points per design unit
        let radius = (self.brush.sewn_width() * 10.0 * px / 2.0) as f32;
        self.want_cursor(crate::cursors::Glyph::Brush(radius.max(1.5)));
        let color = self.new_shape_color();
        let ink = Color32::from_rgb(color[0], color[1], color[2]);
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(at) = ui.input(|i| i.pointer.press_origin())
        {
            self.brush.samples = vec![self.view.to_design(rect, at)];
        }
        if !self.brush.samples.is_empty()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let q = self.view.to_design(rect, pos);
            if self
                .brush
                .samples
                .last()
                .is_none_or(|l| l.dist(q) * px >= 0.75)
            {
                self.brush.samples.push(q);
            }
            ui.ctx().request_repaint();
        }
        // The stroke so far, at the width it will sew.
        if self.brush.samples.len() > 1 {
            let pts: Vec<Pos2> = self
                .brush
                .samples
                .iter()
                .map(|&p| self.view.to_screen(rect, p))
                .collect();
            painter.add(egui::Shape::line(
                pts.clone(),
                Stroke::new((radius * 2.0).max(1.5), ink.gamma_multiply(0.55)),
            ));
            painter.add(egui::Shape::line(
                pts,
                Stroke::new(1.0, Color32::from_rgb(40, 140, 255)),
            ));
        }
        if resp.drag_stopped() {
            let samples = std::mem::take(&mut self.brush.samples);
            self.finish_brush(&samples, px);
        }
        self.hud(
            painter,
            rect,
            "Paint a line: it's smoothed and sewn as satin (or a run) · end where you began to close it · Smoothness and width in the bar above",
        );
        self.brush_options(ui.ctx(), ui.make_persistent_id("brush-options"), rect);
    }

    /// Fit the painted samples and add the line.
    fn finish_brush(&mut self, samples: &[P], px: f64) {
        if samples.len() < 2 {
            return;
        }
        let length: f64 = samples.windows(2).map(|w| w[0].dist(w[1])).sum();
        if length * px < 4.0 {
            return; // a click, not a stroke
        }
        let (tol_pt, passes) = self.brush.fidelity();
        let tol = tol_pt / px;
        // Ending on the start (on screen) closes it.
        let closed = samples.len() > 8
            && samples[0].dist(samples[samples.len() - 1]) * px < 10.0
            && length * px > 60.0;
        let Some(sp) = bezier::fit_stroke(samples, tol, passes, closed) else {
            return;
        };
        let n = sp.anchors.len();
        let path = vec![sp];
        let (kind, width) = (self.brush.kind, self.brush.sewn_width());
        let color = self.new_shape_color();
        let mut idx = 0;
        match self.structural(|d| {
            idx = d.add_drawing_stroke(&path, color, kind, width)?;
            Ok(())
        }) {
            Ok(()) => {
                self.set_selection(&[idx]);
                self.status = match kind {
                    Kind::Satin => format!(
                        "Painted a {}satin line {width:.1} mm wide ({n} points).",
                        if closed { "closed " } else { "" }
                    ),
                    _ => format!(
                        "Painted a {}running-stitch line ({n} points).",
                        if closed { "closed " } else { "" }
                    ),
                };
            }
            Err(e) => self.status = format!("Couldn't add the line: {e:#}"),
        }
    }

    /// The bar at the top of the canvas: how the line sews, its width, and
    /// how smooth.
    fn brush_options(&mut self, ctx: &egui::Context, id: egui::Id, rect: Rect) {
        egui::Area::new(id)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(rect.center_top() + Vec2::new(0.0, space::M))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .corner_radius(9)
                    .inner_margin(egui::Margin::symmetric(space::S as i8, space::XS as i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = space::S;
                            let p = pal_ui(ui);
                            let label = |ui: &mut egui::Ui, t: &str| {
                                ui.label(egui::RichText::new(t).size(12.0).color(p.secondary));
                            };
                            crate::theme::segmented(
                                ui,
                                "brush-kind",
                                &mut self.brush.kind,
                                &[(Kind::Satin, "Satin"), (Kind::Run, "Run")],
                            );
                            if self.brush.kind == Kind::Satin {
                                label(ui, "Width");
                                ui.add(
                                    egui::DragValue::new(&mut self.brush.width_mm)
                                        .range(0.5..=8.0)
                                        .speed(0.05)
                                        .fixed_decimals(1)
                                        .suffix(" mm"),
                                )
                                .on_hover_text("Satin column width. Keep it under 7 mm.");
                            }
                            label(ui, "Smoothness");
                            ui.add(
                                egui::Slider::new(&mut self.brush.smoothness, 0.0..=100.0)
                                    .show_value(false),
                            )
                            .on_hover_text("How much of the hand's wobble is taken out");
                        });
                    });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Tool;
    use egui::Key;
    use egui_kittest::Harness;

    #[test]
    fn b_paints_a_smooth_satin_line() {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        h.key_press(Key::B);
        h.run_steps(1);
        assert_eq!(h.state().tool, Tool::Brush);
        let c = h.state().canvas_rect.center();
        // A wobbly wave, painted.
        let at = |k: usize| {
            let t = k as f32 / 60.0;
            c + Vec2::new(
                -200.0 + t * 400.0,
                (t * std::f32::consts::TAU).sin() * 60.0
                    + if k.is_multiple_of(2) { 1.5 } else { -1.5 },
            )
        };
        h.hover_at(at(0));
        h.run_steps(1);
        h.drag_at(at(0));
        h.run_steps(1);
        for k in 1..=60 {
            h.hover_at(at(k));
            h.run_steps(1);
        }
        h.drop_at(at(60));
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), 1);
        let s = &h.state().doc.shapes[0];
        assert_eq!(s.params.kind, Some(Kind::Satin));
        let zigzag::svg::Geometry::Stroke { width, .. } = &s.geometry else {
            panic!("a stroke: {:?}", s.geometry);
        };
        assert!((width - 20.0).abs() < 0.5, "2 mm wide: {width}");
        let path = h.state().doc.edit_path(0).unwrap();
        assert!(!path[0].closed);
        assert!(
            path[0].anchors.len() <= 8,
            "smoothed to a few points: {}",
            path[0].anchors.len()
        );
        assert_eq!(h.state().undo.len(), 1);
        assert!(h.state().design.stitch_count() > 50, "it sews");
    }
}
