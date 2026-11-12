//! Layer tools. Every object in the Objects list is a layer of the design:
//! sewn in list order, in its own thread.
//!
//! - **Add Outline…** (Object › Path): a new layer on top of each selected
//!   fill, following its edge as a satin border or a running stitch.
//! - **Merge Layers** (Object, ⌘E): the selected layers become one, in the
//!   thread and with the stitch settings of the one on top.
//! - **A thread per layer:** click a layer's swatch in the Objects list for
//!   the thread picker (in `dock.rs`, beside the list it belongs to).
//! - **Stops panel:** every point where the machine halts, in sewing order:
//!   changes of thread, pauses after an object (with a note saying what to
//!   do there, printed on the worksheet too) and an appliqué's two steps.
//!   Add a pause after the selection, or take one out.

use crate::app::Studio;
use crate::icons::{self, Icon};
use crate::theme::{self, caption, pal_ui};
use eframe::egui::{self, Color32, Key, Pos2, Rect, Sense, Stroke, Vec2};
use zigzag::design::BlockStart;
use zigzag::document::{KnockdownSpec, OutlineSpec, OutlineStitch};
use zigzag::params::Kind;
use zigzag::svg::Geometry;

/// The Add Outline window's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutlineDialog {
    pub stitch: OutlineStitch,
    /// Satin border width, mm.
    pub width: f64,
    /// Outside (+) or inside (−) the fill's edge, mm.
    pub offset: f64,
    /// Sew the outline in the fill's own thread rather than `color`.
    pub match_fill: bool,
    pub color: [u8; 3],
}

impl Default for OutlineDialog {
    fn default() -> Self {
        let d = OutlineSpec::default();
        OutlineDialog {
            stitch: d.stitch,
            width: d.width_mm,
            offset: d.offset_mm,
            match_fill: false,
            color: [0, 0, 0],
        }
    }
}

impl OutlineDialog {
    pub fn spec(&self) -> OutlineSpec {
        OutlineSpec {
            stitch: self.stitch,
            width_mm: self.width,
            offset_mm: self.offset,
            color: (!self.match_fill).then_some(self.color),
        }
    }
}

/// The Add Knockdown window's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KnockdownDialog {
    /// How far past the design it reaches, mm.
    pub margin: f64,
    /// The lattice's row spacing, mm.
    pub spacing: f64,
    /// Sew it in the fabric's color (the document's preview color).
    pub match_fabric: bool,
    pub color: [u8; 3],
}

impl Default for KnockdownDialog {
    fn default() -> Self {
        let d = KnockdownSpec::default();
        KnockdownDialog {
            margin: d.margin_mm,
            spacing: d.spacing_mm,
            match_fabric: true,
            color: d.color,
        }
    }
}

/// What a click in the Stops panel asks for.
enum StopEdit {
    Select(Vec<usize>),
    Remove(usize),
    Note(usize, String),
}

impl Studio {
    /// The selected objects an outline can go on: the filled ones.
    pub(crate) fn outlinable_selection(&self) -> Vec<usize> {
        self.sel_indices()
            .into_iter()
            .filter(|&i| matches!(self.doc.shapes[i].geometry, Geometry::Fill { .. }))
            .collect()
    }

    /// Object › Path › Add Outline…: open the window, with the settings
    /// used last time in this session.
    pub fn open_outline_dialog(&mut self) {
        if self.outlinable_selection().is_empty() {
            self.status = "Select a filled object to add an outline to.".into();
            return;
        }
        self.layout.outline_dialog = Some(self.layout.outline_last.unwrap_or_default());
    }

    /// Add an outline layer on top of each selected fill (one undo step);
    /// the new layers end up selected.
    pub fn add_outline_selected(&mut self, spec: OutlineSpec) {
        let idx = self.outlinable_selection();
        let mut new = Vec::new();
        match self.structural(|d| {
            new = d.add_outline(&idx, &spec)?;
            Ok(())
        }) {
            Ok(()) => {
                self.set_selection(&new);
                self.status = format!(
                    "Added {} ({}), sewn on top of {}.",
                    if new.len() == 1 {
                        "an outline layer".to_string()
                    } else {
                        format!("{} outline layers", new.len())
                    },
                    spec.stitch.label(),
                    if new.len() == 1 {
                        "its fill"
                    } else {
                        "their fills"
                    },
                );
            }
            Err(e) => self.status = format!("Couldn't add an outline: {e:#}"),
        }
    }

