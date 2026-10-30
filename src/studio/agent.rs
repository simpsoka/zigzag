//! Prompt an AI agent from inside the app (like Paper's in-app agent): the
//! Claude Code or Codex CLI runs in the background, connected to this
//! studio's MCP server, so its tool calls edit the open design live. It
//! lives in the Agent panel (docked, floating or tabbed like any panel); the
//! transcript shows what it said and each step, with a spinner on the one
//! running, and the canvas marks the objects it's working on.

use crate::app::Studio;
use crate::icons::{self, Icon};
use crate::theme::{caption, pal_ui, space};
use eframe::egui::{self, Color32, Key, Rect, Sense, Stroke, Vec2};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AgentKind {
    Claude,
    #[default]
    Codex,
}

impl AgentKind {
    /// Both, in menu order.
    pub const ALL: [AgentKind; 2] = [AgentKind::Codex, AgentKind::Claude];

    fn index(self) -> usize {
        self as usize
    }

    /// How to install its command-line tool on a Mac: (how, command) pairs,
    /// the first recommended, then how to sign in.
    pub fn install_steps(self) -> (&'static [(&'static str, &'static str)], &'static str) {
        match self {
            AgentKind::Codex => (
                &[
                    ("With Homebrew", "brew install --cask codex"),
                    ("Or with npm", "npm install -g @openai/codex"),
                ],
                "Then run `codex` once in Terminal and choose Sign in with ChatGPT.",
            ),
            AgentKind::Claude => (
                &[
                    ("Recommended", "curl -fsSL https://claude.ai/install.sh | bash"),
                    ("Or with Homebrew", "brew install --cask claude-code"),
                ],
                "Then run `claude` once in Terminal to sign in.",
            ),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "Codex",
        }
    }
    fn program(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
        }
    }
    fn key(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
        }
    }
}

/// One line of the transcript.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    You(String),
    Said(String),
    /// A tool call: (tool, short summary of its arguments, Some(ok) once done).
    Tool(String, String, Option<bool>),
    Error(String),
    Done(String),
}

/// Something the agent process reported.
enum Event {
    Line(String),
    Stderr(String),
    Exit(Option<i32>),
    StdoutClosed,
}

struct Run {
    child: Arc<Mutex<Child>>,
    rx: Receiver<Event>,
    stderr: Vec<String>,
    exit: Option<Option<i32>>,
    stdout_closed: bool,
}

#[derive(Default)]
pub struct AgentState {
    pub kind: AgentKind,
    pub prompt: String,
    pub log: Vec<Entry>,
    /// Claude's session id or Codex's thread id, to continue the chat.
    pub session: Option<String>,
    /// Focus the prompt field next frame.
    pub focus: bool,
    run: Option<Run>,
    /// When the running prompt was sent.
    pub started: Option<Instant>,
    /// What agents are doing on the canvas.
    pub work: Work,
    /// The panel's height apart from the transcript (last frame).
    pub chrome_h: f32,
    /// Tool calls by id → their line in the log (to mark results).
    calls: std::collections::HashMap<String, usize>,
    /// The user's login-shell PATH (GUI apps on macOS don't get it).
    path: Option<Option<String>>,
    config: Option<PathBuf>,
    /// Where each agent's program is (by [`AgentKind::index`]): `None`
    /// until looked up, then `Some(None)` if it isn't installed.
    pub found: [Option<Option<PathBuf>>; 2],
    /// A lookup running off the UI thread: (login PATH, what was found).
    lookup: Option<Receiver<(Option<String>, [Option<PathBuf>; 2])>>,
}

/// Extra instructions for the agent, on top of the MCP server's own.
const BRIEF: &str = "You are the design agent inside ZigZag, a machine-embroidery editor. \
The person is watching the canvas while you work. Make the design they ask for on the open document \
using the ZigZag MCP tools (get_design first; add_svg for artwork you write as SVG, add_text for lettering, \
transform_objects to size and place, set_object for stitch types and thread colors). Keep everything inside the hoop. \
Embroidery wants bold, simple shapes: no gradients, no hairlines, few colors, nothing smaller than about 1.5 mm. \
After making changes, call render_preview to look at the stitches and fix problems, then check_design. \
Don't save or export unless asked. Finish with one or two sentences on what you made.";

/// PATH from the login shell, so `claude` and `codex` (and node) are found
/// even when the app is opened from Finder.
fn login_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let out = Command::new(&shell)
        .args(["-ilc", "printf '__TL__%s__TL__' \"$PATH\""])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    let a = s.find("__TL__")? + 6;
    let b = s[a..].find("__TL__")? + a;
    Some(s[a..b].to_string())
}

