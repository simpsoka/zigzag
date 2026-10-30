//! The contextual task bar, after Illustrator's (and VectorCraft's
//! `canvas.rs` task bar): a small floating bar under the selection with the
//! next things you're likely to do to it — how it's stitched, its thread,
//! editing its points, rounding its corners, uniting several, duplicating.
//! View › Contextual Task Bar (or its ••• menu) hides it.

use crate::app::{Studio, Tool};
use crate::commands::Action;
use crate::icons::Icon;
use crate::theme::{icon_button, pal_ui, space};
use eframe::egui::{self, Color32, Rect, Sense, Stroke, Vec2};
use zigzag::boolean::PathOp;

/// Height of the bar's buttons.
const BUTTON: f32 = 28.0;

impl Studio {
    /// Whether the bar shows: the setting is on, the Selection tool is
    /// active with something selected, and nothing is being dragged.
    pub fn task_bar_shown(&self) -> bool {
        self.dock.task_bar
            && self.tool == Tool::Select
            && !self.sel_indices().is_empty()
            && self.drag.is_none()
            && !self.playing
            && !self.panning
    }

    /// Draw the task bar under the selection (above it when there's no
    /// room), kept inside the canvas and clear of the prompt bar.
    pub fn task_bar(&mut self, ctx: &egui::Context, id: egui::Id, canvas: Rect) {
        if !self.task_bar_shown() {
            return;
        }
        let b = self.selection_bounds();
        if b.is_empty() {
            return;
        }
        let (lo, hi) = (
            self.view.to_screen(canvas, b.min),
            self.view.to_screen(canvas, b.max),
        );
        let sel = Rect::from_two_pos(lo, hi);
        // Its size last frame (an estimate the first time).
        let size = egui::AreaState::load(ctx, id)
            .and_then(|s| s.size)
            .unwrap_or(Vec2::new(260.0, BUTTON + 8.0));
        let bottom_limit = canvas.bottom() - self.hud_lift - space::M - size.y;
        let below = sel.bottom() + 24.0;
        let y = if below <= bottom_limit {
            below
        } else {
            (sel.top() - 24.0 - size.y).max(canvas.top() + space::M)
        };
        let x = (sel.center().x - size.x / 2.0).clamp(
            canvas.left() + space::M,
            (canvas.right() - space::M - size.x).max(canvas.left() + space::M),
        );
        let idx = self.sel_indices();
        let single = (idx.len() == 1).then(|| idx[0]);
        let mut act: Option<Action> = None;
        let mut picker_at: Option<egui::Pos2> = None;
        let mut text_edit: Option<(usize, zigzag::document::TextSpec)> = None;
        egui::Area::new(id)
            .order(egui::Order::Middle)
            .fixed_pos(egui::pos2(x, y))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .corner_radius(9)
                    .inner_margin(egui::Margin::same(space::XS as i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = space::XS;
                            let p = pal_ui(ui);
                            // How it's stitched: the common type, a menu of the others.
                            let kinds: Vec<_> = idx
                                .iter()
                                .map(|&i| self.doc.shapes[i].params.kind)
                                .collect();
                            let kind = kinds
                                .first()
                                .copied()
                                .flatten()
                                .filter(|k| kinds.iter().all(|x| *x == Some(*k)));
                            // The types every selected object can take.
                            let mut allowed =
                                crate::widgets::allowed_kinds(&self.doc.shapes[idx[0]]);
                            for &i in &idx[1..] {
                                let a = crate::widgets::allowed_kinds(&self.doc.shapes[i]);
                                allowed.retain(|k| a.contains(k));
                            }
                            let label = kind.map_or("Mixed", crate::widgets::kind_name);
                            let (r, resp) =
                                ui.allocate_exact_size(Vec2::new(96.0, BUTTON), Sense::click());
                            resp.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Stitch type")
                            });
                            let open = egui::Popup::is_id_open(
                                ui.ctx(),
                                egui::Popup::default_response_id(&resp),
                            );
                            if resp.hovered() || open {
                                ui.painter().rect_filled(r, 6.0, p.hover);
                            }
                            if let Some(k) = kind {
                                crate::widgets::draw_kind_icon(
                                    ui.painter(),
                                    Rect::from_center_size(
                                        egui::pos2(r.left() + 14.0, r.center().y),
                                        Vec2::splat(16.0),
                                    ),
                                    k,
                                    p.text,
                                );
                            }
                            ui.painter().text(
                                egui::pos2(r.left() + 28.0, r.center().y),
                                egui::Align2::LEFT_CENTER,
                                label,
                                egui::FontId::proportional(12.0),
                                p.text,
                            );
                            crate::icons::paint(
                                ui.painter(),
                                Rect::from_center_size(
                                    egui::pos2(r.right() - 10.0, r.center().y),
                                    Vec2::splat(8.0),
                                ),
                                Icon::ChevronDown,
                                p.secondary,
                            );
                            egui::Popup::menu(&resp).show(|ui| {
                                ui.set_min_width(200.0);
                                ui.spacing_mut().item_spacing.y = 0.0;
                                for k in allowed {
                                    if crate::flyout::menu_row(
                                        ui,
                                        crate::flyout::RowIcon::Kind(k),
                                        crate::widgets::kind_name(k),
                                        "",
                                        kind == Some(k),
                                        true,
                                    )
                                    .clicked()
                                    {
                                        act = Some(Action::Kind(k));
                                        ui.close();
                                    }
                                }
                            });
                            // The thread: click for the picker.
                            let color = self.doc.shapes[idx[0]].color;
                            let (r, resp) =
                                ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click());
                            resp.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Button, true, "Thread color")
                            });
                            if resp.hovered() {
                                ui.painter().rect_filled(r, 6.0, p.hover);
                            }
                            let sw = Rect::from_center_size(r.center(), Vec2::splat(16.0));
                            ui.painter().rect_filled(
                                sw,
                                4.0,
                                Color32::from_rgb(color[0], color[1], color[2]),
                            );
                            ui.painter().rect_stroke(
                                sw,
                                4.0,
                                Stroke::new(1.0, p.separator),
                                egui::StrokeKind::Inside,
                            );
                            if resp
                                .on_hover_text(format!(
                                    "Thread: {}",
                                    crate::threads::nearest(self.view_tools.brand, color).1
                                ))
                                .clicked()
                            {
                                picker_at = Some(r.left_top());
                            }
                            // Text: its font, size, style and alignment right here.
                            if let Some(i) = single
                                && let Some(mut spec) = self.doc.lettering(i)
                            {
                                separator(ui);
                                let mut changed = false;
                                let fonts = self.fonts();
                                let mut filter = std::mem::take(&mut self.lettering.font_filter);
                                egui::ComboBox::from_id_salt("taskbar-font")
                                    .selected_text(egui::RichText::new(&spec.font).size(12.0))
                                    .width(130.0)
                                    .height(320.0)
                                    .show_ui(ui, |ui| {
                                        ui.add(
                                            egui::TextEdit::singleline(&mut filter)
                                                .hint_text("Search fonts"),
                                        );
                                        let f = filter.to_lowercase();
                                        for name in fonts.iter().filter(|n| {
                                            f.is_empty() || n.to_lowercase().contains(&f)
                                        }) {
                                            if ui
                                                .selectable_value(
                                                    &mut spec.font,
                                                    name.clone(),
                                                    name,
                                                )
                                                .changed()
                                            {
                                                changed = true;
                                            }
                                        }
                                    })
                                    .response
                                    .on_hover_text("Font");
                                self.lettering.font_filter = filter;
                                let r = ui.add(crate::units::drag_mm(
                                    &mut spec.size_mm,
                                    3.0..=40.0,
                                    0.5,
                                ));
                                if r.drag_stopped() || r.lost_focus() {
                                    changed = true;
                                }
                                r.on_hover_text("Letter height");
                                if ui
                                    .toggle_value(&mut spec.bold, egui::RichText::new("B").strong())
                                    .on_hover_text("Bold")
                                    .changed()
                                {
                                    changed = true;
                                }
                                if ui
                                    .toggle_value(
                                        &mut spec.italic,
                                        egui::RichText::new("I").italics(),
                                    )
                                    .on_hover_text("Italic")
                                    .changed()
                                {
                                    changed = true;
                                }
                                for (a, icon, name) in [
                                    (
                                        zigzag::document::Align::Left,
                                        Icon::AlignLeft,
                                        "Align Left",
                                    ),
                                    (
                                        zigzag::document::Align::Center,
                                        Icon::AlignCenter,
                                        "Align Center",
                                    ),
                                    (
                                        zigzag::document::Align::Right,
                                        Icon::AlignRight,
                                        "Align Right",
                                    ),
                                ] {
                                    if icon_button(ui, icon, name, spec.align == a, BUTTON)
                                        .clicked()
                                        && spec.align != a
                                    {
                                        spec.align = a;
                                        changed = true;
                                    }
                                }
                                if changed {
                                    text_edit = Some((i, spec));
                                }
                            }
                            separator(ui);
                            if single.is_some()
                                && icon_button(ui, Icon::Direct, "Edit Points", false, BUTTON)
                                    .on_hover_text("Edit points (Direct Selection, A)")
                                    .clicked()
                            {
                                act = Some(Action::Tool(Tool::Direct));
                            }
                            if icon_button(ui, Icon::RoundCorner, "Round Corners", false, BUTTON)
                                .on_hover_text("Round Corners…")
                                .clicked()
                            {
                                act = Some(Action::RoundCorners);
                            }
                            if icon_button(ui, Icon::Simplify, "Simplify", false, BUTTON)
                                .on_hover_text("Simplify… (fewer points, smoother curves)")
                                .clicked()
                            {
                                act = Some(Action::SimplifyPath);
                            }
                            if idx.len() >= 2
                                && icon_button(ui, Icon::PathUnite, "Unite", false, BUTTON)
                                    .on_hover_text("Unite into one shape (Pathfinder)")
                                    .clicked()
                            {
                                act = Some(Action::Pathfinder(PathOp::Unite));
                            }
                            if icon_button(ui, Icon::Duplicate, "Duplicate", false, BUTTON)
                                .clicked()
                            {
                                act = Some(Action::Duplicate);
                            }
                            separator(ui);
                            let more =
                                icon_button(ui, Icon::More, "Task bar options", false, BUTTON);
                            egui::Popup::menu(&more).show(|ui| {
                                ui.set_min_width(200.0);
                                if crate::flyout::menu_row(
                                    ui,
                                    crate::flyout::RowIcon::None,
                                    "Hide Task Bar",
                                    "",
                                    false,
                                    true,
                                )
                                .clicked()
                                {
                                    act = Some(Action::ToggleTaskBar);
                                    ui.close();
                                }
                            });
                        });
                    });
            });
        if let Some(at) = picker_at {
            // The thread picker opens above the bar.
            self.picker = match self.picker {
                Some(_) => None,
                None => Some(egui::pos2(
                    at.x,
                    (at.y - 440.0).max(canvas.top() + space::M),
                )),
            };
        }
        if let Some((i, spec)) = text_edit {
            match self.update_lettering(i, &spec) {
                Ok(()) => {}
                Err(e) => self.status = format!("Couldn't update the text: {e:#}"),
            }
        }
        if let Some(a) = act {
            if a == Action::ToggleTaskBar {
                self.status = "Task bar hidden. View › Contextual Task Bar shows it again.".into();
            }
            self.run_action(a);
        }
    }
}

