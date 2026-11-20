//! Panels, Illustrator-style. Each panel lives in the left dock, the right
//! dock, or floats over the canvas; drag a panel by its header to move it
//! (between panels, to the other dock, or out onto the canvas). A dock can
//! collapse to a strip of icons (» at its top): click an icon and the panel
//! flies out beside the strip. Panels can also be collapsed, hidden, and
//! shown again from the Window menu. The layout is remembered.

use crate::app::{Studio, Tool};
use crate::commands::Flag;
use crate::icons::{self, Icon};
use crate::theme::{self, Form, callout, caption, form_for, icon_button, pal_ui, value_row};
use crate::widgets::*;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use zigzag::checks::Level;
use zigzag::design;
use zigzag::params::{FABRICS, Kind, Params, defaults, underlay};
use zigzag::svg::Geometry;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelId {
    Objects,
    Stitch,
    Transform,
    Lettering,
    Threads,
    Document,
    /// The in-app agent (Claude Code or Codex): its conversation and prompt.
    Agent,
}

/// How many panels there are.
pub const N: usize = 7;

enum ObjRow {
    Group {
        id: String,
        depth: usize,
        members: Vec<usize>,
    },
    Item {
        i: usize,
        depth: usize,
    },
}

pub const ALL: [PanelId; N] = [
    PanelId::Objects,
    PanelId::Stitch,
    PanelId::Transform,
    PanelId::Lettering,
    PanelId::Threads,
    PanelId::Document,
    PanelId::Agent,
];

/// Where a panel lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Left,
    Right,
    Float,
}

impl Place {
    fn key(self) -> &'static str {
        match self {
            Place::Left => "left",
            Place::Right => "right",
            Place::Float => "float",
        }
    }
    fn side(self) -> usize {
        match self {
            Place::Left => 0,
            _ => 1,
        }
    }
}

/// Width of a dock collapsed to icons, and of floating and flyout panels.
pub const ICON_STRIP_W: f32 = 40.0;
pub const FLOAT_W: f32 = 300.0;

/// A floating (or flyout) panel's width: the Agent's conversation is wider.
pub fn float_w(p: PanelId) -> f32 {
    if p == PanelId::Agent { 360.0 } else { FLOAT_W }
}

/// Panel interaction that isn't saved: the open flyout, a panel being
/// dragged, and where things were drawn this frame (for dropping).
#[derive(Default)]
pub struct DockUi {
    pub flyout: Option<PanelId>,
    drag: Option<PanelDrag>,
    /// Panels drawn this frame: (where, which, whole panel rect).
    drawn: Vec<(Place, PanelId, Rect)>,
    /// The dock columns this frame (left, right).
    columns: [Option<Rect>; 2],
    /// Where the panel being drawn lives.
    pub(crate) current: Option<Place>,
    /// Height of each dock column (for sizing the Layers list).
    pub(crate) column_h: [f32; 2],
    /// The panel being drawn sits in a rounded window (floating, flyout).
    pub(crate) in_window: bool,
    /// The tab group being drawn: its lead and the tabs showing.
    tabs: Option<(PanelId, Vec<PanelId>)>,
    /// The Agent panel's whole height when it's pinned to the bottom of
    /// the dock being drawn.
    pub(crate) pinned_h: Option<f32>,
    /// The next panel header drawn is the first in this dock's column: it
    /// carries the dock's « / » (collapse to icons), at its right end.
    collapse_btn: Option<Place>,
    /// Layers being dragged to another place in the sewing order: the
    /// objects (by uid), and what the copy at the pointer says.
    pub(crate) layer_drag: Option<(Vec<usize>, String)>,
}

#[derive(Clone, Copy)]
struct PanelDrag {
    id: PanelId,
    /// Pointer offset from the panel header's top-left.
    grab: Vec2,
    /// The whole group moves (dragged by its header), not one tab.
    whole: bool,
}

/// Where a dragged panel would land.
#[derive(Clone, Copy, Debug, PartialEq)]
enum DropAt {
    /// In a dock, before a panel (or at the end).
    Dock(Place, Option<PanelId>),
    /// Into a panel's tab group.
    Tab(PanelId),
    Float,
}

impl PanelId {
    pub fn icon(self) -> Icon {
        match self {
            PanelId::Objects => Icon::Layers,
            PanelId::Stitch => Icon::Stitch,
            PanelId::Transform => Icon::Transform,
            PanelId::Lettering => Icon::Text,
            PanelId::Threads => Icon::Swatches,
            PanelId::Document => Icon::Page,
            PanelId::Agent => Icon::Sparkle,
        }
    }

    /// The name a saved layout knows the panel by. (The Layers panel was
    /// called Objects once, and layouts saved then still say so.)
    fn key(self) -> &'static str {
        match self {
            PanelId::Objects => "Objects",
            p => p.title(),
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            PanelId::Objects => "Layers",
            PanelId::Stitch => "Stitch",
            PanelId::Transform => "Transform",
            PanelId::Lettering => "Lettering",
            PanelId::Threads => "Threads",
            PanelId::Document => "Document",
            PanelId::Agent => "Agent",
        }
    }
    pub(crate) fn index(self) -> usize {
        ALL.iter().position(|&p| p == self).unwrap()
    }
}

#[derive(Clone, PartialEq)]
pub struct DockState {
    pub left: bool,
    pub right: bool,
    hidden: [bool; N],
    collapsed: [bool; N],
    /// The Threads panel shows a palette to click (its menu's Show Palette).
    /// Off by default: the thread picker has the same palettes.
    pub show_palette: bool,
    /// Layers lists what's sewn first at the top (its menu's List in Sewing
    /// Order; View › Sewing Order is the numbers on the canvas),
    /// as the Threads panel does and embroidery software's object lists.
    /// Off by default: what's on top is at the top, as in Illustrator.
    pub layers_sewing_order: bool,
    /// Where each panel lives.
    pub place: [Place; N],
    /// Panel order within the docks.
    pub order: Vec<PanelId>,
    /// Top-left corners of floating panels.
    pub float_at: [Pos2; N],
    /// Each dock (left, right) collapsed to a strip of icons.
    pub iconic: [bool; 2],
    /// Tab groups: the panel each one is a tab of (the group's lead), and
    /// each lead's tab in front.
    pub tab_of: [Option<PanelId>; N],
    pub active: [Option<PanelId>; N],
    /// The contextual task bar under the selection.
    pub task_bar: bool,
    /// How much of its dock the Agent panel takes (it sits at the bottom).
    pub agent_share: f32,
    /// The unit lengths are shown in (View › Units).
    pub units: crate::units::Unit,
    /// Where the layout is remembered between launches (`None`: not saved).
    path: Option<std::path::PathBuf>,
}

fn default_place(p: PanelId) -> Place {
    match p {
        // The conversation gets a whole dock; everything else is on the right.
        PanelId::Agent => Place::Left,
        _ => Place::Right,
    }
}

impl Default for DockState {
    fn default() -> Self {
        // Tests start without the Agent panel, so the others have the
        // whole dock (its own tests open it).
        let mut hidden = [false; N];
        hidden[PanelId::Agent.index()] = cfg!(test);
        DockState {
            left: true,
            right: true,
            hidden,
            collapsed: [false; N],
            show_palette: false,
            layers_sewing_order: false,
            units: Default::default(),
            place: ALL.map(default_place),
            order: ALL.to_vec(),
            float_at: std::array::from_fn(|k| {
                Pos2::new(360.0 + 24.0 * k as f32, 120.0 + 24.0 * k as f32)
            }),
            iconic: [false, false],
            tab_of: [None; N],
            active: [None; N],
            task_bar: true,
            agent_share: 0.55,
            path: None,
        }
    }
}

fn layout_file() -> Option<std::path::PathBuf> {
    config_file("panels.json")
}

/// A settings file in ZigZag's folder (Application Support on a Mac).
pub(crate) fn config_file(name: &str) -> Option<std::path::PathBuf> {
    Some(config_dir()?.join(name))
}

/// ZigZag's settings folder. The app used to be called Threadline: the first
/// time the new folder is missing and the old one is there, the old one is
/// moved over, so sessions, panel layouts and palettes come along.
fn config_dir() -> Option<std::path::PathBuf> {
    use std::sync::OnceLock;
    static DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        // In the browser: a folder of the files kept in memory (`zigzag::vfs`).
        if cfg!(target_arch = "wasm32") {
            return Some(std::path::PathBuf::from("/zigzag"));
        }
        let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
        let (new, old) = if cfg!(target_os = "macos") {
            let base = home.join("Library/Application Support");
            (base.join("ZigZag"), base.join("threadline"))
        } else {
            let base = std::env::var_os("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or(home.join(".config"));
            (base.join("zigzag"), base.join("threadline"))
        };
        if !new.exists() && old.is_dir() {
            // Best effort: if the move fails the app simply starts fresh.
            let _ = zigzag::vfs::rename(&old, &new);
        }
        Some(new)
    })
    .clone()
}

impl DockState {
    /// The panel layout from last time (or the default), saved from now on.
    pub fn load() -> DockState {
        Self::load_from(layout_file())
    }

    fn load_from(path: Option<std::path::PathBuf>) -> DockState {
        let mut d = DockState {
            path: path.clone(),
            ..DockState::default()
        };
        let Some(v) = path
            .and_then(|f| zigzag::vfs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        else {
            return d;
        };
        d.left = v["left"].as_bool().unwrap_or(true);
        d.right = v["right"].as_bool().unwrap_or(true);
        d.show_palette = v["show_palette"].as_bool().unwrap_or(false);
        d.layers_sewing_order = v["layers_sewing_order"].as_bool().unwrap_or(false);
        d.units = v["units"]
            .as_str()
            .and_then(crate::units::Unit::named)
            .unwrap_or_default();
        d.task_bar = v["task_bar"].as_bool().unwrap_or(true);
        d.agent_share = v["agent_share"].as_f64().unwrap_or(0.55).clamp(0.15, 0.95) as f32;
        for (k, id) in ALL.iter().enumerate() {
            let e = &v["panels"][id.key()];
            d.hidden[k] = e["hidden"].as_bool().unwrap_or(false);
            d.collapsed[k] = e["collapsed"].as_bool().unwrap_or(false);
            d.place[k] = match e["place"].as_str() {
                Some("left") => Place::Left,
                Some("right") => Place::Right,
                Some("float") => Place::Float,
                _ => default_place(*id),
            };
            if let (Some(x), Some(y)) = (e["x"].as_f64(), e["y"].as_f64()) {
                d.float_at[k] = Pos2::new(x as f32, y as f32);
            }
        }
        let mut order: Vec<PanelId> = v["order"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| ALL.iter().copied().find(|p| Some(p.key()) == t.as_str()))
            .collect();
        for p in ALL {
            if !order.contains(&p) {
                order.push(p);
            }
        }
        d.order = order;
        // A layout saved before the Agent panel: the Layers list moves to
        // the top of the right dock, and the Agent has the left to itself.
        if v["panels"].get("Agent").is_none() {
            d.place[PanelId::Objects.index()] = Place::Right;
            d.place[PanelId::Agent.index()] = Place::Left;
            d.order.retain(|&p| p != PanelId::Objects);
            d.order.insert(0, PanelId::Objects);
            d.left = true;
        }
        d.iconic = [
            v["iconic"]["left"].as_bool().unwrap_or(false),
            v["iconic"]["right"].as_bool().unwrap_or(false),
        ];
        let by_title = |t: Option<&str>| ALL.iter().copied().find(|p| Some(p.key()) == t);
        for (k, id) in ALL.iter().enumerate() {
            let e = &v["panels"][id.key()];
            d.tab_of[k] = by_title(e["tab_of"].as_str()).filter(|l| l != id);
            d.active[k] = by_title(e["active"].as_str());
        }
        // A lead can't itself be a tab of another: flatten.
        for k in 0..ALL.len() {
            if let Some(l) = d.tab_of[k]
                && d.tab_of[l.index()].is_some()
            {
                d.tab_of[k] = None;
            }
        }
        for k in 0..ALL.len() {
            if let Some(l) = d.tab_of[k] {
                d.place[k] = d.place[l.index()];
            }
        }
        d
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        let panels: serde_json::Map<String, serde_json::Value> = ALL
            .iter()
            .enumerate()
            .map(|(k, id)| {
                (
                    id.key().to_string(),
                    serde_json::json!({
                        "hidden": self.hidden[k],
                        "collapsed": self.collapsed[k],
                        "place": self.place[k].key(),
                        "x": self.float_at[k].x,
                        "y": self.float_at[k].y,
                        "tab_of": self.tab_of[k].map(|l| l.key()),
                        "active": self.active[k].map(|a| a.key()),
                    }),
                )
            })
            .collect();
        let v = serde_json::json!({
            "left": self.left,
            "right": self.right,
            "show_palette": self.show_palette,
            "layers_sewing_order": self.layers_sewing_order,
            "units": self.units.suffix(),
            "task_bar": self.task_bar,
            "agent_share": self.agent_share,
            "panels": panels,
            "order": self.order.iter().map(|p| p.key()).collect::<Vec<_>>(),
            "iconic": { "left": self.iconic[0], "right": self.iconic[1] },
        });
        if let Some(dir) = path.parent() {
            let _ = zigzag::vfs::create_dir_all(dir);
        }
        let _ = zigzag::vfs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default());
    }
}

impl DockState {
    pub fn visible(&self, p: PanelId) -> bool {
        !self.hidden[p.index()]
    }
    pub fn toggle(&mut self, p: PanelId) {
        let i = p.index();
        self.hidden[i] = !self.hidden[i];
        if !self.hidden[i] {
            self.collapsed[i] = false;
            match self.place[i] {
                Place::Left => self.left = true,
                Place::Right => self.right = true,
                Place::Float => {}
            }
        }
    }

    pub fn place_of(&self, p: PanelId) -> Place {
        self.place[p.index()]
    }

    /// Move panel `p` to `to`, before panel `before` (or at the end). A
    /// group's lead takes its tabs along; a tab leaves its group.
    pub fn move_panel(&mut self, p: PanelId, to: Place, before: Option<PanelId>) {
        if self.tab_of[p.index()].is_some() {
            self.separate(p);
        }
        let group = self.members(p);
        let before = before.filter(|b| !group.contains(b));
        self.order.retain(|x| !group.contains(x));
        let at = before
            .and_then(|b| self.order.iter().position(|&x| x == b))
            .unwrap_or(self.order.len());
        for (k, &q) in group.iter().enumerate() {
            self.order.insert(at + k, q);
            self.place[q.index()] = to;
        }
        // Moving a hidden panel shows it; a group moved by a showing tab
        // keeps its hidden lead hidden.
        if group.iter().all(|q| self.hidden[q.index()]) {
            self.hidden[p.index()] = false;
        }
        match to {
            Place::Left => self.left = true,
            Place::Right => self.right = true,
            Place::Float => {}
        }
    }

    /// The group a panel leads or belongs to.
    pub fn lead_of(&self, p: PanelId) -> PanelId {
        self.tab_of[p.index()].unwrap_or(p)
    }

    /// A group's panels: its lead, then its tabs in panel order.
    pub fn members(&self, lead: PanelId) -> Vec<PanelId> {
        let mut v = vec![lead];
        v.extend(
            self.order
                .iter()
                .copied()
                .filter(|&q| self.tab_of[q.index()] == Some(lead)),
        );
        v
    }

    /// Put panel `p` (and its own tabs, if it leads a group) into the group
    /// of `onto`, in front.
    pub fn join_tab(&mut self, p: PanelId, onto: PanelId) {
        let lead = self.lead_of(onto);
        if self.lead_of(p) == lead {
            return;
        }
        let moving = if self.tab_of[p.index()].is_none() {
            self.members(p)
        } else {
            vec![p]
        };
        for q in moving {
            self.tab_of[q.index()] = Some(lead);
            self.place[q.index()] = self.place[lead.index()];
            self.hidden[q.index()] = false;
            // Tabs follow their lead in the order.
            self.order.retain(|&x| x != q);
            let after = self
                .order
                .iter()
                .rposition(|&x| self.lead_of(x) == lead)
                .map_or(self.order.len(), |i| i + 1);
            self.order.insert(after, q);
        }
        self.active[lead.index()] = Some(p);
    }

    /// Take panel `p` out of its group (a lead hands the group to its next tab).
    pub fn separate(&mut self, p: PanelId) {
        if self.tab_of[p.index()].take().is_some() {
            return;
        }
        let followers: Vec<PanelId> = self.members(p).into_iter().skip(1).collect();
        if let Some((&new, rest)) = followers.split_first() {
            self.tab_of[new.index()] = None;
            for &q in rest {
                self.tab_of[q.index()] = Some(new);
            }
            self.float_at[new.index()] = self.float_at[p.index()];
            // The one leaving steps aside rather than covering the group.
            self.float_at[p.index()] = self.float_at[p.index()] + Vec2::new(28.0, 28.0);
            self.collapsed[new.index()] = self.collapsed[p.index()];
            let front = self.active[p.index()].take().filter(|a| *a != p);
            self.active[new.index()] = front;
        }
    }

    /// Back to the standard layout (Window › Reset Panels).
    pub fn reset_layout(&mut self) {
        let path = self.path.take();
        let (show_palette, units) = (self.show_palette, self.units);
        let layers_sewing_order = self.layers_sewing_order;
        *self = DockState {
            path,
            show_palette,
            layers_sewing_order,
            units,
            ..DockState::default()
        };
    }
    pub fn collapsed(&self, p: PanelId) -> bool {
        self.collapsed[p.index()]
    }

    /// Show panel `p` open: unhidden, its group expanded with it in front,
    /// its dock on.
    pub fn reveal(&mut self, p: PanelId) {
        let lead = self.lead_of(p);
        self.hidden[p.index()] = false;
        self.collapsed[lead.index()] = false;
        if lead != p {
            self.active[lead.index()] = Some(p);
        }
        match self.place[lead.index()] {
            Place::Left => self.left = true,
            Place::Right => self.right = true,
            Place::Float => {}
        }
    }
}

impl Studio {
    /// The name a panel's header shows: the Agent's is the agent's own
    /// ("Claude Code", "Codex").
    pub fn panel_name(&self, p: PanelId) -> &'static str {
        if p == PanelId::Agent {
            self.agent.kind.name()
        } else {
            p.title()
        }
    }
}

/// Where a panel's open/closed animation lives (the same in any dock).
fn body_id(lead: PanelId) -> egui::Id {
    egui::Id::unique(("panel-body", lead.index()))
}

/// The height of a panel header.
pub const HEADER_H: f32 = 26.0;

impl Studio {
    /// A panel: a header bar that collapses it, then its body.
    fn panel(
        &mut self,
        ui: &mut egui::Ui,
        id: PanelId,
        body: impl FnOnce(&mut Studio, &mut egui::Ui),
    ) {
        self.panel_with(ui, id, None, body);
    }

