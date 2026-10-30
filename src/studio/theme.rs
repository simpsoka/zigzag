//! A macOS-flavoured look for the studio: system font, light and dark
//! palettes that follow the system appearance, accent blue, rounded controls,
//! and a few helpers for Mac-style forms (right-aligned labels, grouped
//! sections, segmented controls).

use crate::icons::{self, Icon};
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Pos2, Rect,
    Response, Sense, Shadow, Stroke, TextStyle, Theme, Vec2, Visuals,
};
use std::sync::Arc;

/// Colors for custom painting, matching the active appearance.
#[derive(Clone, Copy, Debug)]
pub struct Pal {
    pub dark: bool,
    /// Title bar / toolbar.
    pub toolbar: Color32,
    /// Docks and the tool strip.
    pub dock: Color32,
    /// Panel headers inside docks.
    pub header: Color32,
    /// Around the hoop.
    pub desk: Color32,
    pub separator: Color32,
    pub text: Color32,
    pub secondary: Color32,
    pub tertiary: Color32,
    pub accent: Color32,
    /// Hover wash for rows and icon buttons.
    pub hover: Color32,
    /// Pressed / selected-but-unfocused wash.
    pub pressed: Color32,
    pub control: Color32,
    pub control_border: Color32,
    pub warning: Color32,
    pub error: Color32,
    /// Floating HUD capsules on the canvas.
    pub hud: Color32,
}

pub const LIGHT: Pal = Pal {
    dark: false,
    toolbar: Color32::from_rgb(246, 246, 246),
    dock: Color32::from_rgb(242, 242, 242),
    header: Color32::from_rgb(234, 234, 234),
    desk: Color32::from_rgb(222, 222, 222),
    separator: Color32::from_rgb(214, 214, 214),
    text: Color32::from_rgb(29, 29, 31),
    secondary: Color32::from_rgb(110, 110, 115),
    tertiary: Color32::from_rgb(160, 160, 165),
    accent: Color32::from_rgb(0, 122, 255),
    hover: Color32::from_rgba_premultiplied(0, 0, 0, 13),
    pressed: Color32::from_rgba_premultiplied(0, 0, 0, 26),
    control: Color32::WHITE,
    control_border: Color32::from_rgb(204, 204, 204),
    warning: Color32::from_rgb(255, 149, 0),
    error: Color32::from_rgb(255, 59, 48),
    hud: Color32::from_rgba_premultiplied(30, 30, 30, 215),
};

pub const DARK: Pal = Pal {
    dark: true,
    toolbar: Color32::from_rgb(44, 44, 46),
    dock: Color32::from_rgb(36, 36, 38),
    header: Color32::from_rgb(44, 44, 46),
    desk: Color32::from_rgb(24, 24, 26),
    separator: Color32::from_rgb(58, 58, 60),
    text: Color32::from_rgb(235, 235, 240),
    secondary: Color32::from_rgb(152, 152, 157),
    tertiary: Color32::from_rgb(99, 99, 102),
    accent: Color32::from_rgb(10, 132, 255),
    hover: Color32::from_rgba_premultiplied(16, 16, 16, 16),
    pressed: Color32::from_rgba_premultiplied(28, 28, 28, 28),
    control: Color32::from_rgb(64, 64, 68),
    control_border: Color32::from_rgb(78, 78, 82),
    warning: Color32::from_rgb(255, 159, 10),
    error: Color32::from_rgb(255, 69, 58),
    hud: Color32::from_rgba_premultiplied(20, 20, 22, 225),
};

pub fn pal(ctx: &egui::Context) -> Pal {
    if ctx.theme() == Theme::Dark {
        DARK
    } else {
        LIGHT
    }
}

pub fn pal_ui(ui: &egui::Ui) -> Pal {
    if ui.visuals().dark_mode { DARK } else { LIGHT }
}

