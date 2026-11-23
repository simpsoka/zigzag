//! Slash commands in the Agent panel's prompt: typing `/` opens a menu of
//! them above the field (arrows move, Tab completes, Enter runs, Esc
//! closes), and a command that takes a choice lists its choices next
//! (`/model ` the models, `/effort ` the efforts).

use crate::agent::{AgentKind, Entry};
use crate::app::Studio;
use crate::theme::{pal_ui, space};
use eframe::egui::{self, Key, Modifiers, Rect, Sense, Stroke, Vec2};

/// A command: its name (without the slash), whether it takes a choice,
/// and what it does.
pub struct Command {
    pub name: &'static str,
    pub takes: Option<&'static str>,
    pub about: &'static str,
}

pub const COMMANDS: [Command; 8] = [
    Command {
        name: "model",
        takes: Some("name"),
        about: "Choose the model the agent runs on",
    },
    Command {
        name: "effort",
        takes: Some("level"),
        about: "How hard the agent thinks before it acts",
    },
    Command {
        name: "agent",
        takes: Some("codex or claude"),
        about: "Switch between Codex and Claude Code (a new chat)",
    },
    Command {
        name: "new",
        takes: None,
        about: "Start a new chat: the agent forgets this one",
    },
    Command {
        name: "history",
        takes: Some("chat"),
        about: "Open an earlier chat about this design",
    },
    Command {
        name: "stop",
        takes: None,
        about: "Stop the agent",
    },
    Command {
        name: "fix",
        takes: None,
        about: "Ask the agent to fix the design's problems",
    },
    Command {
        name: "help",
        takes: None,
        about: "List these commands",
    },
];

fn command(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name.eq_ignore_ascii_case(name))
}

/// A row of the menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: String,
    /// What the command takes, shown quieter after it ("name").
    pub hint: String,
    pub about: String,
    /// The prompt once it's picked.
    pub fill: String,
    /// Picking it runs it (else it's completed, for its choices next).
    pub run: bool,
    /// The choice in use now.
    pub current: bool,
}

