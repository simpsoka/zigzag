//! Object › Test Swatches…: copies of the selection in a grid, each with
//! one stitch setting stepped from one value to another and labelled, so a
//! test piece shows which spacing, pull compensation or width suits the
//! fabric (Ink/Stitch's Generate Test Swatches).

use crate::app::{Studio, shape_bounds};
use crate::theme::space;
use eframe::egui::{self, Color32, Key, Vec2};
use zigzag::document::TextSpec;
use zigzag::geom::P;
use zigzag::params::{Kind, Params};

/// The setting a swatch grid steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Spacing,
    Length,
    Pull,
    Width,
    Angle,
}

impl Setting {
    pub const ALL: [Setting; 5] = [
        Setting::Spacing,
        Setting::Length,
        Setting::Pull,
        Setting::Width,
        Setting::Angle,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Setting::Spacing => "Spacing",
            Setting::Length => "Stitch length",
            Setting::Pull => "Pull compensation",
            Setting::Width => "Satin width",
            Setting::Angle => "Fill angle",
        }
    }

    fn is_length(self) -> bool {
        self != Setting::Angle
    }

    /// Does the setting mean anything for this stitch type?
    pub fn applies(self, kind: Kind) -> bool {
        match self {
            Setting::Spacing => matches!(kind, Kind::Fill | Kind::Satin | Kind::Contour),
            Setting::Length => matches!(kind, Kind::Fill | Kind::Run | Kind::Contour),
            Setting::Pull => matches!(kind, Kind::Fill | Kind::Satin),
            Setting::Width => kind == Kind::Satin,
            Setting::Angle => kind == Kind::Fill,
        }
    }

    /// A sensible range to try, around what the object has now.
    pub fn default_range(
        self,
        p: &Params,
        f: &zigzag::params::Fabric,
        width: f64,
    ) -> (f64, f64) {
        let round = |v: f64| (v * 100.0).round() / 100.0;
        match self {
            Setting::Spacing => {
                let s = p.spacing.unwrap_or(f.fill_spacing);
                (round(s * 0.7), round(s * 1.3))
            }
            Setting::Length => (2.0, 4.0),
            Setting::Pull => (0.0, 0.4),
            Setting::Width => (round(width * 0.7), round(width * 1.3)),
            Setting::Angle => (0.0, 90.0),
        }
    }

    pub fn set(self, p: &mut Params, v: f64) {
        match self {
            Setting::Spacing => p.spacing = Some(v),
            Setting::Length => p.len = Some(v),
            Setting::Pull => p.pull = Some(v),
            Setting::Width => p.width = Some(v),
            Setting::Angle => p.angle = Some(v),
        }
    }

    /// The value as a readout, in the current unit.
    pub fn label(self, v: f64) -> String {
        if self.is_length() {
            let u = crate::units::current();
            format!(
                "{} {}",
                zigzag::params::fmt_num(u.amount(v * 10.0)),
                u.suffix()
            )
        } else {
            format!("{v:.0}°")
        }
    }

    /// The value alone, short, for the lettering under a swatch.
    fn short(self, v: f64) -> String {
        if self.is_length() {
            zigzag::params::fmt_num(crate::units::current().amount(v * 10.0))
        } else {
            format!("{v:.0}°")
        }
    }
}

#[derive(Clone)]
pub struct SwatchDialog {
    pub setting: Setting,
    pub from: f64,
    pub to: f64,
    pub steps: usize,
    pub columns: usize,
    pub labels: bool,
}

/// The values `steps` swatches take, `from` to `to` inclusive.
pub fn values(from: f64, to: f64, steps: usize) -> Vec<f64> {
    let n = steps.max(1);
    (0..n)
        .map(|k| {
            let t = if n == 1 {
                0.0
            } else {
                k as f64 / (n - 1) as f64
            };
            let v = from + (to - from) * t;
            (v * 1000.0).round() / 1000.0
        })
        .collect()
}

impl Studio {
    /// Open Object › Test Swatches… for the selection.
    pub fn open_swatch_dialog(&mut self) {
        let Some(i) = self.sel_index() else { return };
        let s = &self.doc.shapes[i];
        let kind = s.params.kind.unwrap_or(Kind::Fill);
        let setting = Setting::ALL
            .into_iter()
            .find(|st| st.applies(kind))
            .unwrap_or(Setting::Spacing);
        let f = self.doc.settings.fabric();
        let (from, to) = setting.default_range(&s.params, &f, crate::app::mean_satin_width(s));
        self.layout.swatch_dialog = Some(SwatchDialog {
            setting,
            from,
            to,
            steps: 5,
            columns: 5,
            labels: true,
        });
    }

