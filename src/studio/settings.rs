//! Settings (ZigZag › Settings…, ⌘,): what's set once for this computer and
//! then left alone. Your machines and which one new documents start on;
//! units, appearance and how stitches are drawn; which agent the Agent
//! panel runs and the server other agents connect to. What changes from
//! design to design (fabric, hoop, the layers' stitches) is in the panels;
//! what's flipped while working (the grid, the density map) in View.

use crate::app::Studio;
use crate::dialog;
use crate::icons::Icon;
use crate::machines::hoop_name;
use crate::theme::{ROW_H, caption, hline, icon_button, pal_ui, section_break, semibold, space};
use eframe::egui::{self, Vec2};
use std::path::PathBuf;
use zigzag::document::{DEFAULT_MIN_STITCH_MM, HOOP_SIDE_MM, MIN_STITCH_MM};
use zigzag::formats::{Lock, LockStyle, MACHINE_FORMATS};
use zigzag::machine::profile::{self, Profile};

/// Light, dark, or whichever the system is in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => "Match System",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }

    pub fn theme(self) -> egui::ThemePreference {
        match self {
            Appearance::System => egui::ThemePreference::System,
            Appearance::Light => egui::ThemePreference::Light,
            Appearance::Dark => egui::ThemePreference::Dark,
        }
    }
}

/// The settings that aren't kept anywhere else (`settings.json` in
/// ZigZag's settings folder). Machines, units and the agent keep their own
/// files, as before there was a Settings window.
#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    pub appearance: Appearance,
    /// Draw stitches on the GPU when there is one.
    pub gpu: bool,
    /// Run the server agents connect to (MCP), on this port. Read when the
    /// app starts; `--mcp-port`, `--no-mcp` and `ZIGZAG_MCP_PORT` win.
    pub agent_server: bool,
    pub agent_port: u16,
    /// The sections that fold away (the machine window's Before You Sew
    /// and Machine Details, the MCP chip's Tools), by name: whether each
    /// was left open. One that isn't here is as it comes.
    pub folds: std::collections::BTreeMap<String, bool>,
    /// Keep a log of the tool calls agents make (see `agent_log`).
    pub agent_log: bool,
    /// Look for a newer ZigZag when it opens (and once a day).
    pub auto_update: bool,
    path: Option<PathBuf>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            appearance: Appearance::System,
            gpu: true,
            agent_server: true,
            agent_port: crate::mcp::DEFAULT_PORT,
            folds: Default::default(),
            agent_log: true,
            auto_update: true,
            path: None,
        }
    }
}

impl Prefs {
    /// The settings from last time; saved from now on.
    pub fn load() -> Prefs {
        Self::load_from(crate::dock::config_file("settings.json"))
    }

    pub(crate) fn load_from(path: Option<PathBuf>) -> Prefs {
        let mut p = Prefs {
            path: path.clone(),
            ..Default::default()
        };
        let Some(v) = path
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        else {
            return p;
        };
        if let Some(a) = v["appearance"]
            .as_str()
            .and_then(|s| Appearance::ALL.into_iter().find(|a| a.id() == s))
        {
            p.appearance = a;
        }
        if let Some(g) = v["gpu"].as_bool() {
            p.gpu = g;
        }
        if let Some(on) = v["agent_server"].as_bool() {
            p.agent_server = on;
        }
        for (name, open) in v["sections"].as_object().into_iter().flatten() {
            if let Some(open) = open.as_bool() {
                p.folds.insert(name.clone(), open);
            }
        }
        // (How Before You Sew alone was kept, for a few builds.)
        if let Some(open) = v["sew_checklist"].as_bool() {
            p.folds.entry("before-you-sew".into()).or_insert(open);
        }
        if let Some(on) = v["agent_log"].as_bool() {
            p.agent_log = on;
        }
        if let Some(on) = v["auto_update"].as_bool() {
            p.auto_update = on;
        }
        if let Some(port) = v["agent_port"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .filter(|n| *n >= 1024)
        {
            p.agent_port = port;
        }
        p
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        let v = serde_json::json!({
            "appearance": self.appearance.id(),
            "gpu": self.gpu,
            "agent_server": self.agent_server,
            "agent_port": self.agent_port,
            "sections": self.folds,
            "agent_log": self.agent_log,
            "auto_update": self.auto_update,
        });
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default());
    }