/// Where `kind`'s program is: $ZIGZAG_<NAME> (for tests), the login PATH,
/// this process's PATH, then the usual install places.
fn locate(kind: AgentKind, login: Option<&str>) -> Option<PathBuf> {
    let env = format!("ZIGZAG_{}", kind.key().to_uppercase());
    if let Some(p) = std::env::var_os(env) {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for p in [login.map(str::to_string), std::env::var("PATH").ok()]
        .into_iter()
        .flatten()
    {
        dirs.extend(std::env::split_paths(&p));
    }
    for d in [
        ".local/bin",
        ".claude/local",
        ".npm-global/bin",
        ".bun/bin",
        ".cargo/bin",
    ] {
        dirs.push(PathBuf::from(&home).join(d));
    }
    dirs.push("/opt/homebrew/bin".into());
    dirs.push("/usr/local/bin".into());
    dirs.into_iter()
        .map(|d| d.join(kind.program()))
        .find(|p| p.is_file())
}

impl AgentState {
    /// The agent last used, remembered between launches.
    pub fn load() -> AgentState {
        let config = crate::dock::config_file("agent.json");
        let kind = config
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|v| match v["agent"].as_str() {
                Some("claude") => AgentKind::Claude,
                _ => AgentKind::Codex,
            })
            .unwrap_or_default();
        AgentState {
            kind,
            config,
            ..Default::default()
        }
    }

    fn save(&self) {
        let Some(f) = &self.config else { return };
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(
            f,
            serde_json::json!({ "agent": self.kind.key() }).to_string(),
        );
    }

    pub fn running(&self) -> bool {
        self.run.is_some()
    }

    /// What it's doing now: the step of a tool call still running, else thinking.
    pub fn status(&self) -> &'static str {
        match self.log.last() {
            Some(Entry::Tool(name, _, None)) => activity(name),
            _ => "Thinking",
        }
    }

    pub fn stop(&mut self) {
        if let Some(r) = self.run.take() {
            if let Ok(mut c) = r.child.lock() {
                let _ = c.kill();
            }
            self.log.push(Entry::Error("Stopped.".into()));
        }
    }

    /// Switch between Claude Code and Codex (a new conversation; remembered).
    pub fn set_kind(&mut self, kind: AgentKind) {
        if kind != self.kind {
            self.new_chat();
            self.kind = kind;
            self.save();
        }
    }

    pub fn new_chat(&mut self) {
        self.stop();
        self.log.clear();
        self.calls.clear();
        self.session = None;
    }

    /// PATH from the login shell (looked up once).
    fn shell_path(&mut self) -> Option<String> {
        if let Some(p) = &self.path {
            return p.clone();
        }
        let path = login_path();
        self.path = Some(path.clone());
        path
    }

    /// Where the agent's program is (see [`locate`]); remembered for the
    /// panel's install steps.
    fn find(&mut self, kind: AgentKind) -> Option<PathBuf> {
        let login = self.shell_path();
        let found = locate(kind, login.as_deref());
        self.found[kind.index()] = Some(found.clone());
        found
    }

    /// Whether `kind`'s program is installed: `None` while that's still
    /// being found out, off the UI thread (the login shell can take a
    /// moment), with a repaint when it's known.
    pub fn installed(&mut self, kind: AgentKind, ctx: &egui::Context) -> Option<bool> {
        if let Some(rx) = &self.lookup
            && let Ok((login, found)) = rx.try_recv()
        {
            self.path = Some(login);
            for (slot, f) in self.found.iter_mut().zip(found) {
                *slot = Some(f);
            }
            self.lookup = None;
        }
        if let Some(f) = &self.found[kind.index()] {
            return Some(f.is_some());
        }
        if self.lookup.is_none() {
            let (tx, rx) = mpsc::channel();
            let known = self.path.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let login = known.unwrap_or_else(login_path);
                let found = [AgentKind::Claude, AgentKind::Codex]
                    .map(|k| locate(k, login.as_deref()));
                let _ = tx.send((login, found));
                ctx.request_repaint();
            });
            self.lookup = Some(rx);
        }
        None
    }

    /// Look again (after installing one), reading the login PATH afresh.
    pub fn check_again(&mut self) {
        self.path = None;
        self.found = [None, None];
        self.lookup = None;
    }

    /// The command line for one prompt.
    fn command(
        &mut self,
        program: PathBuf,
        mcp_url: &str,
        bridge: Option<PathBuf>,
        port: u16,
    ) -> Command {
        let mut cmd = Command::new(program);
        match self.kind {
            AgentKind::Claude => {
                let config = serde_json::json!({
                    "mcpServers": { "zigzag": { "type": "http", "url": mcp_url } }
                });
                cmd.args(["-p", "--output-format", "stream-json", "--verbose"])
                    .args(["--mcp-config", &config.to_string(), "--strict-mcp-config"])
                    // Only the design tools: no shell, no file edits.
                    .args(["--tools", "", "--allowedTools", "mcp__zigzag"])
                    .args(["--append-system-prompt", BRIEF]);
                if let Some(s) = &self.session {
                    cmd.args(["--resume", s]);
                }
            }
            AgentKind::Codex => {
                cmd.arg("exec");
                if let Some(s) = &self.session {
                    cmd.args(["resume", s]);
                }
                cmd.args(["--json", "--skip-git-repo-check"]);
                // Codex starts MCP servers as commands: the stdio bridge.
                match bridge {
                    Some(b) => {
                        cmd.arg("-c").arg(format!(
                            "mcp_servers.zigzag.command={}",
                            Value::String(b.to_string_lossy().into())
                        ));
                        cmd.arg("-c")
                            .arg(format!("mcp_servers.zigzag.args=[\"--port\", \"{port}\"]"));
                    }
                    None => {
                        cmd.arg("-c").arg(format!(
                            "mcp_servers.zigzag.url={}",
                            Value::String(mcp_url.into())
                        ));
                    }
                }
                // `codex exec` can't ask anyone, so a tool that needs
                // approval (anything not read-only: every edit) is cancelled.
                // These are the design tools only, and every edit undoes:
                // approve them, as Claude Code's --allowedTools does.
                cmd.arg("-c")
                    .arg("mcp_servers.zigzag.default_tools_approval_mode=\"approve\"");
                cmd.arg("-");
            }
        }
        if let Some(p) = self.shell_path() {
            cmd.env("PATH", p);
        }
        // A quiet folder of its own, away from your files.
        if let Some(dir) = self
            .config
            .as_ref()
            .and_then(|c| c.parent())
            .map(|d| d.join("agent"))
        {
            let _ = std::fs::create_dir_all(&dir);
            cmd.current_dir(dir);
        } else {
            cmd.current_dir(std::env::temp_dir());
        }
        cmd
    }

    /// Send the prompt. `mcp`: the studio's MCP endpoint (if it's running).
    pub fn send(&mut self, mcp: Option<std::net::SocketAddr>, ctx: &egui::Context) {
        let text = self.prompt.trim().to_string();
        if text.is_empty() || self.running() {
            return;
        }
        self.log.push(Entry::You(text.clone()));
        let Some(addr) = mcp else {
            self.log.push(Entry::Error(
                "The agent works through the studio's MCP server, which is off (started with --no-mcp).".into(),
            ));
            return;
        };
        let Some(program) = self.find(self.kind) else {
            let (ways, sign_in) = self.kind.install_steps();
            self.log.push(Entry::Error(format!(
                "{} isn't installed (no `{}` command found). Install it in Terminal with `{}`{}. {sign_in} Then try again.",
                self.kind.name(),
                self.kind.program(),
                ways[0].1,
                ways.get(1)
                    .map(|w| format!(" (or `{}`)", w.1))
                    .unwrap_or_default(),
            )));
            return;
        };
        let bridge = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join("zigzag-mcp")))
            .filter(|p| p.is_file());
        let url = format!("http://{addr}/mcp");
        let mut cmd = self.command(program, &url, bridge, addr.port());
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.log.push(Entry::Error(format!(
                    "Couldn't start {}: {e}",
                    self.kind.name()
                )));
                return;
            }
        };
        // The prompt goes in on stdin (no shell quoting to worry about).
        let first = self.session.is_none();
        let prompt = if self.kind == AgentKind::Codex && first {
            format!("{BRIEF}\n\n{text}")
        } else {
            text
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(prompt.as_bytes());
        }
        let (tx, rx) = mpsc::channel();
        let stdout_closed = child.stdout.is_none();
        if let Some(out) = child.stdout.take() {
            let tx = tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let _ = tx.send(Event::Line(line));
                    ctx.request_repaint();
                }
                // This sender's EOF follows every stdout line. Process exit
                // is reported by another thread and may arrive before them.
                let _ = tx.send(Event::StdoutClosed);
                ctx.request_repaint();
            });
        }
        let child = Arc::new(Mutex::new(child));
        if let Some(err) = child.lock().ok().and_then(|mut c| c.stderr.take()) {
            let tx = tx.clone();
            let child = child.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let _ = tx.send(Event::Stderr(line));
                }
                // stderr closes when the process ends: report how it ended.
                let code = loop {
                    if let Ok(mut c) = child.lock()
                        && let Ok(Some(s)) = c.try_wait()
                    {
                        break s.code();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                };
                let _ = tx.send(Event::Exit(code));
                ctx.request_repaint();
            });
        }
        self.prompt.clear();
        self.started = Some(Instant::now());
        self.run = Some(Run {
            child,
            rx,
            stderr: Vec::new(),
            exit: None,
            stdout_closed,
        });
    }

    /// Take in what the agent printed since last frame.
    pub fn poll(&mut self) {
        let Some(run) = &mut self.run else { return };
        let mut lines = Vec::new();
        while let Ok(ev) = run.rx.try_recv() {
            match ev {
                Event::Line(l) => lines.push(l),
                Event::Stderr(l) => {
                    run.stderr.push(l);
                    if run.stderr.len() > 20 {
                        run.stderr.remove(0);
                    }
                }
                Event::Exit(code) => run.exit = Some(code),
                Event::StdoutClosed => run.stdout_closed = true,
            }
        }
        let ended = if run.stdout_closed { run.exit } else { None };
        for l in lines {
            let Ok(v) = serde_json::from_str::<Value>(&l) else {
                continue;
            };
            let entries = match self.kind {
                AgentKind::Claude => claude_event(&v, &mut self.session),
                AgentKind::Codex => codex_event(&v, &mut self.session),
            };
            for (id, e) in entries {
                self.push(id, e);
            }
        }
        if let Some(code) = ended {
            let stderr = self.run.take().map(|r| r.stderr).unwrap_or_default();
            let finished = matches!(self.log.last(), Some(Entry::Done(_) | Entry::Error(_)));
            if !finished {
                let tail = stderr
                    .iter()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .cloned()
                    .unwrap_or_default();
                self.log.push(if code == Some(0) {
                    Entry::Done(String::new())
                } else {
                    Entry::Error(if tail.is_empty() {
                        format!("{} stopped (exit code {code:?}).", self.kind.name())
                    } else {
                        tail
                    })
                });
            }
        }
    }

    /// Add an entry; a tool result (same id as its call) updates the call.
    fn push(&mut self, id: Option<String>, e: Entry) {
        if let (Entry::Tool(_, _, Some(ok)), Some(id)) = (&e, &id)
            && let Some(&k) = self.calls.get(id)
            && let Some(Entry::Tool(_, _, state)) = self.log.get_mut(k)
        {
            *state = Some(*ok);
            return;
        }
        if let (Entry::Tool(..), Some(id)) = (&e, id) {
            self.calls.insert(id, self.log.len());
        }
        // The same error again since the last prompt (every edit refused
        // for one reason): once is enough.
        if let Entry::Error(m) = &e
            && self
                .log
                .iter()
                .rev()
                .take_while(|x| !matches!(x, Entry::You(_)))
                .any(|x| matches!(x, Entry::Error(n) if n == m))
        {
            return;
        }
        self.log.push(e);
    }
}