    pub fn swatch_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.layout.swatch_dialog.take() else {
            return;
        };
        let Some(i) = self.sel_index() else { return };
        let kind = self.doc.shapes[i].params.kind.unwrap_or(Kind::Fill);
        let (mut ok, mut cancel) = (false, false);
        let mut open = true;
        let f = self.doc.settings.fabric();
        let width = crate::app::mean_satin_width(&self.doc.shapes[i]);
        let base = self.doc.shapes[i].params.clone();
        egui::Window::new("Test Swatches")
            .title_frame(crate::theme::title_frame(ctx))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 90.0))
            .show(ctx, |ui| {
                crate::theme::caption(
                    ui,
                    "Copies of the selection, each with one setting stepped, to stitch out and compare.",
                );
                ui.add_space(space::S);
                crate::theme::form(ui, "swatch-form", |fm| {
                    fm.row("Setting", |ui, lid| {
                        let before = d.setting;
                        egui::ComboBox::from_id_salt("swatch-setting")
                            .selected_text(d.setting.name())
                            .width(160.0)
                            .show_ui(ui, |ui| {
                                for st in Setting::ALL.into_iter().filter(|st| st.applies(kind)) {
                                    ui.selectable_value(&mut d.setting, st, st.name());
                                }
                            })
                            .response
                            .labelled_by(lid);
                        if d.setting != before {
                            (d.from, d.to) = d.setting.default_range(&base, &f, width);
                        }
                    });
                    fm.row("From", |ui, lid| {
                        let r = if d.setting.is_length() {
                            ui.add(crate::units::drag_mm(&mut d.from, 0.0..=400.0, 0.01))
                        } else {
                            ui.add(egui::DragValue::new(&mut d.from).range(-90.0..=90.0).suffix("°"))
                        };
                        r.labelled_by(lid);
                    });
                    fm.row("To", |ui, lid| {
                        let r = if d.setting.is_length() {
                            ui.add(crate::units::drag_mm(&mut d.to, 0.0..=400.0, 0.01))
                        } else {
                            ui.add(egui::DragValue::new(&mut d.to).range(-90.0..=90.0).suffix("°"))
                        };
                        r.labelled_by(lid);
                    });
                    fm.row("Swatches", |ui, lid| {
                        ui.add(egui::DragValue::new(&mut d.steps).range(2..=24))
                            .labelled_by(lid);
                        ui.label("in");
                        ui.add(egui::DragValue::new(&mut d.columns).range(1..=12))
                            .on_hover_text("Columns");
                        ui.label("columns");
                    });
                    fm.row("Labels", |ui, _| {
                        ui.checkbox(&mut d.labels, "Stitch the value under each")
                            .on_hover_text("Small running-stitch lettering");
                    });
                });
                ui.add_space(space::S);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width().max(260.0), 24.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let accent = crate::theme::pal_ui(ui).accent;
                        ok = ui
                            .add(
                                egui::Button::new(egui::RichText::new("Make").color(Color32::WHITE))
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
            match self.make_swatches(&d) {
                Ok(n) => {
                    self.status = format!(
                        "Made {n} swatches, {} from {} to {}.",
                        d.setting.name().to_lowercase(),
                        d.setting.label(d.from),
                        d.setting.label(d.to)
                    )
                }
                Err(e) => self.status = format!("Couldn't make swatches: {e:#}"),
            }
        } else if !(cancel || !open) {
            self.layout.swatch_dialog = Some(d);
        }
    }

    /// Lay the swatches out to the right of the selection: copies in a grid,
    /// each with its value set and (optionally) lettered underneath. One
    /// undo step; the new objects are selected.
    pub fn make_swatches(&mut self, d: &SwatchDialog) -> anyhow::Result<usize> {
        let idx = self.sel_indices();
        if idx.is_empty() {
            anyhow::bail!("select the object to test first");
        }
        let mut b = zigzag::geom::Bounds::empty();
        for &i in &idx {
            let sb = shape_bounds(&self.doc.shapes[i]);
            b.add(sb.min);
            b.add(sb.max);
        }
        let vals = values(d.from, d.to, d.steps);
        let gap = 30.0; // 3 mm between swatches
        let label_h = if d.labels { 35.0 } else { 0.0 };
        // Cells wide enough for the longest label (2.5 mm letters).
        let label_w = if d.labels {
            vals.iter()
                .map(|&v| d.setting.short(v).len())
                .max()
                .unwrap_or(0) as f64
                * 18.0
        } else {
            0.0
        };
        let (w, h) = (b.width().max(label_w) + gap, b.height() + gap + label_h);
        let cols = d.columns.max(1);
        let color = self.doc.shapes[idx[0]].color;
        let setting = d.setting;
        let labels = d.labels;
        let mut made = Vec::new();
        self.structural(|doc| {
            for (k, &v) in vals.iter().enumerate() {
                let (col, row) = (k % cols, k / cols);
                let off = P::new((col + 1) as f64 * w, row as f64 * h);
                let new = doc.duplicate(&idx, off)?;
                for &j in &new {
                    let p = &mut doc.shapes[j].params;
                    if setting.applies(p.kind.unwrap_or(Kind::Fill)) {
                        setting.set(p, v);
                    }
                }
                made.extend(new);
                if labels {
                    let spec = TextSpec {
                        text: setting.short(v),
                        size_mm: 2.5,
                        bold: false,
                        color,
                        kind: Kind::Run,
                        ..TextSpec::default()
                    };
                    let c = b.center() + off;
                    let under = (c.x / 10.0, (b.max.y + off.y + gap * 0.5 + 12.0) / 10.0);
                    let t = doc.add_lettering(&spec, under)?;
                    made.push(t);
                }
            }
            Ok(())
        })?;
        self.replan();
        let n = made.len();
        self.set_selection(&made);
        Ok(n.min(vals.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

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
    fn swatches_step_a_setting_across_labelled_copies() {
        let mut h = studio();
        let sun = index_of(&h, "sun");
        h.state_mut().set_selection(&[sun]);
        h.run_steps(1);
        let n = h.state().doc.shapes.len();
        h.state_mut()
            .run_action(crate::commands::Action::TestSwatches);
        h.run_steps(2);
        let d = h
            .state()
            .layout
            .swatch_dialog
            .as_ref()
            .expect("dialog open");
        assert_eq!(d.setting, Setting::Spacing);
        assert!(d.from < d.to);
        h.get_by_label("Make").click();
        h.run_steps(3);
        assert!(h.state().layout.swatch_dialog.is_none());
        // Five copies, five labels, one undo step; the copies step the spacing.
        assert_eq!(h.state().doc.shapes.len(), n + 10, "{}", h.state().status);
        let spacings: Vec<f64> = h.state().doc.shapes[n..]
            .iter()
            .filter(|s| s.params.kind == Some(Kind::Fill))
            .filter_map(|s| s.params.spacing)
            .collect();
        assert_eq!(spacings.len(), 5);
        assert!(spacings.windows(2).all(|w| w[1] > w[0]), "{spacings:?}");
        assert!(
            h.state().status.starts_with("Made 5 swatches"),
            "{}",
            h.state().status
        );
        let undo = h.state().undo.len();
        h.state_mut().run_action(crate::commands::Action::Undo);
        h.run_steps(2);
        assert_eq!(h.state().doc.shapes.len(), n);
        assert_eq!(h.state().undo.len(), undo - 1);
    }

    #[test]
    fn settings_copy_and_paste_select_same_and_reverse() {
        let mut h = studio();
        let (sun, disc, ring) = (
            index_of(&h, "sun"),
            index_of(&h, "disc"),
            index_of(&h, "ring"),
        );
        h.state_mut().doc.shapes[sun].params.spacing = Some(0.22);
        h.state_mut().doc.shapes[sun].params.pull = Some(0.3);
        h.state_mut().set_selection(&[sun]);
        h.run_steps(2);
        h.state_mut()
            .run_action(crate::commands::Action::CopySettings);
        h.state_mut().set_selection(&[disc]);
        h.run_steps(1);
        h.state_mut()
            .run_action(crate::commands::Action::PasteSettings);
        h.run_steps(2);
        let p = &h.state().doc.shapes[disc].params;
        assert_eq!(p.kind, Some(Kind::Fill));
        assert!((p.spacing.unwrap() - 0.22).abs() < 1e-9 && (p.pull.unwrap() - 0.3).abs() < 1e-9);
        assert_ne!(
            h.state().doc.shapes[disc].color,
            h.state().doc.shapes[sun].color,
            "thread kept"
        );
        // Select Same: both fills.
        h.state_mut()
            .run_action(crate::commands::Action::SelectSameKind);
        h.run_steps(1);
        let sel = h.state().sel_indices();
        assert!(
            sel.contains(&sun) && sel.contains(&disc) && !sel.contains(&ring),
            "{sel:?}"
        );
        // Reverse the ring's path: its first point moves to where its last was.
        h.state_mut().set_selection(&[ring]);
        h.run_steps(1);
        let before = h.state_mut().editable_path().unwrap();
        h.state_mut()
            .run_action(crate::commands::Action::ReversePath);
        h.run_steps(2);
        let after = h.state_mut().editable_path().unwrap();
        assert_eq!(before[0].anchors.len(), after[0].anchors.len());
        let (a0, b_last) = (before[0].anchors[0].p, after[0].anchors.last().unwrap().p);
        let (a_last, b0) = (before[0].anchors.last().unwrap().p, after[0].anchors[0].p);
        assert!(
            a0.dist(b_last) < 1e-6 && a_last.dist(b0) < 1e-6 || before[0].closed,
            "{a0:?} {b0:?}"
        );
        assert!(h.state().status.starts_with("Reversed"));
    }

    #[test]
    fn values_step_evenly_and_round() {
        assert_eq!(values(0.2, 0.4, 5), vec![0.2, 0.25, 0.3, 0.35, 0.4]);
        assert_eq!(values(1.0, 1.0, 1), vec![1.0]);
        assert_eq!(values(0.0, 90.0, 2), vec![0.0, 90.0]);
    }
}