    /// Whether the section called `name` is open: as it was left, or
    /// `usual` if it hasn't been touched.
    pub fn fold_open(&self, name: &str, usual: bool) -> bool {
        self.folds.get(name).copied().unwrap_or(usual)
    }

    /// Open or fold the section called `name`, and keep it that way.
    pub fn set_fold(&mut self, name: &str, open: bool) {
        self.folds.insert(name.to_string(), open);
        self.save();
    }

    /// Whether these are kept in a file (not in tests): what else is kept
    /// beside them goes by it.
    pub fn kept(&self) -> bool {
        self.path.is_some()
    }

    /// The port the agent server starts on, or `None` for no server.
    pub fn agent_port(&self) -> Option<u16> {
        self.agent_server.then_some(self.agent_port)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    Machines,
    General,
    Agent,
}

impl Section {
    const ALL: [Section; 3] = [Section::Machines, Section::General, Section::Agent];

    fn label(self) -> &'static str {
        match self {
            Section::Machines => "Machines",
            Section::General => "General",
            Section::Agent => "Agent",
        }
    }
}

/// The Settings window: whether it's open, and where in it you are.
#[derive(Clone, Debug, Default)]
pub struct SettingsUi {
    pub open: bool,
    pub section: Section,
    /// The machine shown in Machines (an id).
    pub machine: Option<String>,
    /// The hoop being typed in to add to it, mm.
    pub new_hoop: (f64, f64),
}

/// The window, laid out as Illustrator's Preferences are: the sections
/// listed down the left, the chosen one's settings to the right in named
/// groups with a rule between them, and every row's label in one column.
///
/// How wide its contents are, and how tall under the heading.
const WIDTH: f32 = 720.0;
const BODY_H: f32 = 480.0;
/// The settings beside the list: as tall as it, less Done at the foot
/// (whose bottom edge is level with the list's) and the gap above it.
const PANE_H: f32 = BODY_H - dialog::BUTTON_H - space::M;
/// The list of sections (and the padding inside its well), and the gap
/// between it and the settings.
const SIDE_W: f32 = 168.0;
const WELL: f32 = 6.0;
const GUTTER: f32 = 20.0;
const PANE_W: f32 = WIDTH - SIDE_W - GUTTER;
/// The column of row labels; a text field or menu; a hoop in a list.
const LABEL_W: f32 = 112.0;
const FIELD_W: f32 = 280.0;
const HOOP_W: f32 = 260.0;

/// One section in the list down the left.
fn section_row(ui: &mut egui::Ui, label: &str, on: bool) -> egui::Response {
    let p = pal_ui(ui);
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, on, label));
    if on {
        ui.painter().rect_filled(rect, 6.0, p.accent);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, p.hover);
    }
    ui.painter().text(
        rect.left_center() + Vec2::new(space::S + 2.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        if on { egui::Color32::WHITE } else { p.text },
    );
    resp
}

/// A named group of settings, with a rule above it unless it's the first.
fn group(ui: &mut egui::Ui, title: &str, first: bool) {
    if !first {
        section_break(ui);
    }
    let p = pal_ui(ui);
    ui.label(
        egui::RichText::new(title)
            .font(semibold(13.0))
            .color(p.text),
    );
}

/// One setting: its label, right-aligned in the label column, and what
/// `add` puts beside it. Gives `add` the label's id to be `labelled_by`.
fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui, egui::Id) -> R) -> R {
    let p = pal_ui(ui);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = space::M;
        let l = ui
            .allocate_ui_with_layout(
                Vec2::new(LABEL_W, ROW_H),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.set_min_width(LABEL_W);
                    ui.label(egui::RichText::new(label).color(p.secondary))
                },
            )
            .inner;
        ui.horizontal(|ui| {
            ui.set_min_height(ROW_H);
            ui.spacing_mut().item_spacing.x = space::S;
            add(ui, l.id)
        })
        .inner
    })
    .inner
}