/// System UI font: San Francisco on macOS, else Inter or another clean sans.
fn system_font() -> Option<(String, Vec<u8>, u32)> {
    for path in [
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/SFNSText.ttf",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            return Some(("system-ui".into(), bytes, 0));
        }
    }
    let db = zigzag::svg::fontdb();
    for family in [
        "Inter",
        "Cantarell",
        "Helvetica Neue",
        "Helvetica",
        "Arial",
        "DejaVu Sans",
    ] {
        let query = usvg::fontdb::Query {
            families: &[usvg::fontdb::Family::Name(family)],
            ..Default::default()
        };
        if let Some(id) = db.query(&query)
            && let Some((bytes, index)) = db.with_face_data(id, |d, i| (d.to_vec(), i))
        {
            return Some(("system-ui".into(), bytes, index));
        }
    }
    None
}

fn visuals(p: &Pal) -> Visuals {
    let mut v = if p.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let r = CornerRadius::same(6);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.dock;
    w.noninteractive.weak_bg_fill = p.dock;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.separator);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = p.control;
    w.inactive.weak_bg_fill = p.control;
    w.inactive.bg_stroke = Stroke::new(1.0, p.control_border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;
    let hover_bg = if p.dark {
        Color32::from_rgb(76, 76, 80)
    } else {
        Color32::from_rgb(247, 247, 247)
    };
    let active_bg = if p.dark {
        Color32::from_rgb(90, 90, 95)
    } else {
        Color32::from_rgb(232, 232, 232)
    };
    w.hovered.bg_fill = hover_bg;
    w.hovered.weak_bg_fill = hover_bg;
    w.hovered.bg_stroke = Stroke::new(1.0, p.control_border);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = active_bg;
    w.active.weak_bg_fill = active_bg;
    w.active.bg_stroke = Stroke::new(1.0, p.control_border);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;
    w.open = w.hovered;
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = p.accent;
    v.faint_bg_color = p.header;
    v.extreme_bg_color = if p.dark {
        Color32::from_rgb(28, 28, 30)
    } else {
        Color32::WHITE
    };
    v.text_edit_bg_color = Some(v.extreme_bg_color);
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.error;
    v.panel_fill = p.dock;
    v.window_fill = if p.dark {
        Color32::from_rgb(40, 40, 42)
    } else {
        Color32::from_rgb(250, 250, 250)
    };
    v.window_stroke = Stroke::new(
        1.0,
        if p.dark {
            Color32::from_rgb(70, 70, 74)
        } else {
            Color32::from_rgb(208, 208, 208)
        },
    );
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    let shadow = Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 110 } else { 45 }),
    };
    v.window_shadow = shadow;
    v.popup_shadow = shadow;
    v.slider_trailing_fill = true;
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;
    v.striped = false;
    v.weak_text_color = Some(p.secondary);
    v
}

fn style_for(p: &Pal, base: &egui::Style) -> egui::Style {
    let mut s = base.clone();
    s.visuals = visuals(p);
    s.text_styles = [
        (TextStyle::Heading, FontId::proportional(15.0)),
        (TextStyle::Body, FontId::proportional(13.0)),
        (TextStyle::Button, FontId::proportional(13.0)),
        (TextStyle::Small, FontId::proportional(11.0)),
        (TextStyle::Monospace, FontId::monospace(12.0)),
    ]
    .into();
    let sp = &mut s.spacing;
    sp.item_spacing = Vec2::new(space::S, space::S);
    sp.button_padding = Vec2::new(10.0, 3.0);
    sp.interact_size = Vec2::new(24.0, 22.0);
    sp.slider_width = 120.0;
    sp.combo_width = 140.0;
    sp.menu_margin = Margin::same(space::XS as i8 + 2);
    sp.window_margin = pad();
    sp.icon_width = 14.0;
    sp.icon_width_inner = 8.0;
    sp.scroll.floating = true;
    sp.scroll.bar_width = 7.0;
    s.interaction.tooltip_delay = 0.6;
    s
}