    /// A panel whose header also shows `detail`: (what it's showing, a
    /// figure on the right), like the Stitch panel's object and stitch count.
    fn panel_with(
        &mut self,
        ui: &mut egui::Ui,
        id: PanelId,
        detail: Option<(String, String)>,
        body: impl FnOnce(&mut Studio, &mut egui::Ui),
    ) {
        if !self.dock.visible(id) {
            return;
        }
        let tabs = self.dock_ui.tabs.clone();
        // A group collapses and moves as one, by its lead.
        let key = tabs.as_ref().map_or(id, |t| t.0);
        let p = pal_ui(ui);
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 26.0),
            Sense::click_and_drag(),
        );
        // Drag the header to move the panel (or the whole group).
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(at) = ui.input(|i| i.pointer.press_origin())
        {
            self.begin_panel_drag(key, rect, at, true);
        }
        if resp.hovered() && !resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{} panel", id.title()))
        });
        if let Some((what, figure)) = &detail {
            resp.clone().on_hover_text(format!("{what} · {figure}"));
        }
        // The first panel in a dock holds the dock's « / » at its right end,
        // so the dock needs no bar of its own.
        let collapse_side = if self.dock_ui.in_window {
            None
        } else {
            self.dock_ui.collapse_btn.take()
        };
        let inset = if collapse_side.is_some() { 24.0 } else { 0.0 };
        let painter = ui.painter();
        // In a floating window the header follows its rounded top corners.
        let corners = if self.dock_ui.in_window {
            egui::CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            }
        } else {
            egui::CornerRadius::ZERO
        };
        painter.rect_filled(rect, corners, p.header);
        painter.hline(
            rect.x_range(),
            rect.top() + 0.5,
            Stroke::new(1.0, p.separator),
        );
        painter.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0, p.separator),
        );
        // Alone in a dock there's nothing to make room for: it can't be
        // collapsed (no chevron, no Collapse in its menu).
        let alone = tabs.is_none()
            && !self.dock_ui.in_window
            && match self.dock_ui.current {
                Some(side @ (Place::Left | Place::Right)) => self.panels_in(side).len() == 1,
                _ => false,
            };
        if alone && self.dock.collapsed(key) {
            self.dock.collapsed[key.index()] = false;
        }
        let collapsed = self.dock.collapsed(key);
        if !alone {
            let chev = Rect::from_center_size(
                Pos2::new(rect.left() + 14.0, rect.center().y),
                Vec2::splat(9.0),
            );
            icons::paint(
                painter,
                chev,
                if collapsed {
                    Icon::ChevronRight
                } else {
                    Icon::ChevronDown
                },
                p.secondary,
            );
        }
        let title_x = rect.left() + if alone { 12.0 } else { 26.0 };
        let title = match &tabs {
            // "Agent: Claude Code ⌄": the name picks the agent.
            None if id == PanelId::Agent => {
                let font = egui::FontId::proportional(12.0);
                let a = painter.text(
                    Pos2::new(title_x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    "Agent:",
                    font.clone(),
                    p.secondary,
                );
                let n = painter.text(
                    Pos2::new(a.right() + 4.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    self.agent.kind.name(),
                    font,
                    p.text,
                );
                let chev = Rect::from_center_size(
                    Pos2::new(n.right() + 9.0, rect.center().y + 1.0),
                    Vec2::splat(8.0),
                );
                icons::paint(painter, chev, Icon::ChevronDown, p.secondary);
                a.union(chev)
            }
            None => painter.text(
                Pos2::new(title_x, rect.center().y),
                egui::Align2::LEFT_CENTER,
                self.panel_name(id),
                egui::FontId::proportional(12.0),
                p.text,
            ),
            Some((lead, members)) => {
                // One tab per panel: click to bring it to the front, drag
                // it out of the group.
                let mut x = rect.left() + 24.0;
                let mut last = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::ZERO);
                for &t in members {
                    let galley = ui.painter().layout_no_wrap(
                        self.panel_name(t).to_string(),
                        egui::FontId::proportional(12.0),
                        p.text,
                    );
                    let tab = Rect::from_min_max(
                        Pos2::new(x, rect.top() + 3.0),
                        Pos2::new(x + galley.size().x + 16.0, rect.bottom()),
                    );
                    let front = t == id;
                    let tr = ui.interact(
                        tab,
                        ui.make_persistent_id(("panel-tab", lead.index(), t.index())),
                        Sense::click_and_drag(),
                    );
                    tr.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::Role::Tab,
                            true,
                            front,
                            format!("{} tab", t.title()),
                        )
                    });
                    if front {
                        let r = egui::CornerRadius {
                            nw: 5,
                            ne: 5,
                            sw: 0,
                            se: 0,
                        };
                        ui.painter().rect_filled(tab, r, p.dock);
                    } else if tr.hovered() {
                        let r = egui::CornerRadius {
                            nw: 5,
                            ne: 5,
                            sw: 0,
                            se: 0,
                        };
                        ui.painter().rect_filled(tab, r, p.hover);
                    }
                    ui.painter().galley(
                        Pos2::new(tab.left() + 8.0, tab.center().y - galley.size().y / 2.0),
                        galley,
                        if front { p.text } else { p.secondary },
                    );
                    if tr.clicked() {
                        self.dock.active[lead.index()] = Some(t);
                        self.dock.collapsed[lead.index()] = false;
                    }
                    if tr.drag_started_by(egui::PointerButton::Primary)
                        && let Some(at) = ui.input(|i| i.pointer.press_origin())
                    {
                        self.begin_panel_drag(t, tab, at, false);
                    }
                    last = tab;
                    x = tab.right() + 2.0;
                }
                last
            }
        };
        let painter = ui.painter();
        if let Some((what, figure)) = &detail {
            let right = painter.text(
                Pos2::new(rect.right() - 34.0 - inset, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                figure,
                egui::FontId::proportional(11.0),
                p.secondary,
            );
            // The name, cut to fit between the title and the figure.
            let room = (right.left() - title.right() - 22.0).max(0.0);
            let galley = painter.layout(
                what.clone(),
                egui::FontId::proportional(12.0),
                p.secondary,
                f32::INFINITY,
            );
            let mut job = egui::text::LayoutJob::simple_singleline(
                what.clone(),
                egui::FontId::proportional(12.0),
                p.secondary,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(room);
            let g = if galley.size().x > room {
                painter.layout_job(job)
            } else {
                galley
            };
            painter.galley(
                Pos2::new(title.right() + 8.0, rect.center().y - g.size().y / 2.0),
                g,
                p.secondary,
            );
        }
        // The agent's name opens a small menu of agents.
        let mut picked_agent = false;
        if id == PanelId::Agent && tabs.is_none() {
            let pick = Rect::from_min_max(
                Pos2::new(title.left() - 5.0, rect.top() + 3.0),
                Pos2::new(title.right() + 5.0, rect.bottom() - 3.0),
            );
            let presp = ui.interact(pick, ui.make_persistent_id("agent-pick"), Sense::click());
            presp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, "Choose Agent")
            });
            let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&presp));
            if presp.hovered() || open {
                ui.painter().rect_filled(pick, 4.0, p.hover);
            }
            picked_agent = presp.clicked();
            egui::Popup::menu(&presp)
                .align(egui::RectAlign::BOTTOM_START)
                .show(|ui| {
                    ui.set_min_width(150.0);
                    // (A menu's own buttons sit tight to the left: room.)
                    ui.spacing_mut().button_padding = Vec2::new(12.0, 4.0);
                    use crate::agent::AgentKind;
                    for k in AgentKind::ALL {
                        if ui
                            .add(egui::Button::new(k.name()).selected(self.agent.kind == k))
                            .clicked()
                        {
                            self.agent.set_kind(k);
                            ui.close();
                        }
                    }
                });
        }
        // The panel menu, at the right of the header (Illustrator's flyout).
        let mrect = Rect::from_center_size(
            Pos2::new(rect.right() - 16.0 - inset, rect.center().y),
            Vec2::new(22.0, 20.0),
        );
        let mut collapse_clicked = false;
        if let Some(side) = collapse_side {
            let crect = Rect::from_center_size(
                Pos2::new(rect.right() - 16.0, rect.center().y),
                Vec2::new(22.0, 20.0),
            );
            let cresp = ui.interact(
                crect,
                ui.make_persistent_id(("dock-collapse", side.side())),
                Sense::click(),
            );
            cresp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, "Collapse to Icons")
            });
            if cresp.hovered() {
                ui.painter().rect_filled(crect, 4.0, p.hover);
            }
            icons::paint(
                ui.painter(),
                Rect::from_center_size(crect.center(), Vec2::splat(10.0)),
                if side == Place::Left {
                    Icon::ChevronsLeft
                } else {
                    Icon::ChevronsRight
                },
                if cresp.hovered() { p.text } else { p.secondary },
            );
            if cresp.on_hover_text("Collapse to icons").clicked() {
                self.dock.iconic[side.side()] = true;
                collapse_clicked = true;
            }
        }
        let mresp = ui.interact(
            mrect,
            ui.make_persistent_id(("panel-menu", id.index())),
            Sense::click(),
        );
        mresp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{} menu", id.title()))
        });
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&mresp));
        if mresp.hovered() || open {
            ui.painter().rect_filled(mrect, 4.0, p.hover);
        }
        icons::paint(
            ui.painter(),
            Rect::from_center_size(mrect.center(), Vec2::splat(11.0)),
            Icon::Menu,
            if mresp.hovered() || open {
                p.text
            } else {
                p.secondary
            },
        );
        // The Agent's header: + starts a new chat.
        if id == PanelId::Agent {
            let nrect = Rect::from_center_size(
                Pos2::new(rect.right() - 40.0 - inset, rect.center().y),
                Vec2::new(22.0, 20.0),
            );
            let some = !self.agent.log.is_empty() || self.agent.running();
            let nresp = ui.interact(
                nrect,
                ui.make_persistent_id("agent-new-chat"),
                if some { Sense::click() } else { Sense::hover() },
            );
            nresp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, some, "New Chat"));
            if some && nresp.hovered() {
                ui.painter().rect_filled(nrect, 4.0, p.hover);
            }
            icons::paint(
                ui.painter(),
                Rect::from_center_size(nrect.center(), Vec2::splat(11.0)),
                Icon::Plus,
                if !some {
                    p.tertiary
                } else if nresp.hovered() {
                    p.text
                } else {
                    p.secondary
                },
            );
            if nresp
                .on_hover_text("New chat: the agent forgets this conversation")
                .clicked()
            {
                self.agent.new_chat();
            }
        }
        egui::Popup::menu(&mresp)
            .align(egui::RectAlign::BOTTOM_END)
            .show(|ui| {
                ui.set_min_width(170.0);
                if id == PanelId::Agent {
                    // (Which agent: the name in the header.)
                    if ui.button("New Chat").clicked() {
                        self.agent.new_chat();
                        ui.close();
                    }
                    ui.separator();
                }
                if id == PanelId::Objects {
                    let on = self.dock.layers_sewing_order;
                    if ui
                        .add(egui::Button::new("List in Sewing Order").selected(on))
                        .on_hover_text(
                            "List the layers as they sew, the first at the top. Off: what's on top is at the top, as in Illustrator.",
                        )
                        .clicked()
                    {
                        self.dock.layers_sewing_order = !on;
                        ui.close();
                    }
                    ui.separator();
                }
                if id == PanelId::Threads {
                    if ui
                        .add(egui::Button::new("Show Palette").selected(self.dock.show_palette))
                        .clicked()
                    {
                        self.dock.show_palette = !self.dock.show_palette;
                        ui.close();
                    }
                    let name = self.palettes.name(self.palettes.source);
                    if ui
                        .button(format!("Match All to {name}"))
                        .on_hover_text(
                            "Change every color in the design to the nearest one in this palette",
                        )
                        .clicked()
                    {
                        self.match_all_to_palette();
                        ui.close();
                    }
                    ui.separator();
                }
                if !alone {
                    if ui
                        .button(if collapsed { "Expand" } else { "Collapse" })
                        .clicked()
                    {
                        self.dock.collapsed[key.index()] = !collapsed;
                        ui.close();
                    }
                    ui.separator();
                }
                let here = self.dock.place_of(key);
                for (to, label) in [
                    (Place::Left, "Dock Left"),
                    (Place::Right, "Dock Right"),
                    (Place::Float, "Float"),
                ] {
                    if to != here && ui.button(label).clicked() {
                        self.dock.move_panel(key, to, None);
                        self.dock_ui.flyout = None;
                        ui.close();
                    }
                }
                if tabs.is_some() && ui.button(format!("Separate {}", id.title())).clicked() {
                    // Out of the group, just below it.
                    let place = self.dock.place_of(key);
                    let group = self.dock.members(key);
                    let last = self
                        .dock
                        .order
                        .iter()
                        .rposition(|x| group.contains(x))
                        .unwrap_or(0);
                    let next = self.dock.order[last + 1..].iter().copied().find(|&x| {
                        self.dock.tab_of[x.index()].is_none() && self.dock.place_of(x) == place
                    });
                    self.dock.separate(id);
                    self.dock.move_panel(id, place, next);
                    ui.close();
                }
                ui.separator();
                if ui.button("Hide Panel").clicked() {
                    self.dock.toggle(id);
                    self.dock_ui.flyout = None;
                    ui.close();
                }
            });
        if resp.clicked() && !mresp.clicked() && !collapse_clicked && !picked_agent && !alone {
            self.dock.collapsed[key.index()] = !collapsed;
        }
        if id == PanelId::Agent && collapsed {
            // Working with the transcript put away: the step and the time
            // after the title (the transcript's own spinner shows otherwise).
            self.paint_agent_header(ui, title, rect);
        }
        // The body opens and closes smoothly.
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            body_id(key),
            !collapsed,
        );
        state.set_open(!collapsed);
        state.show_body_unindented(ui, |ui| {
            egui::Frame::new()
                .inner_margin(theme::pad())
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    body(self, ui);
                });
        });
    }

    // ------------------------------------------------------------ left dock

    /// The panels (group leads) in a place, in order, that have something
    /// to show.
    pub fn panels_in(&self, place: Place) -> Vec<PanelId> {
        let mut v: Vec<PanelId> = self
            .dock
            .order
            .iter()
            .copied()
            .filter(|&p| {
                self.dock.tab_of[p.index()].is_none()
                    && self.dock.place_of(p) == place
                    && !self.group_shown(p).is_empty()
            })
            .collect();
        // Lettering comes to the top of its dock while text is selected,
        // as Illustrator brings the Character panel forward: its settings
        // aren't buried under the others.
        if let Some(k) = v.iter().position(|&p| p == PanelId::Lettering)
            && k > 0
        {
            let l = v.remove(k);
            v.insert(0, l);
        }
        v
    }

    /// The panels of a group that are showing (visible, with something to show).
    pub fn group_shown(&self, lead: PanelId) -> Vec<PanelId> {
        self.dock
            .members(lead)
            .into_iter()
            .filter(|&q| self.dock.visible(q) && self.applies(q))
            .collect()
    }

    /// Every panel showing in a place, tabs included (for icon strips).
    pub fn panels_flat_in(&self, place: Place) -> Vec<PanelId> {
        self.panels_in(place)
            .into_iter()
            .flat_map(|l| self.group_shown(l))
            .collect()
    }

    /// Lettering only shows with lettering selected.
    fn applies(&self, p: PanelId) -> bool {
        match p {
            PanelId::Lettering => {
                self.sel_indices().len() == 1
                    && self.doc.xml.contains("data-tl-lettering")
                    && self
                        .sel_index()
                        .is_some_and(|i| self.doc.lettering(i).is_some())
            }
            _ => true,
        }
    }

    /// Whether a dock has anything in it (or is a drop target mid-drag).
    pub fn dock_shown(&self, side: Place) -> bool {
        let on = match side {
            Place::Left => self.dock.left,
            _ => self.dock.right,
        };
        on && (!self.panels_in(side).is_empty() || self.dock_ui.drag.is_some())
    }

    pub fn dock_iconic(&self, side: Place) -> bool {
        self.dock.iconic[side.side()]
    }

    /// One panel, header and body.
    fn show_panel(&mut self, ui: &mut egui::Ui, id: PanelId, place: Place) {
        self.dock_ui.current = Some(place);
        let top = ui.cursor().top();
        // A tab group shows one header with a tab each, and its front tab.
        let lead = self.dock.lead_of(id);
        let shown = self.group_shown(lead);
        let id = if shown.len() > 1 {
            let front = self.dock.active[lead.index()]
                .filter(|a| shown.contains(a))
                .unwrap_or(shown[0]);
            self.dock_ui.tabs = Some((lead, shown));
            front
        } else {
            shown.first().copied().unwrap_or(id)
        };
        match id {
            PanelId::Stitch => {
                let d = self.stitch_header();
                self.panel_with(ui, id, d, |s, ui| s.stitch_body(ui));
                crate::profile::mark("d:stitch");
            }
            PanelId::Transform => {
                self.panel(ui, id, |s, ui| s.transform_body(ui));
                crate::profile::mark("d:transform");
            }
            PanelId::Lettering => {
                self.panel(ui, id, |s, ui| {
                    if let Some(i) = s.sel_index() {
                        s.lettering_panel(ui, i);
                    }
                });
                crate::profile::mark("d:lettering");
            }
            PanelId::Threads => {
                self.panel(ui, id, |s, ui| s.threads_body(ui));
                crate::profile::mark("d:threads");
            }
            PanelId::Document => {
                self.panel(ui, id, |s, ui| s.document_body(ui));
                crate::profile::mark("d:document");
            }
            PanelId::Objects => {
                self.panel(ui, id, |s, ui| s.objects_body(ui));
                crate::profile::mark("d:objects");
            }
            PanelId::Agent => {
                self.panel(ui, id, |s, ui| s.agent_body(ui));
                crate::profile::mark("d:agent");
            }
        }
        self.dock_ui.tabs = None;
        let r = Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.cursor().top());
        self.dock_ui.drawn.push((place, lead, r));
    }

    /// A dock: its panels in a column, or (collapsed) a strip of icons.
    pub fn dock_column(&mut self, ui: &mut egui::Ui, side: Place) {
        let p = pal_ui(ui);
        self.dock_ui.columns[side.side()] = Some(ui.max_rect());
        self.dock_ui.column_h[side.side()] = ui.max_rect().height();
        let ids = self.panels_in(side);
        if self.dock_iconic(side) {
            // A header-high bar over the icons holding » / « (expand), level
            // with the headers beside it.
            let (bar, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), HEADER_H), Sense::hover());
            ui.painter().rect_filled(bar, 0.0, p.header);
            ui.painter().hline(
                bar.x_range(),
                bar.bottom() - 0.5,
                Stroke::new(1.0, p.separator),
            );
            let b = Rect::from_center_size(bar.center(), Vec2::new(22.0, 20.0));
            let resp = ui.interact(
                b,
                ui.make_persistent_id(("dock-expand", side.side())),
                Sense::click(),
            );
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, "Expand Panels")
            });
            if resp.hovered() {
                ui.painter().rect_filled(b, 4.0, p.hover);
            }
            icons::paint(
                ui.painter(),
                Rect::from_center_size(b.center(), Vec2::splat(10.0)),
                if side == Place::Left {
                    Icon::ChevronsRight
                } else {
                    Icon::ChevronsLeft
                },
                if resp.hovered() { p.text } else { p.secondary },
            );
            if resp.on_hover_text("Expand the panels").clicked() {
                self.dock.iconic[side.side()] = false;
                self.dock_ui.flyout = None;
            }
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = theme::space::XS;
                ui.add_space(theme::space::S);
                for id in self.panels_flat_in(side) {
                    let on = self.dock_ui.flyout == Some(id);
                    // Click for the flyout; drag to move the panel elsewhere.
                    let (rect, r) =
                        ui.allocate_exact_size(Vec2::splat(30.0), Sense::click_and_drag());
                    r.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::Button, true, on, id.title())
                    });
                    let bg = if on {
                        Some(if p.dark {
                            Color32::from_rgb(70, 70, 74)
                        } else {
                            Color32::from_rgb(222, 222, 224)
                        })
                    } else if r.hovered() {
                        Some(p.hover)
                    } else {
                        None
                    };
                    if let Some(bg) = bg {
                        ui.painter().rect_filled(rect, 6.0, bg);
                    }
                    icons::paint(
                        ui.painter(),
                        rect.shrink(8.0),
                        id.icon(),
                        if on { p.accent } else { p.text },
                    );
                    self.dock_ui.drawn.push((side, id, rect));
                    if r.drag_started_by(egui::PointerButton::Primary)
                        && let Some(at) = ui.input(|i| i.pointer.press_origin())
                    {
                        self.begin_panel_drag(id, rect, at, false);
                    }
                    if r.on_hover_text(id.title()).clicked() {
                        self.dock_ui.flyout = if on { None } else { Some(id) };
                        // A tab opens with its group, itself in front.
                        let lead = self.dock.lead_of(id);
                        self.dock.active[lead.index()] = Some(id);
                    }
                }
            });
            return;
        }
        // The Agent's group sits at the bottom of the dock, the rest above
        // it (scrolling on their own), a divider between to share the height.
        let pinned = ids
            .iter()
            .copied()
            .find(|&l| self.group_shown(l).contains(&PanelId::Agent));
        let rest: Vec<PanelId> = ids.iter().copied().filter(|&l| Some(l) != pinned).collect();
        ui.spacing_mut().item_spacing.y = 0.0;
        let total = ui.available_height();
        const DIVIDER: f32 = 7.0;
        let mut top_h = total;
        let mut block = None;
        if let Some(lead) = pinned {
            let full = if rest.is_empty() {
                total
            } else {
                (total * self.dock.agent_share)
                    .clamp(200.0_f32.min(total), (total - 120.0).max(200.0))
            };
            // Collapsing it gives the others the room back, smoothly.
            let open = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                body_id(lead),
                !self.dock.collapsed(lead),
            )
            .openness(ui.ctx());
            let h = HEADER_H + (full - HEADER_H) * open;
            if !rest.is_empty() {
                top_h = (total - h - DIVIDER).max(0.0);
            }
            block = Some((lead, full));
        }
        // The first panel's header carries the dock's « / ».
        self.dock_ui.collapse_btn = Some(side);
        if !rest.is_empty() || pinned.is_none() {
            egui::ScrollArea::vertical()
                .id_salt(("dock", side.side()))
                .auto_shrink([false, false])
                .max_height(top_h)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for id in rest {
                        self.show_panel(ui, id, side);
                    }
                    if self.dock_ui.drag.is_some() {
                        // Room to drop at the end of the column.
                        ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 60.0),
                            Sense::hover(),
                        );
                    }
                });
            if let Some((lead, _)) = block {
                // The divider: drag it to give the Agent more or less room.
                let (r, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), DIVIDER), Sense::drag());
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Splitter, true, "Agent panel divider")
                });
                let on = resp.hovered() || resp.dragged();
                if on && !self.dock.collapsed(lead) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                }
                ui.painter().hline(
                    r.x_range(),
                    r.center().y,
                    Stroke::new(
                        if on { 2.0 } else { 1.0 },
                        if on { p.accent } else { p.separator },
                    ),
                );
                if resp.dragged() && !self.dock.collapsed(lead) && total > 0.0 {
                    let share = self.dock.agent_share - resp.drag_delta().y / total;
                    self.dock.agent_share = share.clamp(0.15, 0.95);
                }
            }
        }
        if let Some((lead, full)) = block {
            ui.spacing_mut().item_spacing.y = 0.0;
            self.dock_ui.pinned_h = Some(full);
            self.show_panel(ui, lead, side);
            self.dock_ui.pinned_h = None;
        }
        // (Nothing in the dock to carry it: an empty dock mid-drag.)
        self.dock_ui.collapse_btn = None;
    }

    /// Panels outside the docks: floating ones, and the flyout from an icon strip.
    pub fn floating_panels(&mut self, ctx: &egui::Context) {
        for id in self.panels_in(Place::Float) {
            let at = self.dock.float_at[id.index()];
            egui::Area::new(egui::Id::unique(("float-panel", id.index())))
                .order(egui::Order::Middle)
                .fixed_pos(at)
                .constrain(true)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .inner_margin(egui::Margin::ZERO)
                        .corner_radius(8)
                        .show(ui, |ui| {
                            ui.set_width(float_w(id));
                            ui.spacing_mut().item_spacing.y = 0.0;
                            self.dock_ui.in_window = true;
                            self.show_panel(ui, id, Place::Float);
                            self.dock_ui.in_window = false;
                        });
                });
        }
        // The flyout beside an icon strip.
        let Some(id) = self.dock_ui.flyout else {
            return;
        };
        let place = self.dock.place_of(id);
        let side_iconic = place != Place::Float && self.dock_iconic(place);
        let anchor = self
            .dock_ui
            .drawn
            .iter()
            .find(|(pl, p, _)| *p == id && *pl == place)
            .map(|x| x.2);
        let (Some(anchor), true) = (anchor, side_iconic) else {
            self.dock_ui.flyout = None;
            return;
        };
        let (pivot, pos) = match place {
            Place::Left => (
                egui::Align2::LEFT_TOP,
                Pos2::new(anchor.right() + theme::space::S, anchor.top()),
            ),
            _ => (
                egui::Align2::RIGHT_TOP,
                Pos2::new(anchor.left() - theme::space::S, anchor.top()),
            ),
        };
        let max_h = (ctx.content_rect().bottom() - pos.y - 40.0).max(200.0);
        let area = egui::Area::new(egui::Id::unique("panel-flyout"))
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(pos)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::ZERO)
                    .corner_radius(8)
                    .show(ui, |ui| {
                        ui.set_width(float_w(id));
                        egui::ScrollArea::vertical()
                            .id_salt("flyout-scroll")
                            .max_height(max_h)
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                self.dock_ui.in_window = true;
                                self.show_panel(ui, id, place);
                                self.dock_ui.in_window = false;
                            });
                    });
            });
        // A click elsewhere closes it (not one in its own menus).
        let outside = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !area.response.rect.contains(p) && !anchor.contains(p))
        });
        if outside && !egui::Popup::is_any_open(ctx) {
            self.dock_ui.flyout = None;
        }
    }

    /// Where a dragged panel would land if dropped at `pos`: on another
    /// panel's header (into its tab group), in a dock before a panel, or
    /// floating.
    fn drop_target(&self, pos: Pos2, d: PanelDrag) -> DropAt {
        let own = if d.whole {
            self.dock.lead_of(d.id)
        } else {
            d.id
        };
        // Another panel's header (an icon in a strip counts as one): into
        // its group. Its own group's header leaves things as they are.
        for (_, lead, r) in &self.dock_ui.drawn {
            let header = Rect::from_min_size(r.min, Vec2::new(r.width(), r.height().min(26.0)));
            // Its top edge inserts above it instead (an icon has no edge).
            let edge = if r.height() > 30.0 { 7.0 } else { 0.0 };
            if header.contains(pos) && pos.y > header.top() + edge {
                return DropAt::Tab(self.dock.lead_of(*lead));
            }
        }
        for side in [Place::Left, Place::Right] {
            if self.dock_ui.columns[side.side()].is_some_and(|r| r.contains(pos)) {
                let before = self
                    .dock_ui
                    .drawn
                    .iter()
                    .filter(|(pl, p, _)| *pl == side && *p != own)
                    .find(|(_, _, r)| pos.y < r.center().y)
                    .map(|x| x.1);
                return DropAt::Dock(side, before);
            }
        }
        DropAt::Float
    }

    /// Start dragging a panel by its header.
    fn begin_panel_drag(&mut self, id: PanelId, header: Rect, pointer: Pos2, whole: bool) {
        self.dock_ui.drag = Some(PanelDrag {
            id,
            grab: pointer - header.min,
            whole,
        });
        self.dock_ui.flyout = None;
    }

    /// Feedback while a panel is dragged, and the drop. Call after the
    /// panels are drawn.
    pub fn panel_drag(&mut self, ctx: &egui::Context) {
        // Each frame starts with a fresh record of where panels were drawn.
        let Some(d) = self.dock_ui.drag else {
            self.dock_ui.drawn.clear();
            self.dock_ui.columns = [None, None];
            return;
        };
        let (pos, down) = ctx.input(|i| (i.pointer.interact_pos(), i.pointer.primary_down()));
        let Some(pos) = pos else {
            self.dock_ui.drag = None;
            return;
        };
        let target = self.drop_target(pos, d);
        let pal = crate::theme::pal(ctx);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("panel-drag"),
        ));
        // Whether the dragged panel floats on its own already (it then just
        // follows the pointer).
        let floats_alone = self.dock.place_of(d.id) == Place::Float
            && (d.whole || self.dock.members(self.dock.lead_of(d.id)).len() == 1);
        match target {
            DropAt::Float => {
                if floats_alone {
                    let lead = self.dock.lead_of(d.id);
                    self.dock.float_at[lead.index()] = pos - d.grab;
                } else {
                    let r = Rect::from_min_size(pos - d.grab, Vec2::new(FLOAT_W, 26.0));
                    painter.rect_filled(r, 6.0, pal.accent.gamma_multiply(0.25));
                    painter.rect_stroke(
                        r,
                        6.0,
                        Stroke::new(1.0, pal.accent),
                        egui::StrokeKind::Inside,
                    );
                    painter.text(
                        r.left_center() + Vec2::new(10.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        d.id.title(),
                        egui::FontId::proportional(12.0),
                        pal.text,
                    );
                }
            }
            DropAt::Tab(lead) => {
                // The header it would join, outlined.
                if lead != self.dock.lead_of(d.id)
                    && let Some((_, _, r)) = self.dock_ui.drawn.iter().find(|x| x.1 == lead)
                {
                    let header =
                        Rect::from_min_size(r.min, Vec2::new(r.width(), r.height().min(26.0)));
                    painter.rect_filled(header, 4.0, pal.accent.gamma_multiply(0.2));
                    painter.rect_stroke(
                        header,
                        4.0,
                        Stroke::new(2.0, pal.accent),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            DropAt::Dock(side, before) => {
                let own = self.dock.lead_of(d.id);
                let col = self.dock_ui.columns[side.side()].unwrap_or(Rect::NOTHING);
                let y = match before {
                    Some(b) => self
                        .dock_ui
                        .drawn
                        .iter()
                        .find(|x| x.0 == side && x.1 == b)
                        .map(|x| x.2.top())
                        .unwrap_or(col.top()),
                    None => self
                        .dock_ui
                        .drawn
                        .iter()
                        .filter(|x| x.0 == side && x.1 != own)
                        .map(|x| x.2.bottom())
                        .fold(col.top() + 20.0, f32::max),
                };
                painter.line_segment(
                    [
                        Pos2::new(col.left() + 4.0, y),
                        Pos2::new(col.right() - 4.0, y),
                    ],
                    Stroke::new(3.0, pal.accent),
                );
            }
        }
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        if !down {
            self.dock_ui.drag = None;
            match target {
                DropAt::Tab(lead) => {
                    if lead != self.dock.lead_of(d.id) {
                        if !d.whole {
                            self.dock.separate(d.id);
                        }
                        self.dock.join_tab(d.id, lead);
                    }
                }
                DropAt::Float if floats_alone => {}
                DropAt::Float => {
                    if !d.whole {
                        self.dock.separate(d.id);
                    }
                    self.dock.float_at[d.id.index()] = pos - d.grab;
                    self.dock.move_panel(d.id, Place::Float, None);
                }
                DropAt::Dock(side, before) => {
                    if !d.whole {
                        self.dock.separate(d.id);
                    }
                    self.dock.move_panel(d.id, side, before);
                }
            }
        }
        ctx.request_repaint();
        self.dock_ui.drawn.clear();
        self.dock_ui.columns = [None, None];
    }

    /// The Layers list as a tree: groups (open or closed) and their objects,
    /// what's on top first (as Illustrator's Layers, and VectorCraft's): the
    /// list reads as the stack does, so the last layer sewn is the first row
    /// and the background the last. Or, with the panel menu's List in Sewing Order,
    /// the other way up: what's sewn first is first. Layers aren't groups
    /// (see `svg::layer_ids`), and a group of one object is listed as the
    /// object.
    fn object_rows(&self) -> Vec<ObjRow> {
        let n = self.doc.shapes.len();
        let mut members: std::collections::HashMap<&str, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, s) in self.doc.shapes.iter().enumerate() {
            for g in &s.groups {
                members.entry(g.as_str()).or_default().push(i);
            }
        }
        let mut rows = Vec::new();
        let mut stack: Vec<String> = Vec::new();
        let sewn = self.dock.layers_sewing_order;
        for i in (0..n).map(|k| if sewn { k } else { n - 1 - k }) {
            // A group of one (a shape and its outline are one) is shown as
            // just its object.
            let gs: Vec<&String> = self.doc.shapes[i]
                .groups
                .iter()
                .filter(|g| !crate::layout::one_thing(&self.doc.shapes, &members[g.as_str()]))
                .collect();
            let mut k = 0;
            while k < stack.len() && k < gs.len() && stack[k] == *gs[k] {
                k += 1;
            }
            stack.truncate(k);
            for g in &gs[k..] {
                if stack.iter().all(|s| self.layout.expanded.contains(s)) {
                    rows.push(ObjRow::Group {
                        id: (*g).clone(),
                        depth: stack.len(),
                        members: members[g.as_str()].clone(),
                    });
                }
                stack.push((*g).clone());
            }
            if stack.iter().all(|s| self.layout.expanded.contains(s)) {
                rows.push(ObjRow::Item {
                    i,
                    depth: stack.len(),
                });
            }
        }
        rows
    }

    fn objects_body(&mut self, ui: &mut egui::Ui) {
        let p = pal_ui(ui);
        let rows = self.object_rows();
        // The list fills its dock when it's alone there; sharing, it's as
        // tall as its rows, up to a third of the dock (then it scrolls).
        let list_h = match self.dock_ui.current {
            Some(Place::Float) | None => 320.0,
            Some(side) if self.dock_iconic(side) => 320.0,
            Some(side) => {
                let h = self.dock_ui.column_h[side.side()];
                if self.panels_in(side).len() > 1 {
                    (rows.len().max(3) as f32 * 26.0 + 8.0).clamp(110.0, (h * 0.36).max(110.0))
                } else {
                    (h - 20.0 - 26.0 - 64.0).max(120.0)
                }
            }
        };
        let sel = self.sel_index();
        let all_sel: std::collections::HashSet<usize> = self.sel_indices().into_iter().collect();
        // Objects with a warning, found once rather than per row.
        let warned: std::collections::HashSet<usize> = self
            .issues
            .iter()
            .filter(|x| x.level >= Level::Warning)
            .filter_map(|x| x.shape)
            .collect();
        let toggle = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        let mut clicked = None;
        let mut group_click: Option<Vec<usize>> = None;
        // A name being typed for an object that's no longer there (another
        // document, an undo) is dropped.
        if self
            .renaming
            .as_ref()
            .is_some_and(|r| !self.doc.shapes.iter().any(|s| s.uid == r.0))
        {
            self.renaming = None;
        }
        // The name typed, when it's done with: (uid, name), or `None` for
        // the name left as it was.
        let mut renamed: Option<Option<(usize, String)>> = None;
        // Only the rows in view are laid out and painted (a traced design
        // has hundreds; each row is text, an icon and a hover check).
        const ROW: f32 = 26.0;
        // An eye or a lock clicked: whose, which, and with Option held
        // (then it's everything else's).
        let mut flag_click: Option<(Vec<usize>, Flag, bool)> = None;
        let others = ui.input(|i| i.modifiers.alt);
        // Layers dragged to another place in the sewing order: where the
        // pointer is while some are (the rows under it say where they'd
        // go: before which layer, and the line that shows it, its height
        // and its left end); and a drag begun this frame.
        let over = self
            .dock_ui
            .layer_drag
            .as_ref()
            .and_then(|_| ui.input(|i| i.pointer.interact_pos()));
        let mut drop: Option<(usize, f32, f32)> = None;
        let mut drag_start: Option<(Vec<usize>, String)> = None;
        // Which way up the list is: a row up is a layer later, unless it's
        // in sewing order (then it's a layer earlier).
        let sewn = self.dock.layers_sewing_order;
        egui::ScrollArea::vertical()
            .id_salt("objects")
            .max_height(list_h)
            .auto_shrink([false, false])
            .show_rows(ui, ROW, rows.len(), |ui, range| {
                // The last layer's row laid out, for a drop below them all.
                let mut last_row: Option<(Rect, f32)> = None;
                let list = ui.clip_rect();
                let shown_to_the_end = range.end == rows.len();
                if self.doc.shapes.is_empty() {
                    ui.add_space(8.0);
                    caption(
                        ui,
                        "No layers yet. Open or drop an SVG, add text, or draw with the pen.",
                    );
                }
                for row in &rows[range] {
                    let (i, depth) = match row {
                        ObjRow::Group { id, depth, members } => {
                            let indent = *depth as f32 * 14.0 + GUTTER;
                            let (rect, resp) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), ROW),
                                Sense::click_and_drag(),
                            );
                            let body = row_body(rect);
                            let gutter = gutter_flag(rect, resp.interact_pointer_pos());
                            // Dragged: the group's layers go (with the rest
                            // of the selection, when they're all in it).
                            if resp.drag_started_by(egui::PointerButton::Primary)
                                && gutter.is_none()
                            {
                                let mut moving = members.clone();
                                if members.iter().all(|m| all_sel.contains(m)) {
                                    moving.extend(all_sel.iter().filter(|m| !members.contains(m)));
                                }
                                drag_start = Some((moving, Studio::group_label(id)));
                            }
                            // Dragged over: above the group, or below it
                            // (at the top of it, while it's open).
                            if let Some(q) = over.filter(|q| rect.contains(*q)) {
                                let open = self.layout.expanded.contains(id);
                                let (first, last) = (
                                    members.iter().copied().min().unwrap_or(0),
                                    members.iter().copied().max().unwrap_or(0),
                                );
                                let (above, below) = if sewn {
                                    (first, last + 1)
                                } else {
                                    (last + 1, first)
                                };
                                drop = Some(if q.y < rect.center().y {
                                    (above, rect.top(), rect.left() + indent)
                                } else if open {
                                    (above, rect.bottom(), rect.left() + indent + 14.0)
                                } else {
                                    (below, rect.bottom(), rect.left() + indent)
                                });
                            }
                            eye_and_lock(
                                ui.painter(),
                                rect,
                                resp.hover_pos(),
                                self.flagged(members, Flag::Hide),
                                self.flagged(members, Flag::Lock),
                                &p,
                            );
                            let open = self.layout.expanded.contains(id);
                            let label = Studio::group_label(id);
                            let all = members.iter().all(|m| all_sel.contains(m));
                            let some = members.iter().any(|m| all_sel.contains(m));
                            resp.widget_info(|| {
                                egui::WidgetInfo::selected(egui::Role::ListItem, true, all, &label)
                            });
                            let painter = ui.painter();
                            if all {
                                painter.rect_filled(body, 5.0, p.accent);
                            } else if resp.hovered() {
                                painter.rect_filled(body, 5.0, p.hover);
                            } else if some {
                                painter.rect_filled(body, 5.0, p.accent.gamma_multiply(0.18));
                            }
                            let fg = if all { Color32::WHITE } else { p.text };
                            let fg2 = if all {
                                Color32::from_white_alpha(200)
                            } else {
                                p.secondary
                            };
                            let chev = Rect::from_center_size(
                                Pos2::new(rect.left() + indent + 10.0, rect.center().y),
                                Vec2::splat(9.0),
                            );
                            icons::paint(
                                painter,
                                chev,
                                if open {
                                    Icon::ChevronDown
                                } else {
                                    Icon::ChevronRight
                                },
                                fg2,
                            );
                            // A folder-ish stack of two squares.
                            let f = Rect::from_center_size(
                                Pos2::new(rect.left() + indent + 27.0, rect.center().y + 1.0),
                                Vec2::new(13.0, 10.0),
                            );
                            painter.rect_stroke(
                                f,
                                2.0,
                                Stroke::new(1.2, fg2),
                                egui::StrokeKind::Middle,
                            );
                            painter.line_segment(
                                [
                                    f.left_top() + Vec2::new(2.0, -3.0),
                                    f.right_top() + Vec2::new(-2.0, -3.0),
                                ],
                                Stroke::new(1.2, fg2),
                            );
                            let stitches: usize = members
                                .iter()
                                .map(|&m| self.design.per_shape.get(m).copied().unwrap_or(0))
                                .sum();
                            let right = rect.right() - 8.0;
                            let galley = painter.layout_no_wrap(
                                fmt_int(stitches),
                                egui::FontId::proportional(11.0),
                                fg2,
                            );
                            let cw = galley.size().x;
                            painter.galley(
                                Pos2::new(right - cw, rect.center().y - galley.size().y / 2.0),
                                galley,
                                fg2,
                            );
                            let name_rect = Rect::from_min_max(
                                Pos2::new(rect.left() + indent + 40.0, rect.top()),
                                Pos2::new(right - cw - 6.0, rect.bottom()),
                            );
                            painter.with_clip_rect(name_rect).text(
                                Pos2::new(name_rect.left(), rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                format!("{label}  ·  {}", members.len()),
                                egui::FontId::proportional(13.0),
                                fg,
                            );
                            let on_chevron = resp
                                .interact_pointer_pos()
                                .is_some_and(|q| q.x < rect.left() + indent + 18.0);
                            if let Some(flag) = gutter {
                                if resp.clicked() {
                                    flag_click = Some((members.clone(), flag, others));
                                }
                            } else if (resp.clicked() && on_chevron) || resp.double_clicked() {
                                if open {
                                    self.layout.expanded.remove(id);
                                } else {
                                    self.layout.expanded.insert(id.clone());
                                }
                            } else if resp.clicked() || (resp.secondary_clicked() && !all) {
                                group_click = Some(members.clone());
                            }
                            resp.on_hover_text(format!(
                                "{label}: {} layers, {} stitches. Click to select the group.",
                                members.len(),
                                fmt_int(stitches)
                            ))
                            .context_menu(|ui| self.object_menu_items(ui));
                            continue;
                        }
                        ObjRow::Item { i, depth } => (*i, *depth),
                    };
                    let indent = depth as f32 * 14.0 + GUTTER;
                    let (kind, color, hidden, locked) = {
                        let s = &self.doc.shapes[i];
                        (
                            s.params.kind.unwrap_or(Kind::Fill),
                            s.color,
                            s.params.hidden(),
                            s.params.locked(),
                        )
                    };
                    let count = self.design.per_shape.get(i).copied().unwrap_or(0);
                    let (rect, resp) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), ROW),
                        Sense::click_and_drag(),
                    );
                    let label = self.names.get(i).cloned().unwrap_or_default();
                    let selected = all_sel.contains(&i);
                    // Dragged over: above this layer, or below it.
                    if let Some(q) = over.filter(|q| rect.contains(*q)) {
                        let (above, below) = if sewn { (i, i + 1) } else { (i + 1, i) };
                        drop = Some(if q.y < rect.center().y {
                            (above, rect.top(), rect.left() + indent)
                        } else {
                            (below, rect.bottom(), rect.left() + indent)
                        });
                    }
                    last_row = Some((rect, indent));
                    resp.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::ListItem, true, selected, &label)
                    });
                    let painter = ui.painter();
                    let body = row_body(rect);
                    let gutter = gutter_flag(rect, resp.interact_pointer_pos());
                    eye_and_lock(painter, rect, resp.hover_pos(), hidden, locked, &p);
                    if selected {
                        let c = if Some(i) == sel {
                            p.accent
                        } else {
                            p.accent.gamma_multiply(0.55)
                        };
                        painter.rect_filled(body, 5.0, c);
                    } else if resp.hovered() || self.hovered == Some(i) {
                        painter.rect_filled(body, 5.0, p.hover);
                    }
                    let fg = if selected { Color32::WHITE } else { p.text };
                    let fg2 = if selected {
                        Color32::from_white_alpha(200)
                    } else {
                        p.secondary
                    };
                    let dot = Pos2::new(rect.left() + indent + 12.0, rect.center().y);
                    painter.circle_filled(dot, 5.5, crate::canvas::color32(color));
                    painter.circle_stroke(
                        dot,
                        5.5,
                        Stroke::new(1.0, Color32::from_black_alpha(50)),
                    );
                    let icon = Rect::from_center_size(
                        Pos2::new(rect.left() + indent + 30.0, rect.center().y),
                        Vec2::splat(13.0),
                    );
                    draw_kind_icon(painter, icon, kind, fg2);
                    // (Off, or hidden: dimmed, and saying which.)
                    let name_color = if kind == Kind::Skip || hidden {
                        fg2
                    } else {
                        fg
                    };
                    let right = rect.right() - 8.0;
                    let count_text = if hidden {
                        "hidden".to_string()
                    } else if kind == Kind::Skip {
                        "off".to_string()
                    } else {
                        fmt_int(count)
                    };
                    let galley =
                        painter.layout_no_wrap(count_text, egui::FontId::proportional(11.0), fg2);
                    let cw = galley.size().x;
                    painter.galley(
                        Pos2::new(right - cw, rect.center().y - galley.size().y / 2.0),
                        galley,
                        fg2,
                    );
                    let warn = warned.contains(&i);
                    let mut name_right = right - cw - 6.0;
                    if warn {
                        let wr = Rect::from_center_size(
                            Pos2::new(name_right - 6.0, rect.center().y),
                            Vec2::splat(11.0),
                        );
                        icons::paint(
                            painter,
                            wr,
                            Icon::Warning,
                            if selected { Color32::WHITE } else { p.warning },
                        );
                        name_right -= 16.0;
                    }
                    let name_rect = Rect::from_min_max(
                        Pos2::new(rect.left() + indent + 42.0, rect.top()),
                        Pos2::new(name_right, rect.bottom()),
                    );
                    let uid = self.doc.shapes[i].uid;
                    let editing = self.renaming.as_ref().is_some_and(|r| r.0 == uid);
                    if let Some((_, text, fresh)) = self.renaming.as_mut().filter(|_| editing) {
                        // The name as a field to type in, where it was.
                        let field = Rect::from_min_max(
                            Pos2::new(name_rect.left() - 4.0, rect.top() + 2.0),
                            Pos2::new(name_rect.right(), rect.bottom() - 2.0),
                        );
                        let out = ui
                            .scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
                                egui::TextEdit::singleline(text)
                                    .font(egui::FontId::proportional(13.0))
                                    .desired_width(field.width())
                                    .margin(egui::Margin::symmetric(4, 2))
                                    .show(ui)
                            })
                            .inner;
                        out.response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Layer name")
                        });
                        if *fresh {
                            // Just opened: the keyboard, and the whole name
                            // selected, so typing replaces it.
                            *fresh = false;
                            out.response.request_focus();
                            let mut state = out.state;
                            let end = egui::text::CCursor::new(text.chars().count());
                            state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(0),
                                    end,
                                )));
                            state.store(ui.ctx(), out.response.id);
                        } else if out.response.lost_focus() {
                            // Enter, or a click somewhere else, keeps the
                            // name; Escape leaves the old one.
                            let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
                            renamed = Some((!escaped).then(|| (uid, text.clone())));
                        }
                    } else {
                        painter.with_clip_rect(name_rect).text(
                            Pos2::new(name_rect.left(), rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &label,
                            egui::FontId::proportional(13.0),
                            name_color,
                        );
                    }
                    // A double-click on the name itself renames it (as in
                    // Illustrator's Layers). One click only selects, however
                    // soon after another; so does a double-click anywhere
                    // else on the row, or one with Shift or ⌘ held.
                    let on_name = resp
                        .interact_pointer_pos()
                        .is_some_and(|q| name_rect.contains(q));
                    if resp.double_clicked() && on_name && !toggle && !editing && gutter.is_none() {
                        let name = label.strip_suffix(" (outline)").unwrap_or(&label);
                        self.renaming = Some((uid, name.to_string(), true));
                    }
                    // What's locked or hidden can't be selected, here
                    // either (as in Illustrator's Layers): its eye and
                    // lock are what a click on its row is for.
                    // Dragged: this layer, or the selected ones when it's
                    // one of several (as VectorCraft's Layers, and
                    // Illustrator's): to another place in the sewing order.
                    if resp.drag_started_by(egui::PointerButton::Primary)
                        && gutter.is_none()
                        && !editing
                    {
                        drag_start = Some(if selected && all_sel.len() > 1 {
                            (
                                all_sel.iter().copied().collect(),
                                format!("{} layers", all_sel.len()),
                            )
                        } else {
                            (vec![i], label.clone())
                        });
                    }
                    if let Some(flag) = gutter {
                        if resp.clicked() {
                            flag_click = Some((vec![i], flag, others));
                        }
                    } else if !hidden && !locked {
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        if resp.secondary_clicked() && !selected {
                            clicked = Some(i);
                        }
                    }
                    resp.context_menu(|ui| self.object_menu_items(ui));
                }
                // (A little below the list counts: one as tall as its rows
                // has nowhere under the last of them to drop on.)
                if let Some(q) = over.filter(|q| {
                    list.x_range().contains(q.x) && q.y >= list.top() && q.y <= list.bottom() + 28.0
                }) {
                    // Below the last row: under them all (sewn first, or
                    // in sewing order last).
                    if drop.is_none()
                        && shown_to_the_end
                        && let Some((rect, indent)) = last_row.filter(|(r, _)| q.y >= r.bottom())
                    {
                        let end = if sewn { self.doc.shapes.len() } else { 0 };
                        drop = Some((end, rect.bottom(), rect.left() + indent));
                    }
                    if let Some((_, y, x)) = drop {
                        ui.painter().line_segment(
                            [Pos2::new(x, y), Pos2::new(list.right() - 4.0, y)],
                            Stroke::new(2.0, p.accent),
                        );
                    }
                    // Near the top or bottom of the list, it scrolls.
                    let edge = 22.0;
                    let by = if q.y < list.top() + edge {
                        6.0
                    } else if q.y > list.bottom() - edge {
                        -6.0
                    } else {
                        0.0
                    };
                    if by != 0.0 {
                        ui.scroll_with_delta(Vec2::new(0.0, by));
                        ui.ctx().request_repaint();
                    }
                }
            });
        self.layer_drag(ui, drag_start, drop);
        if let Some(i) = clicked {
            if toggle {
                self.toggle_selected(i);
            } else if !(ui.input(|i| i.pointer.secondary_released()) && all_sel.contains(&i)) {
                self.set_selection(&[i]);
            }
        }
        if let Some((of, flag, others)) = flag_click {
            // Option-click: every other object instead (Illustrator's Hide
            // Others and Lock Others), or all of them back when they
            // already are.
            let of: Vec<usize> = if others {
                (0..self.doc.shapes.len())
                    .filter(|i| !of.contains(i))
                    .collect()
            } else {
                of
            };
            let on = !self.flagged(&of, flag);
            self.set_flag(&of, flag, on);
        }
        if let Some(done) = renamed {
            self.renaming = None;
            if let Some((uid, name)) = done {
                self.rename_object(uid, &name);
            }
        }
        if let Some(members) = group_click {
            if toggle {
                let mut cur = self.sel_indices();
                if members.iter().all(|m| cur.contains(m)) {
                    cur.retain(|m| !members.contains(m));
                } else {
                    cur.extend(members.iter().filter(|m| !all_sel.contains(m)));
                }
                self.set_selection(&cur);
            } else {
                self.set_selection(&members);
            }
        }
        // Bottom bar, like a Mac source list.
        theme::hline(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let add = icon_button(ui, Icon::Plus, "Add", false, 24.0);
            egui::Popup::menu(&add).show(|ui| {
                if ui.button("Add Text…").clicked() {
                    self.open_text_window();
                }
                if ui.button("Place SVG…").clicked() {
                    self.run_action(crate::commands::Action::Place);
                }
                if ui.button("Draw with Pen").clicked() {
                    self.set_tool(Tool::Pen);
                }
            });
            let has = !self.sel_indices().is_empty();
            ui.add_enabled_ui(has, |ui| {
                if icon_button(ui, Icon::Minus, "Delete", false, 24.0).clicked() {
                    self.delete_selection();
                }
                ui.add_space(theme::space::S);
                // Up the list is up the stack, sewn later; in sewing
                // order it's sewn earlier.
                let (up, down) = if self.dock.layers_sewing_order {
                    (("Sew earlier", -1), ("Sew later", 1))
                } else {
                    (("Sew later", 1), ("Sew earlier", -1))
                };
                if icon_button(ui, Icon::ArrowUp, up.0, false, 24.0).clicked() {
                    self.move_selected(up.1);
                }
                if icon_button(ui, Icon::ArrowDown, down.0, false, 24.0).clicked() {
                    self.move_selected(down.1);
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let n = self.doc.shapes.len();
                caption(
                    ui,
                    format!("{n} {}", if n == 1 { "layer" } else { "layers" }),
                );
            });
        });
    }

    /// Layers dragged in the Layers list (after VectorCraft's): `start`
    /// begins a drag of those objects, called `label`; while one's under
    /// way a dimmed copy of the row follows the pointer; and when the
    /// button's let go they move to `drop` (before the object numbered so
    /// in the sewing order, which in the list is under its row; the line
    /// there shows where), as one undo step. Escape, or
    /// letting go anywhere else, leaves them where they were.
    fn layer_drag(
        &mut self,
        ui: &egui::Ui,
        start: Option<(Vec<usize>, String)>,
        drop: Option<(usize, f32, f32)>,
    ) {
        if let Some((moving, label)) = start {
            let uids = moving
                .iter()
                .filter_map(|&i| self.doc.shapes.get(i))
                .map(|s| s.uid)
                .collect();
            self.dock_ui.layer_drag = Some((uids, label));
        }
        let Some((uids, label)) = self.dock_ui.layer_drag.clone() else {
            return;
        };
        let (down, released, escape, at) = ui.input(|i| {
            (
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.key_pressed(egui::Key::Escape),
                i.pointer.interact_pos(),
            )
        });
        if escape || (!down && !released) {
            self.dock_ui.layer_drag = None;
            return;
        }
        if released {
            self.dock_ui.layer_drag = None;
            if let Some((before, ..)) = drop {
                let moving = self.uid_indices(&uids);
                self.move_layers(&moving, before);
            }
            return;
        }
        // The copy at the pointer.
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if let Some(at) = at {
            let p = pal_ui(ui);
            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::unique("layer-drag-ghost"),
            ));
            let galley =
                painter.layout_no_wrap(label, egui::FontId::proportional(13.0), Color32::WHITE);
            let r = Rect::from_min_size(
                at + Vec2::new(12.0, -11.0),
                galley.size() + Vec2::new(16.0, 8.0),
            );
            painter.rect_filled(r, 5.0, p.accent.gamma_multiply(0.75));
            painter.galley(r.min + Vec2::new(8.0, 4.0), galley, Color32::WHITE);
        }
        ui.ctx().request_repaint();
    }

    /// Stitch type and editing commands for the selection (context menus).
    /// Give the object with this uid the name typed for it in the Objects
    /// list (one undo step). Nothing typed, or the name it has: no change.
    pub(crate) fn rename_object(&mut self, uid: usize, name: &str) {
        let Some(i) = self.doc.shapes.iter().position(|s| s.uid == uid) else {
            return;
        };
        let name = name.trim();
        let had = self.names.get(i).cloned().unwrap_or_default();
        if name.is_empty() || name == had.strip_suffix(" (outline)").unwrap_or(&had) {
            return;
        }
        match self.structural(|d| d.rename(i, name)) {
            Ok(()) => {
                // (Writing the name can move objects in the list.)
                if let Some(at) = self.doc.shapes.iter().position(|s| s.uid == uid) {
                    self.set_selection(&[at]);
                }
                self.names = zigzag::svg::display_names(&self.doc.shapes);
                self.status = format!("Renamed to {name}.");
            }
            Err(e) => self.status = format!("Couldn't rename it: {e}."),
        }
    }

    pub fn object_menu_items(&mut self, ui: &mut egui::Ui) {
        use crate::commands::Action as A;
        use crate::flyout::{RowIcon, menu_heading, menu_row, menu_separator};
        let Some(i) = self.sel_index() else {
            ui.label("Nothing selected");
            return;
        };
        ui.set_min_width(240.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        // Stitch types, each with its icon; a square beside the current one.
        menu_heading(ui, "Stitch Type");
        let current = self.doc.shapes[i].params.kind.unwrap_or(Kind::Fill);
        for k in allowed_kinds(&self.doc.shapes[i]) {
            if menu_row(ui, RowIcon::Kind(k), kind_name(k), "", current == k, true).clicked() {
                self.set_kind_selected(k);
                ui.close();
            }
        }
        menu_separator(ui);
        self.action_row(
            ui,
            A::Duplicate,
            "Duplicate",
            RowIcon::Icon(Icon::Duplicate),
        );
        self.action_row(ui, A::Copy, "Copy", RowIcon::None);
        self.action_row(ui, A::Paste, "Paste", RowIcon::None);
        menu_separator(ui);
        self.action_row(ui, A::FlipH, "Flip Horizontal", RowIcon::Icon(Icon::FlipH));
        self.action_row(ui, A::FlipV, "Flip Vertical", RowIcon::Icon(Icon::FlipV));
        // The arrows point where it goes in the Layers list.
        let (last, first) = if self.dock.layers_sewing_order {
            (Icon::ArrowDown, Icon::ArrowUp)
        } else {
            (Icon::ArrowUp, Icon::ArrowDown)
        };
        self.action_row(ui, A::BringToFront, "Sew Last", RowIcon::Icon(last));
        self.action_row(ui, A::SendToBack, "Sew First", RowIcon::Icon(first));
        // One path drawing several shapes: offer to split it up.
        let compound = match &self.doc.shapes[i].geometry {
            zigzag::svg::Geometry::Fill { rings, .. } => rings.len() > 1,
            zigzag::svg::Geometry::Stroke { lines, .. } => lines.len() > 1,
        };
        if compound && self.doc.lettering(i).is_none() {
            menu_separator(ui);
            self.action_row(
                ui,
                A::ReleaseCompound,
                "Release Compound Path",
                RowIcon::None,
            );
        }
        // A filled shape with no outline yet: offer one.
        if self.can_outline(i) {
            menu_separator(ui);
            self.action_row(
                ui,
                A::AddOutline,
                "Add Outline",
                RowIcon::Icon(Icon::AddOutline),
            );
        } else if self.outline_index(i).is_some() {
            menu_separator(ui);
            self.action_row(
                ui,
                A::RemoveOutline,
                "Remove Outline",
                RowIcon::Icon(Icon::NoOutline),
            );
        }
        menu_separator(ui);
        if menu_row(ui, RowIcon::Icon(Icon::Close), "Delete", "", false, true).clicked() {
            self.delete_selection();
            ui.close();
        }
    }

    /// The design's issues, each naming its object (click to select it).
    pub(crate) fn issues_body(&mut self, ui: &mut egui::Ui) {
        let p = pal_ui(ui);
        if self.issues.is_empty() {
            caption(ui, "No problems found.");
            return;
        }
        let mut pick = None;
        egui::ScrollArea::vertical()
            .id_salt("issues")
            .max_height(380.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for issue in &self.issues {
                    let (c, icon) = match issue.level {
                        Level::Error => (p.error, Icon::Warning),
                        Level::Warning => (p.warning, Icon::Warning),
                        Level::Info => (p.secondary, Icon::Info),
                    };
                    ui.horizontal_top(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(13.0), Sense::hover());
                        icons::paint(ui.painter(), r, icon, c);
                        ui.vertical(|ui| {
                            if let Some(i) = issue.shape
                                && ui
                                    .link(
                                        egui::RichText::new(
                                            self.names.get(i).cloned().unwrap_or_default(),
                                        )
                                        .size(12.0),
                                    )
                                    .clicked()
                            {
                                pick = Some(i);
                            }
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&issue.message)
                                        .size(12.0)
                                        .color(p.secondary),
                                )
                                .wrap(),
                            );
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        if let Some(i) = pick {
            self.set_selection(&[i]);
        }
    }

    // ------------------------------------------------------------ right dock

    /// What the Stitch panel's header shows: the selection and its stitches.
    fn stitch_header(&self) -> Option<(String, String)> {
        let i = self.sel_index()?;
        let mut sel = self.sel_indices();
        let count = sel.len();
        let what = if count > 1 {
            sel.sort_unstable();
            match self.doc.shapes[i]
                .groups
                .iter()
                .find(|g| self.group_members(g) == sel)
            {
                Some(g) => format!("{} · {count} objects", Studio::group_label(g)),
                None => format!("{count} objects"),
            }
        } else {
            self.names.get(i).cloned().unwrap_or_default()
        };
        // (A shape selected with its own outline is the shape.)
        let what = match self.subjects()[..] {
            [only] if count > 1 => self.names.get(only).cloned().unwrap_or_default(),
            _ => what,
        };
        let stitches: usize = sel
            .iter()
            .map(|&j| self.design.per_shape.get(j).copied().unwrap_or(0))
            .sum();
        Some((what, format!("{} stitches", fmt_int(stitches))))
    }

    fn stitch_body(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = theme::space::S;
        let p = pal_ui(ui);
        // (A shape selected with its own outline is the shape.)
        let subjects = self.subjects();
        let Some(&i) = subjects.first() else {
            stitch_type_guide(ui);
            return;
        };
        let count = subjects.len();
        let name = self.names.get(i).cloned().unwrap_or_default();
        if count > 1 {
            caption(
                ui,
                format!("Showing {name}'s settings. Stitch types apply to all."),
            );
        }
        // The stitch type: the main decision for every object.
        let allowed = allowed_kinds(&self.doc.shapes[i]);
        let current = self.doc.shapes[i].params.kind.unwrap_or(Kind::Fill);
        let filled = matches!(self.doc.shapes[i].geometry, Geometry::Fill { .. });
        let mut kind_clicked = None;
        // Tiles in four even columns that fill the width.
        let gap = theme::space::XS;
        let tile_w = ((ui.available_width() - 3.0 * gap) / 4.0).floor();
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(gap);
            // (Every type, always in the same place: the ones this object
            // can't take are dimmed, and say why.)
            for row in crate::widgets::ALL_KINDS.chunks(4) {
                ui.horizontal(|ui| {
                    for &k in row {
                        if !allowed.contains(&k) {
                            kind_tile(ui, k, false, false, tile_w, 52.0)
                                .on_hover_text(crate::widgets::kind_unavailable(k));
                        } else if kind_tile(ui, k, k == current, true, tile_w, 52.0)
                            .on_hover_text(kind_help(k, filled))
                            .clicked()
                        {
                            kind_clicked = Some(k);
                        }
                    }
                });
            }
        });
        if let Some(k) = kind_clicked {
            self.set_kind_selected(k);
        }
        // The thread color is picked from the swatch in the tool strip.
        let f = self.doc.settings.fabric();
        theme::section_break(ui);

        let default_width = design::default_satin_width(&self.doc.shapes[i]);
        // Only a satin needs its width (worked out once, not every frame;
        // not at all while its text is being typed, when every keystroke
        // would mean finding the rails of every letter again; and not
        // before its stitches are planned: see `satin_info_shown`).
        let satin_width = if current == Kind::Satin && !self.typing_into(i) {
            self.satin_info_shown(i)
                .map_or(default_width, |info| info.mean_width)
        } else if current == Kind::Satin {
            default_width
        } else {
            0.0
        };
        let mut to_angle_tool = false;
        let color_now = self.doc.shapes[i].color;
        let advanced = self.prefs.advanced;
        let pr = &mut self.doc.shapes[i].params;
        let left_out = form_for(ui, "stitch-form", advanced, |fm| match current {
            Kind::Fill => {
                fm.row("Angle", |ui, lid| {
                    let shown = pr.angle.unwrap_or(defaults::FILL_ANGLE);
                    let mut v = shown;
                    ui.spacing_mut().slider_width = 100.0;
                    let r = ui
                        .add(
                            egui::Slider::new(&mut v, -90.0..=90.0)
                                .show_value(false)
                                .step_by(1.0),
                        )
                        .labelled_by(lid);
                    let d = ui.add(egui::DragValue::new(&mut v).range(-90.0..=90.0).suffix("°"));
                    // Only a user's change counts: the slider snaps to whole
                    // degrees, and a non-uniform scale leaves fractions.
                    let touched = r.dragged()
                        || r.clicked()
                        || r.has_focus()
                        || d.dragged()
                        || d.has_focus()
                        || d.lost_focus();
                    if (r.changed() || d.changed()) && touched && v != shown {
                        pr.angle = Some(v);
                    }
                    if ui
                        .add(egui::Button::new("Draw").small())
                        .on_hover_text("Drag across the shape to set the angle (G)")
                        .clicked()
                    {
                        to_angle_tool = true;
                    }
                });
                value_row(
                    fm,
                    "Spacing",
                    &mut pr.spacing,
                    f.fill_spacing,
                    0.15..=0.8,
                    " mm",
                    0.01,
                    "Row spacing: smaller is denser",
                );
                if fm.advanced(pr.len.is_some()) {
                    value_row(
                        fm,
                        "Length",
                        &mut pr.len,
                        f.fill_len,
                        1.5..=7.0,
                        " mm",
                        0.1,
                        "Longest stitch in a row",
                    );
                }
                if fm.advanced(pr.pull.is_some()) {
                    value_row(
                        fm,
                        "Pull comp.",
                        &mut pr.pull,
                        f.fill_pull,
                        0.0..=0.6,
                        " mm",
                        0.05,
                        "Extends rows at the edges to counter fabric pulling in",
                    );
                }
                if fm.advanced(pr.underlay.is_some()) {
                    fm.row("Underlay", |ui, _| {
                        underlay_box(
                            ui,
                            pr,
                            if f.fill_edge_walk {
                                "Edge walk + rows"
                            } else {
                                "Stabilizing rows"
                            },
                        )
                    });
                }
            }
            Kind::Satin => {
                if filled {
                    fm.row("Shape", |ui, _| {
                        let mut border = pr.is_border();
                        if theme::segmented(
                            ui,
                            "satin-shape",
                            &mut border,
                            &[(false, "Column"), (true, "Border")],
                        ) {
                            pr.border = border.then_some(true);
                        }
                    });
                }
                // (Overlapping strokes aren't offered: they leave more letters
                // with seams and satin piled three deep than columns do. An
                // object that has them can be put back.)
                if filled && !pr.is_border() && pr.satin_overlap() {
                    fm.row("Columns", |ui, _| {
                        let mut overlap = pr.satin_overlap();
                        let r = ui.scope(|ui| {
                            theme::segmented(
                                ui,
                                "satin-columns",
                                &mut overlap,
                                &[(false, "Strokes"), (true, "Overlapping")],
                            )
                        });
                        r.response.on_hover_text("This text is set to overlapping strokes, which are no longer offered: they leave more seams and piled satin than columns. Strokes puts it back.");
                        if r.inner {
                            pr.overlap = overlap.then_some(true);
                        }
                    });
                }
                // How a filled satin's strokes meet: cut, corners squared, one
                // stroke run on and the other ended against it (the default);
                // or mitred, each stroke a column (its chordal axis), where
                // that sews cleanly (Auto) or everywhere.
                if filled
                    && !pr.is_border()
                    && !pr.satin_overlap()
                    && fm.advanced(pr.cut.is_some() || pr.mitred.is_some())
                {
                    fm.row("Corners", |ui, _| {
                        // (0 Auto, 1 Mitred, 2 Cut.)
                        let mut how: u8 = if pr.satin_cut() {
                            2
                        } else if pr.satin_mitred() {
                            1
                        } else {
                            0
                        };
                        let r = ui.scope(|ui| {
                            theme::segmented(
                                ui,
                                "satin-corners",
                                &mut how,
                                &[(0, "Auto"), (1, "Mitred"), (2, "Cut")],
                            )
                        });
                        r.response.on_hover_text("Mitred: each stroke of a letter is its own column, and where two meet at a corner both end on the line between them. Cut: the shape is cut into columns where its strokes part, corners squared, one stroke running on and the other ending against it. Auto: mitred, unless a corner is too sharp for a mitre to sew cleanly (the point of a V) or a join can't be read clearly, when it's cut. Cut is the default.");
                        if r.inner {
                            // (Cut, the default, is nothing set.)
                            pr.cut = (how != 2).then_some(false);
                            pr.mitred = (how == 1).then_some(true);
                        }
                    });
                }
                fm.row("Stitch", |ui, _| {
                    use zigzag::stitch::satin::SatinStyle;
                    let mut st = pr.satin_style();
                    if theme::segmented(
                        ui,
                        "satin-stitch",
                        &mut st,
                        &[
                            (SatinStyle::Plain, "Satin"),
                            (SatinStyle::SStitch, "S-stitch"),
                            (SatinStyle::Zigzag, "Zig-zag"),
                        ],
                    ) {
                        pr.style = (st != SatinStyle::Plain).then_some(st);
                    }
                });
                if fm.advanced(pr.colors.is_some()) {
                    fm.row("Threads", |ui, _| {
                    // Multicolor satin: the threads across the column.
                    let mut colors = pr.colors.clone().unwrap_or_default();
                    let mut changed = false;
                    for c in colors.iter_mut() {
                        changed |= ui.color_edit_button_srgb(c).changed();
                    }
                    if colors.len() < 4
                        && ui
                            .add(egui::Button::new("+").small())
                            .on_hover_text("Add a thread: the column is split across its width, one strip per thread")
                            .clicked()
                    {
                        let last = colors.last().copied().unwrap_or(color_now);
                        if colors.is_empty() {
                            colors.push(color_now);
                        }
                        colors.push([255 - last[0], 255 - last[1], 255 - last[2]]);
                        changed = true;
                    }
                    if !colors.is_empty()
                        && ui
                            .add(egui::Button::new("−").small())
                            .on_hover_text("Remove the last thread")
                            .clicked()
                    {
                        colors.pop();
                        if colors.len() < 2 {
                            colors.clear();
                        }
                        changed = true;
                    }
                    if colors.is_empty() {
                        caption(ui, "one");
                    }
                    if changed {
                        pr.colors = (colors.len() >= 2).then_some(colors);
                    }
                });
                }

                if !filled || pr.is_border() {
                    value_row(
                        fm,
                        "Width",
                        &mut pr.width,
                        default_width,
                        0.6..=10.0,
                        " mm",
                        0.1,
                        "",
                    );
                }
                value_row(
                    fm,
                    "Density",
                    &mut pr.spacing,
                    f.satin_spacing,
                    0.2..=1.0,
                    " mm",
                    0.01,
                    "Distance between zigzags (auto-adjusted for width unless set)",
                );
                if fm.advanced(pr.pull.is_some()) {
                    value_row(
                        fm,
                        "Pull comp.",
                        &mut pr.pull,
                        f.satin_pull,
                        0.0..=0.6,
                        " mm",
                        0.05,
                        "Extra width per side; satins pull in narrower than drawn",
                    );
                }
                if fm.advanced(pr.satin_underlay.is_some() || pr.underlay.is_some()) {
                    fm.row("Underlay", |ui, _| {
                    let mut auto = pr.satin_underlay.is_none() && pr.underlay_on();
                    if ui
                        .checkbox(&mut auto, "Automatic")
                        .on_hover_text(format!(
                            "None under {} mm on this fabric, center walk to 4 mm, contour + zigzag above",
                            zigzag::params::fmt_num(f.satin_underlay_from)
                        ))
                        .changed()
                    {
                        pr.satin_underlay = if auto {
                            None
                        } else {
                            Some(pr.satin_underlay_for(&f, satin_width))
                        };
                        pr.underlay = None;
                    }
                });
                    let current_ul = pr.satin_underlay_for(&f, satin_width);
                    fm.wide(|ui| {
                        for (bit, name, tip) in [
                            (
                                underlay::CENTER,
                                "Center",
                                "A run down the middle: anchors narrow columns",
                            ),
                            (
                                underlay::CONTOUR,
                                "Contour",
                                "Runs just inside both edges: crisp edges on wide columns",
                            ),
                            (
                                underlay::ZIGZAG,
                                "Zigzag",
                                "Sparse zigzag: lifts and supports wide columns",
                            ),
                        ] {
                            let mut on = current_ul & bit != 0;
                            if ui.checkbox(&mut on, name).on_hover_text(tip).changed() {
                                pr.satin_underlay = Some(if on {
                                    current_ul | bit
                                } else {
                                    current_ul & !bit
                                });
                                pr.underlay = None;
                            }
                        }
                    });
                }
                if fm.advanced(pr.short_stitches.is_some()) {
                    fm.row("Curves", |ui, _| {
                        let mut short = pr.short_stitches_on();
                        if ui
                            .checkbox(&mut short, "Short stitches")
                            .on_hover_text(
                                "Shortens every other stitch on the inside of tight curves",
                            )
                            .changed()
                        {
                            pr.short_stitches = Some(short);
                        }
                    });
                }

                if fm.advanced(pr.even.is_some()) {
                    fm.row("Zigzag", |ui, _| {
                    let mut even = pr.satin_even();
                    if ui
                        .checkbox(&mut even, "Even")
                        .on_hover_text("Each step along the column is shared between the stitch over and the stitch back, so both slant the same. Off: one stitch goes straight across and the next takes the whole step.")
                        .changed()
                    {
                        pr.even = (even != defaults::SATIN_EVEN).then_some(even);
                    }
                });
                }

                if fm.advanced(pr.lean.is_some()) {
                    fm.row("Lean", |ui, lid| {
                    let mut v = pr.satin_lean();
                    let r = ui
                        .add(
                            egui::DragValue::new(&mut v)
                                .range(-zigzag::params::MAX_LEAN..=zigzag::params::MAX_LEAN)
                                .speed(0.5)
                                .fixed_decimals(0)
                                .suffix("°"),
                        )
                        .labelled_by(lid)
                        .on_hover_text("How far the stitches lean from square across the column. A few degrees stops them sinking between a knit's rows; more shows a slant, like a leaf's veins.");
                    if r.changed() {
                        pr.lean = (v != 0.0).then_some(v);
                    }
                });
                }

                if fm.advanced(pr.split.is_some()) {
                    fm.row("Auto split", |ui, lid| {
                        let mut on = pr.satin_split().is_some();
                        if ui
                            .checkbox(&mut on, "")
                            .labelled_by(lid)
                            .on_hover_text(
                                "Split stitches longer than this so wide columns don't snag",
                            )
                            .changed()
                        {
                            pr.split = Some(if on { defaults::SATIN_SPLIT } else { 0.0 });
                        }
                        if let Some(mut v) = pr.satin_split() {
                            let r = ui.add(
                                egui::DragValue::new(&mut v)
                                    .range(3.0..=12.0)
                                    .speed(0.1)
                                    .fixed_decimals(1)
                                    .suffix(" mm"),
                            );
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Slider, true, "Split length")
                            });
                            if r.changed() {
                                pr.split = Some(v);
                            }
                        }
                    });
                }
            }
            Kind::Contour => {
                fm.row("Style", |ui, _| {
                    let mut spiral = pr.spiral == Some(true);
                    if theme::segmented(
                        ui,
                        "contour-style",
                        &mut spiral,
                        &[(false, "Rings"), (true, "Spiral")],
                    ) {
                        pr.spiral = spiral.then_some(true);
                    }
                });
                value_row(
                    fm,
                    "Spacing",
                    &mut pr.spacing,
                    defaults::CONTOUR_SPACING,
                    0.25..=1.5,
                    " mm",
                    0.05,
                    "Distance between the outline-following rows",
                );
                if fm.advanced(pr.len.is_some()) {
                    value_row(
                        fm,
                        "Length",
                        &mut pr.len,
                        f.run_len,
                        1.0..=5.0,
                        " mm",
                        0.1,
                        "",
                    );
                }
            }
            Kind::Blanket => {
                value_row(
                    fm,
                    "Spacing",
                    &mut pr.spacing,
                    defaults::BLANKET_SPACING,
                    1.0..=6.0,
                    " mm",
                    0.1,
                    "Distance between prongs",
                );
                value_row(
                    fm,
                    "Prong",
                    &mut pr.width,
                    defaults::BLANKET_WIDTH,
                    0.8..=6.0,
                    " mm",
                    0.1,
                    "Prong length",
                );
            }
            Kind::Applique => {
                value_row(
                    fm,
                    "Cover",
                    &mut pr.width,
                    defaults::APPLIQUE_WIDTH,
                    1.5..=6.0,
                    " mm",
                    0.1,
                    "Satin cover width",
                );
                if fm.advanced(pr.spacing.is_some()) {
                    value_row(
                        fm,
                        "Density",
                        &mut pr.spacing,
                        f.satin_spacing,
                        0.2..=1.0,
                        " mm",
                        0.01,
                        "",
                    );
                }
                if fm.advanced(pr.underlay.is_some()) {
                    fm.row("Underlay", |ui, _| underlay_box(ui, pr, "Under the cover"));
                }
            }
            Kind::Manual => {}
            Kind::Meander => {
                value_row(
                    fm,
                    "Spacing",
                    &mut pr.spacing,
                    defaults::MEANDER_SPACING,
                    1.0..=8.0,
                    " mm",
                    0.1,
                    "How close the wandering line comes to itself",
                );
                if fm.advanced(pr.len.is_some()) {
                    value_row(
                        fm,
                        "Length",
                        &mut pr.len,
                        f.run_len,
                        1.0..=5.0,
                        " mm",
                        0.1,
                        "",
                    );
                }
            }
            Kind::Ripple => {
                fm.row("Style", |ui, _| {
                    let mut spiral = pr.spiral == Some(true);
                    if theme::segmented(
                        ui,
                        "ripple-style",
                        &mut spiral,
                        &[(false, "Rings"), (true, "Spiral")],
                    ) {
                        pr.spiral = spiral.then_some(true);
                    }
                });
                value_row(
                    fm,
                    "Spacing",
                    &mut pr.spacing,
                    defaults::RIPPLE_SPACING,
                    0.5..=6.0,
                    " mm",
                    0.1,
                    "Distance between the rows",
                );
                if fm.advanced(pr.len.is_some()) {
                    value_row(
                        fm,
                        "Length",
                        &mut pr.len,
                        f.run_len,
                        1.0..=5.0,
                        " mm",
                        0.1,
                        "",
                    );
                }
            }
            Kind::Cross => {
                value_row(
                    fm,
                    "Cell",
                    &mut pr.cell,
                    defaults::CROSS_CELL,
                    1.0..=8.0,
                    " mm",
                    0.1,
                    "Size of each X",
                );
            }
            Kind::Tartan => {
                // The sett: a colour and width per stripe (mirrored into a
                // symmetric pattern), one row each.
                let mut sett = pr.tartan_sett(color_now);
                let mut changed = false;
                let mut remove = None;
                let n = sett.len();
                for (k, (c, w)) in sett.iter_mut().enumerate() {
                    fm.row(if k == 0 { "Stripes" } else { "" }, |ui, _| {
                        changed |= ui.color_edit_button_srgb(c).changed();
                        changed |= ui.add(crate::units::drag_mm(w, 0.5..=30.0, 0.1)).changed();
                        if n > 1
                            && ui
                                .add(egui::Button::new("−").small())
                                .on_hover_text("Remove this stripe")
                                .clicked()
                        {
                            remove = Some(k);
                        }
                    });
                }
                fm.row("", |ui, _| {
                    if sett.len() < 8
                        && ui
                            .add(egui::Button::new("+ Stripe").small())
                            .on_hover_text("The stripes repeat mirrored: a b c b a b c…")
                            .clicked()
                    {
                        let last = sett.last().map(|s| s.0).unwrap_or(color_now);
                        sett.push(([255 - last[0], 255 - last[1], 255 - last[2]], 2.0));
                        changed = true;
                    }
                });
                if let Some(k) = remove {
                    sett.remove(k);
                    changed = true;
                }
                if changed {
                    pr.sett = Some(sett);
                }
                fm.row("Angle", |ui, lid| {
                    let mut v = pr.angle.unwrap_or(0.0);
                    let r = ui
                        .add(egui::DragValue::new(&mut v).range(-90.0..=90.0).suffix("°"))
                        .labelled_by(lid);
                    if r.changed() {
                        pr.angle = Some(v);
                    }
                });
                if fm.advanced(pr.spacing.is_some()) {
                    value_row(
                        fm,
                        "Spacing",
                        &mut pr.spacing,
                        f.fill_spacing,
                        0.2..=0.8,
                        " mm",
                        0.01,
                        "Row spacing within each stripe",
                    );
                }
            }
            Kind::Run => {
                value_row(
                    fm,
                    "Length",
                    &mut pr.len,
                    f.run_len,
                    1.0..=5.0,
                    " mm",
                    0.1,
                    "",
                );
                fm.row("Passes", |ui, _| {
                    let mut r = pr.run_repeats();
                    if theme::segmented(ui, "passes", &mut r, &[(1, "Single"), (3, "Triple")]) {
                        pr.repeats = Some(r);
                    }
                });
            }
            Kind::Skip => fm.wide(|ui| {
                caption(ui, "This object won't be stitched.");
            }),
        });
        if to_angle_tool {
            self.tool = Tool::Angle;
        }
        let left_out = left_out
            + if current != Kind::Skip {
                self.sewing_rows(ui, i)
            } else {
                0
            };
        for issue in self.issues.iter().filter(|x| x.shape == Some(i)) {
            let (c, icon) = if issue.level >= Level::Warning {
                (p.warning, Icon::Warning)
            } else {
                (p.secondary, Icon::Info)
            };
            callout(ui, c, icon, &issue.message);
        }
        // (In the controls' column, under the rows it resets: not out in
        // the labels' column.)
        let reset = ui
            .horizontal(|ui| {
                ui.add_space(theme::LABEL_W + theme::space::S);
                ui.add(egui::Button::new("Reset to Defaults").small())
                    .on_hover_text("Back to the fabric's defaults for this stitch type")
                    .clicked()
            })
            .inner;
        self.left_out_note(ui, left_out);
        if reset {
            let kind = self.doc.shapes[i].params.kind;
            self.doc.shapes[i].params = Params {
                kind,
                ..Params::default()
            };
        }
    }

    /// Where object `i` starts and ends sewing, and what happens after it
    /// (Ink/Stitch's start, end, trim and stop commands).
    /// An advanced row: returns 1 when it's left out (Basic, nothing set).
    fn sewing_rows(&mut self, ui: &mut egui::Ui, i: usize) -> usize {
        use crate::view_tools::Marker;
        let advanced = self.prefs.advanced;
        let pr = &mut self.doc.shapes[i].params;
        let set = pr.start.is_some() || pr.end.is_some() || pr.trim.is_some() || pr.stop.is_some();
        if !advanced && !set {
            return 1;
        }
        theme::section_break(ui);
        let p = pal_ui(ui);
        let picking = self.view_tools.marker_pick;
        let mut pick: Option<Option<Marker>> = None;
        let mut clear: Option<Marker> = None;
        form_for(ui, "sewing-form", advanced, |fm| {
            fm.advanced(set);
            fm.row("Sewing", |ui, _| {
                for (m, label, set) in [
                    (Marker::Start, "Start", pr.start.is_some()),
                    (Marker::End, "End", pr.end.is_some()),
                ] {
                    let on = picking == Some(m);
                    let text = egui::RichText::new(label).color(if set && !on {
                        p.accent
                    } else {
                        p.text
                    });
                    let r = ui
                        .add(egui::Button::new(text).selected(on).small())
                        .on_hover_text(match m {
                            Marker::Start => {
                                "Click on the outline where this object should start sewing (drag the marker to move it)"
                            }
                            Marker::End => {
                                "Click on the outline where this object should finish (the thread goes on from there)"
                            }
                        });
                    if r.clicked() {
                        pick = Some(if on { None } else { Some(m) });
                    }
                    if set && r.secondary_clicked() {
                        clear = Some(m);
                    }
                }
                if (pr.start.is_some() || pr.end.is_some())
                    && ui
                        .add(egui::Button::new("Clear").small())
                        .on_hover_text("Let the planner choose where to start and end")
                        .clicked()
                {
                    pr.start = None;
                    pr.end = None;
                    pick = Some(None);
                }
                ui.add_space(theme::space::S);
                let mut trim = pr.trim == Some(true);
                if ui
                    .checkbox(&mut trim, "Trim")
                    .on_hover_text("Cut the thread after this object, even if the next one is close")
                    .changed()
                {
                    pr.trim = trim.then_some(true);
                }
                let mut stop = pr.stop == Some(true);
                if ui
                    .checkbox(&mut stop, "Stop")
                    .on_hover_text("Pause the machine after this object (to lay foam, or trim appliqué fabric)")
                    .changed()
                {
                    pr.stop = stop.then_some(true);
                }
            });
        });
        if let Some(m) = clear {
            match m {
                Marker::Start => pr.start = None,
                Marker::End => pr.end = None,
            }
        }
        if let Some(m) = pick {
            self.view_tools.marker_pick = m;
            if let Some(m) = m {
                self.status = format!(
                    "Click on the object's outline where it should {}. Esc cancels.",
                    match m {
                        Marker::Start => "start sewing",
                        Marker::End => "finish",
                    }
                );
            }
        }
        0
    }

    /// The threads in sewing order: one row per color block, a stop where
    /// the same thread comes twice running. Click a row for its objects;
    /// click its swatch to change that block's thread.
    pub(crate) fn sequence_list(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let p = pal_ui(ui);
        if self.design.blocks.is_empty() {
            caption(ui, "Nothing to sew yet.");
            return;
        }
        let selected = self.sel_indices();
        let mut pick = None;
        let mut picker = None;
        for (k, blk) in self.design.blocks.iter().enumerate() {
            let n: usize = blk.runs.iter().map(|r| r.pts.len()).sum();
            let stop = k > 0 && self.design.blocks[k - 1].color == blk.color;
            let (rect, resp) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), theme::ROW_H),
                Sense::click(),
            );
            let objects = self.design.block_shapes.get(k).cloned().unwrap_or_default();
            let on = !selected.is_empty()
                && objects.len() == selected.len()
                && objects.iter().all(|i| selected.contains(i));
            if on {
                ui.painter()
                    .rect_filled(rect, 5.0, p.accent.gamma_multiply(0.18));
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 5.0, p.hover);
            }
            let sw = Rect::from_center_size(
                Pos2::new(
                    rect.left() + 20.0 + theme::LIST_SWATCH / 2.0,
                    rect.center().y,
                ),
                Vec2::splat(theme::LIST_SWATCH),
            );
            let swr = ui.interact(
                sw,
                ui.make_persistent_id(("block-swatch", k)),
                Sense::click(),
            );
            let painter = ui.painter();
            painter.text(
                Pos2::new(rect.left() + 4.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                format!("{}", k + 1),
                egui::FontId::proportional(11.0),
                p.secondary,
            );
            painter.rect_filled(sw, 3.0, crate::canvas::color32(blk.color));
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
            let (_, name, number) = crate::threads::nearest(self.view_tools.brand, blk.color);
            let name = crate::threads::label(name, number);
            let label = if stop {
                format!("{name} · stop")
            } else {
                name
            };
            painter.text(
                Pos2::new(sw.right() + theme::space::S, rect.center().y),
                egui::Align2::LEFT_CENTER,
                &label,
                egui::FontId::proportional(12.0),
                if on { p.accent } else { p.text },
            );
            painter.text(
                Pos2::new(rect.right() - 4.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                fmt_int(n),
                egui::FontId::proportional(11.0),
                p.secondary,
            );
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{} {label}", k + 1))
            });
            let names: Vec<String> = objects
                .iter()
                .filter_map(|&i| self.names.get(i).cloned())
                .collect();
            if swr.on_hover_text("Change this block's thread").clicked() {
                pick = Some(objects.clone());
                let w = crate::threads::PICKER_W + 2.0 * theme::space::M;
                picker = Some(Pos2::new(
                    rect.left() - theme::space::M - theme::space::S - w,
                    rect.top() - theme::space::M,
                ));
            } else if resp
                .on_hover_text(format!("Sews: {}", names.join(", ")))
                .clicked()
            {
                pick = Some(objects);
            }
        }
        if let Some(v) = pick {
            self.set_selection(&v);
        }
        if let Some(at) = picker {
            self.open_picker(at, None);
        }
    }

    fn document_body(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = theme::space::S;
        let fabric = self.doc.settings.fabric();
        let advanced = self.prefs.advanced;
        let usual = zigzag::document::Settings::default();
        let left_out = form_for(ui, "doc-form", advanced, |fm: &mut Form| {
            self.machine_rows(fm);
            fm.row("Fabric", |ui, lid| {
                egui::ComboBox::from_id_salt("fabric")
                    .selected_text(fabric.name)
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        for f in FABRICS {
                            ui.selectable_value(
                                &mut self.doc.settings.fabric,
                                f.id.to_string(),
                                f.name,
                            )
                            .on_hover_text(f.about);
                        }
                    })
                    .response
                    .labelled_by(lid)
                    .on_hover_text(fabric.about);
            });
            // The hoops to choose from: the machine's, then the standard
            // ones. Any other size is a custom hoop.
            let hoops = self.hoop_menu();
            let here = self.doc.settings.hoop;
            let listed = hoops
                .iter()
                .position(|x| zigzag::machine::profile::same_hoop(x.0, here));
            let custom = self.hoop_custom || listed.is_none();
            fm.row("Hoop", |ui, lid| {
                egui::ComboBox::from_id_salt("hoop")
                    .selected_text(match listed {
                        Some(k) if !self.hoop_custom => hoops[k].1.as_str(),
                        _ => "Custom",
                    })
                    .width(170.0)
                    .height(400.0)
                    .show_ui(ui, |ui| {
                        for (k, (size, name, rule)) in hoops.iter().enumerate() {
                            if *rule {
                                ui.separator();
                            }
                            let on = !custom && listed == Some(k);
                            if ui.selectable_label(on, name).clicked() {
                                self.doc.settings.hoop = *size;
                                self.hoop_custom = false;
                            }
                        }
                        ui.separator();
                        if ui
                            .selectable_label(custom, "Custom")
                            .on_hover_text(
                                "Any other frame: type its sewing field's width and height",
                            )
                            .clicked()
                        {
                            self.hoop_custom = true;
                        }
                    })
                    .response
                    .labelled_by(lid);
            });
            if custom {
                // The sewing field of any other frame, width by height.
                fm.row("Size", |ui, lid| {
                    let (w, h) = &mut self.doc.settings.hoop;
                    let side = zigzag::document::HOOP_SIDE_MM;
                    ui.add(crate::units::drag_mm(w, side.clone(), 1.0))
                        .labelled_by(lid)
                        .on_hover_text("Width of the hoop's sewing field");
                    ui.label("×");
                    ui.add(crate::units::drag_mm(h, side, 1.0))
                        .on_hover_text("Height of the hoop's sewing field")
                        .widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::SpinButton, true, "Hoop height")
                        });
                });
            }
            if fm.advanced(self.doc.settings.remove_overlaps != usual.remove_overlaps) {
                fm.row("Layers", |ui, _| {
                    ui.checkbox(&mut self.doc.settings.remove_overlaps, "Remove overlaps")
                        .on_hover_text(
                            "Don't fill areas that objects on top will cover (keeps 0.5 mm of overlap)",
                        );
                });
            }
            fm.row("Order", |ui, _| {
                ui.checkbox(&mut self.doc.settings.hat_order, "Cap order")
                    .on_hover_text("Sew from the center out and the bottom up, grouping colors, so puckering is pushed away");
            });
            if fm.advanced(self.doc.settings.over_sewn != usual.over_sewn) {
                fm.row("Travel", |ui, _| {
                    ui.checkbox(&mut self.doc.settings.over_sewn, "Sew over fill")
                        .on_hover_text(
                            "Where the thread can't get from one part of an object to the next out of sight (under stitches still to come, or under what's sewn on top): off, it's cut; on, it's sewn over the fill already there. On saves cuts, and the running stitch shows on top of the fill.",
                        );
                });
            }
            // Connect: gaps this short are sewn across in the open rather
            // than cut. A bar to drag, the cuts it saves beside it, and what
            // it sews lit up on the canvas meanwhile.
            let mut peek = false;
            if fm.advanced(self.doc.settings.connect_mm != usual.connect_mm) {
                fm.row("Connect", |ui, lid| {
                    let mm = &mut self.doc.settings.connect_mm;
                    ui.spacing_mut().slider_width = 100.0;
                    let bar = ui
                        .add(
                            egui::Slider::new(mm, 0.0..=CONNECT_BAR_MM)
                                .show_value(false)
                                .step_by(0.1)
                                .clamping(egui::SliderClamping::Never),
                        )
                        .labelled_by(lid)
                        .on_hover_text(
                            "Sew across gaps up to this long with running stitches instead of cutting the thread, between objects or the parts of one. These stitches show: they light up on the canvas as you drag. (Travel that's out of sight, under a fill or what's sewn on top, is made anyway.)",
                        );
                    bar.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Slider, true, "Connect gaps")
                    });
                    let field = ui.add(crate::units::drag_mm(mm, 0.0..=50.0, 0.1));
                    peek = [&bar, &field]
                        .iter()
                        .any(|r| r.hovered() || r.dragged() || r.has_focus() || r.changed());
                });
                let (cuts, bridged) = (self.design.trim_count(), self.design.bridges.len());
                fm.wide(|ui| {
                    caption(
                        ui,
                        connect_note(self.doc.settings.connect_mm, cuts, bridged),
                    );
                });
            }
            if peek {
                self.bridges_peek =
                    Some(web_time::Instant::now() + std::time::Duration::from_secs(6));
                fm.ui.ctx().request_repaint();
            }
            fm.row("Preview on", |ui, lid| {
                ui.color_edit_button_srgb(&mut self.doc.settings.cloth)
                    .labelled_by(lid)
                    .on_hover_text("Your hat's color, for the preview (saved with the design; doesn't change the stitches)");
            });
            if fm.advanced(self.doc.settings.narrow_satin_mm != usual.narrow_satin_mm) {
                self.narrow_satin_row(fm);
            }
        });
        self.left_out_note(ui, left_out);
    }

    /// Under a panel's settings in Basic: how many more there are, and
    /// where they're turned on (Settings › General: one choice for every
    /// panel).
    fn left_out_note(&self, ui: &mut egui::Ui, left_out: usize) {
        if self.prefs.advanced || left_out == 0 {
            return;
        }
        ui.horizontal(|ui| {
            ui.add_space(theme::LABEL_W + theme::space::S);
            caption(
                ui,
                format!(
                    "{left_out} more {}: Show advanced settings, in Settings › General.",
                    if left_out == 1 { "setting" } else { "settings" }
                ),
            );
        });
    }

    // ------------------------------------------------------------ transform

    fn transform_body(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = theme::space::S;
        if self.sel_indices().is_empty() {
            caption(ui, "Nothing selected.");
            return;
        }
        self.layout_panel(ui);
    }
}