fn short(v: &Value) -> String {
    let s = match v {
        Value::Object(m) => m
            .iter()
            .filter(|(k, _)| k.as_str() != "svg")
            .map(|(k, v)| match v {
                Value::String(s) => format!("{k}: {s}"),
                other => format!("{k}: {other}"),
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    };
    let s: String = s.chars().take(80).collect();
    s
}

fn tool_name(n: &str) -> String {
    n.trim_start_matches("mcp__zigzag__").to_string()
}

/// Claude Code's stream-json events.
fn claude_event(v: &Value, session: &mut Option<String>) -> Vec<(Option<String>, Entry)> {
    let mut out = Vec::new();
    if let Some(s) = v["session_id"].as_str() {
        *session = Some(s.to_string());
    }
    match v["type"].as_str() {
        Some("assistant") => {
            for c in v["message"]["content"].as_array().into_iter().flatten() {
                match c["type"].as_str() {
                    Some("text") => {
                        let t = c["text"].as_str().unwrap_or("").trim();
                        if !t.is_empty() {
                            out.push((None, Entry::Said(t.to_string())));
                        }
                    }
                    Some("tool_use") => out.push((
                        c["id"].as_str().map(str::to_string),
                        Entry::Tool(
                            tool_name(c["name"].as_str().unwrap_or("tool")),
                            short(&c["input"]),
                            None,
                        ),
                    )),
                    _ => {}
                }
            }
        }
        Some("user") => {
            for c in v["message"]["content"].as_array().into_iter().flatten() {
                if c["type"] == "tool_result" {
                    let ok = !c["is_error"].as_bool().unwrap_or(false);
                    out.push((
                        c["tool_use_id"].as_str().map(str::to_string),
                        Entry::Tool(String::new(), String::new(), Some(ok)),
                    ));
                }
            }
        }
        Some("result") => {
            if v["is_error"].as_bool().unwrap_or(false) || v["subtype"] != "success" {
                let msg = v["result"].as_str().map(str::to_string).unwrap_or_else(|| {
                    format!("Stopped: {}", v["subtype"].as_str().unwrap_or("error"))
                });
                out.push((None, Entry::Error(msg)));
            } else {
                let turns = v["num_turns"].as_u64().unwrap_or(0);
                let secs = v["duration_ms"].as_f64().unwrap_or(0.0) / 1000.0;
                out.push((None, Entry::Done(format!("{turns} turns · {secs:.0} s"))));
            }
        }
        _ => {}
    }
    out
}

/// What to say when one of Codex's tool calls failed with `message`.
fn codex_tool_error(tool: &str, message: &str) -> String {
    let m = message.to_ascii_lowercase();
    if m.contains("cancel") || m.contains("requires approval") {
        // Approval needed, and nobody to ask: Codex's exec refuses the call
        // ("user cancelled…" or "…requires approval, but approval policy is
        // never", by version).
        return "Codex cancelled the edit: it wanted approval for ZigZag's tools, which it can't ask for when run from here. Updating Codex (`npm i -g @openai/codex`) should fix it.".into();
    }
    if message.is_empty() {
        format!("{tool} failed.")
    } else {
        format!("{tool}: {message}")
    }
}

/// Codex's `exec --json` events.
fn codex_event(v: &Value, session: &mut Option<String>) -> Vec<(Option<String>, Entry)> {
    let mut out = Vec::new();
    match v["type"].as_str() {
        Some("thread.started") => {
            if let Some(t) = v["thread_id"].as_str() {
                *session = Some(t.to_string());
            }
        }
        Some("item.started") => {
            let it = &v["item"];
            let id = it["id"].as_str().map(str::to_string);
            match it["type"].as_str() {
                Some("mcp_tool_call") => out.push((
                    id,
                    Entry::Tool(
                        tool_name(it["tool"].as_str().unwrap_or("tool")),
                        short(&it["arguments"]),
                        None,
                    ),
                )),
                Some("command_execution") => out.push((
                    id,
                    Entry::Tool(
                        "shell".into(),
                        it["command"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(80)
                            .collect(),
                        None,
                    ),
                )),
                _ => {}
            }
        }
        Some("item.completed") => {
            let it = &v["item"];
            let id = it["id"].as_str().map(str::to_string);
            match it["type"].as_str() {
                Some("agent_message") => {
                    let t = it["text"].as_str().unwrap_or("").trim();
                    if !t.is_empty() {
                        out.push((None, Entry::Said(t.to_string())));
                    }
                }
                Some("mcp_tool_call") => {
                    let ok = it["status"] != "failed" && it["error"].is_null();
                    let tool = tool_name(it["tool"].as_str().unwrap_or("tool"));
                    out.push((id, Entry::Tool(tool.clone(), String::new(), Some(ok))));
                    if !ok {
                        // Why, under the call (once per run for the same reason).
                        let m = it["error"]["message"].as_str().unwrap_or("").trim();
                        out.push((None, Entry::Error(codex_tool_error(&tool, m))));
                    }
                }
                Some("command_execution") => out.push((
                    id,
                    Entry::Tool(
                        "shell".into(),
                        String::new(),
                        Some(it["status"] == "completed"),
                    ),
                )),
                _ => {}
            }
        }
        Some("turn.completed") => out.push((None, Entry::Done(String::new()))),
        Some("turn.failed") => out.push((
            None,
            Entry::Error(
                v["error"]["message"]
                    .as_str()
                    .unwrap_or("The turn failed.")
                    .to_string(),
            ),
        )),
        Some("error") => {
            let m = v["message"].as_str().unwrap_or("");
            if !m.starts_with("Reconnecting") {
                out.push((None, Entry::Error(m.to_string())));
            }
        }
        _ => {}
    }
    out
}

/// What an agent is doing on the canvas: the objects its last calls
/// touched, and what the last call was doing. Fed by every MCP call (the
/// in-app agent's, or one connected from outside), drawn over the design.
#[derive(Default)]
pub struct Work {
    touches: Vec<Touch>,
    /// What the last call was doing ("Placing").
    pub label: &'static str,
    /// When the last call came in.
    last: Option<Instant>,
}

impl Work {
    /// Forget what it was working on (another document came forward).
    pub fn clear(&mut self) {
        self.touches.clear();
    }
}

struct Touch {
    uid: usize,
    at: Instant,
    /// When it stopped being worked on (a later call moved on, or the agent finished).
    gone: Option<Instant>,
}

/// How long a touched object takes to fade out.
const FADE: Duration = Duration::from_millis(450);
/// How long an outside agent's work shows after its last call.
const LINGER: Duration = Duration::from_millis(1500);

impl Work {
    /// A call came in that touched `uids` (none: it only looked).
    fn note(&mut self, tool: &str, uids: &[usize]) {
        let now = Instant::now();
        self.label = activity(tool);
        self.last = Some(now);
        if uids.is_empty() {
            return;
        }
        // The earlier pieces are done; these are being worked on.
        for t in &mut self.touches {
            if !uids.contains(&t.uid) && t.gone.is_none() {
                t.gone = Some(now);
            }
        }
        for &u in uids {
            match self.touches.iter_mut().find(|t| t.uid == u) {
                Some(t) if t.gone.is_none() => {}
                Some(t) => {
                    t.at = now;
                    t.gone = None;
                }
                None => self.touches.push(Touch {
                    uid: u,
                    at: now,
                    gone: None,
                }),
            }
        }
    }

    /// The objects being worked on right now (not fading out).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn active(&self) -> Vec<usize> {
        self.touches
            .iter()
            .filter(|t| t.gone.is_none())
            .map(|t| t.uid)
            .collect()
    }
}

/// What a tool call is doing, in a word or two.
pub fn activity(tool: &str) -> &'static str {
    match tool {
        "add_svg" => "Drawing",
        "add_text" | "edit_text" | "list_fonts" => "Lettering",
        "set_object" | "group_colors" => "Restyling",
        "transform_objects" | "move_object" => "Placing",
        "duplicate_objects" => "Duplicating",
        "delete_object" => "Removing",
        "round_corners" => "Rounding corners",
        "trace_image" => "Tracing",
        "render_preview" => "Looking",
        "check_design" => "Checking",
        "get_design" | "get_svg" | "select" => "Reading",
        "set_document" => "Setting up",
        "batch" => "Making changes",
        "undo" => "Undoing",
        "save" | "export" => "Saving",
        _ => "Working",
    }
}

/// A spinner: a turning arc around `at`.
pub fn spinner(p: &egui::Painter, at: egui::Pos2, r: f32, color: Color32, time: f64) {
    let a0 = (time * std::f64::consts::TAU * 0.9) as f32;
    let n = 20;
    let pts: Vec<egui::Pos2> = (0..=n)
        .map(|k| {
            let a = a0 + 4.4 * k as f32 / n as f32;
            at + Vec2::new(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(egui::Shape::line(
        pts,
        Stroke::new((r * 0.32).max(1.4), color),
    ));
    // 30 frames a second is plenty for a spinner.
    p.ctx().request_repaint_after(Duration::from_millis(33));
}

/// A cheap fingerprint of a shape, to tell which ones a call changed.
fn fingerprint(s: &zigzag::svg::Shape) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.color.hash(&mut h);
    format!("{:?}", s.params).hash(&mut h);
    let mut eat = |pts: &[zigzag::geom::P]| {
        pts.len().hash(&mut h);
        for p in pts {
            p.x.to_bits().hash(&mut h);
            p.y.to_bits().hash(&mut h);
        }
    };
    match &s.geometry {
        zigzag::svg::Geometry::Fill { rings, .. } => rings.iter().for_each(|r| eat(r)),
        zigzag::svg::Geometry::Stroke { lines, .. } => lines.iter().for_each(|(l, _)| eat(l)),
    }
    h.finish()
}

impl Studio {
    /// Take in the agent's output; every frame, whether its panel shows or not.
    pub fn agent_tick(&mut self, ctx: &egui::Context) {
        self.agent.poll();
        if self.agent.running() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Run an MCP call and note which objects it touched, for the canvas.
    pub fn run_mcp_call(&mut self, name: &str, args: &Value) -> crate::mcp::ToolResult {
        let uid = |s: &Studio, i: usize| s.doc.shapes.get(i).map(|x| x.uid);
        // The objects it names (a look at one counts as working on it).
        let mut named: Vec<usize> = Vec::new();
        if args.get("object").is_some()
            && let Ok(i) = self.resolve(args.get("object"))
        {
            named.extend(uid(self, i));
        }
        if args.get("objects").is_some()
            && let Ok(v) = self.resolve_many(args.get("objects"))
        {
            named.extend(v.into_iter().filter_map(|i| uid(self, i)));
        }
        // Bring the stitches up to date first, so only the call's own
        // changes count.
        if self.dirty_needs_plan() {
            self.replan();
        }
        let before: std::collections::HashMap<usize, u64> = self
            .doc
            .shapes
            .iter()
            .map(|s| (s.uid, fingerprint(s)))
            .collect();
        let result = self.run_tool(name, args);
        // What it added, or else what it changed. (A new document isn't a
        // piece of the design being worked on.)
        let added: Vec<usize> = self
            .doc
            .shapes
            .iter()
            .filter(|s| !before.contains_key(&s.uid))
            .map(|s| s.uid)
            .collect();
        let mut touched: Vec<usize> = if name == "open_file" {
            Vec::new()
        } else if !added.is_empty() {
            added
        } else {
            self.doc
                .shapes
                .iter()
                .filter(|s| before.get(&s.uid) != Some(&fingerprint(s)))
                .map(|s| s.uid)
                .collect()
        };
        for u in named {
            if !touched.contains(&u) && self.doc.shapes.iter().any(|s| s.uid == u) {
                touched.push(u);
            }
        }
        self.agent.work.note(name, &touched);
        result
    }

    /// Draw the agent's work over the design: the pieces it's on get a
    /// moving outline, a shimmer, and (once, above them) what it's doing.
    pub fn draw_agent_work(&mut self, painter: &egui::Painter, canvas: Rect) {
        if self.agent.work.touches.is_empty() {
            return;
        }
        let now = Instant::now();
        let running = self.agent.running();
        let work = &mut self.agent.work;
        let recent = work.last.is_some_and(|t| now - t < LINGER);
        if !running && !recent {
            for t in &mut work.touches {
                t.gone.get_or_insert(now);
            }
        }
        work.touches
            .retain(|t| t.gone.is_none_or(|g| now - g < FADE));
        let p = crate::theme::pal(painter.ctx());
        let time = painter.ctx().input(|i| i.time);
        let mut union = Rect::NOTHING;
        let mut shown = 0.0f32;
        for t in &self.agent.work.touches {
            let Some(s) = self.doc.shapes.iter().find(|s| s.uid == t.uid) else {
                continue;
            };
            let b = crate::app::shape_bounds(s);
            if b.is_empty() {
                continue;
            }
            let r = Rect::from_two_pos(
                self.view.to_screen(canvas, b.min),
                self.view.to_screen(canvas, b.max),
            )
            .expand(5.0);
            let fade_in = ((now - t.at).as_secs_f32() / 0.2).min(1.0);
            let fade_out = t
                .gone
                .map_or(1.0, |g| 1.0 - (now - g).as_secs_f32() / FADE.as_secs_f32());
            let k = (fade_in * fade_out).clamp(0.0, 1.0);
            working(painter, r, k, time, p.accent);
            if t.gone.is_none() {
                union = union.union(r);
                shown = shown.max(k);
            }
        }
        if shown > 0.0 && union.is_positive() {
            let label = if running {
                format!("{} · {}", self.agent.kind.name(), self.agent.status())
            } else {
                self.agent.work.label.to_string()
            };
            badge(painter, union, canvas, &label, shown, time, p.accent);
        }
        // The shimmer and fades don't need every frame.
        painter
            .ctx()
            .request_repaint_after(Duration::from_millis(33));
    }

    /// The Agent panel's header, after its title, while the panel is
    /// collapsed: a spinner, the step it's on, and how long it's been
    /// working (open, the transcript shows that itself).
    pub fn paint_agent_header(&self, ui: &egui::Ui, title: Rect, header: Rect) {
        if !self.agent.running() {
            return;
        }
        let p = pal_ui(ui);
        let time = ui.input(|i| i.time);
        let painter = ui.painter();
        spinner(
            painter,
            egui::pos2(title.right() + 13.0, header.center().y),
            4.5,
            p.accent,
            time,
        );
        let secs = self.agent.started.map_or(0, |t| t.elapsed().as_secs());
        let right = painter.text(
            egui::pos2(header.right() - 58.0, header.center().y),
            egui::Align2::RIGHT_CENTER,
            format!("{secs} s"),
            egui::FontId::proportional(11.0),
            p.secondary,
        );
        let room = (right.left() - title.right() - 34.0).max(0.0);
        let mut job = egui::text::LayoutJob::simple_singleline(
            format!("{}…", self.agent.status()),
            egui::FontId::proportional(12.0),
            p.secondary,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(room);
        let g = painter.layout_job(job);
        painter.galley(
            egui::pos2(title.right() + 24.0, header.center().y - g.size().y / 2.0),
            g,
            p.secondary,
        );
    }

    /// The transcript's height: what's left of the panel's share of its
    /// dock (it sits at the bottom), or a good size floating.
    fn agent_transcript_h(&self) -> f32 {
        match self.dock_ui.pinned_h {
            Some(h) if !self.dock_ui.in_window => {
                let pad = crate::theme::pad().sum().y;
                (h - crate::dock::HEADER_H - pad - self.agent.chrome_h).max(120.0)
            }
            _ => 380.0,
        }
    }

    /// Open the Agent panel (wherever it lives) with the prompt focused.
    pub fn ask_agent(&mut self) {
        use crate::dock::{PanelId, Place};
        self.dock.reveal(PanelId::Agent);
        let place = self.dock.place_of(self.dock.lead_of(PanelId::Agent));
        if place != Place::Float && self.dock_iconic(place) {
            self.dock_ui.flyout = Some(PanelId::Agent);
        }
        self.agent.focus = true;
    }

    /// Whether the Agent panel is out where you can use it.
    pub fn agent_showing(&self) -> bool {
        use crate::dock::{PanelId, Place};
        if !self.dock.visible(PanelId::Agent) {
            return false;
        }
        match self.dock.place_of(self.dock.lead_of(PanelId::Agent)) {
            Place::Float => true,
            side => {
                let on = if side == Place::Left {
                    self.dock.left
                } else {
                    self.dock.right
                };
                on && (!self.dock_iconic(side) || self.dock_ui.flyout == Some(PanelId::Agent))
            }
        }
    }

    /// The tool strip's Agent button: open the panel (its dock too), or
    /// put it away again (a dock it has to itself closes with it).
    pub fn toggle_agent(&mut self) {
        use crate::dock::{PanelId, Place};
        if !self.agent_showing() {
            self.ask_agent();
            return;
        }
        let place = self.dock.place_of(self.dock.lead_of(PanelId::Agent));
        if self.dock_ui.flyout == Some(PanelId::Agent) {
            self.dock_ui.flyout = None;
        } else if place != Place::Float && self.panels_flat_in(place) == [PanelId::Agent] {
            match place {
                Place::Left => self.dock.left = false,
                _ => self.dock.right = false,
            }
        } else {
            self.dock.toggle(PanelId::Agent);
        }
    }

    /// A dock with only the Agent in it doesn't collapse to a strip with one
    /// icon: it closes (the tool strip's Agent button opens it again).
    pub fn tidy_agent_dock(&mut self) {
        use crate::dock::{PanelId, Place};
        for (k, side) in [Place::Left, Place::Right].into_iter().enumerate() {
            if self.dock.iconic[k] && self.panels_flat_in(side) == [PanelId::Agent] {
                self.dock.iconic[k] = false;
                if self.dock_ui.flyout == Some(PanelId::Agent) {
                    self.dock_ui.flyout = None;
                } else {
                    match side {
                        Place::Left => self.dock.left = false,
                        _ => self.dock.right = false,
                    }
                }
            }
        }
    }

    /// The Agent panel: which agent, the conversation, and the prompt.
    pub fn agent_body(&mut self, ui: &mut egui::Ui) {
        let p = pal_ui(ui);
        let h = self.agent_transcript_h();
        let mut send = false;
        let top = ui.cursor().top();

        let a = &mut self.agent;
        // The conversation: plain, on the panel itself.
        let transcript_h = egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(0, space::S as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let inner_h = h - 2.0 * space::S;
                if a.log.is_empty() && !a.running() {
                    let (r, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), inner_h),
                        Sense::hover(),
                    );
                    // Its program isn't on this Mac: how to install it instead.
                    let kind = a.kind;
                    if a.installed(kind, &ui.ctx().clone()) == Some(false) {
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(r)
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                        );
                        child.add_space((inner_h * 0.5 - 170.0).max(8.0));
                        install_steps(&mut child, a);
                        return;
                    }
                    // Nothing yet: what it can do, and a few ideas to start from.
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(r)
                            .layout(egui::Layout::top_down(egui::Align::Center)),
                    );
                    child.add_space((inner_h * 0.5 - 100.0).max(8.0));
                    let top = child.cursor().top();
                    icons::paint(
                        child.painter(),
                        Rect::from_center_size(egui::pos2(r.center().x, top + 12.0), Vec2::splat(22.0)),
                        Icon::Sparkle,
                        p.tertiary,
                    );
                    child.add_space(32.0);
                    child.add(
                        egui::Label::new(
                            egui::RichText::new(format!(
                                "Ask {} for a design, or a change to this one. It works on the canvas as you watch.",
                                a.kind.name()
                            ))
                            .size(12.0)
                            .color(p.secondary),
                        )
                        .halign(egui::Align::Center)
                        .wrap(),
                    );
                    child.add_space(space::M);
                    // A little air between the ideas, and room inside each.
                    child.spacing_mut().item_spacing.y = space::S;
                    child.spacing_mut().button_padding = Vec2::new(12.0, 6.0);
                    for idea in [
                        "A round camp badge with a pine tree",
                        "Make the lettering satin",
                        "Simplify this for a 4x4 hoop",
                    ] {
                        if child
                            .add(egui::Button::new(egui::RichText::new(idea).size(11.5)).corner_radius(10))
                            .clicked()
                        {
                            a.prompt = idea.to_string();
                            a.focus = true;
                        }
                    }
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("agent-transcript")
                    .min_scrolled_height(inner_h)
                    .max_height(inner_h)
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = space::S;
                        let time = ui.input(|i| i.time);
                        for e in &a.log {
                            entry(ui, e, time);
                        }
                        // Between steps: thinking (a step running shows its own spinner).
                        if a.running() && !matches!(a.log.last(), Some(Entry::Tool(_, _, None))) {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                                spinner(ui.painter(), r.center(), 4.5, p.accent, time);
                                let secs = a.started.map_or(0, |t| t.elapsed().as_secs());
                                caption(ui, format!("{}… {secs} s", a.status()));
                            });
                        }
                    });
            })
            .response
            .rect
            .height();
        // Just a divider between the conversation and the prompt.
        let (line, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
        ui.painter().hline(
            line.x_range(),
            line.center().y,
            Stroke::new(1.0, p.separator),
        );
        // The prompt: one line (it grows with Shift-Enter), Send beside it.
        let field_id = ui.make_persistent_id("agent-prompt");
        // (Above it, its own space; below, the panel's padding is enough.)
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 0,
                right: 0,
                top: space::S as i8,
                bottom: 0,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let button_w = 56.0;
                    let button_h = 24.0;
                    // The text inset like the button's label.
                    ui.add_space(space::S);
                    let field_w = ui.available_width() - button_w - space::S;
                    let r = ui.add_sized(
                        [field_w, button_h],
                        egui::TextEdit::multiline(&mut a.prompt)
                            .id(field_id)
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .desired_rows(1)
                            .desired_width(field_w)
                            // As tall as the button, the text centered on it.
                            .min_size(Vec2::new(field_w, button_h))
                            .align(egui::Align2::LEFT_CENTER)
                            .hint_text("Describe a design, or a change…")
                            .return_key(None),
                    );
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Prompt")
                    });
                    let r = r.on_hover_text("Enter sends · Shift-Enter for a new line");
                    if a.focus {
                        r.request_focus();
                        a.focus = false;
                    }
                    // Enter sends; Shift-Enter makes a new line.
                    if r.has_focus()
                        && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift)
                    {
                        send = true;
                    }
                    if r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && i.modifiers.shift)
                    {
                        a.prompt.push('\n');
                    }
                    if a.running() {
                        if ui
                            .add_sized([button_w, button_h], egui::Button::new("Stop"))
                            .clicked()
                        {
                            a.stop();
                        }
                    } else {
                        let ok = !a.prompt.trim().is_empty();
                        let b =
                            egui::Button::new(egui::RichText::new("Send").color(Color32::WHITE))
                                .fill(p.accent);
                        if ui
                            .add_enabled_ui(ok, |ui| ui.add_sized([button_w, button_h], b))
                            .inner
                            .clicked()
                        {
                            send = true;
                        }
                    }
                });
            });
        // Everything but the transcript, to size it next frame.
        a.chrome_h = (ui.cursor().top() - top - transcript_h).max(0.0);
        if send {
            self.agent_send(ui.ctx());
        }
    }

    /// Send the prompt. What's selected is what the agent starts on.
    pub fn agent_send(&mut self, ctx: &egui::Context) {
        if self.agent.prompt.trim().is_empty() || self.agent.running() {
            return;
        }
        let addr = self.mcp.as_ref().map(|m| m.addr);
        self.agent.send(addr, ctx);
        if self.agent.running() {
            let sel: Vec<usize> = self
                .sel_indices()
                .iter()
                .filter_map(|&i| self.doc.shapes.get(i).map(|s| s.uid))
                .collect();
            self.agent.work.note("get_design", &sel);
        }
    }
}

