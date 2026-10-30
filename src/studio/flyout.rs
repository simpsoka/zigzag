//! Tool groups, Illustrator-style. A slot in the tool strip holds a group
//! (the Pen's: Pen, Add Anchor Point, Delete Anchor Point, Anchor Point; the
//! Brush's: Brush, Smooth, Path Eraser, Join; the shapes'), showing the one last used. Press and hold it, or right-click,
//! for the flyout: each tool with its icon and key, a square beside the one
//! in the slot; release or click on one to take it. Option-click the slot to
//! step through the group. The strip at the flyout's right edge tears the
//! group off into a small floating toolbar of its own.

use crate::app::{Studio, Tool};
use crate::icons::{self, Icon};
use crate::theme::{icon_button, pal_ui};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

/// Illustrator's Pen group: (tool, name, key, icon).
pub const PEN_GROUP: [(Tool, &str, &str, Icon); 4] = [
    (Tool::Pen, "Pen Tool", "P", Icon::Pen),
    (Tool::AddAnchor, "Add Anchor Point Tool", "+", Icon::PenAdd),
    (
        Tool::DeleteAnchor,
        "Delete Anchor Point Tool",
        "-",
        Icon::PenDelete,
    ),
    (
        Tool::Anchor,
        "Anchor Point Tool",
        "Shift+C",
        Icon::AnchorPoint,
    ),
];

/// VectorCraft's Brush group: the Brush and the path clean-up tools.
pub const BRUSH_GROUP: [(Tool, &str, &str, Icon); 4] = [
    (Tool::Brush, "Brush Tool", "B", Icon::Brush),
    (Tool::Smooth, "Smooth Tool", "", Icon::Smooth),
    (Tool::PathEraser, "Path Eraser Tool", "", Icon::PathEraser),
    (Tool::Join, "Join Tool", "", Icon::Join),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Pen,
    Shape,
    Brush,
}

impl Group {
    fn title(self) -> &'static str {
        match self {
            Group::Pen => "Pen Tools",
            Group::Shape => "Shape Tools",
            Group::Brush => "Brush Tools",
        }
    }
}

/// One tool in a group's flyout.
struct Item {
    icon: Icon,
    name: String,
    key: &'static str,
    /// The one the slot shows.
    in_slot: bool,
    /// The tool in use.
    active: bool,
}

/// The open flyout and the torn-off toolbars.
#[derive(Default)]
pub struct Flyouts {
    /// The group whose flyout is open, where (its top left), and whether it
    /// was opened by holding the button down (then releasing on an item takes it).
    open: Option<(Group, Pos2, bool)>,
    /// Torn-off groups and where they float (top left).
    pub torn: Vec<(Group, Pos2)>,
}

/// How long the button is held before its flyout opens (seconds).
const HOLD: f64 = 0.35;

impl Studio {
    fn group_items(&self, g: Group) -> Vec<Item> {
        match g {
            Group::Pen => PEN_GROUP
                .iter()
                .map(|&(t, name, key, icon)| Item {
                    icon,
                    name: name.into(),
                    key,
                    in_slot: self.view_tools.pen_slot == t,
                    active: self.tool == t,
                })
                .collect(),
            Group::Brush => BRUSH_GROUP
                .iter()
                .map(|&(t, name, key, icon)| Item {
                    icon,
                    name: name.into(),
                    key,
                    in_slot: self.view_tools.brush_slot == t,
                    active: self.tool == t,
                })
                .collect(),
            Group::Shape => crate::shapes::KINDS
                .iter()
                .map(|&k| Item {
                    icon: k.icon(),
                    name: format!("{} Tool", k.name()),
                    key: k.key(),
                    in_slot: self.shapes.kind == k,
                    active: self.tool == Tool::Shape && self.shapes.kind == k,
                })
                .collect(),
        }
    }

    /// Take tool `k` of group `g`.
    fn choose(&mut self, g: Group, k: usize) {
        match g {
            Group::Pen => self.set_tool(PEN_GROUP[k].0),
            Group::Brush => self.set_tool(BRUSH_GROUP[k].0),
            Group::Shape => {
                self.shapes.kind = crate::shapes::KINDS[k];
                self.set_tool(Tool::Shape);
            }
        }
    }

    /// The group's tool in its slot (index into its items).
    fn slot_index(&self, g: Group) -> usize {
        self.group_items(g)
            .iter()
            .position(|i| i.in_slot)
            .unwrap_or(0)
    }