/// How far the Connect bar goes (mm; the field beside it goes further).
const CONNECT_BAR_MM: f64 = 20.0;

/// Under the Connect bar: the cuts there are, and how many it saved.
pub(crate) fn connect_note(connect_mm: f64, cuts: usize, bridged: usize) -> String {
    let n = |k: usize| format!("{k} cut{}", if k == 1 { "" } else { "s" });
    if bridged > 0 {
        // (In words: the app's font has no arrow.)
        format!("{} down to {cuts}", n(cuts + bridged))
    } else if connect_mm > 0.0 && cuts > 0 {
        format!("{}: none this short", n(cuts))
    } else if cuts > 0 {
        format!("{}: drag to sew across the short ones", n(cuts))
    } else {
        "No cuts".to_string()
    }
}

/// The Layers list's eye and lock columns, at the left of every row (as
/// in Illustrator's Layers, and VectorCraft's): how wide each is, and
/// the two together.
const FLAG_COL: f32 = 18.0;
const GUTTER: f32 = 2.0 * FLAG_COL + 2.0;

/// The part of a row to the right of its eye and lock.
fn row_body(row: Rect) -> Rect {
    Rect::from_min_max(Pos2::new(row.left() + GUTTER, row.top()), row.max)
}

/// Which column of a row the pointer at `at` is in, if either.
fn gutter_flag(row: Rect, at: Option<Pos2>) -> Option<Flag> {
    let x = at?.x - row.left();
    if !(0.0..2.0 * FLAG_COL).contains(&x) {
        None
    } else if x < FLAG_COL {
        Some(Flag::Hide)
    } else {
        Some(Flag::Lock)
    }
}

