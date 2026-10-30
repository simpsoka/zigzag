//! Window chrome: the unified title bar / toolbar, the tool strip, and the
//! status bar with the stitch player.

use crate::app::{SPM, Studio, Tool};
use crate::commands::{Action, TOOLS};
use crate::icons::Icon;
use crate::theme::{caption, icon_button, pal_ui};
use crate::widgets::fmt_int;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

/// Room for the traffic-light buttons when the title bar is transparent.
const TRAFFIC_LIGHTS: f32 = 70.0;

impl Studio {
    pub fn titlebar(&mut self, ui: &mut egui::Ui) {
        let p = pal_ui(ui);
        let ctx = ui.ctx().clone();
        let rect = ui.max_rect();
        // Empty toolbar space moves and zooms the window, as on macOS.
        let bg = ui.interact(
            rect,
            ui.make_persistent_id("titlebar-bg"),
            Sense::click_and_drag(),
        );
        if cfg!(target_os = "macos") {
            if bg.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bg.double_clicked() {
                let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
            }
        }
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if cfg!(target_os = "macos") {
                ui.add_space(TRAFFIC_LIGHTS);
            } else {
                self.menu_bar(ui);
                ui.add_space(4.0);
            }
            if icon_button(ui, Icon::LeftPanel, "Show or Hide Objects", false, 28.0).clicked() {
                self.run_action(Action::ToggleLeftDock);
            }
            ui.add_space(4.0);
            // The document's name, and whether it has unsaved changes.
            let name = if self.doc.name.is_empty() {
                "Untitled"
            } else {
                &self.doc.name
            };
            ui.label(egui::RichText::new(name).size(13.0).color(p.text));
            if self.modified {
                ui.label(
                    egui::RichText::new("— Edited")
                        .size(13.0)
                        .color(p.secondary),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, Icon::RightPanel, "Show or Hide Panels", false, 28.0).clicked() {
                    self.run_action(Action::ToggleRightDock);
                }
                ui.add_space(6.0);
                self.send_button(ui);
                let share = icon_button(ui, Icon::Share, "Export", false, 28.0);
                egui::Popup::menu(&share).show(|ui| {
                    ui.set_min_width(260.0);
                    caption(ui, "Export for your machine");
                    for (ext, label) in zigzag::formats::MACHINE_FORMATS {
                        if ui
                            .add_enabled(!self.design.blocks.is_empty(), egui::Button::new(label))
                            .clicked()
                        {
                            self.run_action(Action::Export(ext));
                        }
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            !self.design.blocks.is_empty(),
                            egui::Button::new("Production Worksheet (HTML)…"),
                        )
                        .clicked()
                    {
                        self.run_action(Action::Export("html"));
                    }
                });
                let view = icon_button(ui, Icon::ViewOptions, "View Options", false, 28.0);
                egui::Popup::from_toggle_button_response(&view)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        ui.set_min_width(200.0);
                        ui.checkbox(&mut self.view_tools.grid, "Grid");
                        ui.checkbox(&mut self.view_tools.points, "Needle points");
                        ui.checkbox(&mut self.show_jumps, "Jump stitches");
                        ui.horizontal(|ui| {
                            ui.color_edit_button_srgb(&mut self.doc.settings.cloth);
                            ui.label("Fabric color");
                        });
                        ui.separator();
                        if ui.button("Zoom to Fit").clicked() {
                            self.request_fit();
                        }
                    });
            });
        });
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0, p.separator),
        );
    }

    /// The toolbar's Send: the Skitch window (connecting to the Skitch).
    fn send_button(&mut self, ui: &mut egui::Ui) {
        if icon_button(ui, Icon::Send, "Send to Skitch", self.machine.open, 28.0)
            .on_hover_text("Sew it on your Skitch")
            .clicked()
        {
            self.send_action();
        }
    }

    /// The vertical tool strip, Illustrator-style.
    pub fn tool_strip(&mut self, ui: &mut egui::Ui) {
        let p = pal_ui(ui);
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.add_space(8.0);
            for (t, label, key) in TOOLS {
                // Groups: the Pen's (Pen, Add / Delete Anchor Point, Anchor
                // Point) and the shapes', each in one slot with a flyout.
                match t {
                    Tool::Pen => {
                        self.group_button(ui, crate::flyout::Group::Pen);
                        continue;
                    }
                    Tool::Shape => {
                        self.group_button(ui, crate::flyout::Group::Shape);
                        continue;
                    }
                    Tool::Brush => {
                        self.group_button(ui, crate::flyout::Group::Brush);
                        continue;
                    }
                    _ => {}
                }
                let icon = match t {
                    Tool::Select => Icon::Select,
                    Tool::Direct => Icon::Direct,
                    Tool::Hand => Icon::Hand,
                    Tool::Brush => Icon::Brush,
                    Tool::Text => Icon::Text,
                    Tool::Angle => Icon::Angle,
                    Tool::Measure => Icon::Measure,
                    _ => continue,
                };
                let tip = if t == Tool::Text {
                    format!("Type ({}): click to place text, click text to edit it, click a selected path to put text on it", key.name())
                } else {
                    format!("{label} ({})", key.name())
                };
                let r = icon_button(ui, icon, label, self.tool == t, 30.0).on_hover_text(tip);
                if r.clicked() {
                    self.set_tool(t);
                }
            }
            ui.add_space(6.0);
            let (r, _) = ui.allocate_exact_size(Vec2::new(22.0, 1.0), Sense::hover());
            ui.painter()
                .hline(r.x_range(), r.center().y, Stroke::new(1.0, p.separator));
            ui.add_space(6.0);
            if icon_button(ui, Icon::ZoomFit, "Zoom to Fit", false, 30.0).clicked() {
                self.request_fit();
            }
            // The selection's thread, like Illustrator's fill swatch: click
            // it for the color picker.
            ui.add_space(10.0);
            let color = self.sel_index().map(|i| self.doc.shapes[i].color);
            let sense = if color.is_some() {
                Sense::click()
            } else {
                Sense::hover()
            };
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(24.0), sense);
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, color.is_some(), "Thread Color")
            });
            match color {
                Some(c) => {
                    ui.painter().rect_filled(r, 5.0, crate::canvas::color32(c));
                    ui.painter().rect_stroke(
                        r,
                        5.0,
                        Stroke::new(1.0, Color32::from_black_alpha(70)),
                        egui::StrokeKind::Inside,
                    );
                }
                None => {
                    // No selection: an empty swatch with a slash, like "None".
                    ui.painter().rect_stroke(
                        r,
                        5.0,
                        Stroke::new(1.0, p.tertiary),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().line_segment(
                        [
                            r.left_bottom() + Vec2::new(4.0, -4.0),
                            r.right_top() + Vec2::new(-4.0, 4.0),
                        ],
                        Stroke::new(1.0, p.error),
                    );
                }
            }
            let resp = resp.on_hover_text(match color {
                Some(c) => format!(
                    "Thread: {} · click to change",
                    crate::threads::nearest(self.view_tools.brand, c).1
                ),
                None => "Select objects to set their thread color".into(),
            });
            if resp.clicked() {
                self.picker = match self.picker {
                    Some(_) => None,
                    None => Some(r.right_top() + Vec2::new(10.0, -8.0)),
                };
            }
            // A floating panel rather than a popup, so the palette menus
            // inside it can open their own popups.
            if let Some(at) = self.picker {
                let ctx = ui.ctx().clone();
                let area = egui::Area::new(ui.make_persistent_id("thread-picker"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(at)
                    .show(&ctx, |ui| {
                        egui::Frame::popup(ui.style())
                            .inner_margin(crate::theme::pad())
                            .show(ui, |ui| self.thread_picker(ui));
                    });
                // A click anywhere else closes it (but not one in its menus).
                let outside = ctx.input(|i| {
                    i.pointer.any_pressed()
                        && i.pointer
                            .interact_pos()
                            .is_some_and(|p| !area.response.rect.contains(p) && !r.contains(p))
                });
                if outside && !egui::Popup::is_any_open(&ctx) {
                    self.picker = None;
                }
            }
            // At the bottom, apart from the tools: the agent's panel.
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                let on = self.agent_showing();
                let r = icon_button(ui, Icon::Sparkle, "Agent", on, 30.0).on_hover_text(format!(
                    "{} (Cmd+K){}",
                    self.agent.kind.name(),
                    if self.agent.running() {
                        " · working"
                    } else {
                        ""
                    }
                ));
                if r.clicked() {
                    self.toggle_agent();
                }
            });
        });
    }

    pub fn status_bar(&mut self, ui: &mut egui::Ui) {
        let p = pal_ui(ui);
        let total = self.design.stitch_count();
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let icon = if self.playing {
                Icon::Pause
            } else {
                Icon::Play
            };
            ui.add_enabled_ui(total > 0, |ui| {
                if icon_button(
                    ui,
                    icon,
                    if self.playing { "Pause" } else { "Play" },
                    false,
                    22.0,
                )
                .on_hover_text("Play the stitch-out (Space)")
                .clicked()
                {
                    self.toggle_play();
                }
            });
            let mut pos = self.player.unwrap_or(total);
            ui.spacing_mut().slider_width = 220.0;
            let slider = ui
                .add_enabled(
                    total > 0,
                    egui::Slider::new(&mut pos, 0..=total.max(1)).show_value(false),
                )
                .on_hover_text("Scrub through the sewing order");
            slider.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Slider, total > 0, "Stitch position")
            });
            if slider.changed() {
                self.playing = false;
                self.player = if pos >= total { None } else { Some(pos) };
            }
            ui.label(
                egui::RichText::new(format!("{} / {}", fmt_int(pos.min(total)), fmt_int(total)))
                    .size(11.0)
                    .color(p.secondary)
                    .monospace(),
            );
            egui::ComboBox::from_id_salt("speed")
                .selected_text(
                    egui::RichText::new(format!("{}×", (self.speed / 400.0).max(0.25))).size(11.0),
                )
                .width(52.0)
                .show_ui(ui, |ui| {
                    for (v, l) in [
                        (100.0, "0.25×"),
                        (400.0, "1×"),
                        (1600.0, "4×"),
                        (6400.0, "16×"),
                    ] {
                        ui.selectable_value(&mut self.speed, v, l);
                    }
                })
                .response
                .on_hover_text("Playback speed");
            ui.add_space(8.0);
            use crate::canvas::StitchView;
            crate::theme::segmented(
                ui,
                "stitch-view",
                &mut self.view_tools.stitch_view,
                &[
                    (StitchView::Thread, "Thread"),
                    (StitchView::Lines, "Stitch Lines"),
                ],
            );
            let b = self.design_bounds();
            let stats = format!(
                "{} · {} colors · {} trims · ~{:.0} min",
                crate::units::fmt_size(b.width(), b.height()),
                self.design.blocks.len(),
                self.design.trim_count(),
                (total as f64 / SPM).ceil()
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.issues_button(ui);
                self.machine_chip(ui);
                self.mcp_chip(ui);
                ui.add_space(4.0);
                self.units_picker(ui);
                if self.planner.pending {
                    // A big design re-planning on the worker: the stitches
                    // shown are the last plan's until it's done.
                    ui.label(
                        egui::RichText::new("Updating stitches…")
                            .size(11.0)
                            .color(p.secondary),
                    );
                    ui.add(egui::Spinner::new().size(10.0));
                }
                ui.label(egui::RichText::new(stats).size(11.0).color(p.secondary))
                    .on_hover_text(format!(
                        "Estimate at {SPM} stitches/min. Planned in {:.1} ms.",
                        self.plan_ms
                    ));
                ui.add_space(12.0);
                self.status_message(ui);
            });
        });
    }

    /// The unit lengths are shown in (also View › Units).
    fn units_picker(&mut self, ui: &mut egui::Ui) {
        let mut u = self.dock.units;
        egui::ComboBox::from_id_salt("units")
            .selected_text(egui::RichText::new(u.suffix()).size(11.0))
            .width(44.0)
            .show_ui(ui, |ui| {
                for v in crate::units::Unit::ALL {
                    ui.selectable_value(&mut u, v, format!("{} ({})", v.label(), v.suffix()));
                }
            })
            .response
            .on_hover_text("Units for sizes, positions, the grid and measuring");
        if u != self.dock.units {
            self.set_units(u);
        }
    }

    /// The design's issues at the right of the status bar: a warning sign
    /// and how many (colored by the worst), opening the list above it.
    fn issues_button(&mut self, ui: &mut egui::Ui) {
        use zigzag::checks::Level;
        let p = pal_ui(ui);
        let n = self.issues.len();
        let worst = self.issues.iter().map(|i| match i.level {
            Level::Error => 2,
            Level::Warning => 1,
            Level::Info => 0,
        });
        let (color, icon) = match worst.max() {
            Some(2) => (p.error, Icon::Warning),
            Some(1) => (p.warning, Icon::Warning),
            Some(_) => (p.secondary, Icon::Info),
            None => (p.tertiary, Icon::Check),
        };
        let text = if n == 0 { String::new() } else { n.to_string() };
        let galley = ui
            .painter()
            .layout_no_wrap(text, egui::FontId::proportional(11.5), color);
        let w = 26.0 + if n == 0 { 0.0 } else { galley.size().x + 4.0 };
        let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 22.0), egui::Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Issues"));
        let popup_id = egui::Id::unique("issues-popover");
        let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
        if resp.hovered() || open {
            ui.painter().rect_filled(r, 5.0, p.hover);
        }
        crate::icons::paint(
            ui.painter(),
            Rect::from_center_size(Pos2::new(r.left() + 13.0, r.center().y), Vec2::splat(13.0)),
            icon,
            color,
        );
        if n > 0 {
            ui.painter().galley(
                Pos2::new(r.left() + 24.0, r.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
        }
        let resp = resp.on_hover_text(match n {
            0 => "No problems found".to_string(),
            1 => "1 issue".to_string(),
            n => format!("{n} issues"),
        });
        if std::mem::take(&mut self.open_issues) && !open {
            egui::Popup::open_id(ui.ctx(), popup_id);
        }
        egui::Popup::from_toggle_button_response(&resp)
            .id(popup_id)
            .align(egui::RectAlign::TOP_END)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width(320.0)
            .show(|ui| {
                ui.set_width(320.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Issues").strong().size(13.0));
                    if n > 0 {
                        ui.label(
                            egui::RichText::new(n.to_string())
                                .size(12.0)
                                .color(p.secondary),
                        );
                    }
                });
                ui.add_space(4.0);
                self.issues_body(ui);
            });
    }

    fn status_message(&mut self, ui: &mut egui::Ui) {
        let p = pal_ui(ui);
        let w = ui.available_width();
        ui.add_sized(
            [w, 18.0],
            egui::Label::new(
                egui::RichText::new(&self.status)
                    .size(11.0)
                    .color(p.secondary),
            )
            .truncate()
            .halign(egui::Align::LEFT),
        );
    }

    /// A floating HUD capsule on the canvas (tool hints), Mac-style.
    pub fn hud(&self, painter: &egui::Painter, canvas: Rect, msg: &str) {
        let p = crate::theme::pal(painter.ctx());
        let galley = painter.layout(
            msg.into(),
            egui::FontId::proportional(12.0),
            Color32::WHITE,
            canvas.width() - 120.0,
        );
        let size = galley.size() + Vec2::new(28.0, 12.0);
        let pos = Pos2::new(
            canvas.center().x - size.x / 2.0,
            canvas.bottom() - self.hud_lift - size.y - 18.0,
        );
        let bg = Rect::from_min_size(pos, size);
        painter.rect_filled(bg, size.y.min(30.0) / 2.0, p.hud);
        painter.galley(bg.min + Vec2::new(14.0, 6.0), galley, Color32::WHITE);
    }
}