/// The look of a piece being worked on: a soft wash, a light sweeping
/// across, and a marching outline. `k`: how strongly (fading in and out).
fn working(painter: &egui::Painter, r: Rect, k: f32, time: f64, accent: Color32) {
    if k <= 0.0 {
        return;
    }
    let tint = |a: f32| {
        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), (a * k * 255.0) as u8)
    };
    painter.rect_filled(r, 5.0, tint(0.07));
    // The sweep: a slanted band of light crossing every 1.6 s.
    let clip = r.intersect(painter.clip_rect());
    if clip.is_positive() {
        let p = painter.with_clip_rect(clip);
        let ph = ((time / 1.6).fract()) as f32;
        let bw = (r.width().max(r.height()) * 0.16).clamp(14.0, 56.0);
        let slant = r.height() * 0.35;
        let span = r.width() + 2.0 * (bw + slant);
        let cx = r.left() - bw - slant + span * ph;
        let mut mesh = egui::Mesh::default();
        for (dx, a) in [(-bw, 0.0), (0.0, 0.28), (bw, 0.0)] {
            mesh.colored_vertex(egui::pos2(cx + dx + slant, r.top()), tint(a));
            mesh.colored_vertex(egui::pos2(cx + dx - slant, r.bottom()), tint(a));
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 3, 2);
        mesh.add_triangle(2, 3, 4);
        mesh.add_triangle(3, 5, 4);
        p.add(egui::Shape::mesh(mesh));
    }
    painter.rect_stroke(
        r.expand(1.5),
        6.0,
        Stroke::new(4.0, tint(0.16)),
        egui::StrokeKind::Middle,
    );
    let path = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    let offset = ((time * 22.0) % 9.0) as f32;
    painter.extend(egui::Shape::dashed_line_with_offset(
        &path,
        Stroke::new(1.5, tint(1.0)),
        &[5.0],
        &[4.0],
        offset,
    ));
}