    /// A group's slot in the tool strip: the tool it shows, a small
    /// triangle for the group behind it, and the ways to reach the rest.
    pub fn group_button(&mut self, ui: &mut egui::Ui, g: Group) {
        let p = pal_ui(ui);
        let items = self.group_items(g);
        let k = self.slot_index(g);
        let item = &items[k];
        let label = item.name.trim_end_matches(" Tool").to_string();
        let tip = if item.key.is_empty() {
            format!("{label} · hold or right-click for the group")
        } else {
            format!("{label} ({}) · hold or right-click for the group", item.key)
        };
        let r = icon_button(ui, item.icon, &label, item.active, 30.0).on_hover_text(tip);
        // Illustrator's mark for a tool group: a small triangle at the corner.
        let c = r.rect.right_bottom() + Vec2::new(-4.0, -4.0);
        ui.painter().add(egui::Shape::convex_polygon(
            vec![c, c + Vec2::new(-4.0, 0.0), c + Vec2::new(0.0, -4.0)],
            p.secondary,
            Stroke::NONE,
        ));
        let at = Pos2::new(r.rect.right() + 6.0, r.rect.top() - 4.0);
        let alt = ui.input(|i| i.modifiers.alt);
        let open_here = self.flyouts.open.is_some_and(|(o, _, _)| o == g);
        if r.secondary_clicked() {
            self.flyouts.open = if open_here {
                None
            } else {
                Some((g, at, false))
            };
        } else if r.clicked() && !open_here {
            if alt {
                // Option-click: the next tool in the group.
                self.choose(g, (k + 1) % items.len());
            } else {
                self.choose(g, k);
            }
        }
        // Held down: the flyout opens under the pointer, to slide onto a tool.
        if r.is_pointer_button_down_on() && !open_here {
            let held = ui.input(|i| i.pointer.press_start_time().map(|t| i.time - t));
            match held {
                Some(h) if h >= HOLD => self.flyouts.open = Some((g, at, true)),
                Some(_) => ui.ctx().request_repaint(),
                None => {}
            }
        }
        if self.flyouts.open.is_some_and(|(o, _, _)| o == g) {
            self.flyout(ui.ctx(), g, r.rect);
        }
    }