/// A row's eye (there while it's shown) and lock (there while it's
/// locked; faint under the pointer otherwise, to say it can be).
fn eye_and_lock(
    painter: &egui::Painter,
    row: Rect,
    pointer: Option<Pos2>,
    hidden: bool,
    locked: bool,
    p: &theme::Pal,
) {
    let cell = |k: f32, size: f32| {
        Rect::from_center_size(
            Pos2::new(row.left() + (k + 0.5) * FLAG_COL, row.center().y),
            Vec2::splat(size),
        )
    };
    // (The eye is a wide shape and the padlock a narrow one: drawn in
    // boxes of these sizes they come out about the same height and
    // weight, and neither looks the bigger.)
    const EYE: f32 = 11.0;
    const LOCK: f32 = 13.0;
    let over = gutter_flag(row, pointer);
    if !hidden {
        icons::paint(painter, cell(0.0, EYE), Icon::Eye, p.secondary);
    } else if over == Some(Flag::Hide) {
        icons::paint(painter, cell(0.0, EYE), Icon::Eye, p.tertiary);
    }
    if locked {
        icons::paint(painter, cell(1.0, LOCK), Icon::Lock, p.text);
    } else if over == Some(Flag::Lock) {
        icons::paint(painter, cell(1.0, LOCK), Icon::Lock, p.tertiary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn overlapping_strokes_are_offered_only_to_be_put_back() {
        let svg = |id: &str| {
            format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
              <path id="{id}" fill="#123456" d="M10 10 L13 10 L13 19 L27 19 L27 10 L30 10 L30 30 L27 30 L27 21 L13 21 L13 30 L10 30 Z"/>
            </svg>"##
            )
        };
        let open = |id: &str| {
            let doc = zigzag::document::Document::from_svg(&svg(id), "t", None).unwrap();
            let mut h = Harness::builder()
                .with_size(Vec2::new(1400.0, 1000.0))
                .build_ui_state(
                    |ui, s: &mut Studio| s.frame(ui),
                    Studio::with_file(None, None),
                );
            h.state_mut().set_document(doc);
            h.run_steps(3);
            h.state_mut().set_selection(&[0]);
            h.run_steps(3);
            h
        };
        let h = open("H_satin");
        assert!(h.query_by_label("Overlapping").is_none(), "not offered");
        // A file's text that has them: Strokes puts it back, and the row
        // goes.
        let mut h = open("H_satin_overlap");
        assert_eq!(h.state().doc.shapes[0].params.overlap, Some(true));
        h.get_by_label("Strokes").click();
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes[0].params.overlap, None);
        assert!(h.query_by_label("Overlapping").is_none());
    }

    #[test]
    fn a_filled_satins_corners_are_auto_mitred_or_cut() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
              <path id="H_satin" fill="#123456" d="M10 10 L13 10 L13 19 L27 19 L27 10 L30 10 L30 30 L27 30 L27 21 L13 21 L13 30 L10 30 Z"/>
            </svg>"##;
        let doc = zigzag::document::Document::from_svg(svg, "t", None).unwrap();
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.state_mut().prefs.advanced = true;
        h.state_mut().set_document(doc);
        h.run_steps(3);
        h.state_mut().set_selection(&[0]);
        h.run_steps(3);
        // Cut unless it's asked to be mitred where that's clean (Auto) or
        // everywhere; and back.
        let p = |h: &Harness<Studio>| {
            let p = &h.state().doc.shapes[0].params;
            (p.cut, p.mitred)
        };
        assert_eq!(p(&h), (None, None));
        h.get_by_label("Auto").click();
        h.run_steps(3);
        assert_eq!(p(&h), (Some(false), None));
        h.get_by_label("Mitred").click();
        h.run_steps(3);
        assert_eq!(p(&h), (Some(false), Some(true)));
        h.get_by_label("Cut").click();
        h.run_steps(3);
        assert_eq!(p(&h), (None, None));
    }

    #[test]
    fn layers_are_dragged_to_another_place_in_the_sewing_order() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="40mm" viewBox="0 0 100 40">
            <rect id="one" x="5" y="5" width="15" height="15" fill="#c00000"/>
            <rect id="two" x="25" y="5" width="15" height="15" fill="#00a000"/>
            <rect id="three" x="45" y="5" width="15" height="15" fill="#0000c0"/>
            <rect id="four" x="65" y="5" width="15" height="15" fill="#c0a000"/>
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
        let order = |h: &Harness<'_, Studio>| h.state().names.clone();
        let row = |h: &Harness<'_, Studio>, name: &str| {
            h.query_all_by_label(name)
                .find(|n| n.accesskit_node().role() == egui::accesskit::Role::ListItem)
                .unwrap()
                .rect()
        };
        use egui_kittest::kittest::NodeT;
        let drag = |h: &mut Harness<'_, Studio>, from: Pos2, to: Pos2| {
            h.hover_at(from);
            h.run_steps(1);
            h.drag_at(from);
            h.run_steps(1);
            for k in 1..=4 {
                h.hover_at(from + (to - from) * (k as f32 / 4.0));
                h.run_steps(1);
            }
            h.drop_at(to);
            h.run_steps(3);
        };
        assert_eq!(order(&h), ["one", "two", "three", "four"]);
        // What's on top is listed first, the background last.
        assert!(row(&h, "four").top() < row(&h, "three").top());
        assert!(row(&h, "two").top() < row(&h, "one").top());

        // "four" onto the top half of "two": it goes just above it, sewn
        // right after it.
        let (from, onto) = (row(&h, "four"), row(&h, "two"));
        h.hover_at(from.center());
        h.run_steps(1);
        h.drag_at(from.center());
        h.run_steps(1);
        h.hover_at(onto.center() - Vec2::new(0.0, 6.0));
        h.run_steps(2);
        // Under way: nothing moved yet, a copy of the row at the pointer.
        assert!(h.state().dock_ui.layer_drag.is_some());
        assert_eq!(order(&h), ["one", "two", "three", "four"]);
        h.drop_at(onto.center() - Vec2::new(0.0, 6.0));
        h.run_steps(3);
        assert!(h.state().dock_ui.layer_drag.is_none());
        assert_eq!(order(&h), ["one", "two", "four", "three"]);
        assert_eq!(h.state().undo.len(), 1, "one undo step");
        // The order is the file's: it comes back from it.
        let saved = h.state().doc.to_svg().unwrap();
        let again = zigzag::document::Document::from_svg(&saved, "t", None).unwrap();
        assert_eq!(
            zigzag::svg::display_names(&again.shapes),
            ["one", "two", "four", "three"]
        );

        // Onto the bottom half of a row: under it, sewn before it. Below
        // them all: sewn first.
        let (from, onto) = (row(&h, "three"), row(&h, "two"));
        drag(&mut h, from.center(), onto.center() + Vec2::new(0.0, 6.0));
        assert_eq!(order(&h), ["one", "three", "two", "four"]);
        let (from, last) = (row(&h, "four"), row(&h, "one"));
        drag(&mut h, from.center(), last.center() + Vec2::new(0.0, 40.0));
        assert_eq!(order(&h), ["four", "one", "three", "two"]);
        assert_eq!(h.state().undo.len(), 3);

        // Dropped where it is already, or outside the list: nothing, and
        // nothing to undo. A click still just selects.
        let at = row(&h, "one");
        drag(&mut h, at.center(), at.center() + Vec2::new(0.0, 6.0));
        drag(&mut h, at.center(), at.center() - Vec2::new(400.0, 0.0));
        assert_eq!(order(&h), ["four", "one", "three", "two"]);
        assert_eq!(h.state().undo.len(), 3);
        h.get_by_label("three").click();
        h.run_steps(2);
        assert_eq!(h.state().sel_index(), Some(2));

        // Several selected, one of them dragged: they all go, in their
        // own order, and stay selected.
        h.state_mut().set_selection(&[0, 2]);
        h.run_steps(2);
        let (from, onto) = (row(&h, "three"), row(&h, "two"));
        drag(&mut h, from.center(), onto.center() - Vec2::new(0.0, 6.0));
        assert_eq!(order(&h), ["one", "two", "four", "three"]);
        let mut sel = h.state().sel_indices();
        sel.sort_unstable();
        assert_eq!(sel, [2, 3]);
        // Undo puts them back.
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        h.run_steps(3);
        assert_eq!(order(&h), ["four", "one", "three", "two"]);

        // Escape while dragging calls it off.
        let (from, onto) = (row(&h, "four"), row(&h, "two"));
        h.hover_at(from.center());
        h.run_steps(1);
        h.drag_at(from.center());
        h.run_steps(1);
        h.hover_at(onto.center());
        h.run_steps(2);
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        h.drop_at(onto.center());
        h.run_steps(3);
        assert_eq!(order(&h), ["four", "one", "three", "two"]);
    }

    #[test]
    fn a_group_is_dragged_whole_and_dropped_on_as_one() {
        use egui_kittest::kittest::NodeT;
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="40mm" viewBox="0 0 100 40">
            <g id="pair">
              <rect id="left" x="5" y="5" width="15" height="15" fill="#c00000"/>
              <rect id="right" x="25" y="5" width="15" height="15" fill="#00a000"/>
            </g>
            <rect id="solo" x="45" y="5" width="15" height="15" fill="#0000c0"/>
            <rect id="other" x="65" y="5" width="15" height="15" fill="#c0a000"/>
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
        let order = |h: &Harness<'_, Studio>| h.state().names.clone();
        let row = |h: &Harness<'_, Studio>, name: &str| {
            h.query_all_by_label(name)
                .find(|n| n.accesskit_node().role() == egui::accesskit::Role::ListItem)
                .unwrap()
                .rect()
        };
        let drag = |h: &mut Harness<'_, Studio>, from: Pos2, to: Pos2| {
            h.hover_at(from);
            h.run_steps(1);
            h.drag_at(from);
            h.run_steps(1);
            for k in 1..=4 {
                h.hover_at(from + (to - from) * (k as f32 / 4.0));
                h.run_steps(1);
            }
            h.drop_at(to);
            h.run_steps(3);
        };
        assert_eq!(order(&h), ["left", "right", "solo", "other"]);
        // (The document's only group starts open; closed, for its row.)
        assert!(h.state().layout.expanded.contains("pair"));
        h.state_mut().layout.expanded.clear();
        h.run_steps(2);
        // The group's row (closed) dragged above "solo": both its layers go.
        let (from, onto) = (row(&h, "pair"), row(&h, "solo"));
        drag(&mut h, from.center(), onto.center() - Vec2::new(0.0, 6.0));
        assert_eq!(order(&h), ["solo", "left", "right", "other"]);
        // A layer dropped on the group's top half goes above all of it
        // (sewn after it); on its bottom half, under all of it.
        let (from, onto) = (row(&h, "solo"), row(&h, "pair"));
        drag(&mut h, from.center(), onto.center() - Vec2::new(0.0, 6.0));
        assert_eq!(order(&h), ["left", "right", "solo", "other"]);
        let (from, onto) = (row(&h, "other"), row(&h, "pair"));
        drag(&mut h, from.center(), onto.center() + Vec2::new(0.0, 6.0));
        assert_eq!(order(&h), ["other", "left", "right", "solo"]);
        assert_eq!(h.state().undo.len(), 3);
    }

    #[test]
    fn layers_in_sewing_order_list_and_drop_the_other_way_up() {
        use egui_kittest::kittest::NodeT;
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="40mm" viewBox="0 0 100 40">
            <g id="pair">
              <rect id="left" x="5" y="5" width="15" height="15" fill="#c00000"/>
              <rect id="right" x="25" y="5" width="15" height="15" fill="#00a000"/>
            </g>
            <rect id="solo" x="45" y="5" width="15" height="15" fill="#0000c0"/>
            <rect id="other" x="65" y="5" width="15" height="15" fill="#c0a000"/>
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
        let order = |h: &Harness<'_, Studio>| h.state().names.clone();
        let row = |h: &Harness<'_, Studio>, name: &str| {
            h.query_all_by_label(name)
                .find(|n| n.accesskit_node().role() == egui::accesskit::Role::ListItem)
                .unwrap()
                .rect()
        };
        let drag = |h: &mut Harness<'_, Studio>, from: Pos2, to: Pos2| {
            h.hover_at(from);
            h.run_steps(1);
            h.drag_at(from);
            h.run_steps(1);
            for k in 1..=4 {
                h.hover_at(from + (to - from) * (k as f32 / 4.0));
                h.run_steps(1);
            }
            h.drop_at(to);
            h.run_steps(3);
        };
        // What's on top is at the top, until the panel's menu says
        // otherwise: then what's sewn first is.
        assert!(row(&h, "other").top() < row(&h, "solo").top());
        assert!(row(&h, "solo").top() < row(&h, "pair").top());
        h.get_by_label("Layers menu").click();
        h.run_steps(2);
        h.get_by_label("List in Sewing Order").click();
        h.run_steps(3);
        assert!(h.state().dock.layers_sewing_order);
        assert!(row(&h, "pair").top() < row(&h, "left").top());
        assert!(row(&h, "left").top() < row(&h, "right").top());
        assert!(row(&h, "right").top() < row(&h, "solo").top());
        assert!(row(&h, "solo").top() < row(&h, "other").top());
        assert_eq!(order(&h), ["left", "right", "solo", "other"]);

        // The top half of a row is before it now, the bottom half after,
        // and below them all is last.
        let (from, onto) = (row(&h, "other"), row(&h, "solo"));
        drag(&mut h, from.center(), onto.center() - Vec2::new(0.0, 6.0));
        assert_eq!(order(&h), ["left", "right", "other", "solo"]);
        let (from, onto) = (row(&h, "other"), row(&h, "solo"));
        drag(&mut h, from.center(), onto.center() + Vec2::new(0.0, 6.0));
        assert_eq!(order(&h), ["left", "right", "solo", "other"]);
        let (from, last) = (row(&h, "solo"), row(&h, "other"));
        drag(&mut h, from.center(), last.center() + Vec2::new(0.0, 40.0));
        assert_eq!(order(&h), ["left", "right", "other", "solo"]);
        // A closed group: its top half is before all of it, its bottom
        // half after all of it.
        h.state_mut().layout.expanded.clear();
        h.run_steps(2);
        let (from, onto) = (row(&h, "solo"), row(&h, "pair"));
        drag(&mut h, from.center(), onto.center() - Vec2::new(0.0, 6.0));
        assert_eq!(order(&h), ["solo", "left", "right", "other"]);
        let (from, onto) = (row(&h, "solo"), row(&h, "pair"));
        drag(&mut h, from.center(), onto.center() + Vec2::new(0.0, 6.0));
        assert_eq!(order(&h), ["left", "right", "solo", "other"]);

        // The arrows under the list move the row the way they point.
        h.state_mut().set_selection(&[2]);
        h.run_steps(2);
        h.get_by_label("Sew earlier").click();
        h.run_steps(3);
        assert_eq!(order(&h), ["left", "solo", "right", "other"]);
        let (up, down) = (
            h.get_by_label("Sew earlier").rect(),
            h.get_by_label("Sew later").rect(),
        );
        assert!(up.left() < down.left(), "up arrow first: earlier");
        h.state_mut().dock.layers_sewing_order = false;
        h.run_steps(2);
        let (up, down) = (
            h.get_by_label("Sew later").rect(),
            h.get_by_label("Sew earlier").rect(),
        );
        assert!(up.left() < down.left(), "up arrow first: later");
        h.get_by_label("Sew later").click();
        h.run_steps(3);
        assert_eq!(order(&h), ["left", "right", "solo", "other"]);
    }

    #[test]
    fn a_documents_only_group_starts_open() {
        let open = |body: &str| {
            let svg = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="40mm" viewBox="0 0 100 40">{body}</svg>"##
            );
            let mut s = Studio::with_file(None, None);
            s.set_document(zigzag::document::Document::from_svg(&svg, "t", None).unwrap());
            let mut open: Vec<String> = s.layout.expanded.iter().cloned().collect();
            open.sort();
            open
        };
        let rect = |id: &str, x: u32| {
            format!(r##"<rect id="{id}" x="{x}" y="5" width="10" height="10" fill="#c00000"/>"##)
        };
        let (a, b, c, d) = (rect("a", 5), rect("b", 20), rect("c", 35), rect("d", 50));
        // The whole design in one group, or one group with layers beside it.
        assert_eq!(open(&format!(r#"<g id="all">{a}{b}{c}</g>"#)), ["all"]);
        assert_eq!(open(&format!(r#"<g id="pair">{a}{b}</g>{c}"#)), ["pair"]);
        // Nothing in it but another group: that one too, and no further
        // once there's a choice.
        assert_eq!(
            open(&format!(
                r#"<g id="outer"><g id="inner"><g id="p">{a}{b}</g><g id="q">{c}{d}</g></g></g>"#
            )),
            ["inner", "outer"]
        );
        // Two groups, or none: as they were, closed.
        assert!(open(&format!(r#"<g id="one">{a}{b}</g><g id="two">{c}{d}</g>"#)).is_empty());
        assert!(open(&format!("{a}{b}")).is_empty());
        // The next document doesn't keep the last one's open groups.
        let mut s = Studio::with_file(None, None);
        s.layout.expanded.insert("stale".into());
        s.set_document(zigzag::document::Document::empty());
        assert!(s.layout.expanded.is_empty());
    }

    #[test]
    fn moving_layers_keeps_their_own_order() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="40mm" viewBox="0 0 100 40">
            <rect id="a" x="5" y="5" width="10" height="10" fill="#c00000"/>
            <rect id="b" x="20" y="5" width="10" height="10" fill="#00a000"/>
            <rect id="c" x="35" y="5" width="10" height="10" fill="#0000c0"/>
            <rect id="d" x="50" y="5" width="10" height="10" fill="#c0a000"/>
            <rect id="e" x="65" y="5" width="10" height="10" fill="#00a0c0"/>
        </svg>"##;
        let mut s = Studio::with_file(None, None);
        s.set_document(zigzag::document::Document::from_svg(svg, "t", None).unwrap());
        s.replan();
        let order = |s: &Studio| {
            s.doc
                .shapes
                .iter()
                .map(|x| x.id.clone())
                .collect::<Vec<_>>()
        };
        assert!(s.move_layers(&[3, 1], 0));
        assert_eq!(order(&s), ["b", "d", "a", "c", "e"]);
        assert!(s.move_layers(&[0, 1], 5));
        assert_eq!(order(&s), ["a", "c", "e", "b", "d"]);
        assert!(s.move_layers(&[0, 4], 2));
        assert_eq!(order(&s), ["c", "a", "d", "e", "b"]);
        // Where they are already (before themselves, or just after).
        assert!(!s.move_layers(&[1, 2], 1));
        assert!(!s.move_layers(&[1, 2], 3));
        assert!(!s.move_layers(&[], 0));
        assert!(!s.move_layers(&[9], 0));
        assert_eq!(s.undo.len(), 3);
    }

    #[test]
    fn a_line_shows_every_stitch_type_with_the_ones_for_areas_dimmed() {
        use crate::widgets::{ALL_KINDS, allowed_kinds, kind_name};
        use egui_kittest::kittest::NodeT;
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="30mm" viewBox="0 0 60 30">
            <path id="edge" d="M5 25 L55 25" fill="none" stroke="#203040" stroke-width="2"/>
            <rect id="patch" x="5" y="5" width="20" height="12" fill="#c00000"/>
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
        let line = (0..2)
            .find(|&i| allowed_kinds(&h.state().doc.shapes[i]).len() < ALL_KINDS.len())
            .unwrap();
        let tiles = |h: &Harness<'_, Studio>| -> Vec<(&'static str, bool)> {
            ALL_KINDS
                .iter()
                .map(|&k| {
                    let name = kind_name(k);
                    let on = h
                        .query_all_by_label(name)
                        .filter(|n| n.accesskit_node().role() == egui::accesskit::Role::Button)
                        .any(|n| !n.accesskit_node().is_disabled());
                    (name, on)
                })
                .collect()
        };
        // The line: all twelve are there, and the seven that cover an area
        // can't be chosen.
        h.state_mut().select_index(Some(line));
        h.run_steps(4);
        let on: Vec<&str> = tiles(&h).iter().filter(|t| t.1).map(|t| t.0).collect();
        assert_eq!(on, ["Satin", "Run", "Blanket", "Manual", "Off"]);
        for k in ALL_KINDS {
            assert!(
                h.query_all_by_label(kind_name(k))
                    .any(|n| n.accesskit_node().role() == egui::accesskit::Role::Button),
                "{} has a tile",
                kind_name(k)
            );
        }
        let was = h.state().doc.shapes[line].params.kind;
        let fill = h
            .query_all_by_label("Fill")
            .find(|n| n.accesskit_node().is_disabled())
            .unwrap()
            .rect()
            .center();
        h.hover_at(fill);
        h.run_steps(1);
        h.drag_at(fill);
        h.run_steps(1);
        h.drop_at(fill);
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes[line].params.kind, was);
        // The filled shape: every one can be.
        h.state_mut().select_index(Some(1 - line));
        h.run_steps(4);
        assert!(tiles(&h).iter().all(|t| t.1));
    }

    #[test]
    fn a_double_click_on_a_layers_name_renames_it_and_nothing_less_does() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="30mm" viewBox="0 0 60 30">
            <rect id="patch" x="5" y="5" width="20" height="20" fill="#203040"/>
            <rect id="stripe_satin" x="30" y="5" width="20" height="3" fill="#c00000"/>
        </svg>"##;
        let doc = zigzag::document::Document::from_svg(svg, "t", None).unwrap();
        // (Frames a sixtieth of a second apart, so two clicks can be a
        // double-click or not by how many frames lie between them.)
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 900.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.state_mut().set_document(doc);
        h.run_steps(4);
        // Longer than a double-click takes.
        const A_WHILE: usize = 45;
        // Where a row's name is, and where its thread dot is.
        let row = |h: &Harness<'_, Studio>, name: &str| h.get_by_label(name).rect();
        // (Past the eye and lock columns at the row's left.)
        let on_name = |r: Rect| Pos2::new(r.left() + GUTTER + 70.0, r.center().y);
        let on_dot = |r: Rect| Pos2::new(r.left() + GUTTER + 12.0, r.center().y);
        let click = |h: &mut Harness<'_, Studio>, at: Pos2, times: usize| {
            h.hover_at(at);
            h.run_steps(1);
            for _ in 0..times {
                h.drag_at(at);
                h.run_steps(1);
                h.drop_at(at);
                h.run_steps(1);
            }
            h.run_steps(3);
        };
        let patch = on_name(row(&h, "patch"));
        // One click selects. So does another, a while later: clicking a
        // selected layer again is not asking to rename it.
        click(&mut h, patch, 1);
        assert_eq!(h.state().sel_index(), Some(0));
        assert!(h.state().renaming.is_none());
        h.run_steps(A_WHILE);
        click(&mut h, patch, 1);
        assert!(h.state().renaming.is_none(), "a slow second click");
        h.run_steps(A_WHILE);
        // Nor is a double-click off the name (on the thread dot).
        let dot = on_dot(row(&h, "patch"));
        click(&mut h, dot, 2);
        assert!(h.state().renaming.is_none(), "a double-click on the dot");
        h.run_steps(A_WHILE);
        assert!(h.query_by_label("Layer name").is_none());
        // A double-click on the name: a field, with the name in it.
        click(&mut h, patch, 2);
        assert_eq!(
            h.state().renaming.as_ref().map(|r| (r.0, r.1.as_str())),
            Some((h.state().doc.shapes[0].uid, "patch"))
        );
        assert!(h.query_by_label("Layer name").is_some());
        // Typing replaces it (it was selected); Enter keeps the new name.
        h.get_by_label("Layer name").type_text("Front Patch");
        h.run_steps(2);
        h.key_press(egui::Key::Enter);
        h.run_steps(4);
        assert!(h.state().renaming.is_none());
        assert_eq!(h.state().names[0], "Front Patch");
        assert!(h.state().modified);
        assert!(h.query_by_label("Front Patch").is_some());
        assert!(h.state().doc.to_svg().unwrap().contains("id=\"Front_Patch"));
        // One undo step.
        assert!(h.state_mut().apply_history(crate::app::History::Undo));
        h.state_mut().replan();
        h.run_steps(3);
        assert_eq!(h.state().names[0], "patch");
        // Escape leaves the name alone; so does a name that's a stitch
        // setting, with a word about why.
        h.run_steps(A_WHILE);
        let stripe = on_name(row(&h, "stripe"));
        click(&mut h, stripe, 2);
        assert!(h.state().renaming.is_some());
        h.get_by_label("Layer name").type_text("Never Mind");
        h.run_steps(2);
        h.key_press(egui::Key::Escape);
        h.run_steps(4);
        assert!(h.state().renaming.is_none());
        assert_eq!(h.state().names[1], "stripe");
        h.run_steps(A_WHILE);
        click(&mut h, stripe, 2);
        h.get_by_label("Layer name").type_text("fill");
        h.run_steps(2);
        h.key_press(egui::Key::Enter);
        h.run_steps(4);
        assert_eq!(h.state().names[1], "stripe");
        assert_eq!(h.state().doc.shapes[1].params.kind, Some(Kind::Satin));
        assert!(
            h.state().status.contains("stitch setting"),
            "{}",
            h.state().status
        );
    }

    #[test]
    fn a_changed_row_stays_while_its_dragged_back_to_what_it_was() {
        // Basic, a setting that's been changed: its row shows. Dragged to
        // the value it had (a lean back to 0), the row stays under the
        // pointer until it's let go, and then goes.
        let mut h = Harness::builder()
            .with_size(Vec2::new(400.0, 200.0))
            .build_ui_state(
                |ui, v: &mut f64| {
                    theme::form_for(ui, "t", false, |fm| {
                        if fm.advanced(*v != 0.0) {
                            fm.row("Lean", |ui, lid| {
                                ui.add(egui::DragValue::new(v)).labelled_by(lid);
                            });
                        }
                    });
                },
                4.0,
            );
        h.run_steps(2);
        assert!(h.query_by_label("Lean").is_some());
        let at = h
            .query_all_by_role(egui::accesskit::Role::SpinButton)
            .next()
            .expect("its value")
            .rect()
            .center();
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(2);
        *h.state_mut() = 0.0;
        h.run_steps(3);
        assert!(h.query_by_label("Lean").is_some(), "still in hand");
        h.drop_at(at);
        h.run_steps(3);
        assert!(h.query_by_label("Lean").is_none(), "let go: back to Basic");
    }

    #[test]
    fn basic_shows_what_most_designs_need_and_whatever_was_changed() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
              <path id="H_satin" fill="#123456" d="M10 10 L13 10 L13 19 L27 19 L27 10 L30 10 L30 30 L27 30 L27 21 L13 21 L13 30 L10 30 Z"/>
            </svg>"##;
        let doc = zigzag::document::Document::from_svg(svg, "t", None).unwrap();
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        assert!(h.state().prefs.advanced, "every setting to begin with");
        h.state_mut().prefs.advanced = false;
        h.state_mut().set_document(doc);
        h.run_steps(3);
        h.state_mut().set_selection(&[0]);
        h.run_steps(3);
        let advanced = [
            "Pull comp.",
            "Lean",
            "Corners",
            "Short stitches",
            "Auto split",
            "Trim",
            "Remove overlaps",
            "Connect gaps",
        ];
        assert!(h.query_by_label("Density").is_some());
        for name in advanced {
            assert!(h.query_by_label(name).is_none(), "{name} is advanced");
        }
        // Changed out of sight (by an agent, say): it shows, though Basic.
        h.state_mut().doc.shapes[0].params.lean = Some(4.0);
        h.state_mut().doc.settings.connect_mm = 3.0;
        h.run_steps(3);
        assert!(h.query_by_label("Lean").is_some());
        assert!(h.query_by_label("Connect gaps").is_some());
        assert!(h.query_by_label("Pull comp.").is_none());
        // Advanced: everything.
        // (Each panel says how many more it has, and where they're
        // turned on: Settings › General.)
        assert_eq!(
            h.query_all_by_label_contains("Show advanced settings")
                .count(),
            2
        );
        h.state_mut().prefs.advanced = true;
        h.run_steps(3);
        assert_eq!(
            h.query_all_by_label_contains("Show advanced settings")
                .count(),
            0
        );
        for name in advanced {
            assert!(h.query_by_label(name).is_some(), "{name} shows in Advanced");
        }
    }

    #[test]
    fn the_unit_is_remembered() {
        let f = std::env::temp_dir().join(format!("tl-units-{}.json", std::process::id()));
        let mut d = DockState::load_from(Some(f.clone()));
        assert_eq!(d.units, crate::units::Unit::Mm, "millimetres to start");
        d.units = crate::units::Unit::In;
        d.save();
        assert_eq!(
            DockState::load_from(Some(f.clone())).units,
            crate::units::Unit::In
        );
        d.reset_layout();
        assert_eq!(d.units, crate::units::Unit::In, "Reset Panels keeps it");
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn stitch_types_show_as_icons() {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1500.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        let blurb = kind_blurb(Kind::Satin);
        assert!(h.query_by_label(blurb).is_none(), "icons only");
        h.get_by_label("Satin"); // the icon, named for screen readers
        assert!(h.query_by_label_contains("Tip: name layers").is_none());
        assert!(h.query_by_label_contains("Select an object").is_none());
        // Panel menus no longer offer descriptions.
        h.get_by_label("Transform menu").click();
        h.run_steps(2);
        assert!(h.query_by_label("Show Descriptions").is_none());
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        // The menu also hides the panel.
        h.get_by_label("Threads menu").click();
        h.run_steps(2);
        h.get_by_label("Hide Panel").click();
        h.run_steps(2);
        assert!(!h.state().dock.visible(PanelId::Threads));
    }

    fn harness() -> Harness<'static, Studio> {
        let path =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/badge.svg"));
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(path), None),
            );
        // As it starts outside tests: the Agent in the left dock.
        h.state_mut().dock.reveal(PanelId::Agent);
        h.run_steps(3);
        h
    }

    fn drag(h: &mut Harness<'_, Studio>, from: Pos2, to: Pos2) {
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        for k in 1..=5 {
            h.hover_at(from + (to - from) * (k as f32 / 5.0));
            h.run_steps(1);
        }
        h.drop_at(to);
        h.run_steps(3);
    }

    #[test]
    fn docks_collapse_to_icons_with_flyouts() {
        let mut h = harness();
        // Right dock's » collapses it; there are two (one per dock).
        let n = h.query_all_by_label("Collapse to Icons").count();
        assert_eq!(n, 2);
        h.state_mut()
            .run_action(crate::commands::Action::IconicPanels);
        h.run_steps(2);
        assert!(h.state().dock.iconic[1]);
        assert!(h.query_by_label("Stitch panel").is_none(), "only icons now");
        h.get_by_label("Stitch").click();
        h.run_steps(2);
        assert_eq!(h.state().dock_ui.flyout, Some(PanelId::Stitch));
        h.get_by_label("Stitch panel"); // flown out beside the strip
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert_eq!(h.state().dock_ui.flyout, None);
        h.get_by_label("Expand Panels").click();
        h.run_steps(2);
        assert!(!h.state().dock.iconic[1]);
    }

    #[test]
    fn panels_drag_between_docks_and_out_to_float() {
        let mut h = harness();
        // Threads, dragged by its header onto the canvas, floats.
        let header = h.get_by_label("Threads panel").rect();
        let canvas = h.state().canvas_rect.center();
        drag(&mut h, header.center(), canvas);
        assert_eq!(
            h.state().dock.place_of(PanelId::Threads),
            Place::Float,
            "floated"
        );
        h.get_by_label("Threads panel");
        // Document, dragged into the left dock above the Agent.
        h.state_mut().dock.reveal(PanelId::Agent);
        h.run_steps(3);
        let header = h.get_by_label("Document panel").rect();
        let agent = h.get_by_label("Agent panel").rect();
        // (Its top edge: on the header itself it would join the Agent as a tab.)
        drag(
            &mut h,
            header.center(),
            agent.left_top() + Vec2::new(60.0, 3.0),
        );
        assert_eq!(h.state().dock.place_of(PanelId::Document), Place::Left);
        let left = h.state().panels_in(Place::Left);
        let d = left.iter().position(|&p| p == PanelId::Document).unwrap();
        let i = left.iter().position(|&p| p == PanelId::Agent).unwrap();
        assert!(d < i, "{left:?}");
        // The Agent stays at the bottom, the Document above it.
        h.run_steps(30);
        let doc = h.get_by_label("Document panel").rect();
        let agent = h.get_by_label("Agent panel").rect();
        assert!(doc.top() < agent.top(), "{doc:?} {agent:?}");
        // The floating one docks again from its menu.
        h.get_by_label("Threads menu").click();
        h.run_steps(2);
        h.get_by_label("Dock Right").click();
        h.run_steps(2);
        assert_eq!(h.state().dock.place_of(PanelId::Threads), Place::Right);
        // Reset puts everything back.
        h.state_mut()
            .run_action(crate::commands::Action::ResetPanels);
        h.run_steps(2);
        assert_eq!(h.state().dock.place_of(PanelId::Document), Place::Right);
    }

    #[test]
    fn panels_combine_into_tabs_and_come_apart() {
        let mut h = harness();
        // Threads dropped on the Stitch panel's header joins it as a tab.
        let threads = h.get_by_label("Threads panel").rect();
        let stitch = h.get_by_label("Stitch panel").rect();
        drag(
            &mut h,
            threads.center(),
            stitch.center() + Vec2::new(0.0, 4.0),
        );
        assert_eq!(h.state().dock.lead_of(PanelId::Threads), PanelId::Stitch);
        assert_eq!(
            h.state().dock.active[PanelId::Stitch.index()],
            Some(PanelId::Threads)
        );
        assert!(
            !h.state()
                .panels_in(Place::Right)
                .contains(&PanelId::Threads)
        );
        // Two tabs, Threads in front; click Stitch's tab to switch.
        assert!(h.query_by_label("Threads tab").is_some());
        h.get_by_label("Stitch tab").click();
        h.run_steps(2);
        assert_eq!(
            h.state().dock.active[PanelId::Stitch.index()],
            Some(PanelId::Stitch)
        );
        // Dragging the group's header moves both; to the left dock.
        let header = h.get_by_label("Stitch panel").rect();
        let agent = h.get_by_label("Agent panel").rect();
        drag(
            &mut h,
            header.right_center() - Vec2::new(60.0, 0.0),
            agent.left_top() + Vec2::new(60.0, 3.0),
        );
        assert_eq!(h.state().dock.place_of(PanelId::Stitch), Place::Left);
        assert_eq!(h.state().dock.place_of(PanelId::Threads), Place::Left);
        // Dragging a tab out takes just that panel, here onto the canvas.
        let tab = h.get_by_label("Threads tab").rect();
        let canvas = h.state().canvas_rect.center();
        drag(&mut h, tab.center(), canvas);
        assert_eq!(h.state().dock.lead_of(PanelId::Threads), PanelId::Threads);
        assert_eq!(h.state().dock.place_of(PanelId::Threads), Place::Float);
        assert_eq!(h.state().dock.place_of(PanelId::Stitch), Place::Left);
        assert!(
            h.query_by_label("Stitch tab").is_none(),
            "a group of one is a plain panel"
        );
    }

    #[test]
    fn issues_open_from_the_status_bar() {
        let mut h = harness();
        let id = egui::Id::unique("issues-popover");
        assert!(!egui::Popup::is_id_open(&h.ctx, id));
        h.get_by_label("Issues").click();
        h.run_steps(2);
        assert!(egui::Popup::is_id_open(&h.ctx, id));
        assert!(h.query_by_label("No problems found.").is_some());
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        // A problem shows in the list, and Window > Issues opens it.
        h.state_mut().run_tool(
            "set_object",
            &serde_json::json!({ "object": "sun", "kind": "satin" }),
        );
        h.run_steps(3);
        assert!(!h.state().issues.is_empty());
        egui::Popup::close_id(&h.ctx, id);
        h.state_mut()
            .run_action(crate::commands::Action::ShowIssues);
        h.run_steps(3);
        assert!(egui::Popup::is_id_open(&h.ctx, id));
        assert!(h.query_by_label("No problems found.").is_none());
    }

    #[test]
    fn a_layout_from_before_the_agent_panel_moves_objects_right() {
        let file =
            std::env::temp_dir().join(format!("zigzag-old-panels-{}.json", std::process::id()));
        std::fs::write(
            &file,
            r#"{"left":true,"right":true,"panels":{"Objects":{"place":"left"},"Issues":{"place":"left"},"Stitch":{"place":"right"}},"order":["Objects","Issues","Stitch","Transform"]}"#,
        )
        .unwrap();
        let d = DockState::load_from(Some(file.clone()));
        assert_eq!(d.place_of(PanelId::Objects), Place::Right);
        assert_eq!(d.order[0], PanelId::Objects);
        assert_eq!(d.place_of(PanelId::Agent), Place::Left);
        assert!(d.visible(PanelId::Agent) || cfg!(test));
        // Saved again, it's a layout with the Agent: left as it is.
        let mut d = d;
        d.move_panel(PanelId::Objects, Place::Left, None);
        d.save();
        let e = DockState::load_from(Some(file.clone()));
        assert_eq!(e.place_of(PanelId::Objects), Place::Left);
        // The panel is called Layers now, and layouts still know it by the
        // name they were saved with.
        assert_eq!(PanelId::Objects.title(), "Layers");
        let saved = std::fs::read_to_string(&file).unwrap();
        assert!(
            saved.contains("\"Objects\"") && !saved.contains("Layers"),
            "{saved}"
        );
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn panel_layout_is_remembered() {
        let file = std::env::temp_dir().join(format!("zigzag-panels-{}.json", std::process::id()));
        let mut d = DockState::load_from(Some(file.clone()));
        assert!(d.visible(PanelId::Lettering) && d.left);
        d.toggle(PanelId::Lettering);
        d.collapsed[PanelId::Document.index()] = true;
        d.left = false;
        d.move_panel(PanelId::Threads, Place::Float, None);
        d.float_at[PanelId::Threads.index()] = Pos2::new(500.0, 200.0);
        d.move_panel(PanelId::Document, Place::Left, Some(PanelId::Objects));
        d.iconic = [false, true];
        d.left = false;
        d.join_tab(PanelId::Transform, PanelId::Stitch);
        assert!(!d.layers_sewing_order);
        d.layers_sewing_order = true;
        d.save();
        let e = DockState::load_from(Some(file.clone()));
        assert!(e.layers_sewing_order);
        assert_eq!(e.place_of(PanelId::Threads), Place::Float);
        assert_eq!(
            e.float_at[PanelId::Threads.index()],
            Pos2::new(500.0, 200.0)
        );
        assert_eq!(e.place_of(PanelId::Document), Place::Left);
        let at = |p: PanelId| e.order.iter().position(|&q| q == p).unwrap();
        assert!(
            at(PanelId::Document) < at(PanelId::Objects),
            "{:?}",
            e.order
        );
        assert_eq!(e.iconic, [false, true]);
        assert!(!e.visible(PanelId::Lettering));
        assert!(e.collapsed(PanelId::Document));
        assert!(!e.left && e.right);
        assert_eq!(
            e.lead_of(PanelId::Transform),
            PanelId::Stitch,
            "tab groups too"
        );
        assert_eq!(e.active[PanelId::Stitch.index()], Some(PanelId::Transform));
        let _ = std::fs::remove_file(file);
    }
}