/// The pill over the pieces being worked on: a spinner and what it's doing.
fn badge(
    painter: &egui::Painter,
    over: Rect,
    canvas: Rect,
    label: &str,
    k: f32,
    time: f64,
    accent: Color32,
) {
    let galley = painter.layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(11.5),
        Color32::WHITE,
    );
    let size = Vec2::new(galley.size().x + 32.0, 22.0);
    let mut at = egui::pos2(over.left(), over.top() - 6.0 - size.y);
    if at.y < canvas.top() + 4.0 {
        at.y = over.top() + 6.0;
    }
    at.x = at.x.clamp(
        canvas.left() + 4.0,
        (canvas.right() - size.x - 4.0).max(canvas.left() + 4.0),
    );
    let r = Rect::from_min_size(at, size);
    let a = (k * 235.0) as u8;
    painter.rect_filled(
        r,
        11.0,
        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), a),
    );
    spinner(
        painter,
        egui::pos2(r.left() + 12.0, r.center().y),
        4.0,
        Color32::from_white_alpha(a),
        time,
    );
    painter.galley(
        egui::pos2(r.left() + 22.0, r.center().y - galley.size().y / 2.0),
        galley,
        Color32::from_white_alpha(a),
    );
}

fn entry(ui: &mut egui::Ui, e: &Entry, time: f64) {
    let p = pal_ui(ui);
    match e {
        Entry::You(t) => {
            egui::Frame::new()
                .fill(p.accent.gamma_multiply(0.18))
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(space::S as i8, space::XS as i8 + 2))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(egui::RichText::new(t).size(12.5).color(p.text)).wrap(),
                    );
                });
        }
        Entry::Said(t) => {
            ui.add(egui::Label::new(egui::RichText::new(t).size(12.5).color(p.text)).wrap());
        }
        Entry::Tool(name, args, state) => {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::XS;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                match state {
                    // Still running: a spinner, inline.
                    None => spinner(ui.painter(), r.center(), 4.0, p.accent, time),
                    Some(true) => {
                        ui.painter().circle_filled(r.center(), 3.5, p.accent);
                    }
                    Some(false) => {
                        icons::paint(ui.painter(), r, Icon::Warning, p.warning);
                    }
                }
                ui.label(
                    egui::RichText::new(if state.is_none() {
                        format!("{}…", activity(name))
                    } else {
                        activity(name).to_string()
                    })
                    .size(11.5)
                    .color(if state.is_none() { p.text } else { p.secondary }),
                );
                ui.label(
                    egui::RichText::new(name)
                        .size(10.5)
                        .monospace()
                        .color(p.tertiary),
                );
                if !args.is_empty() {
                    ui.add(
                        egui::Label::new(egui::RichText::new(args).size(10.5).color(p.tertiary))
                            .truncate(),
                    )
                    .on_hover_text(args);
                }
            });
        }
        Entry::Error(t) => crate::theme::callout(ui, p.warning, Icon::Warning, t),
        Entry::Done(t) => {
            if !t.is_empty() {
                caption(ui, format!("Done · {t}"));
            }
        }
    }
}

