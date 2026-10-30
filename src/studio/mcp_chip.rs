//! The MCP chip at the right of the status bar, next to the Skitch's: whether
//! agents can reach the studio, and (on click) everything about it — the
//! address, how to connect each kind of client, who has connected, the
//! recent tool calls and the tools on offer.

use crate::app::Studio;
use crate::theme::{caption, pal_ui, section_break, space};
use eframe::egui::{self, Pos2, Sense, Vec2};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How many recent calls the popover keeps.
const KEEP: usize = 40;
/// How long after a request the chip shows as busy.
const BUSY: Duration = Duration::from_millis(1500);

/// One tool call, as the UI thread answered it.
#[derive(Clone, Debug)]
pub struct CallRecord {
    pub name: String,
    pub at: Instant,
    pub ms: f64,
    /// The error text, when the call failed.
    pub error: Option<String>,
}

#[derive(Default)]
pub struct McpUi {
    /// Newest last.
    pub calls: VecDeque<CallRecord>,
    pub total: u64,
    pub errors: u64,
    /// The port asked for (None: started with `--no-mcp`).
    pub port: Option<u16>,
    /// Why the server didn't start.
    pub start_error: Option<String>,
    /// (name, description) of each tool, read once.
    tools: Option<Vec<(String, String)>>,
}

