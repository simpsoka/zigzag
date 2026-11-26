//! The contextual task bar, after Illustrator's (and VectorCraft's
//! `canvas.rs` task bar): a small floating bar under the selection with the
//! next things you're likely to do to it — how it's stitched, its thread,
//! editing its points, rounding its corners, uniting several, duplicating.
//! View › Contextual Task Bar (or its ••• menu) hides it.
//!
//! The grip at its left end moves it out of the way of the art, as in
//! Illustrator (ported from VectorCraft's `canvas/task_bar.rs`): moved,
//! it keeps that offset from its place by the selection as it follows it;
//! pinned (••• › Pin Bar Position) it stays where it is. Reset Bar
//! Position puts it back.

use crate::app::{Studio, Tool};
use crate::commands::Action;
use crate::icons::Icon;
use crate::theme::{icon_button, pal_ui, space};
use eframe::egui::{self, Color32, Rect, Sense, Stroke, Vec2};
use zigzag::document::Pathfinder;

/// Height of the bar's buttons.
const BUTTON: f32 = 28.0;
/// Width of the stitch type's menu button (and of the outline's stitch
/// types under it).
const KIND_W: f32 = 96.0;
/// Width of the grip that moves the bar (the line drawn in it is thinner).
const GRIP_W: f32 = 14.0;

/// Where the bar sits once its grip has moved it (VectorCraft's
/// `TaskBarPlace`). Not kept from one launch to the next, as in
/// Illustrator: the bar starts by the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TaskBarPlace {
    /// ••• › Pin Bar Position: the bar stays where it is instead of
    /// following the selection.
    pub pinned: bool,
    /// Where a pinned bar's top-left corner is from the canvas's (none
    /// when it was pinned before it showed, until it's drawn). Unpinning
    /// leaves it for the next frame to turn into `offset`.
    pub pin_at: Option<Vec2>,
    /// How far a bar that follows the selection was moved from its place
    /// by it, and in which document (its recovery number): in another,
    /// the bar starts by the selection again.
    pub offset: Option<(u32, Vec2)>,
}

