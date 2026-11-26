//! Help › Contact Support…: a message to the people who make ZigZag, sent
//! through zigzag.diy (`POST /v1/support`), which emails it on. What it's
//! about, the message, and an address to reply to (the account's, when this
//! computer is signed in). Which ZigZag, on what, goes with it.

use crate::app::Studio;
use crate::share::Api;
use eframe::egui;
use serde_json::json;
use std::sync::mpsc;

/// What a message can be about: the server's name for it, and the menu's.
pub const TOPICS: [(&str, &str); 4] = [
    ("bug", "Bug"),
    ("feature", "Feature request"),
    ("business", "Business opportunity"),
    ("other", "Something else"),
];

/// The longest message the server takes.
const MAX_MESSAGE: usize = 5000;

#[derive(Default)]
pub struct SupportUi {
    pub open: bool,
    /// Index into [`TOPICS`].
    pub topic: usize,
    pub message: String,
    pub email: String,
    /// The answer to a message being sent.
    pub sending: Option<mpsc::Receiver<Result<(), String>>>,
    /// Sent: the thanks shows.
    pub sent: bool,
    /// What's sent with it about this computer, read as the window opens.
    pub details: Vec<(&'static str, String)>,
    pub error: Option<String>,
}

/// Which ZigZag this is, on what.
pub fn this_app() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    };
    format!(
        "ZigZag {} on {os} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH
    )
}

/// The system's own version (macOS 26.1, Windows 10.0.26100, Ubuntu
/// 24.04): what the person would read in About This Mac.
pub fn os_version() -> Option<String> {
    let out = |cmd: &str, args: &[&str]| {
        let mut c = std::process::Command::new(cmd);
        // (No console window flashing up on Windows.)
        crate::no_console(&mut c)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    match std::env::consts::OS {
        "macos" => out("sw_vers", &["-productVersion"]).map(|v| format!("macOS {v}")),
        "windows" => out("cmd", &["/C", "ver"]),
        "linux" => std::fs::read_to_string("/etc/os-release").ok().and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        }),
        _ => None,
    }
}