impl McpUi {
    pub fn note(&mut self, name: &str, at: Instant, result: &crate::mcp::ToolResult) {
        let error = result.is_error.then(|| {
            result
                .content
                .iter()
                .find_map(|c| match c {
                    crate::mcp::Content::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        });
        self.total += 1;
        self.errors += u64::from(error.is_some());
        self.calls.push_back(CallRecord {
            name: name.to_string(),
            at,
            ms: at.elapsed().as_secs_f64() * 1000.0,
            error,
        });
        while self.calls.len() > KEEP {
            self.calls.pop_front();
        }
    }

    fn tools(&mut self) -> &[(String, String)] {
        self.tools.get_or_insert_with(|| {
            crate::tools::tool_specs()
                .as_array()
                .map(|v| {
                    v.iter()
                        .map(|t| {
                            let s = |k: &str| t[k].as_str().unwrap_or("").to_string();
                            (s("name"), s("description"))
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
    }
}

/// "just now", "12 s ago", "3 min ago", "2 h ago".
pub fn ago(t: Instant) -> String {
    let s = t.elapsed().as_secs();
    match s {
        0..=1 => "just now".into(),
        2..=59 => format!("{s} s ago"),
        60..=3599 => format!("{} min ago", s / 60),
        _ => format!("{} h ago", s / 3600),
    }
}

/// The `zigzag-mcp` bridge beside this app (in the .app bundle, or the
/// build folder), if there is one.
fn bridge_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.with_file_name(if cfg!(windows) {
        "zigzag-mcp.exe"
    } else {
        "zigzag-mcp"
    });
    p.exists().then_some(p)
}

/// What the chip shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Started with `--no-mcp`.
    Off,
    /// Couldn't start (the port is taken…).
    Failed,
    /// Serving; nobody has connected yet.
    Waiting,
    /// A client has connected.
    Connected,
    /// A request just came in.
    Busy,
}

impl Studio {
    pub(crate) fn mcp_state(&self) -> State {
        let Some(server) = &self.mcp else {
            return if self.mcp_ui.start_error.is_some() {
                State::Failed
            } else {
                State::Off
            };
        };
        let a = server
            .activity
            .lock()
            .map(|a| a.clone())
            .unwrap_or_default();
        let recent_call = self
            .mcp_ui
            .calls
            .back()
            .is_some_and(|c| c.at.elapsed() < BUSY);
        if recent_call || a.last_request.is_some_and(|t| t.elapsed() < BUSY) {
            State::Busy
        } else if !a.clients.is_empty() {
            State::Connected
        } else {
            State::Waiting
        }
    }

    /// The chip, and its popover.
    pub fn mcp_chip(&mut self, ui: &mut egui::Ui) {
        let p = pal_ui(ui);
        let state = self.mcp_state();
        let tip = match state {
            State::Off => "MCP server off",
            State::Failed => "The MCP server couldn't start",
            State::Waiting => "MCP server running — no agents yet",
            State::Connected => "An agent is connected over MCP",
            State::Busy => "An agent is working over MCP",
        };
        let connected = matches!(state, State::Connected | State::Busy);
        // Same grays as the machine chip; green, with a dot, once an agent is on.
        let ink = match state {
            State::Failed => p.error,
            State::Off => p.tertiary,
            State::Waiting => p.secondary,
            State::Connected | State::Busy => crate::sew::GOOD,
        };
        let w = 26.0 + if connected { 8.0 } else { 0.0 };
        let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 22.0), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "MCP status"));
        let popup_id = egui::Id::unique("mcp-popover");
        let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
        if resp.hovered() || open {
            ui.painter().rect_filled(r, 5.0, p.hover);
        }
        let icon_r =
            egui::Rect::from_center_size(Pos2::new(r.left() + 13.0, r.center().y), Vec2::splat(14.0));
        crate::icons::paint(ui.painter(), icon_r, crate::icons::Icon::Mcp, ink);
        if connected {
            let c = Pos2::new(r.left() + 27.0, r.center().y);
            if state == State::Busy {
                // A soft ring breathing around the dot while calls come in.
                let t = ui.input(|i| i.time) as f32;
                let k = 0.5 + 0.5 * (t * 5.0).sin();
                ui.painter().circle_filled(
                    c,
                    3.0 + 2.0 * k,
                    crate::sew::GOOD.gamma_multiply(0.25 * (1.0 - k) + 0.1),
                );
                ui.ctx().request_repaint_after(Duration::from_millis(33));
            }
            ui.painter().circle_filled(c, 3.0, crate::sew::GOOD);
        }
        if state != State::Off {
            // Comes back from busy without anyone touching the window.
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        let resp = resp.on_hover_text(tip);
        egui::Popup::from_toggle_button_response(&resp)
            .id(popup_id)
            .align(egui::RectAlign::TOP_END)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width(360.0)
            .show(|ui| {
                ui.set_width(360.0);
                // Times in it count up.
                ui.ctx().request_repaint_after(Duration::from_secs(1));
                self.mcp_body(ui, state);
            });
    }

    fn mcp_body(&mut self, ui: &mut egui::Ui, state: State) {
        let p = pal_ui(ui);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("MCP Server").strong().size(13.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (text, color) = match state {
                    State::Off => ("Off", p.secondary),
                    State::Failed => ("Not running", p.error),
                    State::Waiting => ("Running", p.secondary),
                    State::Connected => ("Connected", crate::sew::GOOD),
                    State::Busy => ("Working", p.accent),
                };
                ui.label(egui::RichText::new(text).size(11.5).color(color));
            });
        });
        ui.add_space(space::XS);
        let Some(server) = &self.mcp else {
            match &self.mcp_ui.start_error {
                Some(e) => {
                    let port = self.mcp_ui.port.unwrap_or(crate::mcp::DEFAULT_PORT);
                    ui.label(
                        egui::RichText::new(format!("Couldn't start on port {port}: {e}"))
                            .size(12.0)
                            .color(p.error),
                    );
                    caption(
                        ui,
                        "Another copy of the studio may be using the port. Quit it, or start \
                         this one with --mcp-port and another number.",
                    );
                }
                None => {
                    ui.label(egui::RichText::new("The MCP server is off.").size(12.0));
                    caption(
                        ui,
                        "The studio was started with --no-mcp. Start it without that to let \
                         agents work on the canvas.",
                    );
                }
            }
            ui.add_space(space::S);
            caption(
                ui,
                "Agents can still work headless, on a document no one sees: \
                 zigzag-studio --mcp-stdio.",
            );
            return;
        };
        let url = format!("http://{}/mcp", server.addr);
        let port = server.addr.port();
        let activity = server
            .activity
            .lock()
            .map(|a| a.clone())
            .unwrap_or_default();

        copy_row(ui, "Address", &url);
        caption(
            ui,
            "Streamable HTTP, on this Mac only. Agents edit the tab in front; every call \
             is one undo step.",
        );

        section_break(ui);
        ui.label(egui::RichText::new("Connect").strong().size(12.0));
        ui.add_space(space::XS);
        let claude = format!("claude mcp add --transport http zigzag {url}");
        copy_block(ui, "Claude Code", &claude);
        ui.add_space(space::S);
        let bridge = bridge_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "/path/to/zigzag-mcp".into());
        let args = if port == crate::mcp::DEFAULT_PORT {
            String::new()
        } else {
            format!(", \"args\": [\"--port\", \"{port}\"]")
        };
        let desktop = format!(
            "{{ \"mcpServers\": {{ \"zigzag\": {{ \"command\": \"{bridge}\"{args} }} }} }}"
        );
        copy_block(ui, "Claude Desktop and other stdio clients", &desktop);

        section_break(ui);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Clients").strong().size(12.0));
            if let Some(t) = activity.last_request {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    caption(ui, format!("last heard {}", ago(t)));
                });
            }
        });
        ui.add_space(space::XS);
        if activity.clients.is_empty() {
            caption(ui, "No agent has connected since the studio opened.");
        }
        for c in activity.clients.iter().rev() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&c.name).size(12.0));
                if !c.version.is_empty() {
                    ui.label(
                        egui::RichText::new(&c.version)
                            .size(11.0)
                            .color(p.secondary),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    caption(ui, ago(c.at)).on_hover_text(format!(
                        "Connected (initialize) {}. Protocol {}.",
                        ago(c.at),
                        if c.protocol.is_empty() {
                            "?"
                        } else {
                            &c.protocol
                        }
                    ));
                });
            });
        }

        section_break(ui);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Activity").strong().size(12.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let calls = self.mcp_ui.total;
                let mut s = format!(
                    "{} {} · {} requests",
                    calls,
                    if calls == 1 { "call" } else { "calls" },
                    activity.requests
                );
                if self.mcp_ui.errors > 0 {
                    s = format!("{s} · {} failed", self.mcp_ui.errors);
                }
                caption(ui, s);
            });
        });
        ui.add_space(space::XS);
        if self.mcp_ui.calls.is_empty() {
            caption(ui, "No tool calls yet.");
        }
        for c in self.mcp_ui.calls.iter().rev().take(8) {
            ui.horizontal(|ui| {
                let color = if c.error.is_some() { p.error } else { p.text };
                let name = ui.label(
                    egui::RichText::new(&c.name)
                        .size(11.5)
                        .monospace()
                        .color(color),
                );
                if let Some(e) = &c.error {
                    name.on_hover_text(e);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    caption(ui, ago(c.at));
                    caption(ui, format!("{:.0} ms", c.ms));
                });
            });
        }

        section_break(ui);
        let tools = self.mcp_ui.tools().to_vec();
        egui::CollapsingHeader::new(
            egui::RichText::new(format!("Tools ({})", tools.len()))
                .strong()
                .size(12.0),
        )
        .id_salt("mcp-tools")
        .default_open(false)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .show(ui, |ui| {
                    for (name, about) in &tools {
                        ui.label(egui::RichText::new(name).size(11.5).monospace())
                            .on_hover_text(about);
                    }
                });
        });
    }
}