/// A choice for a command: (value, label, about, in use now).
fn choices(
    studio: &mut Studio,
    cmd: &str,
    ctx: &egui::Context,
) -> Vec<(String, String, String, bool)> {
    let a = &mut studio.agent;
    let default = |name: &str, current: bool| {
        (
            "default".to_string(),
            "Default".to_string(),
            format!("Whatever {name} uses on its own"),
            current,
        )
    };
    match cmd {
        "model" => {
            let now = a.model().map(str::to_string);
            let mut out = vec![default(a.kind.name(), now.is_none())];
            for m in a.models(ctx) {
                let on = now.as_ref().is_some_and(|n| n.eq_ignore_ascii_case(&m.id));
                out.push((m.id, m.name, m.about, on));
            }
            // A model chosen by its full name, not in the list: still shown.
            if let Some(n) = now
                && !out.iter().any(|c| c.0.eq_ignore_ascii_case(&n))
            {
                out.push((n.clone(), n, "Chosen by name".into(), true));
            }
            out
        }
        "effort" => {
            let now = a.effort().map(str::to_string);
            let mut out = vec![default(a.kind.name(), now.is_none())];
            for e in a.efforts(ctx) {
                let on = now.as_deref() == Some(e.as_str());
                out.push((e.clone(), e, String::new(), on));
            }
            out
        }
        "history" => studio
            .doc_chats()
            .into_iter()
            .map(|m| {
                let on = studio.agent.hist_id.as_deref() == Some(m.id.as_str());
                let title = if m.title.is_empty() {
                    "Untitled chat".to_string()
                } else {
                    m.title.clone()
                };
                (
                    m.id.clone(),
                    title,
                    format!("{} · {}", m.ago(), m.agent.name()),
                    on,
                )
            })
            .collect(),
        "agent" => AgentKind::ALL
            .iter()
            .map(|&k| {
                (
                    k.key().to_string(),
                    k.name().to_string(),
                    String::new(),
                    k == a.kind,
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The menu for `prompt`: empty unless it's a command being typed.
pub fn rows(studio: &mut Studio, prompt: &str, ctx: &egui::Context) -> Vec<Row> {
    let Some(body) = prompt.strip_prefix('/') else {
        return Vec::new();
    };
    if body.contains('\n') {
        return Vec::new();
    }
    let has = |s: &str, part: &str| s.to_lowercase().contains(&part.to_lowercase());
    if let Some((name, part)) = body.split_once(' ') {
        let Some(cmd) = command(name).filter(|c| c.takes.is_some()) else {
            return Vec::new();
        };
        let part = part.trim();
        return choices(studio, cmd.name, ctx)
            .into_iter()
            .filter(|(v, label, ..)| has(v, part) || has(label, part))
            .map(|(v, label, about, current)| Row {
                label,
                hint: String::new(),
                about,
                fill: format!("/{} {v}", cmd.name),
                run: true,
                current,
            })
            .collect();
    }
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(&body.to_lowercase()))
        .map(|c| Row {
            label: format!("/{}", c.name),
            hint: c.takes.unwrap_or("").to_string(),
            about: c.about.to_string(),
            fill: if c.takes.is_some() {
                format!("/{} ", c.name)
            } else {
                format!("/{}", c.name)
            },
            run: c.takes.is_none(),
            current: false,
        })
        .collect()
}

impl Studio {
    /// Whether a prompt starting with `/` is meant as a command: its
    /// first word is one, or could be a misspelt one (`/Users/me/art.png
    /// trace this` is a message, and is sent).
    pub fn is_slash(text: &str) -> bool {
        let body = text.trim().trim_start_matches('/');
        let name = body.split(char::is_whitespace).next().unwrap_or("");
        command(name).is_some()
            || (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
    }

    /// Run a slash command typed in the prompt (`text` starts with `/`).
    pub fn run_slash(&mut self, text: &str, ctx: &egui::Context) {
        let body = text.trim().trim_start_matches('/');
        let (name, arg) = match body.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (body, ""),
        };
        let say = |s: &mut Studio, t: String| s.agent.log.push(Entry::Info(t));
        // (What isn't a command stays in the box, to be put right.)
        if command(name).is_some() {
            self.agent.prompt.clear();
        }
        let Some(cmd) = command(name) else {
            say(
                self,
                format!("There's no /{name} command. Type / to see them."),
            );
            return;
        };
        match cmd.name {
            "model" if arg.is_empty() => {
                let now = self.agent.model().unwrap_or("its default").to_string();
                say(
                    self,
                    format!(
                        "{} is on {now}. Type /model and a name to change it.",
                        self.agent.kind.name()
                    ),
                );
            }
            "model" => {
                let list = choices(self, "model", ctx);
                // A model's name or its id; anything else as given (a
                // full model name the agent knows).
                let pick = list
                    .iter()
                    .find(|c| c.0.eq_ignore_ascii_case(arg) || c.1.eq_ignore_ascii_case(arg))
                    .map(|c| c.0.clone())
                    .unwrap_or_else(|| arg.to_string());
                let model = (pick != "default").then_some(pick);
                let shown = model.clone().unwrap_or_else(|| "its default model".into());
                let note = self.agent.set_model(model, ctx);
                say(
                    self,
                    format!(
                        "{} will use {shown} from the next message.",
                        self.agent.kind.name()
                    ),
                );
                if let Some(n) = note {
                    say(self, n);
                }
            }
            "effort" if arg.is_empty() => {
                let now = self.agent.effort().unwrap_or("its default").to_string();
                say(
                    self,
                    format!("Effort is {now}. Type /effort and a level to change it."),
                );
            }
            "effort" => {
                let arg = arg.to_lowercase();
                if arg == "default" {
                    self.agent.set_effort(None);
                    say(self, "Effort is back to the default.".into());
                } else if self.agent.efforts(ctx).contains(&arg) {
                    self.agent.set_effort(Some(arg.clone()));
                    say(self, format!("Effort {arg} from the next message."));
                } else {
                    let all = self.agent.efforts(ctx).join(", ");
                    say(self, format!("Effort can be {all} or default."));
                }
            }
            "agent" => {
                let want = AgentKind::ALL.into_iter().find(|k| {
                    k.key().eq_ignore_ascii_case(arg) || k.name().eq_ignore_ascii_case(arg)
                });
                match want {
                    Some(k) if k == self.agent.kind => {
                        say(self, format!("Already talking to {}.", k.name()));
                    }
                    Some(k) => self.agent.set_kind(k),
                    None => say(self, "Type /agent codex, /agent claude or /agent antigravity.".into()),
                }
            }
            "history" if arg.is_empty() => {
                let n = self.doc_chats().len();
                say(
                    self,
                    if n == 0 {
                        "No earlier chats about this design yet.".into()
                    } else {
                        format!(
                            "{n} earlier chat{} about this design: type /history and pick one, or use the clock above.",
                            if n == 1 { "" } else { "s" }
                        )
                    },
                );
            }
            "history" => {
                let chats = self.doc_chats();
                let pick = chats
                    .iter()
                    .find(|m| m.id == arg || m.title.eq_ignore_ascii_case(arg))
                    .map(|m| m.id.clone());
                match pick {
                    Some(id) => self.open_chat(&id),
                    None => say(self, "There's no such chat about this design.".into()),
                }
            }
            "new" => self.agent.new_chat(),
            "stop" => {
                let elsewhere = self.agent_tab().filter(|&i| i != self.tabs.active);
                if self.agent.running() {
                    self.agent.stop();
                } else if let Some(i) = elsewhere {
                    let name = self.tab_infos()[i].name.clone();
                    let mut chat = std::mem::take(&mut self.tabs.list[i].chat);
                    self.agent.stop_parked(&mut chat);
                    self.tabs.list[i].chat = chat;
                    say(
                        self,
                        format!("Stopped {} working on {name}.", self.agent.kind.name()),
                    );
                } else {
                    say(self, "The agent isn't working on anything.".into());
                }
            }
            "fix" => {
                if self.agent.running() {
                    say(self, "The agent is busy: /stop it first, or wait.".into());
                } else if self.issues.is_empty() {
                    say(self, "The design has no problems to fix.".into());
                } else {
                    self.fix_issues_with_agent(ctx);
                }
            }
            _ => {
                let list: Vec<String> = COMMANDS
                    .iter()
                    .map(|c| match c.takes {
                        Some(t) => format!("/{} {t}: {}", c.name, c.about),
                        None => format!("/{}: {}", c.name, c.about),
                    })
                    .collect();
                say(self, list.join("\n"));
            }
        }
    }

    /// The menu's keys, before the prompt field sees them: what was picked
    /// (row, and whether to run it), if anything. `rows` is the menu shown.
    pub fn slash_keys(&mut self, ui: &egui::Ui, rows: &[Row]) -> Option<(usize, bool)> {
        let a = &mut self.agent;
        if a.menu_for != a.prompt {
            a.menu_for = a.prompt.clone();
            a.menu_sel = 0;
        }
        if rows.is_empty() {
            return None;
        }
        a.menu_sel = a.menu_sel.min(rows.len() - 1);
        let n = rows.len();
        let mut pick = None;
        ui.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                a.menu_sel = (a.menu_sel + 1) % n;
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                a.menu_sel = (a.menu_sel + n - 1) % n;
            }
            if i.consume_key(Modifiers::NONE, Key::Tab) {
                pick = Some((a.menu_sel, false));
            }
            if i.consume_key(Modifiers::NONE, Key::Enter) {
                pick = Some((a.menu_sel, true));
            }
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                a.menu_closed = Some(a.prompt.clone());
            }
        });
        pick
    }

    /// Take a row of the menu: complete the prompt to it, and run it if
    /// `run` (Enter, a click) and it's a whole command.
    pub fn slash_pick(&mut self, row: &Row, run: bool, ctx: &egui::Context) {
        self.agent.prompt = row.fill.clone();
        self.agent.focus = true;
        if run && row.run {
            let text = row.fill.clone();
            self.run_slash(&text, ctx);
        }
    }
}