/// The computer's model (Mac15,3) and memory, where the system says.
fn hardware() -> (Option<String>, Option<String>) {
    if !cfg!(target_os = "macos") {
        let mem = std::fs::read_to_string("/proc/meminfo").ok().and_then(|t| {
            let kb: f64 = t.lines().find_map(|l| l.strip_prefix("MemTotal:"))?.trim().trim_end_matches("kB").trim().parse().ok()?;
            Some(format!("{:.0} GB", kb / 1024.0 / 1024.0))
        });
        return (None, mem);
    }
    let sysctl = |k: &str| {
        std::process::Command::new("sysctl")
            .args(["-n", k])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let mem = sysctl("hw.memsize")
        .and_then(|b| b.parse::<f64>().ok())
        .map(|b| format!("{:.0} GB", b / 1024f64.powi(3)));
    (sysctl("hw.model"), mem)
}

/// About this computer and this ZigZag, sent with the message (and shown
/// in the window before it is): nothing that says who someone is (no
/// computer or user name, no files), only what helps make sense of a bug.
pub fn details(studio: &Studio, ctx: Option<&egui::Context>) -> Vec<(&'static str, String)> {
    let mut d = vec![("ZigZag", env!("CARGO_PKG_VERSION").to_string())];
    // (Asked of the system once: it's a program run each time, and the
    // window shouldn't wait on that every time it opens.)
    type System = (Option<String>, (Option<String>, Option<String>));
    static SYSTEM: std::sync::OnceLock<System> = std::sync::OnceLock::new();
    let (os, (model, mem)) = SYSTEM.get_or_init(|| (os_version(), hardware())).clone();
    d.push((
        "System",
        os.unwrap_or_else(|| std::env::consts::OS.to_string()),
    ));
    d.push(("Chip", std::env::consts::ARCH.to_string()));
    if let Some(m) = model {
        d.push(("Model", m));
    }
    if let Some(m) = mem {
        d.push(("Memory", m));
    }
    if let Ok(n) = std::thread::available_parallelism() {
        d.push(("Cores", n.get().to_string()));
    }
    if let Some(c) = ctx {
        d.push(("Screen scale", format!("{:.2}", c.pixels_per_point())));
    }
    d.push((
        "Stitches drawn by",
        if studio.gpu.is_some() { "GPU" } else { "CPU" }.to_string(),
    ));
    if let Some(m) = &studio.doc.settings.machine {
        d.push(("Machine", m.clone()));
    }
    if let Ok(lang) = std::env::var("LANG")
        && !lang.is_empty()
    {
        d.push(("Language", lang.split('.').next().unwrap_or(&lang).to_string()));
    }
    d
}

/// The request's body.
pub fn request(
    topic: usize,
    message: &str,
    email: &str,
    details: &[(&'static str, String)],
) -> serde_json::Value {
    let system: serde_json::Map<String, serde_json::Value> = details
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect();
    let mut b = json!({
        "topic": TOPICS[topic.min(TOPICS.len() - 1)].0,
        "message": message.trim(),
        "app": this_app(),
        "system": system,
    });
    if !email.trim().is_empty() {
        b["email"] = json!(email.trim());
    }
    b
}

/// Send it: signed in as this computer's account if it is (so the reply
/// can go there, and who it is is known), else as no one.
fn send(base: &str, device: Option<&str>, body: &serde_json::Value) -> Result<(), String> {
    let api = match device {
        Some(d) => crate::account::session(base, d).unwrap_or_else(|_| Api::new(base)),
        None => Api::new(base),
    };
    api.json("POST", "/v1/support", body)
        .map(|_| ())
        .map_err(|e| e.say())
}

impl Studio {
    /// Help › Contact Support…: the window, with the account's address to
    /// reply to filled in.
    pub(crate) fn open_support(&mut self) {
        let s = &mut self.support;
        if !s.open {
            s.sent = false;
            s.error = None;
        }
        s.open = true;
        let ctx = self.egui_ctx.clone();
        self.support.details = details(self, ctx.as_ref());
        if self.support.email.is_empty()
            && let Some(a) = self.account.signed_in()
        {
            self.support.email = a.email;
        }
    }

    fn send_support(&mut self) {
        let base = self
            .account
            .base
            .clone()
            .unwrap_or_else(crate::share::server);
        let device = self.account.signed_in().map(|a| a.device);
        let s = &mut self.support;
        let body = request(s.topic, &s.message, &s.email, &s.details);
        let (tx, rx) = mpsc::channel();
        let ctx = self.egui_ctx.clone();
        let _ = std::thread::Builder::new()
            .name("zigzag-support".into())
            .spawn(move || {
                let _ = tx.send(send(&base, device.as_deref(), &body));
                if let Some(c) = &ctx {
                    c.request_repaint();
                }
            });
        s.sending = Some(rx);
        s.error = None;
    }

    pub(crate) fn support_window(&mut self, ctx: &egui::Context) {
        // (The answer is taken even if the window was closed meanwhile.)
        if let Some(rx) = &self.support.sending
            && let Ok(r) = rx.try_recv()
        {
            self.support.sending = None;
            match r {
                Ok(()) => {
                    self.support.sent = true;
                    self.support.message.clear();
                    self.status = "Your message is on its way to ZigZag's support.".into();
                }
                Err(e) => self.support.error = Some(e),
            }
        }
        if !self.support.open {
            return;
        }
        use crate::dialog;
        let mut close = false;
        let mut send = false;
        let s = &mut self.support;
        let busy = s.sending.is_some();
        dialog::show(
            ctx,
            egui::Id::unique("contact-support"),
            dialog::Place::Center,
            |ui| {
                let p = crate::theme::pal_ui(ui);
                if s.sent {
                    dialog::heading(ui, "Thanks for writing");
                    dialog::body(
                        ui,
                        if s.email.trim().is_empty() {
                            "Your message is on its way."
                        } else {
                            "Your message is on its way. We'll reply by email."
                        },
                    );
                    dialog::buttons(ui, |ui| {
                        close = dialog::primary(ui, "OK", true).clicked()
                            || ui.input(|i| {
                                i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape)
                            });
                    });
                    return;
                }
                dialog::heading(ui, "Contact Support");
                dialog::body(
                    ui,
                    "A message to the people who make ZigZag. Which version you have, and on what, goes with it.",
                );
                ui.add_space(crate::theme::space::M);
                let w = ui.available_width().max(dialog::MIN_W);
                let label = |ui: &mut egui::Ui, t: &str| {
                    ui.label(egui::RichText::new(t).size(12.0).color(p.secondary))
                };
                let about = label(ui, "About");
                egui::ComboBox::from_id_salt("support-topic")
                    .selected_text(TOPICS[s.topic].1)
                    .width(w)
                    .show_ui(ui, |ui| {
                        for (k, (_, name)) in TOPICS.iter().enumerate() {
                            ui.selectable_value(&mut s.topic, k, *name);
                        }
                    })
                    .response
                    .labelled_by(about.id);
                ui.add_space(crate::theme::space::S);
                let msg = label(ui, "Message");
                let r = ui
                    .add_enabled(
                        !busy,
                        egui::TextEdit::multiline(&mut s.message)
                            .hint_text("What happened, or what's on your mind")
                            .char_limit(MAX_MESSAGE)
                            .desired_rows(7)
                            .desired_width(w),
                    )
                    .labelled_by(msg.id);
                if !busy && s.message.is_empty() && !r.has_focus() && ui.memory(|m| m.focused().is_none()) {
                    r.request_focus();
                }
                ui.add_space(crate::theme::space::S);
                let mail = label(ui, "Your email, to reply to");
                ui.add_enabled(
                    !busy,
                    egui::TextEdit::singleline(&mut s.email)
                        .hint_text("Optional")
                        .desired_width(w),
                )
                .labelled_by(mail.id);
                // What goes with it, to see before it does.
                ui.add_space(crate::theme::space::S);
                egui::CollapsingHeader::new(
                    egui::RichText::new("Sent with it: about this computer").size(12.0).color(p.secondary),
                )
                .id_salt("support-details")
                .show(ui, |ui| {
                    for (k, v) in &s.details {
                        ui.label(egui::RichText::new(format!("{k}: {v}")).size(12.0).color(p.secondary));
                    }
                });
                if let Some(e) = &s.error {
                    ui.add_space(crate::theme::space::S);
                    ui.label(egui::RichText::new(e).size(12.0).color(p.error));
                }
                let ready = !busy && !s.message.trim().is_empty();
                dialog::buttons(ui, |ui| {
                    send = dialog::primary(ui, if busy { "Sending…" } else { "Send" }, ready)
                        .clicked()
                        && ready;
                    close = dialog::secondary(ui, "Cancel").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape));
                });
            },
        );
        if send {
            self.send_support();
        } else if close {
            // (A message being written is kept for next time; sending,
            // its answer still comes.)
            self.support.open = false;
            if self.support.sent {
                self.support.sent = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_sent_says_the_topic_the_message_and_which_zigzag() {
        let b = request(2, "  Let's talk.\n ", "", &[("ZigZag", "0.1.11".into()), ("Cores", "8".into())]);
        assert_eq!(b["system"], json!({ "ZigZag": "0.1.11", "Cores": "8" }));
        assert_eq!(b["topic"], "business");
        assert_eq!(b["message"], "Let's talk.");
        assert!(b.get("email").is_none(), "no address, none sent");
        assert!(b["app"].as_str().unwrap().starts_with(&format!("ZigZag {} on ", env!("CARGO_PKG_VERSION"))));
        let b = request(0, "It crashed", " me@example.com ", &[]);
        assert_eq!((b["topic"].as_str(), b["email"].as_str()), (Some("bug"), Some("me@example.com")));
        // The topics are among the server's (src/support.ts in zigzag-server;
        // it also takes "account", which isn't offered: there's no billing).
        let keys: Vec<&str> = TOPICS.iter().map(|t| t.0).collect();
        assert_eq!(keys, ["bug", "feature", "business", "other"]);
    }

    #[test]
    fn help_contact_support_sends_what_was_written_and_thanks() {
        use egui_kittest::kittest::Queryable;
        // A stand-in for zigzag.diy: answers one POST and keeps its body.
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        let got = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut c, _) = server.accept().unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = c.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf);
                if let Some(h) = text.find("\r\n\r\n") {
                    let len = text[..h]
                        .lines()
                        .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                        .unwrap_or(0);
                    if buf.len() >= h + 4 + len {
                        let reply = "{\"sent\":true}";
                        let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
                        return String::from_utf8_lossy(&buf).into_owned();
                    }
                }
                if n == 0 {
                    return String::from_utf8_lossy(&buf).into_owned();
                }
            }
        });
        let mut s = Studio::with_file(None, None);
        s.account.base = Some(format!("http://127.0.0.1:{port}"));
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::Vec2::new(1200.0, 800.0))
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), s);
        h.run_steps(2);
        h.state_mut().run_action(crate::commands::Action::ContactSupport);
        h.run_steps(3);
        // Nothing to send yet.
        h.get_by_label("Send").click();
        h.run_steps(2);
        assert!(h.state().support.sending.is_none(), "an empty message isn't sent");
        h.get_by_label("Message").type_text("The S fans out.");
        h.run_steps(2);
        h.get_by_label("Your email, to reply to").click();
        h.run_steps(2);
        h.get_by_label("Your email, to reply to").type_text("me@example.com");
        h.run_steps(2);
        h.get_by_label("Send").click();
        h.run_steps(2);
        let req = got.join().unwrap();
        for _ in 0..200 {
            h.run_steps(1);
            if h.state().support.sent {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(req.starts_with("POST /v1/support "), "{req}");
        let body: serde_json::Value = serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap();
        assert_eq!(body["topic"], "bug");
        assert_eq!(body["message"], "The S fans out.");
        assert_eq!(body["email"], "me@example.com");
        // About this computer, nothing about who's at it.
        let sys = body["system"].as_object().unwrap();
        assert_eq!(sys["ZigZag"], env!("CARGO_PKG_VERSION"));
        assert!(sys.contains_key("System") && sys.contains_key("Chip") && sys.contains_key("Cores"));
        let all = body.to_string();
        if let Ok(user) = std::env::var("USER") {
            assert!(user.len() < 3 || !all.contains(&user), "no user name: {all}");
        }
        assert!(h.state().support.sent);
        h.get_by_label("Thanks for writing");
        // (Once the window has settled at its new, smaller size.)
        h.run_steps(4);
        h.get_by_label("OK").click();
        h.run_steps(4);
        assert!(!h.state().support.open);
    }
}