impl Studio {
    /// Whether the bar shows: the setting is on, the Selection tool is
    /// active with something selected, and nothing is being dragged.
    pub fn task_bar_shown(&self) -> bool {
        self.dock.task_bar
            && self.tool == Tool::Select
            && !self.sel_indices().is_empty()
            && self.drag.is_none()
            && self.path_edit.drag.is_none()
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
        // Anywhere on the canvas, clear of the prompt bar.
        let room_min = canvas.min + Vec2::splat(space::M);
        let room_max = egui::pos2(canvas.right() - space::M - size.x, bottom_limit).max(room_min);
        let keep_in = |p: egui::Pos2| p.clamp(room_min, room_max);
        let x = (sel.center().x - size.x / 2.0).clamp(room_min.x, room_max.x);
        // Its place by the selection, then where the grip moved it.
        let by_sel = egui::pos2(x, y);
        let doc = self.session.rec;
        let place = &mut self.layout.task_bar_place;
        // Just unpinned: it follows the selection from where it was pinned.
        if !place.pinned
            && let Some(at) = place.pin_at.take()
        {
            place.offset = Some((doc, canvas.min + at - by_sel));
        }
        let at = keep_in(match place.pin_at {
            Some(at) => canvas.min + at,
            None => {
                by_sel
                    + place
                        .offset
                        .filter(|(d, _)| *d == doc)
                        .map_or(Vec2::ZERO, |(_, o)| o)
            }
        });
        // Pinned before it showed: held where it is now.
        if place.pinned && place.pin_at.is_none() {
            place.pin_at = Some(at - canvas.min);
        }
        let pinned = place.pinned;
        let mut moved = Vec2::ZERO;
        // (A shape selected with its own outline is the shape: the bar is
        // the shape's, with the outline's row under it.)
        let idx = self.subjects();
        let single = (idx.len() == 1).then(|| idx[0]);
        let mut act: Option<Action> = None;
        // The outline button, clicked: whether the selection has outlines.
        let mut outline: Option<bool> = None;
        let mut outline_removed = false;
        let mut outline_picker: Option<(usize, egui::Pos2)> = None;
        let mut picker_at: Option<egui::Pos2> = None;
        let mut text_edit: Option<(usize, zigzag::document::TextSpec)> = None;
        let mut place_change: Option<PlaceChange> = None;
        egui::Area::new(id)
            .order(egui::Order::Middle)
            .fixed_pos(at)
            .constrain_to(canvas)
            .show(ctx, |ui| {
                ui.set_max_width(crate::theme::canvas_bar_width(canvas));
                egui::Frame::popup(ui.style())
                    .corner_radius(9)
                    .inner_margin(egui::Margin::same(space::XS as i8))
                    .show(ui, |ui| {
                        let top = ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = space::XS;
                            let p = pal_ui(ui);
                            moved = grip(ui);
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
                                ui.allocate_exact_size(Vec2::new(KIND_W, BUTTON), Sense::click());
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
                            use crate::threads::SelThread;
                            let thread = self.selection_thread();
                            let (r, resp) =
                                ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click());
                            resp.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::Role::Button,
                                    true,
                                    if thread == SelThread::Mixed {
                                        "Thread color: several"
                                    } else {
                                        "Thread color"
                                    },
                                )
                            });
                            if resp.hovered() {
                                ui.painter().rect_filled(r, 6.0, p.hover);
                            }
                            let sw = Rect::from_center_size(r.center(), Vec2::splat(16.0));
                            let tip = match thread {
                                SelThread::One(color) => {
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
                                    format!(
                                        "Thread: {}",
                                        crate::threads::nearest(self.view_tools.brand, color).1
                                    )
                                }
                                // Layers in different threads (or, never
                                // here, none): no one color to show.
                                _ => {
                                    crate::threads::paint_mixed(ui.painter(), sw, 4.0, &p);
                                    "Several threads · click to pick one for all the selected layers"
                                        .to_string()
                                }
                            };
                            if resp.on_hover_text(tip).clicked() {
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
                                        changed |= crate::lettering::font_list(
                                            ui,
                                            &mut spec.font,
                                            &fonts,
                                            &mut filter,
                                        );
                                    })
                                    .response
                                    .on_hover_text("Font");
                                if changed {
                                    crate::lettering::picked_font(&mut spec, &fonts);
                                }
                                self.lettering.font_filter = filter;
                                // (Typed, the height is taken when it's entered:
                                // `spec` is read from the document every frame,
                                // so a value set while typing would be read
                                // back as it was and reset what's typed.)
                                let r = ui.add(
                                    crate::units::drag_mm(&mut spec.size_mm, 3.0..=40.0, 0.5)
                                        .update_while_editing(false)
                                        // (Text scaled by its handles past
                                        // what's set here keeps its size
                                        // until this is what's changed.)
                                        .clamp_existing_to_range(false),
                                );
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
                                    (zigzag::document::Align::Left, Icon::AlignLeft, "Align Left"),
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
                            // A shape's outline: its button adds one, and
                            // then shows the outline's thread and opens and
                            // closes its row (the outline stays; the struck-through
                            // outline in its row takes it off).
                            let (outlinable, on) = self.outline_state(&idx);
                            if outlinable {
                                let thread = single
                                    .filter(|_| on)
                                    .and_then(|i| self.outline_index(i))
                                    .map(|o| self.doc.shapes[o].color);
                                let open = thread.is_some() && !self.layout.outline_row_closed;
                                if outline_button(ui, on, thread, open).clicked() {
                                    outline = Some(on);
                                }
                            }
                            if idx.len() >= 2
                                && icon_button(ui, Icon::PathUnite, "Unite", false, BUTTON)
                                    .on_hover_text("Unite into one shape (Pathfinder)")
                                    .clicked()
                            {
                                act = Some(Action::Pathfinder(Pathfinder::Unite));
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
                                if crate::flyout::menu_row(
                                    ui,
                                    crate::flyout::RowIcon::None,
                                    "Pin Bar Position",
                                    "",
                                    pinned,
                                    true,
                                )
                                .clicked()
                                {
                                    place_change = Some(PlaceChange::Pin(!pinned));
                                    ui.close();
                                }
                                if crate::flyout::menu_row(
                                    ui,
                                    crate::flyout::RowIcon::None,
                                    "Reset Bar Position",
                                    "",
                                    false,
                                    true,
                                )
                                .clicked()
                                {
                                    place_change = Some(PlaceChange::Reset);
                                    ui.close();
                                }
                            });
                        })
                        .response
                        .rect;
                // A shape whose outline is on: the bar grows a second row
                // for the outline (how it's sewn, its thread, its width).
                // Only then: a shape without one shows nothing of it.
                if let Some(o) = single
                    .filter(|&i| self.outline_on(i) && !self.layout.outline_row_closed)
                    .and_then(|i| self.outline_index(i))
                {
                    use zigzag::params::Kind;
                    // The row is a darker band across the bottom of the bar
                    // (painted under it, once its size is known).
                    let band = ui.painter().add(egui::Shape::Noop);
                    ui.add_space(space::S);
                    let default_width = zigzag::design::default_satin_width(&self.doc.shapes[o]);
                    let row = ui
                        .horizontal_wrapped(|ui| {
                            // In the top row's columns: how it's sewn under
                            // the shape's stitch type, its thread under the
                            // shape's.
                            ui.spacing_mut().item_spacing.x = space::XS;
                            let p = pal_ui(ui);
                            let s = &mut self.doc.shapes[o];
                            let k = s.params.kind.unwrap_or(Kind::Run);
                            let (kinds, _) =
                                ui.allocate_exact_size(Vec2::new(KIND_W, BUTTON), Sense::hover());
                            for (n, kind) in [Kind::Run, Kind::Satin, Kind::Blanket]
                                .into_iter()
                                .enumerate()
                            {
                                let at = Rect::from_min_size(
                                    kinds.min + Vec2::new(n as f32 * (BUTTON + space::XS), 0.0),
                                    Vec2::splat(BUTTON),
                                );
                                if kind_button(ui, at, kind, kind == k).clicked() && kind != k {
                                    // (A width that was the satin's isn't
                                    // the prongs' length.)
                                    s.params.width = None;
                                    s.params.kind = Some(kind);
                                }
                            }
                            // Its thread: the shape's is a filled swatch,
                            // the outline's the line round an empty one.
                            let (r, resp) =
                                ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click());
                            resp.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Button, true, "Outline thread")
                            });
                            if resp.hovered() {
                                ui.painter().rect_filled(r, 6.0, p.hover);
                            }
                            let sw = Rect::from_center_size(r.center(), Vec2::splat(16.0));
                            let c = s.color;
                            ui.painter().rect_stroke(
                                sw,
                                4.0,
                                Stroke::new(2.5, Color32::from_rgb(c[0], c[1], c[2])),
                                egui::StrokeKind::Inside,
                            );
                            // (Hairlines either side, so a thread as dark
                            // as the bar still shows.)
                            ui.painter().rect_stroke(
                                sw,
                                4.0,
                                Stroke::new(1.0, p.separator),
                                egui::StrokeKind::Outside,
                            );
                            ui.painter().rect_stroke(
                                sw.shrink(2.5),
                                1.5,
                                Stroke::new(1.0, p.separator),
                                egui::StrokeKind::Inside,
                            );
                            let name = crate::threads::nearest(self.view_tools.brand, c).1;
                            if resp
                                .on_hover_text(format!("Outline thread: {name}"))
                                .clicked()
                            {
                                outline_picker = Some((s.uid, r.left_top()));
                            }
                            separator(ui);
                            match k {
                                Kind::Satin | Kind::Blanket => {
                                    let (default, range, tip) = if k == Kind::Satin {
                                        (
                                            default_width,
                                            0.6..=10.0,
                                            "Width of the satin, centered on the shape's edge",
                                        )
                                    } else {
                                        (
                                            zigzag::params::defaults::BLANKET_WIDTH,
                                            1.0..=6.0,
                                            "How far the prongs reach into the shape",
                                        )
                                    };
                                    let shown = s.params.width.unwrap_or(default);
                                    let mut w = shown;
                                    // (Wide enough for what it is now: a
                                    // field narrower than its value would
                                    // pull the value in just by being shown.)
                                    let range: std::ops::RangeInclusive<f64> = range;
                                    let range = range.start().min(shown)..=range.end().max(shown);
                                    let r = ui
                                        .add(
                                            egui::DragValue::new(&mut w)
                                                .range(range)
                                                .speed(0.05)
                                                .fixed_decimals(1)
                                                .suffix(" mm"),
                                        )
                                        .on_hover_text(tip);
                                    r.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::SpinButton,
                                            true,
                                            "Outline width",
                                        )
                                    });
                                    // Only a change someone made.
                                    let touched = r.dragged() || r.has_focus() || r.lost_focus();
                                    if r.changed() && touched && w != shown {
                                        s.params.width = Some(w);
                                    }
                                }
                                Kind::Run => {
                                    let mut passes = s.params.run_repeats();
                                    if crate::theme::segmented(
                                        ui,
                                        "outline-passes",
                                        &mut passes,
                                        &[(1, "Single"), (3, "Triple")],
                                    ) {
                                        s.params.repeats = Some(passes);
                                    }
                                }
                                _ => {}
                            }
                            // Taking it off, at the row's end (under the
                            // top row's, when that's the longer). The rest
                            // of an outline's stitch settings are the
                            // outline's own, picked from its row in Layers.
                            let gap = top.right() - ui.cursor().left() - BUTTON;
                            if gap > 0.0 {
                                ui.add_space(gap);
                            }
                            if icon_button(ui, Icon::NoOutline, "Remove outline", false, BUTTON)
                                .on_hover_text("Remove the outline")
                                .clicked()
                            {
                                outline_removed = true;
                            }
                        })
                        .response
                        .rect;
                    // From the bar's edges in, and down to its foot.
                    let m = space::XS;
                    let p = pal_ui(ui);
                    ui.painter().set(
                        band,
                        egui::epaint::RectShape::filled(
                            Rect::from_min_max(
                                egui::pos2(
                                    top.left().min(row.left()) - m,
                                    (top.bottom() + row.top()) / 2.0,
                                ),
                                egui::pos2(top.right().max(row.right()) + m, row.bottom() + m),
                            ),
                            egui::CornerRadius {
                                nw: 0,
                                ne: 0,
                                sw: 8,
                                se: 8,
                            },
                            Color32::from_black_alpha(if p.dark { 80 } else { 20 }),
                        ),
                    );
                }
                    });
            });
        if let Some((uid, at)) = outline_picker {
            // The thread picker, for the outline alone, above the bar
            // (open for the shape, it becomes the outline's).
            if self.picker.is_some() && self.picker_for == Some(uid) {
                self.close_picker();
            } else {
                let at = egui::pos2(at.x, (at.y - 480.0).max(canvas.top() + space::M));
                self.open_picker(at, Some(uid));
            }
        }
        if outline_removed {
            self.remove_outline_selected();
        }
        match outline {
            // None yet: one is added, with its row open to set it up in.
            Some(false) => match self.set_outline(&idx, true) {
                Ok(_) => {
                    self.status = "Outline on: a running stitch round the edge. Its stitch, thread and width are in the bar's second row.".into();
                }
                Err(e) => self.status = format!("Couldn't add an outline: {e:#}"),
            },
            // One shape with an outline: its row opens or closes. The
            // outline stays as it is either way.
            Some(true) if single.is_some() => {
                self.layout.outline_row_closed = !self.layout.outline_row_closed;
                self.status = if self.layout.outline_row_closed {
                    "Outline row closed. The outline is still there: its button shows its thread."
                        .into()
                } else {
                    String::new()
                };
            }
            Some(true) => {
                self.status = "These all have outlines. Select one to set its outline up, or Object › Path › Remove Outline takes them all off.".into();
            }
            None => {}
        }
        if let Some(at) = picker_at {
            // The thread picker opens above the bar, for the shape (open
            // for its outline, it becomes the shape's).
            if self.picker.is_some() && self.picker_for.is_none() {
                self.close_picker();
            } else {
                let at = egui::pos2(at.x, (at.y - 440.0).max(canvas.top() + space::M));
                self.open_picker(at, None);
            }
        }
        if let Some((i, spec)) = text_edit {
            match self.update_lettering(i, &spec) {
                Ok(()) => {}
                Err(e) => self.status = format!("Couldn't update the text: {e:#}"),
            }
        }
        let place = &mut self.layout.task_bar_place;
        if moved != Vec2::ZERO {
            let to = keep_in(at + moved);
            if place.pinned {
                place.pin_at = Some(to - canvas.min);
            } else {
                place.offset = Some((doc, to - by_sel));
            }
            ctx.request_repaint();
        }
        match place_change {
            Some(PlaceChange::Pin(on)) => place.pinned = on,
            Some(PlaceChange::Reset) => *place = TaskBarPlace::default(),
            None => {}
        }
        if let Some(a) = act {
            if a == Action::ToggleTaskBar {
                self.status = "Task bar hidden. View › Contextual Task Bar shows it again.".into();
            }
            self.run_action(a);
        }
    }
}