/// A line of explanation under a row, in the controls' column.
fn note(ui: &mut egui::Ui, text: &str) {
    ui.add_space(space::XS - ui.spacing().item_spacing.y);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = space::M;
        ui.allocate_exact_size(Vec2::new(LABEL_W, 0.0), egui::Sense::hover());
        ui.vertical(|ui| caption(ui, text));
    });
}

/// A value that's shown, not changed (one of ZigZag's machines).
fn value(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let p = pal_ui(ui);
    ui.label(egui::RichText::new(text).color(p.text))
}

fn lock_name(s: LockStyle) -> &'static str {
    match s {
        LockStyle::Back => "Back and forth",
        LockStyle::Triangle => "Triangle",
        LockStyle::None => "None",
    }
}

impl Studio {
    /// Open Settings at `section`. In Machines, on the document's machine
    /// (or the one new documents start on).
    pub fn open_settings(&mut self, section: Section) {
        let ui = &mut self.settings_ui;
        ui.open = true;
        ui.section = section;
        let known = |id: &Option<String>| {
            id.as_deref()
                .filter(|id| self.machines.find(id).is_some())
                .map(str::to_string)
        };
        if known(&ui.machine).is_none() {
            ui.machine = known(&self.doc.settings.machine)
                .or_else(|| known(&self.machines.current))
                .or_else(|| profile::builtin().first().map(|p| p.id.clone()));
        }
        if ui.new_hoop == (0.0, 0.0) {
            ui.new_hoop = self.doc.settings.hoop;
        }
    }

    /// Put how the app looks and draws to what the settings say.
    pub(crate) fn apply_prefs(&mut self) {
        self.view_tools.gpu_stitches = self.prefs.gpu;
        // (An appearance forced from outside, for screenshots, stays.)
        let forced = std::env::var_os("ZIGZAG_THEME").is_some()
            || std::env::var_os("THREADLINE_THEME").is_some();
        if let Some(ctx) = self.egui_ctx.as_ref().filter(|_| !forced) {
            ctx.set_theme(self.prefs.appearance.theme());
        }
    }

    /// One of your machines as it now is, from Settings: kept, and the
    /// document in front, if it's set up for that machine, takes on what
    /// changed (its hoop only if the machine no longer has it).
    pub(crate) fn update_machine(&mut self, p: Profile) {
        let id = p.id.clone();
        if !self.machines.replace(p) {
            return;
        }
        self.machines.save();
        if self.doc.settings.machine.as_deref() == Some(id.as_str())
            && let Some(p) = self.machines.find(&id).cloned()
        {
            p.apply(&mut self.doc.settings, true);
            self.hoop_custom = false;
        }
    }