const ROW_H: f32 = 26.0;
const MAX_H: f32 = 300.0;

/// Draw the menu growing up out of the prompt's input area (`input`): as
/// wide as it, its bottom on the input's top edge, square there and
/// rounded above. The row clicked, if any.
pub fn menu(ui: &egui::Ui, rows: &[Row], sel: usize, input: Rect) -> Option<usize> {
    let p = pal_ui(ui);
    let mut clicked = None;
    let radius = 8;
    let frame = egui::Frame::new()
        .fill(ui.visuals().window_fill)
        .stroke(Stroke::new(1.0, p.separator))
        .corner_radius(egui::CornerRadius {
            nw: radius,
            ne: radius,
            sw: 0,
            se: 0,
        })
        .inner_margin(egui::Margin::same(space::XS as i8));
    egui::Area::new(ui.make_persistent_id("agent-slash-menu"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(input.left_top())
        .constrain(false)
        .show(ui.ctx(), |ui| {
            frame.show(ui, |ui| {
                let w = input.width() - 2.0 * space::XS - 2.0;
                ui.set_width(w);
                // As tall as its rows (up to a point): set here, as a list
                // that grows (Codex's models arrive after it opens) would
                // otherwise keep the height it first had.
                let h = (rows.len() as f32 * ROW_H).min(MAX_H);
                egui::ScrollArea::vertical()
                    .max_height(h)
                    .min_scrolled_height(h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (k, row) in rows.iter().enumerate() {
                            if row_ui(ui, row, k == sel, w, &p) {
                                clicked = Some(k);
                            }
                        }
                    });
            });
        });
    clicked
}

/// One row: the command (its hint quieter) or choice, what it does, and a
/// check on the choice in use. Whether it was clicked.
fn row_ui(ui: &mut egui::Ui, row: &Row, selected: bool, w: f32, p: &crate::theme::Pal) -> bool {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::click());
    let name = if row.hint.is_empty() {
        row.label.clone()
    } else {
        format!("{} {}", row.label, row.hint)
    };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &name));
    if selected || resp.hovered() {
        ui.painter().rect_filled(r, 4.0, p.hover);
    }
    if selected && ui.input(|i| i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::ArrowUp)) {
        ui.scroll_to_rect(r, None);
    }
    let font = egui::FontId::proportional(crate::review::FIELD_PT);
    let painter = ui.painter();
    let y = r.center().y;
    let mut label = painter.text(
        egui::pos2(r.left() + space::S, y),
        egui::Align2::LEFT_CENTER,
        &row.label,
        font.clone(),
        p.text,
    );
    if !row.hint.is_empty() {
        label = label.union(painter.text(
            egui::pos2(label.right() + 4.0, y),
            egui::Align2::LEFT_CENTER,
            &row.hint,
            font.clone(),
            p.tertiary,
        ));
    }
    if row.current {
        crate::icons::paint(
            painter,
            Rect::from_center_size(egui::pos2(r.right() - space::S - 6.0, y), Vec2::splat(11.0)),
            crate::icons::Icon::Check,
            p.accent,
        );
    }
    if !row.about.is_empty() {
        let x = label.right() + space::M;
        let room = r.right() - space::S - 18.0 - x;
        if room > 40.0 {
            let mut job = egui::text::LayoutJob::simple_singleline(
                row.about.clone(),
                egui::FontId::proportional(crate::review::FIELD_PT - 1.0),
                p.secondary,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(room);
            let g = painter.layout_job(job);
            painter.galley(egui::pos2(x, y - g.size().y / 2.0), g, p.secondary);
        }
    }
    resp.clicked()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_message_that_starts_with_a_path_isnt_a_command() {
        use crate::app::Studio;
        assert!(Studio::is_slash("/model opus"));
        assert!(
            Studio::is_slash("  /modle"),
            "a misspelt one is still meant as one"
        );
        assert!(!Studio::is_slash("/Users/me/art.png trace this at 80 mm"));
        assert!(!Studio::is_slash("/"));
    }

    use super::*;
    use std::path::PathBuf;

    fn studio() -> Studio {
        let mut s = Studio::with_file(None, None);
        s.agent.config = None; // don't save choices
        s.agent.kind = AgentKind::Claude;
        s.agent.found = [
            Some(Some(PathBuf::from("/usr/local/bin/claude"))),
            Some(None),
            Some(None),
        ];
        s
    }

    #[test]
    fn a_slash_lists_the_commands_and_typing_narrows_them() {
        let mut s = studio();
        let ctx = egui::Context::default();
        let all = rows(&mut s, "/", &ctx);
        assert_eq!(all.len(), COMMANDS.len());
        let m = rows(&mut s, "/mo", &ctx);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].fill, "/model ");
        assert!(!m[0].run, "completes, for the models next");
        assert!(rows(&mut s, "/ne", &ctx)[0].run, "/new runs at once");
        assert!(rows(&mut s, "a fox", &ctx).is_empty());
        assert!(rows(&mut s, "/model x\nmore", &ctx).is_empty());
    }

    #[test]
    fn model_lists_the_models_and_picking_one_sets_it() {
        let mut s = studio();
        let ctx = egui::Context::default();
        let m = rows(&mut s, "/model ", &ctx);
        assert_eq!(m[0].label, "Default");
        assert!(m[0].current);
        assert!(m.iter().any(|r| r.fill == "/model opus"));
        let op = rows(&mut s, "/model op", &ctx);
        assert_eq!(op.len(), 1);
        s.slash_pick(&op[0], true, &ctx);
        assert_eq!(s.agent.model(), Some("opus"));
        assert!(s.agent.prompt.is_empty(), "the command is done");
        assert!(matches!(s.agent.log.last(), Some(Entry::Info(t)) if t.contains("opus")));
        // Now marked in the list; and any full name is taken as given.
        assert!(rows(&mut s, "/model opus", &ctx)[0].current);
        s.run_slash("/model claude-opus-5-5", &ctx);
        assert_eq!(s.agent.model(), Some("claude-opus-5-5"));
        s.run_slash("/model default", &ctx);
        assert_eq!(s.agent.model(), None);
    }

    #[test]
    fn effort_takes_only_the_levels_the_agent_has() {
        let mut s = studio();
        let ctx = egui::Context::default();
        s.run_slash("/effort high", &ctx);
        assert_eq!(s.agent.effort(), Some("high"));
        s.run_slash("/effort ultra", &ctx);
        assert_eq!(s.agent.effort(), Some("high"), "Claude has no ultra");
        assert!(matches!(s.agent.log.last(), Some(Entry::Info(t)) if t.contains("max")));
        s.run_slash("/effort default", &ctx);
        assert_eq!(s.agent.effort(), None);
    }

    #[test]
    fn each_agent_keeps_its_own_model() {
        let mut s = studio();
        let ctx = egui::Context::default();
        s.run_slash("/model sonnet", &ctx);
        s.run_slash("/agent codex", &ctx);
        assert_eq!(s.agent.kind, AgentKind::Codex);
        assert_eq!(s.agent.model(), None);
        s.run_slash("/model gpt-6.2-astra", &ctx);
        s.run_slash("/agent claude", &ctx);
        assert_eq!(s.agent.model(), Some("sonnet"));
    }

    #[test]
    fn other_commands_do_what_they_say() {
        let mut s = studio();
        let ctx = egui::Context::default();
        s.agent.log.push(Entry::Said("hi".into()));
        s.run_slash("/new", &ctx);
        assert!(s.agent.log.is_empty());
        s.run_slash("/stop", &ctx);
        assert!(matches!(s.agent.log.last(), Some(Entry::Info(t)) if t.contains("isn't working")));
        s.run_slash("/help", &ctx);
        assert!(matches!(s.agent.log.last(), Some(Entry::Info(t)) if t.contains("/effort")));
        s.run_slash("/bogus", &ctx);
        assert!(matches!(s.agent.log.last(), Some(Entry::Info(t)) if t.contains("no /bogus")));
    }
}