/// One of the outline's stitch types, as its icon (lit when it's the one).
fn kind_button(
    ui: &mut egui::Ui,
    at: Rect,
    kind: zigzag::params::Kind,
    on: bool,
) -> egui::Response {
    let p = pal_ui(ui);
    let name = crate::widgets::kind_name(kind);
    let resp = ui.interact(
        at,
        ui.make_persistent_id(("outline-kind", name)),
        Sense::click(),
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, name));
    if on {
        let lit = if p.dark {
            Color32::from_rgb(70, 70, 74)
        } else {
            Color32::from_rgb(222, 222, 224)
        };
        ui.painter().rect_filled(at, 6.0, lit);
    } else if resp.hovered() {
        ui.painter().rect_filled(at, 6.0, p.hover);
    }
    crate::widgets::draw_kind_icon(
        ui.painter(),
        Rect::from_center_size(at.center(), Vec2::splat(16.0)),
        kind,
        if on { p.text } else { p.secondary },
    );
    resp.on_hover_text(format!("Outline: {name}"))
}

/// The bar's outline button. Without an outline it's the plain icon. With
/// one, the icon's outer line is drawn in the outline's thread (so it shows
/// that there is one, and which, when its row is closed), and it's lit
/// while the row is `open`.
fn outline_button(
    ui: &mut egui::Ui,
    on: bool,
    thread: Option<[u8; 3]>,
    open: bool,
) -> egui::Response {
    let p = pal_ui(ui);
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, "Outline"));
    if open {
        let lit = if p.dark {
            Color32::from_rgb(70, 70, 74)
        } else {
            Color32::from_rgb(222, 222, 224)
        };
        ui.painter().rect_filled(r, 6.0, lit);
    } else if resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(r, 6.0, p.pressed);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, 6.0, p.hover);
    }
    let icon = r.shrink(BUTTON * 0.22);
    match (on, thread) {
        (false, _) => crate::icons::paint(ui.painter(), icon, Icon::AddOutline, p.secondary),
        // (Several shapes, each with its own: no one thread to show.)
        (true, None) => crate::icons::paint(ui.painter(), icon, Icon::AddOutline, p.text),
        (true, Some(c)) => {
            // The icon, its outer line in the outline's thread: the shape…
            let side = icon.width().min(icon.height());
            let at = |x: f32, y: f32| icon.min + Vec2::new(x * side, y * side);
            ui.painter().rect_filled(
                Rect::from_min_max(at(0.27, 0.27), at(0.73, 0.73)),
                side * 0.08,
                p.secondary,
            );
            // …and the outline, a little heavier than a plain line so its
            // color reads, with a hairline round it for a thread as dark
            // as the bar.
            let ring = Rect::from_min_max(at(0.04, 0.04), at(0.96, 0.96));
            ui.painter().rect_stroke(
                ring,
                side * 0.22,
                Stroke::new(2.0, Color32::from_rgb(c[0], c[1], c[2])),
                egui::StrokeKind::Inside,
            );
            ui.painter().rect_stroke(
                ring,
                side * 0.22,
                Stroke::new(1.0, p.separator),
                egui::StrokeKind::Outside,
            );
        }
    }
    resp.on_hover_text(match (on, open) {
        (false, _) => "Outline: a line of stitches round the shape, in its own thread",
        (true, true) => "Outline: on (click to close its row; the outline stays)",
        (true, false) => "Outline: on (click to open its row)",
    })
}

