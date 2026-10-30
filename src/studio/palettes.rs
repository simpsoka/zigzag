//! Thread palettes: the Brother and Janome charts, and palettes of your own
//! (remembered between launches). The thread color picker and the Threads
//! panel pick from whichever palette is chosen.

use crate::app::Studio;
use crate::icons::{self, Icon};
use crate::theme::pal_ui;
use crate::threads::{Brand, chart, nearest, swatch};
use eframe::egui::{self, Rect, Sense, Stroke, Vec2};
use std::path::PathBuf;

/// Where swatches come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Chart(Brand),
    /// One of your palettes (index into `Palettes::mine`).
    Mine(usize),
}

impl Default for Source {
    fn default() -> Self {
        Source::Chart(Brand::Brother)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UserPalette {
    pub name: String,
    pub colors: Vec<[u8; 3]>,
}

#[derive(Clone, Debug, Default)]
pub struct Palettes {
    pub mine: Vec<UserPalette>,
    pub source: Source,
    /// The palette name being edited (Rename).
    pub renaming: Option<String>,
    /// The picker's Custom Color section is open.
    pub custom_open: bool,
    path: Option<PathBuf>,
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = |k: usize| u8::from_str_radix(&h[k..k + 2], 16).ok();
    Some([v(0)?, v(2)?, v(4)?])
}

impl Palettes {
    /// Your palettes from last time; saved from now on.
    pub fn load() -> Palettes {
        Self::load_from(crate::dock::config_file("palettes.json"))
    }

    fn load_from(path: Option<PathBuf>) -> Palettes {
        let mut p = Palettes {
            path: path.clone(),
            ..Default::default()
        };
        let Some(v) = path
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        else {
            return p;
        };
        for e in v["palettes"].as_array().into_iter().flatten() {
            p.mine.push(UserPalette {
                name: e["name"].as_str().unwrap_or("Palette").to_string(),
                colors: e["colors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c.as_str().and_then(parse_hex))
                    .collect(),
            });
        }
        p
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        let v = serde_json::json!({
            "palettes": self.mine.iter().map(|p| serde_json::json!({
                "name": p.name,
                "colors": p.colors.iter().map(|&c| hex(c)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>()
        });
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default());
    }

    pub fn name(&self, s: Source) -> String {
        match s {
            Source::Chart(Brand::Brother) => "Brother".into(),
            Source::Chart(Brand::Janome) => "Janome".into(),
            Source::Mine(k) => self.mine.get(k).map(|p| p.name.clone()).unwrap_or_default(),
        }
    }

    /// The colors of a palette, each with a name to show.
    pub fn colors(&self, s: Source, naming: Brand) -> Vec<([u8; 3], String)> {
        match s {
            Source::Chart(b) => chart(b)
                .into_iter()
                .map(|(c, n, k)| {
                    (
                        c,
                        match k {
                            Some(k) => format!("{n} ({k})"),
                            None => n.to_string(),
                        },
                    )
                })
                .collect(),
            Source::Mine(k) => self
                .mine
                .get(k)
                .map(|p| {
                    p.colors
                        .iter()
                        .map(|&c| {
                            // A real thread (from a kit, or added from a
                            // chart) by its own name and number.
                            let brother = crate::threads::brother_exact(c)
                                .map(|(n, k)| crate::threads::label(n, Some(k)));
                            let chart = || {
                                [naming, Brand::Brother, Brand::Janome]
                                    .into_iter()
                                    .map(|b| nearest(b, c))
                                    .find(|t| t.0 == c)
                                    .map(|(_, n, k)| crate::threads::label(n, k))
                            };
                            let name = match brother {
                                Some(b) if naming == Brand::Brother => b,
                                b => chart().or(b).unwrap_or_else(|| {
                                    format!("{} · near {}", hex(c), nearest(naming, c).1)
                                }),
                            };
                            (c, name)
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// A new palette (named "Palette N" unless `name` is given). Returns it.
    pub fn create(&mut self, name: Option<String>, colors: Vec<[u8; 3]>) -> Source {
        let name = name.unwrap_or_else(|| {
            let mut n = self.mine.len() + 1;
            while self.mine.iter().any(|p| p.name == format!("Palette {n}")) {
                n += 1;
            }
            format!("Palette {n}")
        });
        let mut colors = colors;
        colors.dedup();
        let mut seen = Vec::new();
        colors.retain(|c| {
            let new = !seen.contains(c);
            seen.push(*c);
            new
        });
        self.mine.push(UserPalette { name, colors });
        self.source = Source::Mine(self.mine.len() - 1);
        self.save();
        self.source
    }

    pub fn add(&mut self, k: usize, c: [u8; 3]) {
        if let Some(p) = self.mine.get_mut(k)
            && !p.colors.contains(&c)
        {
            p.colors.push(c);
            self.save();
        }
    }

    pub fn remove(&mut self, k: usize, c: [u8; 3]) {
        if let Some(p) = self.mine.get_mut(k) {
            p.colors.retain(|x| *x != c);
            self.save();
        }
    }

    pub fn delete(&mut self, k: usize) {
        if k < self.mine.len() {
            self.mine.remove(k);
            self.source = match self.source {
                Source::Mine(j) if j == k => Source::default(),
                Source::Mine(j) if j > k => Source::Mine(j - 1),
                s => s,
            };
            self.save();
        }
    }
}

impl Studio {
    /// The palette chooser and its swatches. `current` is the selection's
    /// thread (highlighted, and what "+" adds). Returns a clicked color.
    pub fn palette_section(
        &mut self,
        ui: &mut egui::Ui,
        current: Option<[u8; 3]>,
        id: &str,
    ) -> Option<[u8; 3]> {
        use crate::theme::space;
        let p = pal_ui(ui);
        let src = self.palettes.source;
        let mut picked = None;
        ui.horizontal(|ui| {
            // The chooser fills the row, less the menu button.
            let field_w = ui.available_width() - 22.0 - ui.spacing().item_spacing.x;
            if let Some(name) = &mut self.palettes.renaming {
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(field_w));
                r.request_focus();
                if r.lost_focus() {
                    let name = self.palettes.renaming.take().unwrap_or_default();
                    if let Source::Mine(k) = src
                        && !name.trim().is_empty()
                    {
                        self.palettes.mine[k].name = name.trim().to_string();
                        self.palettes.save();
                    }
                }
            } else {
                let mut choice = src;
                egui::ComboBox::from_id_salt(format!("{id}-palette"))
                    .selected_text(self.palettes.name(src))
                    .width(field_w)
                    .show_ui(ui, |ui| {
                        for b in [Brand::Brother, Brand::Janome] {
                            let s = Source::Chart(b);
                            ui.selectable_value(&mut choice, s, self.palettes.name(s));
                        }
                        if !self.palettes.mine.is_empty() {
                            ui.separator();
                        }
                        for k in 0..self.palettes.mine.len() {
                            let s = Source::Mine(k);
                            ui.selectable_value(&mut choice, s, self.palettes.name(s));
                        }
                    })
                    .response
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Palette")
                    });
                if choice != src {
                    self.palettes.source = choice;
                    if let Source::Chart(b) = choice {
                        self.view_tools.brand = b;
                    }
                }
            }
            // The palette menu.
            let (r, resp) = ui.allocate_exact_size(Vec2::new(22.0, 20.0), Sense::click());
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, "Palette menu")
            });
            if resp.hovered() {
                ui.painter().rect_filled(r, 4.0, p.hover);
            }
            icons::paint(
                ui.painter(),
                Rect::from_center_size(r.center(), Vec2::splat(11.0)),
                Icon::Menu,
                p.secondary,
            );
            egui::Popup::menu(&resp).show(|ui| {
                ui.set_min_width(200.0);
                if ui.button("New Palette").clicked() {
                    self.palettes.create(None, Vec::new());
                    self.palettes.renaming = Some(self.palettes.name(self.palettes.source));
                }
                ui.menu_button("New Palette from Thread Kit", |ui| {
                    ui.set_min_width(220.0);
                    for (bi, brand) in crate::kits::brands().into_iter().enumerate() {
                        if bi > 0 {
                            ui.separator();
                        }
                        for kit in crate::kits::KITS.iter().filter(|k| k.brand == brand) {
                            if ui
                                .button(kit.name)
                                .on_hover_text(format!(
                                    "{} ({} threads)",
                                    kit.about,
                                    kit.numbers.len()
                                ))
                                .clicked()
                            {
                                self.palettes
                                    .create(Some(kit.name.to_string()), kit.colors());
                                // Name threads after the kit's brand.
                                self.view_tools.brand = kit.brand;
                                ui.close();
                            }
                        }
                    }
                });
                if ui.button("New Palette from This Design").clicked() {
                    let colors = self.design_threads().into_iter().map(|t| t.0).collect();
                    let name = if self.doc.name.is_empty() {
                        None
                    } else {
                        Some(self.doc.name.clone())
                    };
                    self.palettes.create(name, colors);
                }
                if let Source::Mine(k) = src {
                    ui.separator();
                    if ui.button("Rename Palette").clicked() {
                        self.palettes.renaming = Some(self.palettes.name(src));
                    }
                    if ui.button("Delete Palette").clicked() {
                        self.palettes.delete(k);
                    }
                }
            });
        });

        let colors = self.palettes.colors(src, self.view_tools.brand);
        let mine: Vec<(usize, String)> = self
            .palettes
            .mine
            .iter()
            .enumerate()
            .map(|(k, p)| (k, p.name.clone()))
            .collect();
        let mut add: Option<(usize, [u8; 3])> = None;
        let mut remove: Option<(usize, [u8; 3])> = None;
        // Swatches in even columns that fill the width exactly.
        let gap = space::XS;
        let w = ui.available_width();
        let cols = ((w + gap) / (20.0 + gap)).floor().max(1.0);
        let size = ((w - (cols - 1.0) * gap) / cols).floor();
        let cols = cols as usize;
        let mut tiles: Vec<Option<&([u8; 3], String)>> = colors.iter().map(Some).collect();
        if matches!(src, Source::Mine(_)) {
            tiles.push(None); // the + tile
        }
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(gap);
            for row in tiles.chunks(cols) {
                ui.horizontal(|ui| {
                    for tile in row {
                        match tile {
                            Some((c, name)) => {
                                let r = swatch(ui, *c, size, current == Some(*c));
                                r.widget_info(|| {
                                    egui::WidgetInfo::labeled(egui::Role::Button, true, name)
                                });
                                let r = r.on_hover_text(name);
                                if r.clicked() {
                                    picked = Some(*c);
                                }
                                r.context_menu(|ui| {
                                    if let Source::Mine(k) = src
                                        && ui.button("Remove from Palette").clicked()
                                    {
                                        remove = Some((k, *c));
                                        ui.close();
                                    }
                                    for (k, pname) in &mine {
                                        if src != Source::Mine(*k)
                                            && ui.button(format!("Add to {pname}")).clicked()
                                        {
                                            add = Some((*k, *c));
                                            ui.close();
                                        }
                                    }
                                });
                            }
                            None => {
                                // Your palettes: "+" adds the current thread.
                                let Source::Mine(k) = src else { continue };
                                let (r, resp) =
                                    ui.allocate_exact_size(Vec2::splat(size), Sense::click());
                                resp.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::Role::Button,
                                        current.is_some(),
                                        "Add Color",
                                    )
                                });
                                let c = if current.is_some() {
                                    p.secondary
                                } else {
                                    p.tertiary
                                };
                                ui.painter().rect_stroke(
                                    r,
                                    3.0,
                                    Stroke::new(1.0, c),
                                    egui::StrokeKind::Inside,
                                );
                                icons::paint(ui.painter(), r.shrink(size * 0.28), Icon::Plus, c);
                                let resp = resp
                                    .on_hover_text("Add the selection's thread to this palette");
                                if resp.clicked()
                                    && let Some(cur) = current
                                {
                                    add = Some((k, cur));
                                }
                            }
                        }
                    }
                });
            }
        });
        if colors.is_empty() && matches!(src, Source::Mine(_)) {
            crate::theme::caption(ui, "Empty. Select an object and click + to add its thread.");
        }
        if let Some((k, c)) = add {
            self.palettes.add(k, c);
        }
        if let Some((k, c)) = remove {
            self.palettes.remove(k, c);
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_are_remembered() {
        let file =
            std::env::temp_dir().join(format!("zigzag-palettes-{}.json", std::process::id()));
        let mut p = Palettes::load_from(Some(file.clone()));
        assert!(p.mine.is_empty());
        let s = p.create(Some("Camp".into()), vec![[1, 2, 3], [1, 2, 3], [9, 9, 9]]);
        assert_eq!(s, Source::Mine(0));
        assert_eq!(p.mine[0].colors.len(), 2, "no duplicates");
        p.add(0, [255, 0, 0]);
        p.remove(0, [1, 2, 3]);
        let q = Palettes::load_from(Some(file.clone()));
        assert_eq!(q.mine[0].name, "Camp");
        assert_eq!(q.mine[0].colors, vec![[9, 9, 9], [255, 0, 0]]);
        p.delete(0);
        assert_eq!(p.source, Source::default());
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn hex_parses() {
        assert_eq!(parse_hex("#2F4858"), Some([0x2f, 0x48, 0x58]));
        assert_eq!(parse_hex("2f4858"), Some([0x2f, 0x48, 0x58]));
        assert_eq!(parse_hex("#2f48"), None);
    }
}