fn separator(ui: &mut egui::Ui) {
    let p = pal_ui(ui);
    let (r, _) = ui.allocate_exact_size(Vec2::new(1.0, BUTTON - 8.0), Sense::hover());
    ui.painter()
        .vline(r.center().x, r.y_range(), Stroke::new(1.0, p.separator));
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use std::path::PathBuf;

    #[test]
    fn the_task_bar_follows_the_selection_and_acts_on_it() {
        let path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg"));
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(path), None),
            );
        h.run_steps(4);
        assert!(
            h.query_by_label("Stitch type").is_none(),
            "nothing selected, no bar"
        );
        let sun = h.state().names.iter().position(|n| n == "sun").unwrap();
        h.state_mut().set_selection(&[sun]);
        h.run_steps(3);
        assert!(h.query_by_label("Stitch type").is_some());
        // Under the selection.
        let bar = h.get_by_label("Stitch type").rect();
        let b = h.state().selection_bounds();
        let below = h.state().view.to_screen(h.state().canvas_rect, b.max).y;
        assert!(bar.top() > below, "{bar:?} under {below}");
        // Duplicate from the bar.
        let n = h.state().doc.shapes.len();
        h.get_all_by_label("Duplicate")
            .find(|n| n.rect().top() >= bar.top() - 2.0)
            .expect("the bar's Duplicate")
            .click();
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), n + 1);
        // Satin from its menu.
        h.get_by_label("Stitch type").click();
        h.run_steps(2);
        h.get_all_by_label("Satin")
            .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
            .unwrap()
            .click();
        h.run_steps(3);
        let i = h.state().sel_index().unwrap();
        assert_eq!(
            h.state().doc.shapes[i].params.kind,
            Some(zigzag::params::Kind::Satin)
        );
        // Hidden from the View menu's action, it stays hidden.
        h.state_mut().run_action(Action::ToggleTaskBar);
        h.run_steps(2);
        assert!(h.query_by_label("Stitch type").is_none());
        h.state_mut().run_action(Action::ToggleTaskBar);
        h.run_steps(2);
        assert!(h.query_by_label("Stitch type").is_some());
    }
}