/// The agent's command-line tool isn't installed: the commands to install
/// it (with Copy buttons), how to sign in, Check Again, and the other agent
/// if that one is installed.
fn install_steps(ui: &mut egui::Ui, a: &mut AgentState) {
    let p = pal_ui(ui);
    let kind = a.kind;
    let (ways, sign_in) = kind.install_steps();
    ui.spacing_mut().item_spacing.y = space::S;
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        icons::paint(ui.painter(), r, Icon::Info, p.secondary);
        ui.label(
            egui::RichText::new(format!("{} isn't installed on this Mac", kind.name()))
                .size(13.0)
                .strong()
                .color(p.text),
        );
    });
    caption(
        ui,
        format!(
            "ZigZag works with the {} command-line tool (`{}`). Install it in Terminal:",
            kind.name(),
            kind.program()
        ),
    );
    for (how, command) in ways {
        crate::mcp_chip::copy_block(ui, how, command);
    }
    caption(ui, sign_in);
    ui.add_space(space::XS);
    ui.horizontal(|ui| {
        if ui
            .button("Check Again")
            .on_hover_text("Look for it again, after installing it")
            .clicked()
        {
            a.check_again();
        }
        let other = AgentKind::ALL
            .into_iter()
            .find(|&k| k != kind)
            .unwrap_or_default();
        if a.found[other.index()].as_ref().is_some_and(|f| f.is_some())
            && ui.button(format!("Use {} Instead", other.name())).clicked()
        {
            a.set_kind(other);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_claude_stream_json() {
        let mut s = None;
        let lines = [
            json!({"type":"system","subtype":"init","session_id":"abc"}),
            json!({"type":"assistant","message":{"content":[
                {"type":"text","text":"Making a badge."},
                {"type":"tool_use","id":"u1","name":"mcp__zigzag__add_svg","input":{"svg":"<svg/>","width_mm":40}},
                {"type":"tool_use","id":"u2","name":"mcp__zigzag__render_preview","input":{}}
            ]}}),
            // Results for parallel calls: matched by id, not order.
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"u1","is_error":true}]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"u2","is_error":false}]}}),
            json!({"type":"result","subtype":"success","is_error":false,"num_turns":3,"duration_ms":12000,"session_id":"abc"}),
        ];
        let mut a = AgentState::default();
        for l in &lines {
            for (id, e) in claude_event(l, &mut s) {
                a.push(id, e);
            }
        }
        assert_eq!(s.as_deref(), Some("abc"));
        assert_eq!(a.log[0], Entry::Said("Making a badge.".into()));
        assert_eq!(
            a.log[1],
            Entry::Tool("add_svg".into(), "width_mm: 40".into(), Some(false))
        );
        assert_eq!(
            a.log[2],
            Entry::Tool("render_preview".into(), "".into(), Some(true))
        );
        assert!(matches!(&a.log[3], Entry::Done(t) if t.contains("3 turns")));
    }

    #[test]
    fn reads_codex_json() {
        let mut s = None;
        let lines = [
            json!({"type":"thread.started","thread_id":"t1"}),
            json!({"type":"item.started","item":{"id":"1","type":"mcp_tool_call","server":"zigzag","tool":"add_text","arguments":{"text":"CAMP"},"status":"in_progress"}}),
            json!({"type":"item.completed","item":{"id":"1","type":"mcp_tool_call","server":"zigzag","tool":"add_text","status":"failed","error":{"message":"bad"}}}),
            json!({"type":"item.completed","item":{"id":"2","type":"agent_message","text":"Added CAMP."}}),
            json!({"type":"error","message":"Reconnecting... 1/5"}),
            json!({"type":"turn.completed","usage":{}}),
        ];
        let mut a = AgentState::default();
        for l in &lines {
            for (id, e) in codex_event(l, &mut s) {
                a.push(id, e);
            }
        }
        assert_eq!(s.as_deref(), Some("t1"));
        assert_eq!(
            a.log[0],
            Entry::Tool("add_text".into(), "text: CAMP".into(), Some(false))
        );
        assert_eq!(a.log[1], Entry::Error("add_text: bad".into()), "says why");
        assert_eq!(a.log[2], Entry::Said("Added CAMP.".into()));
        assert_eq!(
            a.log.len(),
            4,
            "reconnect notices aren't errors: {:?}",
            a.log
        );
    }

    #[test]
    fn codex_cancelled_edits_are_explained_once() {
        let mut s = None;
        let mut a = AgentState::default();
        a.log.push(Entry::You("make it green".into()));
        for (k, tool) in ["set_object", "transform_objects", "set_object"]
            .iter()
            .enumerate()
        {
            let id = k.to_string();
            for l in [
                json!({"type":"item.started","item":{"id":id,"type":"mcp_tool_call","server":"zigzag","tool":tool,"arguments":{},"status":"in_progress"}}),
                json!({"type":"item.completed","item":{"id":id,"type":"mcp_tool_call","server":"zigzag","tool":tool,"status":"failed","error":{"message": if k == 1 { "user cancelled MCP tool call" } else { "MCP tool call requires approval, but approval policy is never" }}}}),
            ] {
                for (id, e) in codex_event(&l, &mut s) {
                    a.push(id, e);
                }
            }
        }
        let errors: Vec<_> = a
            .log
            .iter()
            .filter(|e| matches!(e, Entry::Error(_)))
            .collect();
        assert_eq!(errors.len(), 1, "{:?}", a.log);
        assert!(matches!(errors[0], Entry::Error(m) if m.contains("approval")));
        let failed = a
            .log
            .iter()
            .filter(|e| matches!(e, Entry::Tool(_, _, Some(false))))
            .count();
        assert_eq!(failed, 3);
    }

    #[test]
    fn codex_is_told_to_approve_the_design_tools() {
        let mut a = AgentState {
            kind: AgentKind::Codex,
            ..Default::default()
        };
        let cmd = a.command("codex".into(), "http://127.0.0.1:1/mcp", None, 1);
        let args: Vec<String> = cmd
            .get_args()
            .map(|x| x.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter()
                .any(|x| x == "mcp_servers.zigzag.default_tools_approval_mode=\"approve\""),
            "{args:?}"
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some("-"),
            "prompt on stdin last"
        );
    }

    #[test]
    fn process_exit_waits_for_unread_stdout() -> anyhow::Result<()> {
        use anyhow::ensure;
        let child = Arc::new(Mutex::new(
            Command::new(std::env::current_exe()?)
                .args(["--exact", "agent::tests::send_process_fixture"])
                .env_remove("ZIGZAG_SEND_FIXTURE")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?,
        ));
        let result = (|| -> anyhow::Result<()> {
            for exit_first in [true, false] {
                let (tx, rx) = mpsc::channel();
                let mut agent = AgentState {
                    kind: AgentKind::Claude,
                    run: Some(Run {
                        child: child.clone(),
                        rx,
                        stderr: Vec::new(),
                        exit: None,
                        stdout_closed: false,
                    }),
                    ..Default::default()
                };
                if exit_first {
                    tx.send(Event::Exit(Some(0)))?;
                    agent.poll();
                    ensure!(agent.running(), "process exit discarded unread stdout");
                }
                for event in [
                    serde_json::json!({"type":"system", "subtype":"init", "session_id":"s1"}),
                    serde_json::json!({"type":"assistant", "message":{"content":[{"type":"text", "text":"last output"}]}}),
                    serde_json::json!({"type":"result", "subtype":"success", "is_error":false, "num_turns":1, "duration_ms":10}),
                ] {
                    tx.send(Event::Line(event.to_string()))?;
                }
                tx.send(Event::StdoutClosed)?;
                agent.poll();
                if !exit_first {
                    ensure!(agent.running(), "stdout EOF was mistaken for process exit");
                    tx.send(Event::Exit(Some(0)))?;
                    agent.poll();
                }
                ensure!(!agent.running(), "completed child stayed active");
                ensure!(
                    agent.session.as_deref() == Some("s1"),
                    "late session was lost"
                );
                ensure!(
                    agent.log.contains(&Entry::Said("last output".into())),
                    "late output was lost"
                );
                ensure!(
                    matches!(agent.log.last(), Some(Entry::Done(summary)) if !summary.is_empty()),
                    "streamed completion was replaced by a generic exit"
                );
            }
            Ok(())
        })();
        let cleanup = child
            .lock()
            .map_err(|_| anyhow::anyhow!("child lock poisoned"))?
            .wait();
        result?;
        cleanup?;
        Ok(())
    }

    const SEND_FIXTURE_COMPLETED: &[u8] = b"zigzag-send-fixture-completed-v1";

    #[test]
    fn send_process_fixture() -> anyhow::Result<()> {
        use anyhow::{Context, ensure};
        if std::env::var_os("ZIGZAG_SEND_FIXTURE").as_deref() != Some(std::ffi::OsStr::new("1")) {
            return Ok(());
        }
        let expected =
            PathBuf::from(std::env::var_os("ZIGZAG_CLAUDE").context("fixture executable")?);
        let input =
            PathBuf::from(std::env::var_os("ZIGZAG_AGENT_STDIN").context("fixture stdin record")?);
        let mut agent = AgentState {
            kind: AgentKind::Claude,
            path: Some(None),
            ..Default::default()
        };
        let prompt = "hello\n\"quoted\" \\ path";
        agent.prompt = prompt.into();
        agent.send(Some("127.0.0.1:1".parse()?), &egui::Context::default());
        let started = Instant::now();
        while agent.running() {
            agent.poll();
            if started.elapsed() >= Duration::from_secs(10) {
                agent.stop();
                anyhow::bail!("agent fixture did not finish: {:?}", agent.log);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        ensure!(
            agent.session.as_deref() == Some("s1"),
            "missing streamed session: {:?}",
            agent.log
        );
        ensure!(
            agent
                .log
                .contains(&Entry::Said("fixture received stdin".into())),
            "missing output: {:?}",
            agent.log
        );
        ensure!(
            matches!(agent.log.last(), Some(Entry::Done(_))),
            "missing completion: {:?}",
            agent.log
        );
        ensure!(agent.prompt.is_empty(), "submitted prompt was not cleared");
        ensure!(
            std::fs::read(&input)? == prompt.as_bytes(),
            "send changed stdin"
        );
        let actual = std::fs::read_to_string(input.with_extension("program"))?;
        ensure!(
            std::fs::canonicalize(actual)? == std::fs::canonicalize(expected)?,
            "send launched a different executable"
        );
        // A successful libtest exit alone also permits an unmatched filter or
        // this fixture's intentional no-op path. Witness every assertion above.
        std::fs::write(input.with_extension("complete"), SEND_FIXTURE_COMPLETED)?;
        Ok(())
    }

    #[test]
    fn runs_an_agent_program_and_streams_its_output() -> anyhow::Result<()> {
        use anyhow::{Context, ensure};
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zigzag-agent-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&dir)?;
        let result = (|| -> anyhow::Result<()> {
            let source = dir.join("fixture.rs");
            std::fs::write(
                &source,
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/agent_process.rs"
                )),
            )?;
            let program = dir.join(format!("selected-agent{}", std::env::consts::EXE_SUFFIX));
            let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
            let built = Command::new(compiler)
                .args(["--edition=2024", "--crate-name", "agent_fixture"])
                .arg(&source)
                .arg("-o")
                .arg(&program)
                .output().context("compiling std-only agent fixture")?;
            ensure!(
                built.status.success(),
                "fixture compile failed: {}",
                String::from_utf8_lossy(&built.stderr)
            );
            // Safe lookup fallback: a wrong lookup invokes a decoy, never an installed agent.
            let decoy = dir.join("decoy");
            std::fs::create_dir(&decoy)?;
            std::fs::copy(&program, decoy.join("claude"))?;
            #[cfg(windows)]
            std::fs::copy(&program, decoy.join("claude.exe"))?;
            let input = dir.join("stdin");
            let output = Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "agent::tests::send_process_fixture",
                    "--nocapture",
                ])
                .env("ZIGZAG_SEND_FIXTURE", "1")
                .env("ZIGZAG_CLAUDE", &program)
                .env("ZIGZAG_AGENT_STDIN", &input)
                .env("PATH", &decoy)
                .output().context("running isolated send fixture")?;
            ensure!(
                output.status.success(),
                "send process fixture failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let completion = std::fs::read(input.with_extension("complete"))
                .context("send fixture did not complete its assertions")?;
            ensure!(
                completion == SEND_FIXTURE_COMPLETED,
                "invalid send fixture completion witness"
            );
            Ok(())
        })();
        let cleanup = std::fs::remove_dir_all(&dir);
        result?;
        cleanup?;
        Ok(())
    }

    fn studio() -> egui_kittest::Harness<'static, Studio> {
        let path =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg"));
        let mut h = egui_kittest::Harness::builder()
            .with_size(Vec2::new(1400.0, 900.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(path), None),
            );
        // As if both agents were installed (the panel's usual start); tests
        // about missing ones say so.
        h.state_mut().agent.found = [
            Some(Some(PathBuf::from("/usr/local/bin/claude"))),
            Some(Some(PathBuf::from("/usr/local/bin/codex"))),
        ];
        h.run_steps(4);
        h
    }

    #[test]
    fn the_agent_panel_opens_with_cmd_k_and_sits_at_the_bottom_of_its_dock() {
        use crate::dock::{PanelId, Place};
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        assert!(
            !h.state().dock.visible(PanelId::Agent),
            "tests start without it"
        );
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::K);
        h.run_steps(20);
        assert!(h.state().dock.visible(PanelId::Agent));
        assert_eq!(h.state().dock.place_of(PanelId::Agent), Place::Left);
        assert!(
            h.ctx.memory(|m| m.focused().is_some()),
            "the prompt has the keyboard"
        );
        // Alone in the left dock: the whole height, its prompt at the bottom.
        let agent = h.get_by_label("Agent panel").rect();
        let prompt = h.get_by_label("Prompt").rect();
        assert!(agent.top() < 120.0, "{agent:?}");
        assert!(prompt.bottom() > 900.0 - 140.0, "{prompt:?}");
        assert!(h.query_by_label("Agent panel divider").is_none());
        assert!(
            h.query_by_label("A round camp badge with a pine tree")
                .is_some()
        );
        // Sharing the dock, it sits at the bottom, under the other panel.
        h.state_mut()
            .dock
            .move_panel(PanelId::Document, Place::Left, None);
        h.run_steps(20);
        let agent = h.get_by_label("Agent panel").rect();
        let doc = h.get_by_label("Document panel").rect();
        assert!(agent.top() > doc.top() + 100.0, "{agent:?} {doc:?}");
        let prompt = h.get_by_label("Prompt").rect();
        assert!(prompt.bottom() > 900.0 - 140.0, "{prompt:?}");
        // The divider gives it more room.
        let share = h.state().dock.agent_share;
        let d = h.get_by_label("Agent panel divider").rect().center();
        h.drag_at(d);
        h.run_steps(1);
        h.hover_at(d - Vec2::new(0.0, 60.0));
        h.run_steps(1);
        h.drop_at(d - Vec2::new(0.0, 100.0));
        h.run_steps(3);
        assert!(
            h.state().dock.agent_share > share + 0.05,
            "{} > {share}",
            h.state().dock.agent_share
        );
        let higher = h.get_by_label("Agent panel").rect();
        assert!(higher.top() < agent.top() - 40.0, "{higher:?} {agent:?}");
        // Collapsed: just its header, at the bottom; the other gets the room.
        h.get_by_label("Agent panel").click();
        h.run_steps(40);
        assert!(h.state().dock.collapsed(PanelId::Agent));
        assert!(h.query_by_label("Prompt").is_none());
        let header = h.get_by_label("Agent panel").rect();
        assert!(
            header.top() > prompt.top() - 40.0,
            "at the bottom: {header:?}"
        );
        // Cmd-K opens it again.
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::K);
        h.run_steps(40);
        assert!(!h.state().dock.collapsed(PanelId::Agent));
        assert!(h.query_by_label("Prompt").is_some());
    }

    #[test]
    fn the_header_names_the_agent_and_switches_it() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().agent.config = None; // don't save the choice
        h.state_mut().agent.kind = AgentKind::Codex;
        h.state_mut().dock.reveal(PanelId::Agent);
        h.run_steps(5);
        assert_eq!(h.state().panel_name(PanelId::Agent), "Codex");
        h.state_mut().agent.log.push(Entry::Said("hi".into()));
        h.run_steps(2);
        h.get_by_label("New Chat").click();
        h.run_steps(2);
        assert!(h.state().agent.log.is_empty(), "+ starts over");
        h.get_by_label("Choose Agent").click();
        h.run_steps(2);
        h.get_by_label("Claude Code").click();
        h.run_steps(2);
        assert_eq!(h.state().agent.kind, AgentKind::Claude);
        assert_eq!(h.state().panel_name(PanelId::Agent), "Claude Code");
    }

    #[test]
    fn codex_is_the_default_agent() {
        assert_eq!(AgentKind::default(), AgentKind::Codex);
        assert_eq!(AgentState::default().kind, AgentKind::Codex);
        assert_eq!(AgentKind::ALL[0], AgentKind::Codex, "first in the menu");
    }

    #[test]
    fn a_missing_cli_shows_how_to_install_it() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().agent.config = None;
        h.state_mut().agent.kind = AgentKind::Codex;
        h.state_mut().agent.found = [Some(None), Some(None)]; // neither installed
        h.state_mut().dock.reveal(PanelId::Agent);
        h.run_steps(5);
        h.get_by_label_contains("Codex isn't installed");
        h.get_by_label("brew install --cask codex");
        h.get_by_label("npm install -g @openai/codex");
        h.get_by_label_contains("Sign in with ChatGPT");
        assert!(
            h.query_by_label("A round camp badge with a pine tree").is_none(),
            "setup instead of ideas"
        );
        assert!(
            h.query_by_label("Use Claude Code Instead").is_none(),
            "not offered: it isn't installed either"
        );
        // Claude Code installed: offered, and a click switches.
        h.state_mut().agent.found[AgentKind::Claude.index()] =
            Some(Some(PathBuf::from("/usr/local/bin/claude")));
        h.run_steps(3);
        h.get_by_label("Use Claude Code Instead").click();
        h.run_steps(3);
        assert_eq!(h.state().agent.kind, AgentKind::Claude);
        assert!(
            h.query_by_label("A round camp badge with a pine tree").is_some(),
            "installed: the usual start"
        );
        // Its own steps when it's missing.
        h.state_mut().agent.found = [Some(None), Some(None)];
        h.run_steps(3);
        h.get_by_label("curl -fsSL https://claude.ai/install.sh | bash");
        h.get_by_label("brew install --cask claude-code");
        h.get_by_label("Check Again");
    }

    #[test]
    fn check_again_forgets_what_was_found() {
        let mut a = AgentState {
            path: Some(Some("/nowhere".into())),
            found: [Some(None), Some(None)],
            ..Default::default()
        };
        a.check_again();
        assert_eq!(a.found, [None, None]);
        assert!(a.path.is_none(), "the login PATH is read again too");
    }

    #[test]
    fn the_tool_strip_button_opens_and_closes_the_agents_dock() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        assert!(!h.state().agent_showing());
        h.get_by_label("Agent").click();
        h.run_steps(5);
        assert!(h.state().agent_showing() && h.state().dock.left);
        assert!(h.query_by_label("Prompt").is_some());
        // Again: the dock it has to itself closes, no strip of one icon left.
        h.get_by_label("Agent").click();
        h.run_steps(3);
        assert!(!h.state().agent_showing());
        assert!(!h.state().dock.left);
        assert!(h.query_by_label("Prompt").is_none());
        // Collapsing that dock to icons closes it too (a layout saved that way as well).
        h.get_by_label("Agent").click();
        h.run_steps(3);
        h.state_mut().dock.iconic[0] = true;
        h.run_steps(3);
        assert!(!h.state().dock.iconic[0] && !h.state().dock.left);
        // Sharing a dock, the button just puts the panel away.
        h.state_mut()
            .dock
            .move_panel(PanelId::Document, crate::dock::Place::Left, None);
        h.run_steps(3);
        assert!(h.state().agent_showing(), "the Document opened its dock");
        h.get_by_label("Agent").click();
        h.run_steps(3);
        assert!(!h.state().dock.visible(PanelId::Agent));
        assert!(h.state().dock.left, "the Document stays");
    }

    #[test]
    fn agent_calls_mark_the_objects_they_work_on() {
        let mut h = studio();
        let uid = |h: &egui_kittest::Harness<'_, Studio>, name: &str| {
            let i = h.state().names.iter().position(|n| n == name).unwrap();
            h.state().doc.shapes[i].uid
        };
        let (sun, swoosh) = (uid(&h, "sun"), uid(&h, "swoosh"));
        let r = h.state_mut().run_mcp_call(
            "set_object",
            &serde_json::json!({ "object": "sun", "kind": "satin" }),
        );
        assert!(!r.is_error);
        assert_eq!(h.state().agent.work.active(), vec![sun]);
        assert_eq!(h.state().agent.work.label, "Restyling");
        h.run_steps(2);
        // A look doesn't move it on.
        h.state_mut()
            .run_mcp_call("check_design", &serde_json::json!({}));
        assert_eq!(h.state().agent.work.active(), vec![sun]);
        assert_eq!(h.state().agent.work.label, "Checking");
        // The next piece: the sun fades, the swoosh is worked on.
        h.state_mut().run_mcp_call(
            "transform_objects",
            &serde_json::json!({ "objects": ["swoosh"], "scale": 1.1 }),
        );
        assert_eq!(h.state().agent.work.active(), vec![swoosh]);
        let sun_touch = h.state().agent.work.touches.iter().find(|t| t.uid == sun);
        assert!(
            sun_touch.is_some_and(|t| t.gone.is_some()),
            "the sun fading out"
        );
        h.run_steps(2);
        // A while after an outside agent's last call, it all fades away.
        let past = Instant::now() - Duration::from_secs(3);
        h.state_mut().agent.work.last = Some(past);
        h.run_steps(1);
        assert!(h.state().agent.work.active().is_empty());
        for t in &mut h.state_mut().agent.work.touches {
            t.gone = Some(past);
        }
        h.run_steps(1);
        assert!(h.state().agent.work.touches.is_empty());
        // A new file isn't something it worked on.
        h.state_mut().run_mcp_call(
            "add_svg",
            &serde_json::json!({ "svg": "<svg xmlns='http://www.w3.org/2000/svg' width='10mm' height='10mm' viewBox='0 0 10 10'><rect id='box' width='10' height='10' fill='#c00'/></svg>" }),
        );
        let added = h.state().doc.shapes.last().unwrap().uid;
        assert_eq!(
            h.state().agent.work.active(),
            vec![added],
            "a new object is one it's on"
        );
    }

    #[test]
    fn the_running_step_is_the_status() {
        let mut a = AgentState::default();
        assert_eq!(a.status(), "Thinking");
        a.push(
            Some("1".into()),
            Entry::Tool("transform_objects".into(), String::new(), None),
        );
        assert_eq!(a.status(), "Placing");
        a.push(
            Some("1".into()),
            Entry::Tool(String::new(), String::new(), Some(true)),
        );
        assert_eq!(a.status(), "Thinking");
    }
}