/// Fonts and both appearances; the system setting picks between them.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    if let Some((name, bytes, index)) = system_font() {
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        fonts.font_data.insert(name.clone(), Arc::new(data));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, name);
    }
    ctx.set_fonts(fonts);
    let base = (*ctx.global_style()).clone();
    ctx.set_style_of(Theme::Light, style_for(&LIGHT, &base));
    ctx.set_style_of(Theme::Dark, style_for(&DARK, &base));
    // Follow the system appearance; ZIGZAG_THEME=light|dark overrides it
    // (THREADLINE_THEME still works, from before the rename).
    let forced = std::env::var("ZIGZAG_THEME").or_else(|_| std::env::var("THREADLINE_THEME"));
    ctx.set_theme(match forced.as_deref() {
        Ok("light") => egui::ThemePreference::Light,
        Ok("dark") => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    });
}

// ---------------------------------------------------------------- spacing

/// The spacing scale (points). Every gap and margin in the app is one of
/// these, so things line up the same way everywhere.
pub mod space {
    /// Between tightly related things: swatches, icons in a row, a label
    /// and its caption.
    pub const XS: f32 = 4.0;
    /// Between items: form rows, buttons, list entries.
    pub const S: f32 = 8.0;
    /// Padding inside panels, popovers and windows; between sections.
    pub const M: f32 = 12.0;
}

/// Height of a row in a list (objects, threads, sewing order).
pub const ROW_H: f32 = 24.0;
/// A color swatch in a list row.
pub const LIST_SWATCH: f32 = 16.0;

/// Inner margin for panels, popovers and windows.
pub fn pad() -> Margin {
    Margin::same(space::M as i8)
}

/// A window's title bar: the window's own frame, but only a little taller
/// than the title (egui's default pads it as much as the window's content).
pub fn title_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::window(&ctx.global_style()).inner_margin(Margin::symmetric(space::M as i8, 6))
}

/// A gap between sections: space, a hairline, space.
pub fn section_break(ui: &mut egui::Ui) {
    ui.add_space(space::M - ui.spacing().item_spacing.y);
    hline(ui);
    ui.add_space(space::M - ui.spacing().item_spacing.y);
}

// ---------------------------------------------------------------- widgets

/// Secondary-colored small caption (section titles, units, hints).
pub fn caption(ui: &mut egui::Ui, text: impl Into<String>) -> Response {
    let p = pal_ui(ui);
    ui.label(
        egui::RichText::new(text.into())
            .size(11.0)
            .color(p.secondary),
    )
}

/// A two-column form: labels right-aligned in a fixed column, controls after.
pub fn form(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut Form)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([space::S, space::S])
        .min_col_width(LABEL_W)
        .show(ui, |ui| {
            let mut f = Form { ui };
            add(&mut f);
        });
}

pub const LABEL_W: f32 = 84.0;

pub struct Form<'a> {
    pub ui: &'a mut egui::Ui,
}

impl Form<'_> {
    /// One row: a right-aligned label and whatever `add` puts next to it.
    /// Returns the label's id so controls can be `labelled_by` it.
    pub fn row<R>(&mut self, label: &str, add: impl FnOnce(&mut egui::Ui, egui::Id) -> R) -> R {
        let p = pal_ui(self.ui);
        let l = self
            .ui
            .allocate_ui_with_layout(
                Vec2::new(LABEL_W, 22.0),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| ui.label(egui::RichText::new(label).color(p.secondary)),
            )
            .inner;
        let r = self.ui.horizontal(|ui| add(ui, l.id)).inner;
        self.ui.end_row();
        r
    }

    /// A full-width row without a label (notes, checkbox groups).
    pub fn wide(&mut self, add: impl FnOnce(&mut egui::Ui)) {
        self.ui.label("");
        self.ui.horizontal_wrapped(add);
        self.ui.end_row();
    }
}