/// A label, a value in monospace, and a Copy button.
fn copy_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        caption(ui, label);
        ui.label(egui::RichText::new(value).size(12.0).monospace());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Copy").clicked() {
                ui.ctx().copy_text(value.to_string());
            }
        });
    });
}

/// A caption over a wrapped monospace block, with a Copy button.
pub(crate) fn copy_block(ui: &mut egui::Ui, label: &str, text: &str) {
    let p = pal_ui(ui);
    ui.horizontal(|ui| {
        caption(ui, label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("Copy")
                .on_hover_text(format!("Copy: {text}"))
                .clicked()
            {
                ui.ctx().copy_text(text.to_string());
            }
        });
    });
    egui::Frame::new()
        .fill(p.control)
        .stroke(egui::Stroke::new(1.0, p.control_border))
        .corner_radius(5.0)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(egui::RichText::new(text).size(11.0).monospace()).wrap());
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use serde_json::json;

    #[test]
    fn clients_are_noted_once_each() {
        let mut a = crate::mcp::Activity::default();
        let hello = |name: &str, v: &str| {
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": { "protocolVersion": "2025-06-18",
                                "clientInfo": { "name": name, "version": v } } })
        };
        a.note_for_test(&hello("claude-code", "2.1.0"));
        a.note_for_test(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
        a.note_for_test(&hello("claude-code", "2.1.0"));
        a.note_for_test(&hello("codex", "0.9"));
        assert_eq!(a.requests, 4);
        let names: Vec<&str> = a.clients.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["claude-code", "codex"]);
        assert_eq!(a.clients[0].protocol, "2025-06-18");
    }

    #[test]
    fn the_chip_says_how_to_connect_and_what_agents_did() {
        let mut s = Studio::with_file(None, None);
        let server =
            crate::mcp::start(0, crate::tools::tool_specs(), String::new(), || {}).expect("a port");
        let url = format!("http://{}/mcp", server.addr);
        s.mcp = Some(server);
        let at = Instant::now();
        let r = s.run_mcp_call("get_design", &json!({}));
        s.mcp_ui.note("get_design", at, &r);
        let bad = crate::mcp::ToolResult::error("no such object");
        s.mcp_ui.note("set_object", Instant::now(), &bad);
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 900.0))
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), s);
        h.run_steps(3);
        assert_eq!(h.state().mcp_state(), State::Busy, "calls just now");
        h.get_by_label("MCP status").click();
        h.run_steps(3);
        assert!(h.query_by_label(&url).is_some(), "the address");
        assert!(
            h.query_by_label(&format!("claude mcp add --transport http zigzag {url}"))
                .is_some(),
            "how to add it to Claude Code"
        );
        assert!(h.query_by_label("get_design").is_some());
        assert!(
            h.query_by_label("set_object").is_some(),
            "the failed one too"
        );
        assert!(h.query_by_label_contains("2 calls").is_some(), "the counts");
    }

    #[test]
    fn the_chip_says_when_the_server_is_off() {
        let mut s = Studio::with_file(None, None);
        s.mcp_ui.port = Some(7878);
        s.mcp_ui.start_error = Some("address in use".into());
        assert_eq!(s.mcp_state(), State::Failed);
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 900.0))
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), s);
        h.run_steps(2);
        h.get_by_label("MCP status").click();
        h.run_steps(3);
        assert!(
            h.query_by_label_contains("Couldn't start on port 7878")
                .is_some()
        );
    }
}