    /// The Add Outline window.
    pub fn outline_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.layout.outline_dialog else {
            return;
        };
        let brand = self.view_tools.brand;
        let mut open = true;
        let (mut ok, mut cancel) = (false, false);
        egui::Window::new("Add Outline")
            .title_frame(theme::title_frame(ctx))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 90.0))
            .show(ctx, |ui| {
                ui.set_width(320.0);
                theme::form(ui, "outline-form", |fm| {
                    fm.row("Stitch", |ui, _| {
                        let options: Vec<(OutlineStitch, &str)> = OutlineStitch::ALL
                            .iter()
                            .map(|&s| {
                                (
                                    s,
                                    match s {
                                        OutlineStitch::Satin => "Satin",
                                        OutlineStitch::Run => "Run",
                                        OutlineStitch::TripleRun => "Triple Run",
                                    },
                                )
                            })
                            .collect();
                        theme::segmented(ui, "outline-stitch", &mut d.stitch, &options);
                    });
                    fm.row("Width", |ui, _| {
                        ui.add_enabled_ui(d.stitch == OutlineStitch::Satin, |ui| {
                            let r = ui.add(crate::units::drag_mm(
                                &mut d.width,
                                OutlineSpec::MIN_WIDTH..=OutlineSpec::MAX_WIDTH,
                                0.1,
                            ));
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::Role::SpinButton,
                                    true,
                                    "Outline width",
                                )
                            });
                        });
                    });
                    fm.row("Offset", |ui, _| {
                        let r = ui.add(crate::units::drag_mm(
                            &mut d.offset,
                            -OutlineSpec::MAX_OFFSET..=OutlineSpec::MAX_OFFSET,
                            0.1,
                        ));
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::SpinButton,
                                true,
                                "Outline offset",
                            )
                        });
                    });
                    fm.row("Thread", |ui, _| {
                        ui.checkbox(&mut d.match_fill, "Same as the fill");
                        if !d.match_fill {
                            ui.color_edit_button_srgb(&mut d.color);
                            let (_, name, number) = crate::threads::nearest(brand, d.color);
                            caption(ui, crate::threads::label(name, number));
                        }
                    });
                });
                ui.add_space(theme::space::S);
                caption(
                    ui,
                    match d.stitch {
                        OutlineStitch::Satin => {
                            "A new layer on top of each selected fill: a satin column centered on its edge (or on a line outside it, or inside if negative). The fill isn't changed."
                        }
                        OutlineStitch::Run => {
                            "A new layer on top of each selected fill: one pass of running stitch along its edge. The fill isn't changed."
                        }
                        OutlineStitch::TripleRun => {
                            "A new layer on top of each selected fill: a bean stitch (three passes) along its edge. The fill isn't changed."
                        }
                    },
                );
                ui.add_space(theme::space::S);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 24.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let accent = pal_ui(ui).accent;
                        ok = ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("OK").color(Color32::WHITE),
                                )
                                .fill(accent),
                            )
                            .clicked()
                            || ui.input(|i| i.key_pressed(Key::Enter));
                        cancel = ui.button("Cancel").clicked()
                            || ui.input(|i| i.key_pressed(Key::Escape));
                    },
                );
            });
        if ok {
            self.layout.outline_dialog = None;
            self.layout.outline_last = Some(d);
            self.add_outline_selected(d.spec());
        } else if cancel || !open {
            self.layout.outline_dialog = None;
        } else {
            self.layout.outline_dialog = Some(d);
        }
    }

    /// What a knockdown would go under: the selection, or with nothing
    /// selected the whole design (leaving out what isn't sewn and any
    /// knockdown already there).
    pub(crate) fn knockdown_targets(&self) -> Vec<usize> {
        let sel = self.sel_indices();
        let all: Vec<usize> = (0..self.doc.shapes.len()).collect();
        if sel.is_empty() { all } else { sel }
            .into_iter()
            .filter(|&i| {
                !matches!(
                    self.doc.shapes[i].params.kind,
                    Some(Kind::Skip | Kind::Knockdown)
                )
            })
            .collect()
    }

    /// Object › Add Knockdown…: open the window.
    pub fn open_knockdown_dialog(&mut self) {
        if self.knockdown_targets().is_empty() {
            self.status = "There's nothing to put a knockdown under yet.".into();
            return;
        }
        self.layout.knockdown_dialog = Some(KnockdownDialog::default());
    }

    /// Add a knockdown layer under the selection (or the whole design), as
    /// one undo step; it ends up selected.
    pub fn add_knockdown(&mut self, spec: KnockdownSpec) {
        let idx = self.knockdown_targets();
        let whole = self.sel_indices().is_empty();
        let mut new = 0;
        match self.structural(|d| {
            new = d.add_knockdown(&idx, &spec)?;
            Ok(())
        }) {
            Ok(()) => {
                self.set_selection(&[new]);
                self.status = format!(
                    "Added a knockdown layer under {}, sewn first. Use a water-soluble topping as well on towels.",
                    if whole { "the design" } else { "the selection" }
                );
            }
            Err(e) => self.status = format!("Couldn't add a knockdown: {e:#}"),
        }
    }

    /// The Add Knockdown window.
    pub fn knockdown_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.layout.knockdown_dialog else {
            return;
        };
        let brand = self.view_tools.brand;
        let cloth = self.doc.settings.cloth;
        let whole = self.sel_indices().is_empty();
        let mut open = true;
        let (mut ok, mut cancel) = (false, false);
        egui::Window::new("Add Knockdown")
            .title_frame(theme::title_frame(ctx))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 90.0))
            .show(ctx, |ui| {
                ui.set_width(320.0);
                theme::form(ui, "knockdown-form", |fm| {
                    fm.row("Margin", |ui, _| {
                        let r = ui.add(crate::units::drag_mm(
                            &mut d.margin,
                            0.0..=KnockdownSpec::MAX_MARGIN,
                            0.1,
                        ));
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::SpinButton,
                                true,
                                "Knockdown margin",
                            )
                        });
                    });
                    fm.row("Spacing", |ui, _| {
                        let r = ui.add(
                            egui::DragValue::new(&mut d.spacing)
                                .range(KnockdownSpec::MIN_SPACING..=KnockdownSpec::MAX_SPACING)
                                .speed(0.05)
                                .fixed_decimals(1)
                                .suffix(" mm"),
                        );
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::SpinButton,
                                true,
                                "Knockdown spacing",
                            )
                        });
                    });
                    fm.row("Thread", |ui, _| {
                        ui.checkbox(&mut d.match_fabric, "Fabric color");
                        let mut c = if d.match_fabric { cloth } else { d.color };
                        ui.add_enabled_ui(!d.match_fabric, |ui| {
                            ui.color_edit_button_srgb(&mut c);
                        });
                        if !d.match_fabric {
                            d.color = c;
                        }
                        let (_, name, number) = crate::threads::nearest(brand, c);
                        caption(ui, crate::threads::label(name, number));
                    });
                });
                ui.add_space(theme::space::S);
                caption(
                    ui,
                    format!(
                        "A light lattice of stitches under {}, reaching a little past it and sewn first. It flattens the pile of towels, fleece and plush so the design doesn't sink in. The fabric color is the document's \"Preview on\" color.",
                        if whole {
                            "the whole design"
                        } else {
                            "the selected objects"
                        }
                    ),
                );
                ui.add_space(theme::space::S);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 24.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let accent = pal_ui(ui).accent;
                        ok = ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("OK").color(Color32::WHITE),
                                )
                                .fill(accent),
                            )
                            .clicked()
                            || ui.input(|i| i.key_pressed(Key::Enter));
                        cancel = ui.button("Cancel").clicked()
                            || ui.input(|i| i.key_pressed(Key::Escape));
                    },
                );
            });
        if ok {
            self.layout.knockdown_dialog = None;
            self.add_knockdown(KnockdownSpec {
                margin_mm: d.margin,
                spacing_mm: d.spacing,
                color: if d.match_fabric { cloth } else { d.color },
            });
        } else if cancel || !open {
            self.layout.knockdown_dialog = None;
        } else {
            self.layout.knockdown_dialog = Some(d);
        }
    }

    /// Object › Merge Layers: the selection becomes one layer (one undo step).
    pub fn merge_selected(&mut self) {
        let idx = self.sel_indices();
        let n = idx.len();
        let mut new = 0;
        match self.structural(|d| {
            new = d.merge(&idx)?;
            Ok(())
        }) {
            Ok(()) => {
                self.set_selection(&[new]);
                self.status = format!(
                    "Merged {n} layers into one, in the top layer's thread and stitch settings."
                );
            }
            Err(e) => self.status = format!("Couldn't merge: {e:#}"),
        }
    }

    // ------------------------------------------------------------- stops

    /// What a halt is, in a few words: "Change to Red 800", "Pause after sun".
    pub(crate) fn halt_label(&self, k: usize, s: &BlockStart) -> String {
        let name = |i: usize| {
            self.names
                .get(i)
                .cloned()
                .unwrap_or_else(|| "an object".to_string())
        };
        let thread = || {
            let c = self.design.blocks.get(k).map(|b| b.color).unwrap_or([0; 3]);
            let (_, n, number) = crate::threads::nearest(self.view_tools.brand, c);
            crate::threads::label(n, number)
        };
        if let Some((i, step)) = s.applique {
            return format!(
                "Pause · {}: {}",
                name(i),
                if step == 1 {
                    "lay the fabric"
                } else {
                    "trim the fabric"
                }
            );
        }
        match (s.stop_after, s.thread_change) {
            (Some(i), true) => format!("Pause after {}, then {}", name(i), thread()),
            (Some(i), false) => format!("Pause after {}", name(i)),
            (None, true) => format!("Change to {}", thread()),
            (None, false) => "Stop (same thread)".to_string(),
        }
    }

    /// Pause the machine after the selection: after whichever selected
    /// object is sewn last.
    pub fn add_stop_after_selection(&mut self) {
        let sel = self.sel_indices();
        let order = &self.design.sew_order;
        let Some(i) = sel
            .iter()
            .copied()
            .max_by_key(|i| order.iter().position(|k| k == i))
        else {
            self.status = "Select the object the machine should pause after.".into();
            return;
        };
        let name = self.names.get(i).cloned().unwrap_or_default();
        if self.doc.shapes[i].params.stop == Some(true) {
            self.status = format!("The machine already pauses after {name}.");
            return;
        }
        // Nothing sewn after it: the machine stops there anyway.
        let at = order.iter().position(|&k| k == i);
        let sews = |k: &usize| self.design.per_shape.get(*k).copied().unwrap_or(0) > 0;
        let last = at.is_some_and(|a| !order[a + 1..].iter().any(sews));
        let _ = self.structural(|d| {
            d.shapes[i].params.stop = Some(true);
            Ok(())
        });
        self.status = if last {
            format!(
                "{name} is sewn last, so the machine stops after it anyway; the pause takes effect if something is sewn later."
            )
        } else {
            format!("The machine will pause after {name}. Add a note to say what to do there.")
        };
    }

    /// Take out the pause after object `i` (and its note).
    pub fn remove_stop(&mut self, i: usize) {
        if i >= self.doc.shapes.len() {
            return;
        }
        let name = self.names.get(i).cloned().unwrap_or_default();
        let _ = self.structural(|d| {
            d.shapes[i].params.stop = None;
            d.shapes[i].params.stop_note = None;
            Ok(())
        });
        self.status = format!("Removed the pause after {name}.");
    }

    /// Say what to do at the pause after object `i` (empty: no note).
    pub fn set_stop_note(&mut self, i: usize, note: &str) {
        let note = Some(note.trim().to_string()).filter(|n| !n.is_empty());
        if i >= self.doc.shapes.len() || self.doc.shapes[i].params.stop_note == note {
            return;
        }
        let _ = self.structural(|d| {
            d.shapes[i].params.stop_note = note;
            Ok(())
        });
    }

    /// The Stops panel: every halt in sewing order.
    pub(crate) fn stops_body(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let p = pal_ui(ui);
        let halts: Vec<(usize, BlockStart)> = (1..self.design.blocks.len())
            .map(|k| (k, self.design.block_start(k)))
            .collect();
        let selected = self.sel_indices();
        let mut edit: Option<StopEdit> = None;
        if halts.is_empty() {
            caption(
                ui,
                if self.design.blocks.is_empty() {
                    "Nothing to sew yet."
                } else {
                    "No stops: the design sews from start to finish in one thread."
                },
            );
        }
        for (n, (k, s)) in halts.iter().enumerate() {
            let (k, n) = (*k, n + 1);
            let label = self.halt_label(k, s);
            // A pause someone added can be taken out and annotated.
            let own = s.stop_after.filter(|&i| i < self.doc.shapes.len());
            // What a click selects: the object paused after, or (a change of
            // thread) what the new thread sews.
            let target: Vec<usize> = match (s.applique, s.stop_after) {
                (Some((i, _)), _) | (None, Some(i)) => vec![i],
                _ => self.design.block_shapes.get(k).cloned().unwrap_or_default(),
            }
            .into_iter()
            .filter(|&i| i < self.doc.shapes.len())
            .collect();
            let (rect, resp) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), theme::ROW_H),
                Sense::click(),
            );
            let x_rect = Rect::from_center_size(
                Pos2::new(rect.right() - 11.0, rect.center().y),
                Vec2::splat(16.0),
            );
            let x = own.map(|_| {
                let r = ui.interact(
                    x_rect,
                    ui.make_persistent_id(("stop-remove", k)),
                    Sense::click(),
                );
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Remove stop {n}"))
                });
                r
            });
            let on = !selected.is_empty()
                && target.len() == selected.len()
                && target.iter().all(|i| selected.contains(i));
            let painter = ui.painter();
            if on {
                painter.rect_filled(rect, 5.0, p.accent.gamma_multiply(0.18));
            } else if resp.hovered() {
                painter.rect_filled(rect, 5.0, p.hover);
            }
            painter.text(
                Pos2::new(rect.left() + 4.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                format!("{n}"),
                egui::FontId::proportional(11.0),
                p.secondary,
            );
            let sw = Rect::from_center_size(
                Pos2::new(
                    rect.left() + 20.0 + theme::LIST_SWATCH / 2.0,
                    rect.center().y,
                ),
                Vec2::splat(theme::LIST_SWATCH),
            );
            if s.is_pause() || !s.thread_change {
                icons::paint(painter, sw.shrink(2.0), Icon::Pause, p.text);
            } else {
                let c = self.design.blocks[k].color;
                painter.rect_filled(sw, 3.0, crate::canvas::color32(c));
                painter.rect_stroke(
                    sw,
                    3.0,
                    Stroke::new(1.0, Color32::from_black_alpha(40)),
                    egui::StrokeKind::Inside,
                );
            }
            let text_rect = Rect::from_min_max(
                Pos2::new(sw.right() + theme::space::S, rect.top()),
                Pos2::new(
                    if own.is_some() {
                        x_rect.left() - 4.0
                    } else {
                        rect.right() - 4.0
                    },
                    rect.bottom(),
                ),
            );
            painter.with_clip_rect(text_rect).text(
                Pos2::new(text_rect.left(), rect.center().y),
                egui::Align2::LEFT_CENTER,
                &label,
                egui::FontId::proportional(12.0),
                if on { p.accent } else { p.text },
            );
            if let Some(x) = &x {
                icons::paint(
                    painter,
                    x_rect.shrink(4.0),
                    Icon::Close,
                    if x.hovered() { p.text } else { p.secondary },
                );
            }
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Stop {n}: {label}"))
            });
            if x.is_some_and(|x| x.on_hover_text("Remove this pause").clicked()) {
                edit = own.map(StopEdit::Remove);
            } else if resp
                .on_hover_text(match (own, s.applique) {
                    (Some(_), _) => "A pause you added. Click to select the object it follows.",
                    (None, Some(_)) => {
                        "Part of the appliqué: the machine waits while you lay or trim the fabric."
                    }
                    _ => "The machine stops for the next thread. Click to select what it sews.",
                })
                .clicked()
                && !target.is_empty()
            {
                edit = Some(StopEdit::Select(target));
            }
            // The note: what to do at this pause. Kept as typed until the
            // field is left, then saved as one undo step.
            if let Some(i) = own {
                let uid = self.doc.shapes[i].uid;
                let mut text = match &self.layout.stop_note_edit {
                    Some((u, t)) if *u == uid => t.clone(),
                    _ => self.doc.shapes[i]
                        .params
                        .stop_note
                        .clone()
                        .unwrap_or_default(),
                };
                let r = ui
                    .horizontal(|ui| {
                        ui.add_space(20.0 + theme::LIST_SWATCH + theme::space::S);
                        ui.add(
                            egui::TextEdit::singleline(&mut text)
                                .id_salt(("stop-note", uid))
                                .hint_text("What to do here (lay the foam…)")
                                .font(egui::FontId::proportional(12.0))
                                .desired_width(f32::INFINITY),
                        )
                    })
                    .inner;
                if r.lost_focus() {
                    self.layout.stop_note_edit = None;
                    edit = Some(StopEdit::Note(i, text));
                } else if r.has_focus() || r.changed() {
                    self.layout.stop_note_edit = Some((uid, text));
                }
                ui.add_space(theme::space::XS);
            }
        }
        ui.spacing_mut().item_spacing.y = theme::space::S;
        ui.add_space(theme::space::S);
        let mut add = false;
        ui.horizontal(|ui| {
            add = ui
                .add_enabled(!selected.is_empty(), egui::Button::new("Add Stop"))
                .on_hover_text("Pause the machine after the selected object (to lay foam, trim fabric, change the needle…)")
                .on_disabled_hover_text("Select the object the machine should pause after")
                .clicked();
            let pauses = halts.iter().filter(|(_, s)| s.is_pause()).count();
            let changes = halts.iter().filter(|(_, s)| s.thread_change).count();
            caption(
                ui,
                format!(
                    "{changes} thread change{} · {pauses} pause{}",
                    if changes == 1 { "" } else { "s" },
                    if pauses == 1 { "" } else { "s" }
                ),
            );
        });
        match edit {
            Some(StopEdit::Select(v)) => self.set_selection(&v),
            Some(StopEdit::Remove(i)) => self.remove_stop(i),
            Some(StopEdit::Note(i, text)) => self.set_stop_note(i, &text),
            None => {}
        }
        if add {
            self.add_stop_after_selection();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Action;
    use crate::dock::PanelId;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use zigzag::params::Kind;

    fn studio() -> Harness<'static, Studio> {
        let path =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg"));
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(path), None),
            );
        h.run_steps(4);
        h
    }

    fn index_of(h: &Harness<'_, Studio>, name: &str) -> usize {
        h.state().names.iter().position(|n| n == name).unwrap()
    }

    #[test]
    fn add_outline_window_makes_a_layer_on_top_of_the_fill() {
        let mut h = studio();
        let n = h.state().doc.shapes.len();
        // Only fills take an outline: not offered for a line.
        let swoosh = index_of(&h, "swoosh");
        h.state_mut().set_selection(&[swoosh]);
        assert!(!h.state().action_enabled(Action::AddOutline));
        let sun = index_of(&h, "sun");
        let sun_uid = h.state().doc.shapes[sun].uid;
        h.state_mut().set_selection(&[sun]);
        assert!(h.state().action_enabled(Action::AddOutline));
        h.state_mut().run_action(Action::AddOutline);
        h.run_steps(2);
        assert!(h.state().layout.outline_dialog.is_some());
        h.get_by_label("OK").click();
        h.run_steps(3);
        assert!(h.state().layout.outline_dialog.is_none());
        assert_eq!(h.state().doc.shapes.len(), n + 1, "{}", h.state().status);
        // The outline is selected, sits straight after the sun, and is a
        // black satin border; the sun is still the fill it was.
        let sun = h
            .state()
            .doc
            .shapes
            .iter()
            .position(|s| s.uid == sun_uid)
            .unwrap();
        assert_eq!(h.state().sel_indices(), vec![sun + 1]);
        let s = h.state();
        let o = &s.doc.shapes[sun + 1];
        assert_eq!(o.params.kind, Some(Kind::Satin));
        assert!(o.params.is_border());
        assert_eq!(o.color, [0, 0, 0]);
        assert_eq!(s.doc.shapes[sun].params.kind, Some(Kind::Fill));
        assert_eq!(s.names[sun + 1], "sun outline");
        assert!(s.design.per_shape[sun + 1] > 20, "it sews");
        // One undo step.
        assert_eq!(h.state().undo.len(), 1);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), n);

        // Escape closes the window without adding anything.
        let sun = index_of(&h, "sun");
        h.state_mut().set_selection(&[sun]);
        h.state_mut().run_action(Action::AddOutline);
        h.run_steps(2);
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert!(h.state().layout.outline_dialog.is_none());
        assert_eq!(h.state().doc.shapes.len(), n);
    }

    #[test]
    fn outline_settings_are_kept_for_next_time_and_can_match_the_fill() {
        let mut h = studio();
        let sun = index_of(&h, "sun");
        let color = h.state().doc.shapes[sun].color;
        h.state_mut().set_selection(&[sun]);
        h.state_mut().run_action(Action::AddOutline);
        h.run_steps(2);
        h.get_by_label("Triple Run").click();
        h.run_steps(2);
        h.get_by_label("Same as the fill").click();
        h.run_steps(2);
        h.get_by_label("OK").click();
        h.run_steps(3);
        let i = h.state().sel_index().unwrap();
        let o = &h.state().doc.shapes[i];
        assert_eq!(
            (o.params.kind, o.params.repeats),
            (Some(Kind::Run), Some(3))
        );
        assert_eq!(o.color, color);
        // The window opens with the same choices next time.
        let disc = index_of(&h, "disc");
        h.state_mut().set_selection(&[disc]);
        h.state_mut().run_action(Action::AddOutline);
        let d = h.state().layout.outline_dialog.unwrap();
        assert_eq!(d.stitch, OutlineStitch::TripleRun);
        assert!(d.match_fill);
    }

    #[test]
    fn add_knockdown_window_puts_a_lattice_under_the_design() {
        let mut h = studio();
        let n = h.state().doc.shapes.len();
        let before = h.state().design.per_shape.clone();
        // Nothing selected: it goes under the whole design.
        h.state_mut().set_selection(&[]);
        assert!(h.state().action_enabled(Action::AddKnockdown));
        h.state_mut().run_action(Action::AddKnockdown);
        h.run_steps(2);
        assert!(h.state().layout.knockdown_dialog.is_some());
        h.get_by_label("OK").click();
        h.run_steps(4);
        assert!(h.state().layout.knockdown_dialog.is_none());
        let s = h.state();
        assert_eq!(s.doc.shapes.len(), n + 1, "{}", s.status);
        // First in the list, selected, a Knockdown in the fabric's color.
        assert_eq!(s.sel_indices(), vec![0]);
        assert_eq!(s.doc.shapes[0].params.kind, Some(Kind::Knockdown));
        assert_eq!(s.doc.shapes[0].color, s.doc.settings.cloth);
        assert_eq!(s.names[0], "base layer");
        assert!(s.design.per_shape[0] > 100);
        assert_eq!(s.design.sew_order[0], 0);
        // Everything on top sews exactly as before.
        assert_eq!(&s.design.per_shape[1..], &before[..]);
        assert_eq!(s.undo.len(), 1);
        // With only the knockdown selected there's nothing to go under.
        assert!(!h.state().action_enabled(Action::AddKnockdown));
        // It's a stitch type like any other in the Stitch panel.
        assert!(crate::widgets::allowed_kinds(&h.state().doc.shapes[0]).contains(&Kind::Knockdown));
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), n);
    }

    #[test]
    fn basting_and_thread_estimates_show_in_the_panels() {
        let mut h = studio();
        let plain = h.state().design.stitch_count();
        // The Threads panel says about how much thread the design takes.
        let total = h.state().thread_len(h.state().design.thread_mm());
        assert!(total.ends_with(" m"), "{total}");
        assert!(
            h.query_by_label_contains(&format!("Thread: about {total} top"))
                .is_some()
        );
        // Basting: a checkbox in the Document panel, 5 mm out by default.
        h.get_by_label("Box at").click();
        h.run_steps(5);
        let s = h.state();
        assert_eq!(s.doc.settings.baste_mm, 5.0);
        assert!(s.design.basting_stitches > 4, "{}", s.status);
        assert_eq!(s.design.stitch_count(), plain + s.design.basting_stitches);
        assert_eq!(s.undo.len(), 1);
        assert!(s.doc.to_svg().unwrap().contains("baste=5"));
        // More stitches, more thread.
        let more = h.state().thread_len(h.state().design.thread_mm());
        assert_ne!(more, total);
        h.get_by_label("Box at").click();
        h.run_steps(5);
        assert_eq!(h.state().doc.settings.baste_mm, 0.0);
        assert_eq!(h.state().design.stitch_count(), plain);
        // Inches as the unit: thread in yards.
        h.state_mut().dock.units = crate::units::Unit::In;
        assert!(h.state().thread_len(9144.0).ends_with("10.0 yd"));
    }

    #[test]
    fn merge_layers_from_the_objects_list() {
        let mut h = studio();
        let n = h.state().doc.shapes.len();
        let (sun, disc) = (index_of(&h, "sun"), index_of(&h, "disc"));
        let top_color = h.state().doc.shapes[sun.max(disc)].color;
        // One layer selected: nothing to merge.
        h.state_mut().set_selection(&[sun]);
        h.run_steps(2);
        assert!(!h.state().action_enabled(Action::MergeLayers));
        h.state_mut().set_selection(&[sun, disc]);
        h.run_steps(2);
        assert!(h.state().action_enabled(Action::MergeLayers));
        h.get_by_label("Merge Layers").click();
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), n - 1, "{}", h.state().status);
        let sel = h.state().sel_indices();
        assert_eq!(sel.len(), 1);
        assert_eq!(h.state().doc.shapes[sel[0]].color, top_color);
        assert_eq!(h.state().undo.len(), 1);
        // ⌘E does the same; here there's one layer selected, so nothing.
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::E);
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), n - 1);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), n);
    }

    #[test]
    fn a_layers_swatch_opens_the_thread_picker_for_that_layer() {
        let mut h = studio();
        let (sun, disc) = (index_of(&h, "sun"), index_of(&h, "disc"));
        h.state_mut().set_selection(&[disc]);
        h.run_steps(2);
        assert!(h.state().picker.is_none());
        h.get_by_label("Thread of sun").click();
        h.run_steps(3);
        assert!(h.state().picker.is_some(), "the picker opened");
        assert_eq!(h.state().sel_indices(), vec![sun], "for that layer");
        // The picker's edits go to the selection: only the sun changes.
        let disc_color = h.state().doc.shapes[disc].color;
        h.state_mut().doc.shapes[sun].color = [1, 2, 3];
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes[disc].color, disc_color);
    }

    #[test]
    fn stops_panel_lists_adds_annotates_and_removes_stops() {
        let mut h = studio();
        // On its own over the canvas, so the whole panel is in view.
        h.state_mut().dock.separate(PanelId::Stops);
        h.state_mut()
            .dock
            .move_panel(PanelId::Stops, crate::dock::Place::Float, None);
        h.state_mut().dock.reveal(PanelId::Stops);
        h.run_steps(3);
        // The badge changes thread between its objects: listed, not pauses.
        let changes = h.state().design.blocks.len() - 1;
        assert!(changes >= 1);
        assert!(h.query_by_label_contains("Stop 1: Change to").is_some());
        assert!(h.query_by_label_contains("Remove stop").is_none());
        // Add a pause after the disc (the first thing sewn).
        let disc = index_of(&h, "disc");
        assert_eq!(h.state().design.sew_order.first(), Some(&disc));
        h.state_mut().set_selection(&[disc]);
        h.run_steps(2);
        h.get_by_label("Add Stop").click();
        h.run_steps(4);
        assert_eq!(h.state().doc.shapes[disc].params.stop, Some(true));
        assert_eq!(h.state().undo.len(), 1);
        let s = h.state().design.block_start(1);
        assert_eq!(s.stop_after, Some(disc));
        assert!(
            h.query_by_label_contains("Stop 1: Pause after disc")
                .is_some(),
            "{}",
            h.state().halt_label(1, &s)
        );
        // Its note is saved in the document, the plan and the SVG.
        h.state_mut().set_stop_note(disc, "  Lay the foam  ");
        h.run_steps(4);
        assert_eq!(
            h.state().doc.shapes[disc].params.stop_note.as_deref(),
            Some("Lay the foam")
        );
        assert_eq!(
            h.state().design.block_start(1).note.as_deref(),
            Some("Lay the foam")
        );
        assert!(
            h.state()
                .doc
                .to_svg()
                .unwrap()
                .contains("note=Lay+the+foam")
        );
        assert_eq!(h.state().undo.len(), 2);
        // The same note again is no new undo step.
        h.state_mut().set_stop_note(disc, "Lay the foam");
        assert_eq!(h.state().undo.len(), 2);
        // Asking twice doesn't add a second pause.
        h.state_mut().add_stop_after_selection();
        assert_eq!(h.state().undo.len(), 2);
        // The × takes the pause (and its note) out again.
        h.get_by_label("Remove stop 1").click();
        h.run_steps(4);
        let p = &h.state().doc.shapes[disc].params;
        assert_eq!((p.stop, p.stop_note.clone()), (None, None));
        assert!(h.query_by_label_contains("Remove stop").is_none());
        assert_eq!(h.state().design.blocks.len() - 1, changes);
    }

    #[test]
    fn a_stops_note_is_typed_in_the_panel_and_saved_when_the_field_is_left() {
        let mut h = studio();
        h.state_mut().dock.separate(PanelId::Stops);
        h.state_mut()
            .dock
            .move_panel(PanelId::Stops, crate::dock::Place::Float, None);
        h.state_mut().dock.reveal(PanelId::Stops);
        let disc = index_of(&h, "disc");
        h.state_mut().set_selection(&[disc]);
        h.state_mut().add_stop_after_selection();
        h.state_mut().set_selection(&[]);
        h.run_steps(4);
        let undo = h.state().undo.len();
        // The one pause has the panel's one text field.
        let field = h.get_by_role(egui::accesskit::Role::TextInput);
        field.focus();
        h.run_steps(2);
        h.get_by_role(egui::accesskit::Role::TextInput)
            .type_text("Trim the fabric");
        h.run_steps(2);
        // Still being typed: nothing saved, no undo steps.
        assert_eq!(h.state().doc.shapes[disc].params.stop_note, None);
        assert_eq!(h.state().undo.len(), undo);
        h.key_press(Key::Enter);
        h.run_steps(4);
        assert_eq!(
            h.state().doc.shapes[disc].params.stop_note.as_deref(),
            Some("Trim the fabric")
        );
        assert_eq!(
            h.state().undo.len(),
            undo + 1,
            "one step for the whole note"
        );
        assert!(h.state().layout.stop_note_edit.is_none());
        // Undo takes the note away and leaves the pause.
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
        h.run_steps(4);
        let p = &h.state().doc.shapes[disc].params;
        assert_eq!((p.stop, p.stop_note.clone()), (Some(true), None));
    }

    #[test]
    fn clicking_a_stop_selects_what_it_belongs_to() {
        let mut h = studio();
        // On its own over the canvas, so every row is in view.
        h.state_mut().dock.separate(PanelId::Stops);
        h.state_mut()
            .dock
            .move_panel(PanelId::Stops, crate::dock::Place::Float, None);
        h.state_mut().dock.reveal(PanelId::Stops);
        let disc = index_of(&h, "disc");
        h.state_mut().set_selection(&[disc]);
        h.state_mut().add_stop_after_selection();
        h.state_mut().set_selection(&[]);
        h.run_steps(4);
        h.get_by_label_contains("Stop 1: Pause after disc").click();
        h.run_steps(3);
        assert_eq!(h.state().sel_indices(), vec![disc]);
        // A change of thread selects what the new thread sews.
        let k = (1..h.state().design.blocks.len())
            .find(|&k| !h.state().design.block_start(k).is_pause())
            .unwrap();
        let want = h.state().design.block_shapes[k].clone();
        h.get_by_label_contains(&format!("Stop {k}: Change to"))
            .click();
        h.run_steps(3);
        let mut got = h.state().sel_indices();
        got.sort_unstable();
        let mut want = want;
        want.sort_unstable();
        assert_eq!(got, want);
    }
}