    /// The open flyout beside its button.
    fn flyout(&mut self, ctx: &egui::Context, g: Group, button: Rect) {
        let Some((_, at, by_hold)) = self.flyouts.open else {
            return;
        };
        let items = self.group_items(g);
        let mut chosen = None;
        let mut tear = false;
        const ROW: f32 = 28.0;
        const STRIP: f32 = 14.0;
        let released = ctx.input(|i| i.pointer.primary_released());
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let area = egui::Area::new(egui::Id::unique(("tool-flyout", g as u8)))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                let p = pal_ui(ui);
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::same(4))
                    .corner_radius(6)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::ZERO;
                            // The tools, one a row.
                            let rows = ui
                                .vertical(|ui| {
                                    for (k, it) in items.iter().enumerate() {
                                        let (r, resp) = ui.allocate_exact_size(
                                            Vec2::new(250.0, ROW),
                                            Sense::click(),
                                        );
                                        resp.widget_info(|| {
                                            egui::WidgetInfo::selected(
                                                egui::Role::MenuItem,
                                                true,
                                                it.in_slot,
                                                &it.name,
                                            )
                                        });
                                        let over = pointer.is_some_and(|q| r.contains(q));
                                        if over {
                                            ui.painter().rect_filled(r, 4.0, p.accent);
                                        }
                                        let ink = if over {
                                            Color32::WHITE
                                        } else if it.active {
                                            p.accent
                                        } else {
                                            p.text
                                        };
                                        // The square beside the one in the slot.
                                        if it.in_slot {
                                            ui.painter().rect_filled(
                                                Rect::from_center_size(
                                                    Pos2::new(r.left() + 10.0, r.center().y),
                                                    Vec2::splat(5.0),
                                                ),
                                                0.0,
                                                if over { Color32::WHITE } else { p.secondary },
                                            );
                                        }
                                        icons::paint(
                                            ui.painter(),
                                            Rect::from_center_size(
                                                Pos2::new(r.left() + 32.0, r.center().y),
                                                Vec2::splat(16.0),
                                            ),
                                            it.icon,
                                            ink,
                                        );
                                        ui.painter().text(
                                            Pos2::new(r.left() + 50.0, r.center().y),
                                            egui::Align2::LEFT_CENTER,
                                            &it.name,
                                            egui::FontId::proportional(13.0),
                                            ink,
                                        );
                                        if !it.key.is_empty() {
                                            ui.painter().text(
                                                Pos2::new(r.right() - 8.0, r.center().y),
                                                egui::Align2::RIGHT_CENTER,
                                                format!("({})", it.key),
                                                egui::FontId::proportional(12.5),
                                                ink,
                                            );
                                        }
                                        // Clicked, or (held open) the button let go over it.
                                        if resp.clicked() || (by_hold && released && over) {
                                            chosen = Some(k);
                                        }
                                    }
                                })
                                .response
                                .rect;
                            // The tear-off strip down the right edge.
                            let (r, resp) = ui.allocate_exact_size(
                                Vec2::new(STRIP, rows.height()),
                                Sense::click(),
                            );
                            resp.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Button, true, "Tear Off")
                            });
                            let over = pointer.is_some_and(|q| r.contains(q));
                            ui.painter().rect_filled(
                                r.shrink2(Vec2::new(1.0, 0.0)),
                                3.0,
                                if over { p.hover } else { p.header },
                            );
                            let c = r.center();
                            ui.painter().add(egui::Shape::convex_polygon(
                                vec![
                                    c + Vec2::new(-2.0, -4.0),
                                    c + Vec2::new(3.0, 0.0),
                                    c + Vec2::new(-2.0, 4.0),
                                ],
                                if over { p.text } else { p.secondary },
                                Stroke::NONE,
                            ));
                            if resp
                                .on_hover_text("Tear off: the group as a toolbar of its own")
                                .clicked()
                                || (by_hold && released && over)
                            {
                                tear = true;
                            }
                        });
                    });
            });
        let rect = area.response.rect;
        if let Some(k) = chosen {
            self.choose(g, k);
            self.flyouts.open = None;
        } else if tear {
            if !self.flyouts.torn.iter().any(|(t, _)| *t == g) {
                self.flyouts
                    .torn
                    .push((g, rect.right_top() + Vec2::new(8.0, 0.0)));
            }
            self.flyouts.open = None;
        } else {
            // Let go anywhere else (when held open), a click outside, or Esc closes it.
            let outside_press = ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|q| !rect.contains(q) && !button.contains(q))
            });
            let let_go_outside = by_hold
                && released
                && pointer.is_some_and(|q| !rect.contains(q) && !button.contains(q));
            let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            if outside_press || let_go_outside || esc {
                self.flyouts.open = None;
            } else if by_hold && released {
                // Let go on the button itself: it stays open to click in.
                self.flyouts.open = Some((g, at, false));
            }
        }
    }

    /// The torn-off groups: small floating toolbars, dragged by their top
    /// bar, closed with ×.
    pub fn torn_toolbars(&mut self, ctx: &egui::Context) {
        let mut close = None;
        let mut take = None;
        for (n, &(g, at)) in self.flyouts.torn.clone().iter().enumerate() {
            let items = self.group_items(g);
            let area = egui::Area::new(egui::Id::unique(("torn-toolbar", g as u8)))
                .order(egui::Order::Middle)
                .fixed_pos(at)
                .constrain(true)
                .show(ctx, |ui| {
                    let p = pal_ui(ui);
                    egui::Frame::popup(ui.style())
                        .inner_margin(egui::Margin::same(3))
                        .corner_radius(6)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(2.0, 2.0);
                            let w = items.len() as f32 * 32.0 - 2.0;
                            // The bar to drag it by, and × to put it away.
                            let (bar, resp) =
                                ui.allocate_exact_size(Vec2::new(w, 12.0), Sense::click_and_drag());
                            resp.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Button, true, g.title())
                            });
                            ui.painter().rect_filled(bar, 3.0, p.header);
                            if resp.dragged() {
                                let d = resp.drag_delta();
                                if let Some(t) = self.flyouts.torn.get_mut(n) {
                                    t.1 += d;
                                }
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                            } else if resp.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                            }
                            let x = Rect::from_center_size(
                                Pos2::new(bar.right() - 7.0, bar.center().y),
                                Vec2::splat(10.0),
                            );
                            let xr = ui.interact(
                                x,
                                ui.make_persistent_id(("torn-close", g as u8)),
                                Sense::click(),
                            );
                            xr.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::Role::Button,
                                    true,
                                    format!("Close {}", g.title()),
                                )
                            });
                            icons::paint(
                                ui.painter(),
                                x.shrink(1.5),
                                Icon::Close,
                                if xr.hovered() { p.text } else { p.tertiary },
                            );
                            if xr.clicked() {
                                close = Some(n);
                            }
                            ui.horizontal(|ui| {
                                for (k, it) in items.iter().enumerate() {
                                    let label = it.name.trim_end_matches(" Tool");
                                    let tip = if it.key.is_empty() {
                                        it.name.clone()
                                    } else {
                                        format!("{} ({})", it.name, it.key)
                                    };
                                    if icon_button(ui, it.icon, label, it.active, 30.0)
                                        .on_hover_text(tip)
                                        .clicked()
                                    {
                                        take = Some((g, k));
                                    }
                                }
                            });
                        });
                });
            let _ = area;
        }
        if let Some(n) = close {
            self.flyouts.torn.remove(n);
        }
        if let Some((g, k)) = take {
            self.choose(g, k);
        }
    }
}

