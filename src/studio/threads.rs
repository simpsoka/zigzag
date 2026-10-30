//! The Threads panel: the design's threads in sewing order (with its color
//! changes and trims) or once each by color (edit one and every object in
//! that thread follows), and real thread charts to pick from.

use crate::app::Studio;
use crate::theme::{caption, pal_ui};
use crate::widgets::fmt_int;
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use zigzag::formats::jef_data::JEF_PALETTE;
use zigzag::formats::pec_data::PEC_PALETTE;
use zigzag::params::Kind;

/// Thread charts we have real data for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Brand {
    #[default]
    Brother,
    Janome,
}

/// A chart's threads: (color, name, catalog number if known).
pub fn chart(b: Brand) -> Vec<([u8; 3], &'static str, Option<&'static str>)> {
    match b {
        Brand::Brother => PEC_PALETTE
            .iter()
            .skip(1)
            .map(|&(r, g, bl, n)| ([r, g, bl], n, brother_number([r, g, bl])))
            .collect(),
        Brand::Janome => JEF_PALETTE
            .iter()
            .skip(1)
            .map(|&(r, g, bl, n, k)| ([r, g, bl], n, Some(k)))
            .collect(),
    }
}

/// Brother Embroidery thread numbers by color: the chart Artspira uses
/// (via Respira's BrotherColor.json, Apache-2.0, Copyright Jan-Henrik
/// Bruhn). The PEC palette's colors are these threads.
const BROTHER_NUMBERS: [([u8; 3], &str); 61] = [
    ([240, 240, 240], "001"),
    ([239, 227, 185], "843"),
    ([255, 255, 179], "010"),
    ([227, 243, 91], "027"),
    ([168, 221, 196], "542"),
    ([168, 222, 235], "017"),
    ([178, 175, 212], "804"),
    ([253, 217, 222], "124"),
    ([252, 187, 196], "079"),
    ([216, 204, 198], "399"),
    ([254, 227, 197], "307"),
    ([168, 168, 168], "005"),
    ([255, 240, 141], "812"),
    ([240, 249, 112], "202"),
    ([158, 214, 125], "502"),
    ([102, 186, 73], "509"),
    ([37, 132, 187], "019"),
    ([145, 95, 172], "612"),
    ([228, 154, 203], "810"),
    ([249, 147, 188], "085"),
    ([246, 74, 138], "086"),
    ([208, 166, 96], "348"),
    ([135, 135, 135], "817"),
    ([254, 179, 67], "126"),
    ([255, 255, 0], "205"),
    ([112, 188, 31], "513"),
    ([0, 135, 119], "534"),
    ([9, 91, 166], "420"),
    ([104, 106, 176], "607"),
    ([145, 54, 151], "620"),
    ([247, 56, 102], "807"),
    ([209, 84, 0], "339"),
    ([186, 152, 0], "328"),
    ([79, 85, 86], "704"),
    ([254, 158, 50], "209"),
    ([255, 217, 17], "206"),
    ([47, 126, 32], "515"),
    ([0, 103, 62], "507"),
    ([10, 85, 163], "405"),
    ([75, 107, 175], "070"),
    ([119, 1, 118], "869"),
    ([199, 1, 86], "107"),
    ([254, 55, 15], "030"),
    ([125, 111, 0], "330"),
    ([41, 49, 51], "707"),
    ([232, 169, 0], "214"),
    ([254, 186, 53], "208"),
    ([67, 86, 7], "517"),
    ([19, 74, 70], "415"),
    ([11, 61, 145], "406"),
    ([78, 41, 144], "614"),
    ([106, 28, 138], "613"),
    ([181, 76, 100], "333"),
    ([237, 23, 31], "800"),
    ([209, 92, 0], "337"),
    ([0, 0, 0], "900"),
    ([42, 19, 1], "058"),
    ([178, 118, 36], "323"),
    ([19, 43, 26], "519"),
    ([0, 56, 34], "808"),
    ([14, 31, 124], "007"),
];