    /// The Settings window, while it's open: its sections down the left,
    /// the chosen one's settings beside them, Done at the foot.
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_ui.open {
            return;
        }
        let mut done = false;
        let modal = dialog::show(
            ctx,
            egui::Id::unique("settings"),
            dialog::Place::Center,
            |ui| {
                ui.set_width(WIDTH);
                dialog::heading(ui, "Settings");
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = GUTTER;
                    let p = pal_ui(ui);
                    egui::Frame::new()
                        .fill(if p.dark { p.dock } else { p.header })
                        .corner_radius(8)
                        .inner_margin(egui::Margin::same(WELL as i8))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(SIDE_W - 2.0 * WELL);
                                ui.set_min_height(BODY_H - 2.0 * WELL);
                                ui.spacing_mut().item_spacing.y = 2.0;
                                for s in Section::ALL {
                                    if section_row(ui, s.label(), self.settings_ui.section == s)
                                        .clicked()
                                    {
                                        self.settings_ui.section = s;
                                    }
                                }
                            });
                        });
                    ui.vertical(|ui| {
                        ui.set_width(PANE_W);
                        ui.spacing_mut().item_spacing.y = space::S;
                        let top = ui.cursor().top();
                        match self.settings_ui.section {
                            Section::Machines => self.settings_machines(ui),
                            Section::General => self.settings_general(ui),
                            Section::Agent => self.settings_agent(ui),
                        }
                        // Done, its foot level with the foot of the list.
                        let foot = top + BODY_H - dialog::BUTTON_H;
                        ui.add_space((foot - ui.cursor().top()).max(0.0));
                        ui.allocate_ui_with_layout(
                            Vec2::new(PANE_W, dialog::BUTTON_H),
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| done = dialog::primary(ui, "Done", true).clicked(),
                        );
                    });
                });
            },
        );
        if done || modal.should_close() {
            self.settings_ui.open = false;
        }
    }

    /// Machines: which machine is shown (with making, copying and
    /// forgetting one), and under a rule, everything about it.
    fn settings_machines(&mut self, ui: &mut egui::Ui) {
        let top = ui.cursor().top();
        let selected = self.settings_ui.machine.clone();
        let shown = selected
            .as_deref()
            .and_then(|id| self.machines.find(id))
            .cloned();
        let mine = selected
            .as_deref()
            .is_some_and(|id| self.machines.is_mine(id));
        let mut select: Option<String> = None;
        let (mut add, mut copy, mut delete) = (false, false, false);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::XS;
            egui::ComboBox::from_id_salt("settings-machines")
                .selected_text(shown.as_ref().map_or("No machine", |p| p.name.as_str()))
                .width(FIELD_W)
                .height(360.0)
                .show_ui(ui, |ui| {
                    // ZigZag's, then yours.
                    for (k, p) in self.machines.all().enumerate() {
                        if k == 0 {
                            caption(ui, "Comes with ZigZag");
                        }
                        if k == profile::builtin().len() {
                            ui.separator();
                            caption(ui, "Yours");
                        }
                        let on = selected.as_deref() == Some(p.id.as_str());
                        if ui.selectable_label(on, &p.name).clicked() {
                            select = Some(p.id.clone());
                        }
                    }
                })
                .response
                .on_hover_text(if mine {
                    "One of your machines. A change reaches the document in front, if it's set up for this machine."
                } else {
                    "Comes with ZigZag, and stays as it is. Duplicate it for one of your own to change."
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                copy = ui
                    .add_enabled(shown.is_some(), egui::Button::new("Duplicate"))
                    .on_hover_text("A machine of your own that starts as a copy of this one")
                    .clicked();
                ui.add_space(space::XS);
                delete = ui
                    .add_enabled_ui(mine, |ui| {
                        icon_button(ui, Icon::Minus, "Delete Machine", false, ROW_H)
                            .on_hover_text(
                                "Forget this machine. Designs set up for it keep their hoop.",
                            )
                            .clicked()
                    })
                    .inner;
                add = icon_button(ui, Icon::Plus, "New Machine", false, ROW_H)
                    .on_hover_text("A machine of your own, starting with this document's hoop")
                    .clicked();
            });
        });
        if shown.is_none() {
            caption(ui, "Choose a machine, or add one of your own.");
        }
        hline(ui);
        if let Some(p) = shown {
            let room = (PANE_H - (ui.cursor().top() - top)).max(ROW_H);
            egui::ScrollArea::vertical()
                .id_salt("settings-machine")
                .max_height(room)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(space::XS);
                    self.settings_machine(ui, p);
                    ui.add_space(space::XS);
                });
        }
        if let Some(id) = select {
            self.settings_ui.machine = Some(id);
        }
        if add {
            let id = self
                .machines
                .add_from("New Machine", &self.doc.settings.clone());
            self.settings_ui.machine = Some(id);
        }
        if copy && let Some(id) = selected.as_deref() {
            self.settings_ui.machine = self.machines.duplicate(id);
        }
        if delete && let Some(id) = selected.as_deref() {
            self.machines.remove(id);
            if self.doc.settings.machine.as_deref() == Some(id) {
                self.doc.settings.machine = None;
            }
            self.settings_ui.machine = self
                .machines
                .current
                .clone()
                .or_else(|| profile::builtin().first().map(|p| p.id.clone()));
        }
    }

    /// One machine: everything about it, to change if it's yours.
    fn settings_machine(&mut self, ui: &mut egui::Ui, was: Profile) {
        let mine = self.machines.is_mine(&was.id);
        let mut p = was.clone();
        let mut yours = self.machines.current.as_deref() == Some(p.id.as_str());
        let started_yours = yours;
        let mut new_hoop = self.settings_ui.new_hoop;
        let format_name = |f: Option<&'static str>| match f {
            Some(f) => MACHINE_FORMATS
                .iter()
                .find(|(ext, _)| *ext == f)
                .map_or_else(|| f.to_uppercase(), |(_, label)| label.to_string()),
            None => "Not set".to_string(),
        };

        group(ui, "Machine", true);
        row(ui, "Name", |ui, lid| {
            if mine {
                ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(FIELD_W))
                    .labelled_by(lid)
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Machine name")
                    });
            } else {
                value(ui, &p.name).labelled_by(lid);
            }
        });
        row(ui, "File format", |ui, lid| {
            if mine {
                egui::ComboBox::from_id_salt("settings-format")
                    .selected_text(format_name(p.format))
                    .width(FIELD_W)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut p.format, None, "Not set");
                        for (ext, label) in MACHINE_FORMATS {
                            ui.selectable_value(&mut p.format, Some(ext), label);
                        }
                    })
                    .response
                    .labelled_by(lid)
                    .on_hover_text("The file this machine reads: Export offers it first");
            } else {
                value(ui, &format_name(p.format)).labelled_by(lid);
            }
        });
        // How a design gets to it: by a file, or straight from ZigZag
        // (which is what puts Send in the toolbar and the File menu).
        let link_name = |l: Option<&'static str>| {
            l.and_then(|l| profile::LINKS.iter().find(|k| k.0 == l))
                .map_or("By file", |k| k.1)
        };
        row(ui, "Connection", |ui, lid| {
            if mine {
                egui::ComboBox::from_id_salt("settings-link")
                    .selected_text(link_name(p.link))
                    .width(FIELD_W)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut p.link, None, link_name(None));
                        for (id, label) in profile::LINKS {
                            ui.selectable_value(&mut p.link, Some(id), label);
                        }
                    })
                    .response
                    .labelled_by(lid)
                    .on_hover_text("How a design gets to this machine. One ZigZag connects to has Send in the toolbar and the File menu; the rest take an exported file.");
            } else {
                value(ui, link_name(p.link)).labelled_by(lid);
            }
        });
        row(ui, "New documents", |ui, _| {
            ui.checkbox(&mut yours, "Start on this machine").on_hover_text(
                "New documents begin in this machine's hoop, with its shortest stitch and locks",
            );
        });

        group(ui, "Hoops", false);
        row(ui, "Sizes", |ui, _| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut drop: Option<usize> = None;
                for (k, &h) in p.hoops.clone().iter().enumerate() {
                    let first = profile::same_hoop(h, p.default_hoop);
                    ui.horizontal(|ui| {
                        ui.set_min_height(ROW_H);
                        if !mine {
                            value(ui, &hoop_name(h));
                            if first && p.hoops.len() > 1 {
                                caption(ui, "new documents start in this one");
                            }
                            return;
                        }
                        ui.allocate_ui_with_layout(
                            Vec2::new(HOOP_W, ROW_H),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(HOOP_W);
                                if ui
                                    .radio(first, hoop_name(h))
                                    .on_hover_text("The hoop a new document starts in")
                                    .clicked()
                                {
                                    p.default_hoop = h;
                                }
                            },
                        );
                        if p.hoops.len() > 1
                            && icon_button(ui, Icon::Close, "Remove Hoop", false, 20.0).clicked()
                        {
                            drop = Some(k);
                        }
                    });
                }
                if let Some(k) = drop {
                    p.hoops.remove(k);
                    if !p.has_hoop(p.default_hoop) {
                        p.default_hoop = p.hoops[0];
                    }
                }
            });
        });
        if mine {
            if p.hoops.len() > 1 {
                note(ui, "New documents start in the chosen hoop.");
            }
            row(ui, "Add", |ui, _| {
                ui.add(crate::units::drag_mm(&mut new_hoop.0, HOOP_SIDE_MM, 1.0))
                    .on_hover_text("Width of the hoop's sewing field")
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::SpinButton, true, "New hoop width")
                    });
                ui.label("×");
                ui.add(crate::units::drag_mm(&mut new_hoop.1, HOOP_SIDE_MM, 1.0))
                    .on_hover_text("Height of the hoop's sewing field")
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::SpinButton, true, "New hoop height")
                    });
                if ui
                    .add_enabled(!p.has_hoop(new_hoop), egui::Button::new("Add Hoop"))
                    .clicked()
                {
                    p.hoops.push(new_hoop);
                }
            });
        }

        group(ui, "Stitches", false);
        row(ui, "Min stitch", |ui, lid| {
            let mut v = p.min_stitch_mm.unwrap_or(DEFAULT_MIN_STITCH_MM);
            if mine {
                ui.add(crate::units::drag_mm(&mut v, MIN_STITCH_MM, 0.01))
                    .labelled_by(lid)
                    .on_hover_text("No stitch shorter than this, for machines that drop short stitches. Fill rows closer together than this are sewn every second row and then the rows between, and other short stitches are merged into their neighbours.");
                p.min_stitch_mm = (v != DEFAULT_MIN_STITCH_MM).then_some(v);
            } else {
                value(ui, &format!("{} mm", zigzag::params::fmt_num(v))).labelled_by(lid);
            }
        });
        row(ui, "Locks", |ui, lid| {
            let mut lock = p.lock.unwrap_or_default();
            if mine {
                egui::ComboBox::from_id_salt("settings-lock")
                    .selected_text(lock_name(lock.style))
                    .width(132.0)
                    .show_ui(ui, |ui| {
                        for s in [LockStyle::Back, LockStyle::Triangle, LockStyle::None] {
                            ui.selectable_value(&mut lock.style, s, lock_name(s));
                        }
                    })
                    .response
                    .labelled_by(lid)
                    .on_hover_text("The small stitches that hold the thread where sewing starts and before each cut, in the files you export. (The Skitch has locks of its own.)");
                if lock.style != LockStyle::None {
                    ui.add(crate::units::drag_mm(&mut lock.len_mm, Lock::LEN_MM, 0.01))
                        .on_hover_text(
                            "How long each lock stitch is (never shorter than the shortest stitch)",
                        )
                        .widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::SpinButton,
                                true,
                                "Lock stitch length",
                            )
                        });
                    ui.label("×");
                    ui.add(
                        egui::DragValue::new(&mut lock.count)
                            .range(2..=Lock::MAX_COUNT)
                            .speed(0.05),
                    )
                    .on_hover_text(
                        "How many lock stitches: twos back and forth, threes round a triangle",
                    )
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::SpinButton, true, "Lock stitches")
                    });
                }
                let lock = Lock::new(lock.style, lock.len_mm, lock.count);
                p.lock = (lock != Lock::default()).then_some(lock);
            } else {
                value(
                    ui,
                    &match lock.style {
                        LockStyle::None => "None".to_string(),
                        s => format!(
                            "{}, {} × {} mm",
                            lock_name(s),
                            lock.count,
                            zigzag::params::fmt_num(lock.len_mm)
                        ),
                    },
                )
                .labelled_by(lid);
            }
        });

        self.settings_ui.new_hoop = new_hoop;
        if yours != started_yours {
            self.machines.current = yours.then(|| p.id.clone());
            self.machines.save();
        }
        // (A name can't be nothing: what was there stays until it's something.)
        if p.name.trim().is_empty() {
            p.name = was.name.clone();
        }
        if mine && p != was {
            self.update_machine(p);
        }
    }

    fn settings_general(&mut self, ui: &mut egui::Ui) {
        let mut prefs = self.prefs.clone();
        let mut units = self.dock.units;
        let gpu = self.gpu.is_some();

        group(ui, "Measurements", true);
        row(ui, "Units", |ui, lid| {
            egui::ComboBox::from_id_salt("settings-units")
                .selected_text(units.label())
                .width(160.0)
                .show_ui(ui, |ui| {
                    for u in crate::units::Unit::ALL {
                        ui.selectable_value(&mut units, u, u.label());
                    }
                })
                .response
                .labelled_by(lid)
                .on_hover_text("What lengths are shown and typed in");
        });

        group(ui, "Appearance", false);
        row(ui, "Theme", |ui, _| {
            let options = Appearance::ALL.map(|a| (a, a.label()));
            crate::theme::segmented(ui, "settings-appearance", &mut prefs.appearance, &options);
        });

        group(ui, "Performance", false);
        row(ui, "Stitches", |ui, _| {
            ui.add_enabled(
                gpu,
                egui::Checkbox::new(&mut prefs.gpu, "Draw stitches on the GPU"),
            )
            .on_disabled_hover_text(
                "This computer's graphics can't draw them: they're drawn by the CPU.",
            );
        });
        note(
            ui,
            "Faster for big designs. Turn it off if stitches don't show, or show wrongly, on this computer.",
        );

        group(ui, "Updates", false);
        let mut check = false;
        row(ui, "Automatic", |ui, _| {
            ui.checkbox(&mut prefs.auto_update, "Look for updates when ZigZag opens")
                .on_hover_text("Asks GitHub for ZigZag's latest release, when it opens and once a day. Nothing about you or your designs is sent.");
        });
        row(ui, "This version", |ui, lid| {
            value(ui, env!("CARGO_PKG_VERSION")).labelled_by(lid);
            check = ui.button("Check Now").clicked();
        });
        if check {
            let ctx = ui.ctx().clone();
            self.updater.check(&ctx, true);
        }

        if units != self.dock.units {
            self.set_units(units);
        }
        if prefs != self.prefs {
            self.prefs = prefs;
            self.prefs.save();
            self.apply_prefs();
        }
    }

    fn settings_agent(&mut self, ui: &mut egui::Ui) {
        use crate::agent::AgentKind;
        let mut prefs = self.prefs.clone();
        let mut kind = self.agent.kind;

        group(ui, "Agent Panel", true);
        row(ui, "Runs", |ui, _| {
            let options = AgentKind::ALL.map(|k| (k, k.name()));
            crate::theme::segmented(ui, "settings-agent-kind", &mut kind, &options);
        });
        note(
            ui,
            "The agent the Agent panel talks to. Changing it starts a new chat.",
        );

        group(ui, "Agent Server", false);
        row(ui, "Connections", |ui, _| {
            ui.checkbox(&mut prefs.agent_server, "Let agents connect");
        });
        note(
            ui,
            "For Claude Code, Claude Desktop, Codex and other agents on this computer (MCP). It listens to this computer only.",
        );
        row(ui, "Port", |ui, lid| {
            ui.add_enabled(
                prefs.agent_server,
                egui::DragValue::new(&mut prefs.agent_port)
                    .range(1024..=u16::MAX)
                    .speed(1.0),
            )
            .labelled_by(lid);
        });
        row(ui, "Status", |ui, lid| {
            value(
                ui,
                &match (&self.mcp_ui.start_error, self.mcp_ui.port) {
                    (Some(e), _) => format!("Not running ({e})"),
                    (None, Some(port)) => format!("Listening on port {port}"),
                    (None, None) => "Off".to_string(),
                },
            )
            .labelled_by(lid);
        });
        note(
            ui,
            "A change to the server takes effect the next time ZigZag opens.",
        );

        group(ui, "Log", false);
        let mut show = false;
        row(ui, "Tool calls", |ui, _| {
            ui.checkbox(&mut prefs.agent_log, "Keep a log of what agents do");
            let file = self.agent_log.file().is_some();
            show = ui
                .add_enabled(file, egui::Button::new("Show Log"))
                .on_disabled_hover_text("Nothing has been logged yet.")
                .clicked();
        });
        note(
            ui,
            "Every tool call: when, which tool, what it was asked and what it answered. Kept on this computer, in ZigZag's settings folder.",
        );
        if show && let Some(file) = self.agent_log.file() {
            crate::agent_log::open_text(file);
        }

        if kind != self.agent.kind {
            self.agent.set_kind(kind);
        }
        if prefs != self.prefs {
            let log = prefs.agent_log != self.prefs.agent_log;
            self.prefs = prefs;
            self.prefs.save();
            if log {
                self.apply_agent_log();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_remembered_and_odd_ones_ignored() {
        let file =
            std::env::temp_dir().join(format!("zigzag-settings-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let mut p = Prefs::load_from(Some(file.clone()));
        assert_eq!(p.appearance, Appearance::System);
        assert!(p.gpu && p.agent_server && p.agent_log && p.auto_update);
        assert!(p.fold_open("before-you-sew", true) && !p.fold_open("machine-details", false));
        assert_eq!(p.agent_port(), Some(crate::mcp::DEFAULT_PORT));
        p.appearance = Appearance::Dark;
        p.gpu = false;
        p.agent_port = 9000;
        p.agent_log = false;
        p.auto_update = false;
        p.set_fold("before-you-sew", false);
        p.set_fold("machine-details", true);
        assert!(!p.fold_open("before-you-sew", true) && p.fold_open("machine-details", false));
        let back = Prefs::load_from(Some(file.clone()));
        assert_eq!(back, p);
        p.agent_server = false;
        p.save();
        assert_eq!(Prefs::load_from(Some(file.clone())).agent_port(), None);
        // A file someone else wrote: what's readable is read, the rest is
        // as it comes.
        std::fs::write(
            &file,
            r#"{"appearance":"plaid","gpu":"yes","agent_port":80}"#,
        )
        .unwrap();
        let odd = Prefs::load_from(Some(file.clone()));
        assert!(odd.folds.is_empty());
        std::fs::write(
            &file,
            r#"{"sew_checklist":false,"sections":{"mcp-tools":true,"x":"open"}}"#,
        )
        .unwrap();
        let kept = Prefs::load_from(Some(file.clone()));
        assert!(!kept.fold_open("before-you-sew", true) && kept.fold_open("mcp-tools", false));
        assert_eq!(kept.folds.len(), 2);
        assert_eq!(odd.appearance, Appearance::System);
        assert!(odd.gpu);
        assert_eq!(odd.agent_port, crate::mcp::DEFAULT_PORT);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_machine_changed_in_settings_reaches_the_document_set_up_for_it() {
        let mut s = Studio::with_file(None, None);
        // One of your own, from the document as it is: its hoop.
        s.doc.settings.hoop = (150.0, 220.0);
        let id = s.machines.add_from("Shop Tajima", &s.doc.settings.clone());
        s.choose_machine(Some(&id));
        assert_eq!(s.doc.settings.hoop, (150.0, 220.0));
        assert_eq!(s.doc.settings.min_stitch_mm, DEFAULT_MIN_STITCH_MM);
        // In Settings it's told it drops short stitches, wants triangle
        // locks and has another hoop: the document follows the first two,
        // and stays in the hoop it's in.
        let mut p = s.machines.find(&id).unwrap().clone();
        p.min_stitch_mm = Some(0.4);
        p.lock = Some(Lock::new(LockStyle::Triangle, 0.5, 3));
        p.hoops.push((300.0, 200.0));
        p.format = profile::machine_format("dst");
        s.update_machine(p);
        assert_eq!(s.doc.settings.min_stitch_mm, 0.4);
        assert_eq!(s.doc.settings.lock.style, LockStyle::Triangle);
        assert_eq!(s.doc.settings.hoop, (150.0, 220.0));
        assert_eq!(s.machine().and_then(|m| m.format), Some("dst"));
        // The hoop it's in taken away: it moves to the machine's default.
        let mut p = s.machines.find(&id).unwrap().clone();
        p.hoops.retain(|&h| !profile::same_hoop(h, (150.0, 220.0)));
        p.default_hoop = p.hoops[0];
        s.update_machine(p);
        assert_eq!(s.doc.settings.hoop, (300.0, 200.0));
        // A document on another machine is left alone; and ZigZag's own
        // machines aren't changed from here.
        s.choose_machine(Some("brother-5x7"));
        let mut p = s.machines.find(&id).unwrap().clone();
        p.min_stitch_mm = Some(0.6);
        s.update_machine(p);
        assert_eq!(s.doc.settings.min_stitch_mm, DEFAULT_MIN_STITCH_MM);
        let mut builtin = s.machines.find("melco-summit").unwrap().clone();
        builtin.min_stitch_mm = Some(0.9);
        s.update_machine(builtin);
        assert_eq!(
            s.machines.find("melco-summit").unwrap().min_stitch_mm,
            Some(0.4)
        );
        // A copy of one of ZigZag's is yours to change.
        let copy = s.machines.duplicate("melco-summit").unwrap();
        assert!(s.machines.is_mine(&copy));
        let c = s.machines.find(&copy).unwrap();
        assert_eq!(
            (c.name.as_str(), c.min_stitch_mm, c.format),
            ("Melco Summit copy", Some(0.4), Some("exp"))
        );
        assert!(!c.tested && c.source.is_empty());
    }
}