/// Slider plus an editable value field, for a setting that falls back to a
/// default until changed. Returns the slider's response.
#[allow(clippy::too_many_arguments)]
pub fn value_row(
    f: &mut Form,
    label: &str,
    value: &mut Option<f64>,
    default: f64,
    range: std::ops::RangeInclusive<f64>,
    suffix: &str,
    step: f64,
    tip: &str,
) -> Response {
    f.row(label, |ui, lid| {
        let shown = value.unwrap_or((default / step).round() * step);
        let mut v = shown;
        ui.spacing_mut().slider_width = 100.0;
        let r = ui
            .add(
                egui::Slider::new(&mut v, range.clone())
                    .show_value(false)
                    .step_by(step),
            )
            .labelled_by(lid);
        let r = if tip.is_empty() {
            r
        } else {
            r.on_hover_text(tip)
        };
        let decimals = if step >= 1.0 {
            0
        } else if step >= 0.1 {
            1
        } else {
            2
        };
        let field = ui.add(
            egui::DragValue::new(&mut v)
                .range(range)
                .speed(step)
                .fixed_decimals(decimals)
                .suffix(suffix),
        );
        if (r.changed() || field.changed()) && v != shown {
            *value = Some(v);
        }
        r
    })
}

/// A borderless square icon button (toolbar / tool strip style).
pub fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    selected: bool,
    size: f32,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, label)
    });
    let p = pal_ui(ui);
    let enabled = ui.is_enabled();
    let bg = if selected {
        Some(if p.dark {
            Color32::from_rgb(70, 70, 74)
        } else {
            Color32::from_rgb(222, 222, 224)
        })
    } else if resp.is_pointer_button_down_on() {
        Some(p.pressed)
    } else if resp.hovered() && enabled {
        Some(p.hover)
    } else {
        None
    };
    if let Some(bg) = bg {
        ui.painter().rect_filled(rect, 6.0, bg);
    }
    let c = if !enabled {
        p.tertiary
    } else if selected {
        p.accent
    } else {
        p.secondary
    };
    icons::paint(ui.painter(), rect.shrink(size * 0.22), icon, c);
    resp.on_hover_text(label)
}

/// Mac segmented control. Returns true when the value changed.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    let p = pal_ui(ui);
    let font = FontId::proportional(12.0);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, l)| {
            ui.painter()
                .layout_no_wrap(l.to_string(), font.clone(), p.text)
                .size()
                .x
                + 22.0
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, 24.0), Sense::hover());
    let track = if p.dark {
        Color32::from_rgb(28, 28, 30)
    } else {
        Color32::from_rgb(227, 227, 229)
    };
    ui.painter().rect_filled(rect, 7.0, track);
    let mut x = rect.left() + 2.0;
    let mut changed = false;
    for (k, ((v, label), w)) in options.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(
            Pos2::new(x, rect.top() + 2.0),
            Vec2::new(*w, rect.height() - 4.0),
        );
        let resp = ui.interact(r, ui.make_persistent_id((id, k)), Sense::click());
        let on = *value == *v;
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, on, *label));
        if on {
            let sel = if p.dark {
                Color32::from_rgb(99, 99, 102)
            } else {
                Color32::WHITE
            };
            ui.painter().rect_filled(r, 5.0, sel);
            if !p.dark {
                ui.painter().rect_stroke(
                    r,
                    5.0,
                    Stroke::new(0.5, Color32::from_black_alpha(30)),
                    egui::StrokeKind::Inside,
                );
            }
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 5.0, p.hover);
        }
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            font.clone(),
            p.text,
        );
        if resp.clicked() && !on {
            *value = *v;
            changed = true;
        }
        x += w;
    }
    changed
}

/// A thin horizontal separator line.
pub fn hline(ui: &mut egui::Ui) {
    let p = pal_ui(ui);
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(r.x_range(), r.center().y, Stroke::new(1.0, p.separator));
}

/// Rounded "callout" box (tips, issues inside a panel).
pub fn callout(ui: &mut egui::Ui, color: Color32, icon: Icon, text: &str) {
    let p = pal_ui(ui);
    egui::Frame::new()
        .fill(color.gamma_multiply(if p.dark { 0.18 } else { 0.12 }))
        .corner_radius(6)
        .inner_margin(Margin::symmetric(space::S as i8, space::XS as i8 + 2))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                icons::paint(ui.painter(), r, icon, color);
                ui.add(egui::Label::new(egui::RichText::new(text).size(12.0).color(p.text)).wrap());
            });
        });
}