/// Brother Embroidery thread names by number, as Brother prints them
/// (Brother's "Thread Color List (BROTHER EMBROIDERY)" for Artspira).
const BROTHER_NAMES: [(&str, &str); 61] = [
    ("001", "White"),
    ("005", "Silver"),
    ("007", "Prussian Blue"),
    ("010", "Cream Brown"),
    ("017", "Light Blue"),
    ("019", "Sky Blue"),
    ("027", "Fresh Green"),
    ("030", "Vermilion"),
    ("058", "Dark Brown"),
    ("070", "Cornflower Blue"),
    ("079", "Salmon Pink"),
    ("085", "Pink"),
    ("086", "Deep Rose"),
    ("107", "Dark Fuchsia"),
    ("124", "Flesh Pink"),
    ("126", "Pumpkin"),
    ("202", "Lemon Yellow"),
    ("205", "Yellow"),
    ("206", "Harvest Gold"),
    ("208", "Orange"),
    ("209", "Tangerine"),
    ("214", "Deep Gold"),
    ("307", "Linen"),
    ("323", "Light Brown"),
    ("328", "Brass"),
    ("330", "Russet Brown"),
    ("333", "Amber Red"),
    ("337", "Reddish Brown"),
    ("339", "Clay Brown"),
    ("348", "Khaki"),
    ("399", "Warm Gray"),
    ("405", "Blue"),
    ("406", "Ultra Marine"),
    ("415", "Peacock Blue"),
    ("420", "Electric Blue"),
    ("502", "Mint Green"),
    ("507", "Emerald Green"),
    ("509", "Leaf Green"),
    ("513", "Lime Green"),
    ("515", "Moss Green"),
    ("517", "Dark Olive"),
    ("519", "Olive Green"),
    ("534", "Teal Green"),
    ("542", "Seacrest"),
    ("607", "Wisteria Violet"),
    ("612", "Lilac"),
    ("613", "Violet"),
    ("614", "Purple"),
    ("620", "Magenta"),
    ("704", "Pewter"),
    ("707", "Dark Gray"),
    ("800", "Red"),
    ("804", "Lavender"),
    ("807", "Carmine"),
    ("808", "Deep Green"),
    ("810", "Light Lilac"),
    ("812", "Cream Yellow"),
    ("817", "Gray"),
    ("843", "Beige"),
    ("869", "Royal Purple"),
    ("900", "Black"),
];

/// The Brother Embroidery thread number for a chart color. (A step of
/// rounding apart counts: the PEC chart has Salmon Pink and Amber Red one
/// unit off Brother's own colors for them.)
pub fn brother_number(c: [u8; 3]) -> Option<&'static str> {
    let close = |k: &[u8; 3]| (0..3).all(|i| k[i].abs_diff(c[i]) <= 1);
    BROTHER_NUMBERS
        .iter()
        .find(|(k, _)| *k == c)
        .or_else(|| BROTHER_NUMBERS.iter().find(|(k, _)| close(k)))
        .map(|(_, n)| *n)
}

/// A Brother Embroidery thread by number: its color and name.
pub fn brother_thread(number: &str) -> Option<([u8; 3], &'static str)> {
    let c = BROTHER_NUMBERS.iter().find(|(_, n)| *n == number)?.0;
    let name = BROTHER_NAMES.iter().find(|(n, _)| *n == number)?.1;
    Some((c, name))
}

/// The Brother Embroidery thread that is exactly this color, if any:
/// (name, number).
pub fn brother_exact(c: [u8; 3]) -> Option<(&'static str, &'static str)> {
    let n = BROTHER_NUMBERS.iter().find(|(k, _)| *k == c)?.1;
    Some((brother_thread(n)?.1, n))
}

/// Nearest thread in a chart.
pub fn nearest(b: Brand, c: [u8; 3]) -> ([u8; 3], &'static str, Option<&'static str>) {
    type Found = ([u8; 3], &'static str, Option<&'static str>);
    thread_local! {
        // Asked for every block every frame (the lists): remembered.
        static MEMO: std::cell::RefCell<std::collections::HashMap<(Brand, [u8; 3]), Found>> =
            Default::default();
    }
    if let Some(f) = MEMO.with(|m| m.borrow().get(&(b, c)).copied()) {
        return f;
    }
    let found = chart(b)
        .into_iter()
        .min_by(|(a, _, _), (b, _, _)| {
            zigzag::formats::perceptual_distance(c, *a)
                .total_cmp(&zigzag::formats::perceptual_distance(c, *b))
        })
        .expect("charts aren't empty");
    MEMO.with(|m| m.borrow_mut().insert((b, c), found));
    found
}