/// A change to where the bar is, from its ••• menu.
enum PlaceChange {
    Pin(bool),
    Reset,
}

/// The grip at the bar's left end: drag it to move the bar. Returns how
/// far it was dragged this frame.
fn grip(ui: &mut egui::Ui) -> Vec2 {
    grip_for(ui, "Move Task Bar")
}

/// A bar's grip (see [`grip`]), named for what it moves.
pub(crate) fn grip_for(ui: &mut egui::Ui, label: &str) -> Vec2 {
    let p = pal_ui(ui);
    let (r, resp) = ui.allocate_exact_size(Vec2::new(GRIP_W, BUTTON), Sense::drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let active = resp.hovered() || resp.dragged();
    crate::icons::grip_dots(
        ui.painter(),
        r.center(),
        true,
        if active { p.secondary } else { p.tertiary },
    );
    if resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        resp.drag_delta()
    } else {
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        Vec2::ZERO
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
    fn the_grip_moves_the_task_bar_and_pinning_holds_it() {
        let path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg"));
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(path), None),
            );
        h.run_steps(4);
        let index = |h: &Harness<'_, Studio>, name: &str| {
            h.state().names.iter().position(|n| n == name).unwrap()
        };
        let select = |h: &mut Harness<'_, Studio>, i: usize| {
            h.state_mut().set_selection(&[i]);
            h.run_steps(3);
        };
        let bar = |h: &Harness<'_, Studio>| h.get_by_label("Move Task Bar").rect().min;
        let drag = |h: &mut Harness<'_, Studio>, by: Vec2| {
            let from = h.get_by_label("Move Task Bar").rect().center();
            h.hover_at(from);
            h.run_steps(1);
            h.drag_at(from);
            h.run_steps(1);
            for k in 1..=4 {
                h.hover_at(from + by * (k as f32 / 4.0));
                h.run_steps(1);
            }
            h.drop_at(from + by);
            h.run_steps(3);
        };
        let menu = |h: &mut Harness<'_, Studio>, item: &str| {
            h.get_by_label("Task bar options").click();
            h.run_steps(2);
            h.get_by_label(item).click();
            h.run_steps(3);
        };
        let near = |a: egui::Pos2, b: egui::Pos2| (a - b).length() < 1.0;
        let (sun, swoosh) = (index(&h, "sun"), index(&h, "swoosh"));
        select(&mut h, swoosh);
        let swoosh_by_sel = bar(&h);
        select(&mut h, sun);
        let by_sel = bar(&h);
        // Moved by its grip, and the selection stays as it was.
        let d = Vec2::new(-80.0, 30.0);
        drag(&mut h, d);
        assert!(
            near(bar(&h), by_sel + d),
            "{:?} vs {:?}",
            bar(&h),
            by_sel + d
        );
        assert_eq!(h.state().sel_indices(), vec![sun]);
        // It follows the selection, as far from its place by it as moved.
        select(&mut h, swoosh);
        assert!(near(bar(&h), swoosh_by_sel + d));
        // Pinned, it stays where it is whatever's selected, and the grip
        // still moves it.
        menu(&mut h, "Pin Bar Position");
        assert!(h.state().layout.task_bar_place.pinned);
        let pinned = bar(&h);
        select(&mut h, sun);
        assert!(near(bar(&h), pinned));
        drag(&mut h, Vec2::new(40.0, 0.0));
        select(&mut h, swoosh);
        assert!(near(bar(&h), pinned + Vec2::new(40.0, 0.0)));
        // Never off the canvas.
        drag(&mut h, Vec2::new(-3000.0, -3000.0));
        let canvas = h.state().canvas_rect;
        let top_left = h.get_by_label("Stitch type").rect().min;
        assert!(canvas.contains(top_left), "{top_left:?} in {canvas:?}");
        // Reset: by the selection again, and following it.
        menu(&mut h, "Reset Bar Position");
        assert_eq!(h.state().layout.task_bar_place, TaskBarPlace::default());
        assert!(near(bar(&h), swoosh_by_sel));
        select(&mut h, sun);
        assert!(near(bar(&h), by_sel));
    }

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
            .find(|n| (n.rect().top() - bar.top()).abs() <= 2.0)
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
        // An outline from the bar: the shape stays selected (its outline is
        // set up in its Stitch panel), and the button takes it off again,
        // keeping it for the next time it's switched on.
        let n = h.state().doc.shapes.len();
        let shape = h.state().sel_index().unwrap();
        let bar_outline = |h: &Harness<Studio>| {
            h.get_all_by_label("Outline")
                .find(|n| n.rect().top() >= bar.top() - 2.0 && n.rect().height() < 40.0)
                .expect("the bar's Outline")
                .rect()
                .center()
        };
        let click = |h: &mut Harness<Studio>, at: egui::Pos2| {
            h.event(egui::Event::PointerMoved(at));
            h.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
            h.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            });
            h.run_steps(3);
        };
        assert!(
            h.query_by_label("Outline thread").is_none(),
            "no outline, no row for it"
        );
        let at = bar_outline(&h);
        click(&mut h, at);
        assert_eq!(h.state().doc.shapes.len(), n + 1);
        assert_eq!(h.state().sel_index(), Some(shape));
        let outline = h.state().outline_index(shape).unwrap();
        assert_eq!(outline, shape + 1);
        assert_eq!(
            h.state().doc.shapes[outline].params.kind,
            Some(zigzag::params::Kind::Run)
        );
        assert!(h.state().outline_on(shape));
        // The bar has a second row for it now: how it's sewn…
        let main = h.get_by_label("Stitch type").rect();
        let thread = h.get_by_label("Outline thread").rect();
        assert!(thread.top() > main.bottom(), "{thread:?} under {main:?}");
        let was = h.state().doc.shapes[shape].params.kind;
        let at = h
            .get_all_by_label("Satin")
            .find(|n| (n.rect().center().y - thread.center().y).abs() <= 4.0)
            .expect("the outline bar's Satin")
            .rect()
            .center();
        click(&mut h, at);
        assert_eq!(
            h.state().doc.shapes[outline].params.kind,
            Some(zigzag::params::Kind::Satin)
        );
        assert_eq!(h.state().doc.shapes[shape].params.kind, was);
        assert!(h.query_by_label("Outline width").is_some());
        // …and its thread: the picker opened from it colors the outline,
        // not the selected shape (opened anywhere else, the selection).
        let at = h.get_by_label("Outline thread").rect().center();
        click(&mut h, at);
        assert!(h.state().picker.is_some());
        assert_eq!(h.state().picker_targets(), [outline]);
        assert!(h.query_by_label("Outline Thread Color").is_some());
        // (And never the outline again by accident: closed, then opened
        // from the shape's own swatch just above, which sits at the very
        // same place on screen, it's the shape's.)
        h.state_mut().close_picker();
        h.run_steps(2);
        assert_eq!(h.state().picker_for, None);
        let at = h
            .get_all_by_label("Thread color")
            .find(|n| (n.rect().center().y - main.center().y).abs() < 20.0)
            .expect("the shape's swatch")
            .rect()
            .center();
        click(&mut h, at);
        assert!(h.state().picker.is_some());
        assert_eq!(h.state().picker_targets(), [shape]);
        assert!(h.query_by_label("Outline Thread Color").is_none());
        // Open for the shape, the outline's swatch makes it the outline's.
        h.state_mut().open_picker(egui::pos2(300.0, 300.0), None);
        assert_eq!(h.state().picker_targets(), [shape]);
        let uid = h.state().doc.shapes[outline].uid;
        h.state_mut()
            .open_picker(egui::pos2(300.0, 300.0), Some(uid));
        assert_eq!(h.state().picker_targets(), [outline]);
        // With something else selected, it's that: not the outline still.
        let other = (0..h.state().doc.shapes.len())
            .find(|&i| i != shape && i != outline)
            .unwrap();
        h.state_mut().set_selection(&[other]);
        assert_eq!(h.state().picker_targets(), [other]);
        h.state_mut().set_selection(&[shape]);
        h.state_mut().close_picker();
        h.run_steps(2);
        // The button closes the outline's row and opens it again. Closed,
        // the outline is exactly as it was (there, sewn, its own thread),
        // and nothing was edited.
        h.state_mut().doc.shapes[outline].color = [1, 2, 3];
        h.state_mut().stable = h.state().doc.clone();
        let undo = h.state().undo.len();
        let at = bar_outline(&h);
        click(&mut h, at);
        assert!(h.state().layout.outline_row_closed);
        assert!(
            h.query_by_label("Outline thread").is_none(),
            "its row is closed"
        );
        assert!(h.state().outline_on(shape));
        assert_eq!(h.state().doc.shapes.len(), n + 1);
        assert_eq!(h.state().doc.shapes[outline].color, [1, 2, 3]);
        assert!(!h.state().doc.shapes[outline].params.hidden());
        assert!(h.state().design.per_shape[outline] > 0, "still sewn");
        assert_eq!(h.state().undo.len(), undo, "closing the row isn't an edit");
        let at = bar_outline(&h);
        click(&mut h, at);
        assert!(!h.state().layout.outline_row_closed);
        assert!(h.query_by_label("Outline thread").is_some());
        assert_eq!(h.state().sel_index(), Some(shape));
        // The struck-through outline in the row removes it (one undo brings it
        // back, as it was); the shape stays, and stays selected.
        let at = h.get_by_label("Remove outline").rect().center();
        click(&mut h, at);
        assert_eq!(h.state().doc.shapes.len(), n, "{}", h.state().status);
        assert!(!h.state().outline_on(shape) && h.state().outline_index(shape).is_none());
        assert_eq!(h.state().sel_index(), Some(shape));
        assert!(h.query_by_label("Outline thread").is_none());
        assert!(h.state_mut().apply_history(crate::app::History::Undo));
        h.run_steps(3);
        assert!(h.state().outline_on(shape));
        assert_eq!(h.state().doc.shapes[outline].color, [1, 2, 3]);
        assert_eq!(
            h.state().doc.shapes[outline].params.kind,
            Some(zigzag::params::Kind::Satin)
        );
        // Added while the row is closed (another shape's was), the row
        // opens: there's an outline to set up.
        let at = h.get_by_label("Remove outline").rect().center();
        click(&mut h, at);
        h.state_mut().layout.outline_row_closed = true;
        let at = bar_outline(&h);
        click(&mut h, at);
        assert!(h.state().outline_on(shape));
        assert!(!h.state().layout.outline_row_closed);
        assert!(h.query_by_label("Outline thread").is_some());
        let outline = h.state().outline_index(shape).unwrap();
        // The outline picked by itself (from its row in Layers, for the
        // rest of its stitch settings) has a bar of its own, and no
        // outline row: an outline has no outline.
        h.state_mut().set_selection(&[outline]);
        h.run_steps(3);
        assert_eq!(h.state().sel_index(), Some(outline));
        assert!(h.query_by_label("Stitch type").is_some());
        assert!(h.query_by_label("Outline thread").is_none());
        assert!(h.query_by_label("Remove outline").is_none());
        h.state_mut().set_selection(&[shape]);
        h.run_steps(3);
        // Hidden from the View menu's action, it stays hidden.
        h.state_mut().run_action(Action::ToggleTaskBar);
        h.run_steps(2);
        assert!(h.query_by_label("Stitch type").is_none());
        h.state_mut().run_action(Action::ToggleTaskBar);
        h.run_steps(2);
        assert!(h.query_by_label("Stitch type").is_some());
    }
    #[test]
    fn a_shape_picked_with_its_outline_is_still_the_shape() {
        use zigzag::params::Kind;
        // Each shape in a group of its own, as Illustrator (and agents)
        // often write them.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="30mm" viewBox="0 0 60 30">
            <g id="L01-patch"><rect id="patch" x="5" y="5" width="20" height="12" fill="#c00000"/></g>
            <g id="L02-dot"><circle id="dot" cx="45" cy="12" r="6" fill="#0000c0"/></g>
        </svg>"##;
        let doc = zigzag::document::Document::from_svg(svg, "t", None).unwrap();
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.state_mut().set_document(doc);
        h.run_steps(4);
        let patch = h.state().names.iter().position(|n| n == "patch").unwrap();
        h.state_mut().set_selection(&[patch]);
        h.state_mut()
            .run_action(crate::commands::Action::AddOutline);
        h.run_steps(3);
        let outline = h.state().outline_index(patch).unwrap();
        // Its group is still a group of one: a click picks the shape (or,
        // on its edge, the outline), not the two as a group.
        assert_eq!(h.state().click_group(patch), None);
        h.state_mut().click_select(patch, false, false);
        assert_eq!(h.state().sel_indices(), [patch]);
        h.state_mut().click_select(outline, false, false);
        assert_eq!(h.state().sel_indices(), [outline]);
        // On the canvas the outline is part of the shape: a click on its
        // stitches (here, on the patch's top edge) picks the shape, and so
        // does pointing at it. Hidden or locked, the shape lets the click
        // through to the outline, which is then all there is to pick.
        let edge = {
            let b = crate::app::shape_bounds(&h.state().doc.shapes[patch]);
            zigzag::geom::P::new((b.min.x + b.max.x) / 2.0, b.min.y - 6.0)
        };
        assert_eq!(
            crate::canvas::hit_test(&h.state().doc.shapes, edge, 2.0),
            Some(outline),
            "the outline is what's there"
        );
        assert_eq!(h.state().pick(edge, 2.0), Some(patch));
        assert_eq!(h.state().shape_of(outline), patch);
        assert_eq!(h.state().shape_of(patch), patch);
        h.state_mut().doc.shapes[patch].params.lock = Some(true);
        assert_eq!(h.state().pick(edge, 2.0), Some(outline));
        h.state_mut().doc.shapes[patch].params.lock = None;
        // Both caught in one selection (a box dragged round the shape):
        // the bar is the shape's, with the outline's row, not a mix.
        h.state_mut().set_selection(&[patch, outline]);
        h.run_steps(3);
        assert_eq!(h.state().subjects(), [patch]);
        assert!(h.query_by_label("Outline thread").is_some());
        assert!(h.query_by_label("Thread color: several").is_none());
        let in_bar = |h: &Harness<Studio>, label: &str| {
            let bar = h.get_by_label("Stitch type").rect();
            h.query_all_by_label(label)
                .any(|n| (n.rect().center().y - bar.center().y).abs() < 20.0)
        };
        assert!(!in_bar(&h, "Unite"), "one shape: nothing to unite");
        assert_eq!(
            h.state().selection_thread(),
            crate::threads::SelThread::One([0xc0, 0, 0])
        );
        assert_eq!(h.state().picker_targets(), [patch]);
        // The stitch type set is the shape's; the outline keeps its own.
        h.state_mut().set_kind_selected(Kind::Satin);
        assert_eq!(h.state().doc.shapes[patch].params.kind, Some(Kind::Satin));
        assert_eq!(h.state().doc.shapes[outline].params.kind, Some(Kind::Run));
        // With another shape as well, it's several objects as before.
        let dot = h.state().names.iter().position(|n| n == "dot").unwrap();
        h.state_mut().set_selection(&[patch, outline, dot]);
        h.run_steps(3);
        assert_eq!(h.state().subjects(), [patch, dot]);
        assert!(h.query_by_label("Outline thread").is_none());
        assert!(in_bar(&h, "Unite"));
    }
    #[test]
    fn showing_a_wide_outline_changes_nothing_and_uniting_leaves_outlines_out() {
        use zigzag::params::Kind;
        use zigzag::svg::Geometry;
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40">
            <rect id="a" x="5" y="5" width="20" height="20" fill="#0000ff"/>
            <rect id="b" x="15" y="10" width="20" height="20" fill="#00a000"/>
            <rect id="wide" x="50" y="5" width="20" height="20" fill="#c00000" stroke="#111111" stroke-width="12"/>
        </svg>"##;
        let doc = zigzag::document::Document::from_svg(svg, "t", None).unwrap();
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1500.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.state_mut().set_document(doc);
        h.run_steps(4);
        let of = |h: &Harness<Studio>, name: &str| {
            h.state().names.iter().position(|n| n == name).unwrap()
        };
        // Artwork's own outline, 12 mm wide: wider than the width field
        // goes. Selecting the shape (the field shows in the bar), or the
        // outline itself (it shows in the Stitch panel), leaves it 12 mm:
        // no edit, nothing to undo, nothing to save.
        let wide = of(&h, "wide");
        let outline = h.state().outline_index(wide).unwrap();
        let width = |h: &Harness<Studio>| match h.state().doc.shapes[outline].geometry {
            Geometry::Stroke { width, .. } => width,
            _ => unreachable!(),
        };
        assert_eq!(h.state().doc.shapes[outline].params.kind, Some(Kind::Satin));
        assert!((width(&h) - 120.0).abs() < 0.5);
        let undo = h.state().undo.len();
        for sel in [wide, outline, wide] {
            h.state_mut().set_selection(&[sel]);
            h.run_steps(6);
            assert_eq!(h.state().doc.shapes[outline].params.width, None);
            assert!((width(&h) - 120.0).abs() < 0.5);
        }
        assert!(h.query_by_label("Outline width").is_some());
        // One set by hand past the field's range (an agent can ask for up
        // to 20 mm) is left as set, too.
        h.state_mut().doc.shapes[outline].params.width = Some(14.0);
        h.state_mut().stable = h.state().doc.clone();
        for sel in [outline, wide] {
            h.state_mut().set_selection(&[sel]);
            h.run_steps(6);
            assert_eq!(h.state().doc.shapes[outline].params.width, Some(14.0));
        }
        assert_eq!(h.state().undo.len(), undo, "nothing was edited");
        assert!(!h.state().modified);

        // Two overlapping shapes, each with an outline, all four selected
        // (a box dragged round them): Unite combines the two shapes, as
        // big as the two and in the front one's thread, and their old
        // outlines go.
        let (a, b) = (of(&h, "a"), of(&h, "b"));
        h.state_mut().set_selection(&[a, b]);
        h.state_mut()
            .run_action(crate::commands::Action::AddOutline);
        h.run_steps(3);
        let (a, b) = (of(&h, "a"), of(&h, "b"));
        let (oa, ob) = (
            h.state().outline_index(a).unwrap(),
            h.state().outline_index(b).unwrap(),
        );
        let n = h.state().doc.shapes.len();
        h.state_mut().set_selection(&[a, oa, b, ob]);
        h.state_mut()
            .run_action(crate::commands::Action::Pathfinder(Pathfinder::Unite));
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), n - 3, "{}", h.state().status);
        let new = h.state().sel_index().unwrap();
        let s = &h.state().doc.shapes[new];
        assert_eq!(s.color, [0, 0xa0, 0], "the front shape's thread");
        let Geometry::Fill { rings, .. } = &s.geometry else {
            panic!("a filled shape");
        };
        let mut bounds = zigzag::geom::Bounds::empty();
        rings.iter().flatten().for_each(|&p| bounds.add(p));
        assert!(
            (bounds.width() - 300.0).abs() < 1.0 && (bounds.height() - 250.0).abs() < 1.0,
            "30 × 25 mm, not an outline's width more: {} × {}",
            bounds.width(),
            bounds.height()
        );
        assert!(
            h.state()
                .doc
                .shapes
                .iter()
                .filter(|s| s.outline)
                .all(|s| s.id.starts_with("wide"))
        );
    }
}