/// How wide menu rows are.
pub const MENU_W: f32 = 250.0;

/// What a menu row shows at its left.
#[derive(Clone, Copy)]
pub enum RowIcon {
    None,
    Icon(Icon),
    /// A stitch type's own icon.
    Kind(zigzag::params::Kind),
}

/// One row of a menu, in the tool flyouts' style: an icon, the name, its key
/// at the right, a small square beside the current choice; the row under
/// the pointer fills blue with white text. (Right-click menus, the task
/// bar's menus.)
pub fn menu_row(
    ui: &mut egui::Ui,
    icon: RowIcon,
    label: &str,
    key: &str,
    current: bool,
    enabled: bool,
) -> egui::Response {
    let p = pal_ui(ui);
    // A menu as wide as its rows: they set it (a popup would let a row
    // take all the room it offers).
    let w = MENU_W.max(ui.min_rect().width());
    let (r, resp) = ui.allocate_exact_size(
        Vec2::new(w, 26.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::MenuItem, enabled, current, label));
    let over = enabled && resp.hovered();
    if over {
        ui.painter().rect_filled(r, 4.0, p.accent);
    }
    let ink = if !enabled {
        p.tertiary
    } else if over {
        Color32::WHITE
    } else {
        p.text
    };
    if current {
        ui.painter().rect_filled(
            Rect::from_center_size(Pos2::new(r.left() + 9.0, r.center().y), Vec2::splat(5.0)),
            0.0,
            if over { Color32::WHITE } else { p.secondary },
        );
    }
    let ir = Rect::from_center_size(Pos2::new(r.left() + 28.0, r.center().y), Vec2::splat(15.0));
    match icon {
        RowIcon::None => {}
        RowIcon::Icon(i) => icons::paint(ui.painter(), ir, i, ink),
        RowIcon::Kind(k) => crate::widgets::draw_kind_icon(ui.painter(), ir, k, ink),
    }
    ui.painter().text(
        Pos2::new(r.left() + 44.0, r.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        ink,
    );
    if !key.is_empty() {
        ui.painter().text(
            Pos2::new(r.right() - 8.0, r.center().y),
            egui::Align2::RIGHT_CENTER,
            key,
            egui::FontId::proportional(12.0),
            if over { Color32::WHITE } else { p.secondary },
        );
    }
    resp
}

/// A small heading over a group of menu rows.
pub fn menu_heading(ui: &mut egui::Ui, text: &str) {
    let p = pal_ui(ui);
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(egui::RichText::new(text).size(11.0).color(p.secondary));
    });
}

/// A thin line between groups of menu rows.
pub fn menu_separator(ui: &mut egui::Ui) {
    let p = pal_ui(ui);
    let (r, _) = ui.allocate_exact_size(Vec2::new(MENU_W, 7.0), Sense::hover());
    ui.painter().hline(
        (r.left() + 6.0)..=(r.right() - 6.0),
        r.center().y,
        Stroke::new(1.0, p.separator),
    );
}

impl Studio {
    /// A menu row for action `a`: its name and shortcut from the command
    /// table, enabled as the action is. True when clicked (it has run).
    pub fn action_row(
        &mut self,
        ui: &mut egui::Ui,
        a: crate::commands::Action,
        label: &str,
        icon: RowIcon,
    ) -> bool {
        let key = crate::commands::all_actions()
            .into_iter()
            .find(|(x, _, _)| *x == a)
            .and_then(|(_, _, sc)| sc)
            .map(|sc| ui.ctx().format_shortcut(&sc.egui()))
            .unwrap_or_default();
        let current = self.action_checked(a).unwrap_or(false);
        if menu_row(ui, icon, label, &key, current, self.action_enabled(a)).clicked() {
            self.run_action(a);
            ui.close();
            return true;
        }
        false
    }
}