pub(crate) fn label(name: &str, number: Option<&str>) -> String {
    match number {
        Some(k) => format!("{name} ({k})"),
        None => name.to_string(),
    }
}

pub fn swatch(ui: &mut egui::Ui, c: [u8; 3], size: f32, selected: bool) -> egui::Response {
    let p = pal_ui(ui);
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(r, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
    let border = if selected {
        Stroke::new(2.0, p.accent)
    } else {
        Stroke::new(1.0, Color32::from_black_alpha(45))
    };
    painter.rect_stroke(r, 3.0, border, egui::StrokeKind::Inside);
    if resp.hovered() {
        painter.rect_stroke(
            r.expand(1.0),
            4.0,
            Stroke::new(1.5, p.text),
            egui::StrokeKind::Outside,
        );
    }
    resp
}

/// Content width of the thread color picker.
pub(crate) const PICKER_W: f32 = 252.0;

impl Studio {
    /// Illustrator-style color picker for the selection's thread: the
    /// nearest real threads, a palette to pick from, and a small custom
    /// color field. Edits apply to every selected object.
    pub fn thread_picker(&mut self, ui: &mut egui::Ui) {
        use crate::theme::space;
        let sel = self.sel_indices();
        let p = pal_ui(ui);
        ui.set_width(PICKER_W);
        ui.spacing_mut().item_spacing.y = space::S;
        // Space between sections, on top of the usual item spacing.
        let section = |ui: &mut egui::Ui| ui.add_space(space::M - space::S);

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Thread Color")
                    .size(13.0)
                    .strong()
                    .color(p.text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::theme::icon_button(ui, crate::icons::Icon::Close, "Close", false, 20.0)
                    .on_hover_text("Close (Esc)")
                    .clicked()
                {
                    self.picker = None;
                }
            });
        });
        let Some(&first) = sel.first() else {
            crate::theme::caption(ui, "Select objects to choose their thread.");
            return;
        };
        let mut c = self.doc.shapes[first].color;
        let before = c;
        let brand = self.view_tools.brand;
        section(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::S;
            swatch(ui, c, 36.0, false);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = space::XS / 2.0;
                ui.add_space(space::XS / 2.0);
                let (_, name, number) = nearest(brand, c);
                let b = if brand == Brand::Brother {
                    "Brother"
                } else {
                    "Janome"
                };
                ui.label(
                    egui::RichText::new(format!("{b} {}", label(name, number)))
                        .size(13.0)
                        .color(p.text),
                );
                caption(ui, format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
            });
        });
        section(ui);
        if let Some(t) = self.palette_section(ui, Some(c), "picker") {
            c = t;
        }
        section(ui);
        // Custom color: a disclosure row, flush with everything else.
        let open = self.palettes.custom_open;
        let (r, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Custom Color"));
        let chev = egui::Rect::from_center_size(
            egui::pos2(r.left() + 5.0, r.center().y),
            Vec2::splat(9.0),
        );
        crate::icons::paint(
            ui.painter(),
            chev,
            if open {
                crate::icons::Icon::ChevronDown
            } else {
                crate::icons::Icon::ChevronRight
            },
            p.secondary,
        );
        ui.painter().text(
            egui::pos2(r.left() + 16.0, r.center().y),
            egui::Align2::LEFT_CENTER,
            "Custom Color",
            egui::FontId::proportional(12.0),
            if resp.hovered() { p.text } else { p.secondary },
        );
        if resp.clicked() {
            self.palettes.custom_open = !open;
        }
        if open {
            crate::widgets::compact_color_picker(ui, &mut c);
        }
        if c != before {
            for &i in &sel {
                self.doc.shapes[i].color = c;
            }
        }
        // Each object back to its own color in the SVG.
        if sel
            .iter()
            .any(|&i| self.doc.shapes[i].color != self.doc.shapes[i].paint)
        {
            section(ui);
            if ui.button("Use the SVG's Color").clicked() {
                for &i in &sel {
                    self.doc.shapes[i].color = self.doc.shapes[i].paint;
                }
            }
        }
    }

    /// Distinct thread colors in sewing order: (color, objects using it, stitches).
    pub fn design_threads(&self) -> Vec<([u8; 3], Vec<usize>, usize)> {
        let mut out: Vec<([u8; 3], Vec<usize>, usize)> = Vec::new();
        for (i, s) in self.doc.shapes.iter().enumerate() {
            if s.params.kind == Some(Kind::Skip) {
                continue;
            }
            let n = self.design.per_shape.get(i).copied().unwrap_or(0);
            match out.iter_mut().find(|(c, _, _)| *c == s.color) {
                Some(e) => {
                    e.1.push(i);
                    e.2 += n;
                }
                None => out.push((s.color, vec![i], n)),
            }
        }
        out
    }

    /// Snap every thread in the design to the nearest color in `palette`.
    pub fn match_to_palette(&mut self, palette: &[[u8; 3]]) -> usize {
        let mut changed = 0;
        for s in &mut self.doc.shapes {
            let Some(&c) = palette.iter().min_by(|a, b| {
                zigzag::formats::perceptual_distance(s.color, **a)
                    .total_cmp(&zigzag::formats::perceptual_distance(s.color, **b))
            }) else {
                continue;
            };
            if c != s.color {
                s.color = c;
                changed += 1;
            }
        }
        changed
    }

    /// Each thread the design uses, once: how many objects and stitches.
    /// Click a row to select its objects, its swatch to change the thread
    /// everywhere it's used.
    fn color_list(&mut self, ui: &mut egui::Ui) {
        let p = pal_ui(ui);
        let brand = self.view_tools.brand;
        let threads = self.design_threads();
        if threads.is_empty() {
            caption(ui, "No threads yet.");
        }
        let selected: Vec<usize> = self.sel_indices();
        let mut select = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut open_picker = None;
        for (c, objects, stitches) in &threads {
            use crate::theme::{LIST_SWATCH, ROW_H, space};
            // Same row layout as the sewing-order list.
            let (rect, resp) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
            let on = !selected.is_empty()
                && objects.iter().all(|i| selected.contains(i))
                && selected.len() == objects.len();
            if on {
                ui.painter()
                    .rect_filled(rect, 5.0, p.accent.gamma_multiply(0.18));
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 5.0, p.hover);
            }
            let sw = egui::Rect::from_min_size(
                egui::pos2(rect.left() + space::XS, rect.center().y - LIST_SWATCH / 2.0),
                Vec2::splat(LIST_SWATCH),
            );
            let swr = ui.interact(
                sw,
                ui.make_persistent_id(("thread-swatch", *c)),
                Sense::click(),
            );
            let painter = ui.painter();
            painter.rect_filled(sw, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
            painter.rect_stroke(
                sw,
                3.0,
                Stroke::new(
                    if swr.hovered() { 1.5 } else { 1.0 },
                    if swr.hovered() {
                        p.text
                    } else {
                        Color32::from_black_alpha(40)
                    },
                ),
                egui::StrokeKind::Inside,
            );
            let (_, name, number) = nearest(brand, *c);
            painter.text(
                egui::pos2(sw.right() + space::S, rect.center().y),
                egui::Align2::LEFT_CENTER,
                label(name, number),
                egui::FontId::proportional(12.0),
                if on { p.accent } else { p.text },
            );
            painter.text(
                egui::pos2(rect.right() - space::XS, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                format!("{} · {}", objects.len(), fmt_int(*stitches)),
                egui::FontId::proportional(11.0),
                p.secondary,
            );
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, label(name, number))
            });
            if swr
                .on_hover_text("Change this thread everywhere it's used")
                .clicked()
            {
                select = Some(objects.clone());
                // Just left of the dock, level with the row.
                let w = PICKER_W + 2.0 * space::M;
                open_picker = Some(egui::pos2(
                    rect.left() - space::M - space::S - w,
                    rect.top() - space::M,
                ));
            } else if resp
                .on_hover_text(format!(
                    "Select the {} object(s) sewn in this thread",
                    objects.len()
                ))
                .clicked()
            {
                select = Some(objects.clone());
            }
        }
        if let Some(at) = open_picker {
            // The picker edits the selection: every object in this thread.
            self.picker = Some(at);
        }
        if let Some(v) = select {
            self.set_selection(&v);
        }
    }

    pub fn threads_body(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = crate::theme::space::S;

        // ---- The design's threads: in sewing order, or once each by color.
        crate::theme::segmented(
            ui,
            "thread-view",
            &mut self.view_tools.thread_order,
            &[(true, "Sewing Order"), (false, "By Color")],
        );
        ui.add_space(crate::theme::space::XS);
        if self.view_tools.thread_order {
            self.sequence_list(ui);
        } else {
            self.color_list(ui);
        }
        ui.spacing_mut().item_spacing.y = crate::theme::space::S;
        ui.add_space(crate::theme::space::S);
        ui.horizontal(|ui| {
            if ui
                .button("Group Colors")
                .on_hover_text("Fewer thread changes, without breaking layering")
                .clicked()
            {
                self.group_colors();
            }
            caption(
                ui,
                format!(
                    "{} changes · {} trims",
                    self.design.blocks.len().saturating_sub(1),
                    self.design.trim_count()
                ),
            );
        });
        // ---- A palette to pick from (off by default: the thread picker
        // has the same palettes; the panel's menu shows it).
        if !self.dock.show_palette {
            return;
        }
        let selected: Vec<usize> = self.sel_indices();
        ui.spacing_mut().item_spacing.y = crate::theme::space::S;
        crate::theme::section_break(ui);
        let has_sel = !selected.is_empty();
        let current = selected.first().map(|&i| self.doc.shapes[i].color);
        if let Some(c) = self.palette_section(ui, current, "threads") {
            if has_sel {
                for &i in &selected {
                    self.doc.shapes[i].color = c;
                }
            } else {
                self.status = "Select objects first, then click a thread to use it.".into();
            }
        }
        if ui
            .add(egui::Button::new("Match All to Palette").small())
            .on_hover_text("Change every color in the design to the nearest one in this palette")
            .clicked()
        {
            self.match_all_to_palette();
        }
    }

    /// Every color in the design to the nearest in the chosen palette.
    pub fn match_all_to_palette(&mut self) {
        let src = self.palettes.source;
        let colors: Vec<[u8; 3]> = self
            .palettes
            .colors(src, self.view_tools.brand)
            .into_iter()
            .map(|(c, _)| c)
            .collect();
        let n = self.match_to_palette(&colors);
        let name = self.palettes.name(src);
        self.status = if colors.is_empty() {
            format!("{name} has no colors yet.")
        } else if n == 0 {
            format!("Every color is already in {name}.")
        } else {
            format!("Matched {n} object(s) to {name}.")
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use std::path::PathBuf;

    fn studio() -> Harness<'static, Studio> {
        let path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg"));
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1500.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(path), None),
            );
        h.run_steps(4);
        h
    }

    #[test]
    fn the_threads_panel_lists_sewing_order_or_colors() {
        let mut h = studio();
        assert!(h.state().view_tools.thread_order, "sewing order first");
        // Four blocks, Ultramarine twice; a row selects what it sews.
        assert!(h.query_by_label("1 Ultramarine (406)").is_some());
        assert!(h.query_by_label("4 Ultramarine (406)").is_some());
        h.get_by_label("2 Pumpkin (126)").click();
        h.run_steps(2);
        let sun = h.state().names.iter().position(|n| n == "sun").unwrap();
        assert_eq!(h.state().sel_indices(), vec![sun]);
        // By color: each thread once.
        h.get_by_label("By Color").click();
        h.run_steps(2);
        assert!(!h.state().view_tools.thread_order);
        assert!(h.query_by_label("1 Ultramarine (406)").is_none());
        assert!(h.query_all_by_label("Ultramarine (406)").count() > 0);
        // Group Colors is there either way.
        assert!(h.query_by_label("Group Colors").is_some());
    }

    #[test]
    fn dark_navy_is_named_blue_not_green() {
        let (_, name, _) = nearest(Brand::Brother, [8, 8, 72]);
        assert!(!name.contains("Green"), "{name}");
    }

    #[test]
    fn the_tool_strip_swatch_opens_a_color_picker_for_the_selection() {
        let mut h = studio();
        // Just the picker's swatches (the Threads panel shows the same ones).
        h.state_mut().dock.toggle(crate::dock::PanelId::Threads);
        h.state_mut().set_selection(&[0, 1]);
        h.run_steps(2);
        h.get_by_label("Thread Color").click();
        h.run_steps(2);
        let (c, name, number) = chart(Brand::Brother)[10];
        h.get_by_label(&label(name, number)).click();
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes[0].color, c);
        assert_eq!(
            h.state().doc.shapes[1].color,
            c,
            "applies to the whole selection"
        );
        assert_eq!(h.state().undo.len(), 1);
        h.get_by_label("Use the SVG's Color").click();
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes[0].color, h.state().doc.shapes[0].paint);

        // Make a palette from the design, add the current thread with +.
        h.get_by_label("Palette menu").click();
        h.run_steps(2);
        h.get_by_label("New Palette from This Design").click();
        h.run_steps(2);
        assert_eq!(h.state().palettes.mine.len(), 1);
        let n = h.state().palettes.mine[0].colors.len();
        assert_eq!(n, 3, "the design's three threads");
        h.state_mut().doc.shapes[0].color = [1, 2, 3];
        h.run_steps(2);
        h.get_by_label("Add Color").click();
        h.run_steps(2);
        assert_eq!(h.state().palettes.mine[0].colors.len(), n + 1);
        // The close button closes the picker.
        h.get_by_label("Close").click();
        h.run_steps(2);
        assert!(h.query_by_label("Add Color").is_none());
    }

    /// A palette made from a thread kit holds just that kit's threads,
    /// named as the maker names them, and Match All keeps the design to
    /// them.
    #[test]
    fn a_palette_from_a_thread_kit() {
        let mut h = studio();
        h.state_mut().dock.toggle(crate::dock::PanelId::Threads);
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        h.get_by_label("Thread Color").click();
        h.run_steps(2);
        h.get_by_label("Palette menu").click();
        h.run_steps(2);
        h.get_by_label("New Palette from Thread Kit").click();
        h.run_steps(2);
        h.get_by_label("Brother 12 Colors").click();
        h.run_steps(3);
        let p = &h.state().palettes;
        assert_eq!(p.mine.len(), 1);
        assert_eq!(p.mine[0].name, "Brother 12 Colors");
        assert_eq!(p.mine[0].colors.len(), 12);
        assert_eq!(p.source, crate::palettes::Source::Mine(0));
        // Swatches are named as threads, not hex codes.
        assert!(h.query_by_label("Flesh Pink (124)").is_some());
        assert!(h.query_by_label("Clay Brown (339)").is_some());

        // Linen is in Brother's 40 set though not in the PEC chart.
        let forty = crate::kits::KITS
            .iter()
            .find(|k| k.name == "Brother 40 Colors")
            .unwrap();
        let linen = crate::threads::brother_thread("307").unwrap().0;
        assert!(forty.colors().contains(&linen));

        // Match All: every object sews in a thread from the kit.
        h.state_mut().match_all_to_palette();
        h.run_steps(2);
        let kit = h.state().palettes.mine[0].colors.clone();
        assert!(
            h.state().doc.shapes.iter().all(|s| kit.contains(&s.color)),
            "every thread is one you have"
        );
    }

    #[test]
    fn threads_group_objects_by_color_and_match_to_a_chart() {
        let mut h = studio();
        let threads = h.state().design_threads();
        // disc and ring share #2f4858.
        assert_eq!(threads.len(), 3);
        assert_eq!(threads[0].1.len(), 2);

        h.state_mut().palettes.source = crate::palettes::Source::Chart(Brand::Janome);
        h.run_steps(1);
        // The palette is off in the panel by default; its menu shows it.
        assert!(h.query_by_label("Match All to Palette").is_none());
        h.state_mut().dock.show_palette = true;
        h.run_steps(2);
        h.get_by_label("Match All to Palette").click();
        h.run_steps(3);
        assert!(h.state().status.contains("Matched"), "{}", h.state().status);
        for s in &h.state().doc.shapes {
            assert!(
                chart(Brand::Janome).iter().any(|(c, _, _)| *c == s.color),
                "{:?} is a Janome thread",
                s.color
            );
        }
        assert_eq!(h.state().undo.len(), 1, "one undo step");
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        h.run_steps(3);
        assert_eq!(h.state().design_threads().len(), 3);
        assert_eq!(h.state().doc.shapes[0].color, [0x2f, 0x48, 0x58]);
    }
}
