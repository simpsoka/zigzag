//! Dialogs, after VectorCraft's: no title bar, a heading in the window's
//! top band, dimmed body text, and one row of buttons at the bottom right
//! (the default in the accent color, the others quiet; a destructive
//! choice like Don't Save set apart to the left of Cancel). Every modal
//! dialog in the studio is drawn with these, so they all look alike.

use crate::theme::{pal_ui, space};
use eframe::egui::{self, Color32, CornerRadius, Response, Sense, Vec2};

/// Inside the dialog's frame.
const MARGIN: i8 = 22;
/// Between the heading and the body, and the body and the buttons: the
/// same, so the dialog's three parts sit evenly.
const GAP_BELOW_HEADING: f32 = 16.0;
const GAP_ABOVE_BUTTONS: f32 = 16.0;
/// Buttons: height, least width, corner radius, label size.
const BUTTON_H: f32 = 28.0;
const BUTTON_MIN_W: f32 = 72.0;
const BUTTON_RADIUS: u8 = 6;
const BUTTON_TEXT: f32 = 13.0;
/// Between buttons, and before one set apart (Don't Save).
const BUTTON_GAP: f32 = space::S;
const APART_GAP: f32 = 28.0;
/// How wide a dialog's text may run before it wraps.
pub const MAX_W: f32 = 420.0;
pub const MIN_W: f32 = 320.0;

/// Where a dialog opens.
#[derive(Clone, Copy)]
pub enum Place {
    /// The middle of the window, the rest of the app dimmed: a question
    /// that has to be answered (Save Changes).
    Center,
    /// Near the top, nothing dimmed: a tool with a live preview on the
    /// canvas that you want to see (Round Corners, Simplify).
    Top,
}

/// A modal dialog with the studio's look; `add` draws its contents (start
/// with [`heading`]).
pub fn show<R>(
    ctx: &egui::Context,
    id: egui::Id,
    place: Place,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::ModalResponse<R> {
    let p = crate::theme::pal(ctx);
    let frame = egui::Frame::window(&ctx.global_style())
        .fill(p.toolbar)
        .stroke(egui::Stroke::new(1.0, p.separator))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(MARGIN));
    let modal = egui::Modal::new(id).frame(frame);
    let modal = match place {
        Place::Center => modal,
        Place::Top => modal.backdrop_color(Color32::TRANSPARENT).area(
            egui::Modal::default_area(id).anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 90.0)),
        ),
    };
    modal.show(ctx, |ui| {
        ui.set_min_width(MIN_W);
        ui.set_max_width(MAX_W);
        add(ui)
    })
}

/// The dialog's heading (wraps when long).
pub fn heading(ui: &mut egui::Ui, text: &str) {
    let p = pal_ui(ui);
    ui.add(
        egui::Label::new(egui::RichText::new(text).size(15.0).strong().color(p.text))
            .wrap()
            .selectable(false),
    );
    ui.add_space(GAP_BELOW_HEADING - ui.spacing().item_spacing.y);
}

/// A line of explanation under the heading, dimmed.
pub fn body(ui: &mut egui::Ui, text: &str) {
    let p = pal_ui(ui);
    ui.add(egui::Label::new(egui::RichText::new(text).size(13.0).color(p.secondary)).wrap());
}

/// The row of buttons along the bottom, right to left: add the default
/// button first, then Cancel, then any set apart with [`apart`].
pub fn buttons<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.add_space(GAP_ABOVE_BUTTONS - ui.spacing().item_spacing.y);
    // As wide as what's above it and as tall as the buttons (a right-to-left
    // layout would otherwise take all the room there is, and the dialog
    // could never be narrower than its widest allowed size).
    let row = Vec2::new(ui.min_rect().width().max(MIN_W), BUTTON_H);
    ui.allocate_ui_with_layout(
        row,
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = BUTTON_GAP;
            add(ui)
        },
    )
    .inner
}

/// Space before a button set apart from the rest (Don't Save).
pub fn apart(ui: &mut egui::Ui) {
    ui.add_space(APART_GAP - BUTTON_GAP);
}

/// The default button (Enter): the accent color.
pub fn primary(ui: &mut egui::Ui, label: &str, enabled: bool) -> Response {
    let p = pal_ui(ui);
    let fg = if enabled { Color32::WHITE } else { p.tertiary };
    let (rect, resp, galley) = allocate(ui, label, fg, enabled);
    let bg = match (enabled, resp.hovered(), resp.is_pointer_button_down_on()) {
        (false, ..) => p.control,
        (true, _, true) => p.accent.gamma_multiply(0.8),
        (true, true, false) => p.accent.gamma_multiply(1.1),
        (true, false, false) => p.accent,
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(BUTTON_RADIUS), bg);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    resp
}

/// Any other button: filled quietly, no border.
pub fn secondary(ui: &mut egui::Ui, label: &str) -> Response {
    let p = pal_ui(ui);
    let (rect, resp, galley) = allocate(ui, label, p.text, true);
    let r = CornerRadius::same(BUTTON_RADIUS);
    ui.painter().rect_filled(rect, r, p.control);
    if resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, r, p.pressed);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, r, p.hover);
    }
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, p.text);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    resp
}

fn allocate(
    ui: &mut egui::Ui,
    label: &str,
    color: Color32,
    enabled: bool,
) -> (egui::Rect, Response, std::sync::Arc<egui::Galley>) {
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(BUTTON_TEXT),
        color,
    );
    let w = (galley.size().x + 32.0).max(BUTTON_MIN_W);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, BUTTON_H), sense);
    (rect, resp, galley)
}
