//! Setting up the Agent panel's agent without a terminal: installing its
//! command-line tool with the maker's own installer, and signing in to it,
//! each from a button in the panel.
//!
//! - Install runs the official one-line installer (the same command the
//!   panel shows for doing it by hand) with nothing to answer: Codex's with
//!   `CODEX_NON_INTERACTIVE`, Claude Code's and Antigravity's need nothing.
//!   They put the program in `~/.local/bin` on a Mac (or Linux), where
//!   `agent::locate` looks; on Windows, Claude Code's goes there too,
//!   Codex's under `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin` and
//!   Antigravity's under `%LOCALAPPDATA%\agy\bin`.
//! - Sign in runs `codex login` or `claude auth login`. Each opens the
//!   browser and finishes by itself once the person signs in there; their
//!   output has the page's address, shown in case the browser didn't open.
//!   Claude's page can show a code to paste instead: the panel has a field
//!   for it, sent to the command's input.
//! - Antigravity has no sign-in command: a first prompt (`agy -p`) signs in
//!   when it's needed, printing the page's address (the panel opens it) and
//!   then waiting a minute for the code that page shows, pasted in. It only
//!   does that with a terminal to talk to, so on a Mac or Linux it's run in
//!   one of its own through `script`; on Windows it gets a console window,
//!   where the code is pasted. Its result line says whether it worked (it
//!   may not exit after it, so it's stopped then).
//! - Whether the agent is signed in is asked once (`codex login status`,
//!   `claude auth status --json`, `agy models`), off the UI thread, and again after a
//!   turn ends in an error. Only a plain "not signed in" counts: anything
//!   else (an older version, a stand-in) is taken as signed in, so the
//!   panel never stands in the way of an agent that works.

use crate::agent::AgentKind;
use eframe::egui;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// What the panel is doing to set the agent up.
#[derive(Default)]
pub enum Step {
    #[default]
    Idle,
    Installing(Job),
    SigningIn(Job),
    /// The last install or sign-in didn't work: why, for the panel.
    Failed {
        /// The agent it was for: another agent isn't held up by it.
        kind: AgentKind,
        install: bool,
        why: String,
    },
}

/// A setup command running in the background.
pub struct Job {
    pub kind: AgentKind,
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<String>,
    /// The last few lines it printed (the latest last).
    pub lines: Vec<String>,
    /// The sign-in page's address, once it's printed.
    pub url: Option<String>,
    /// A code pasted from the sign-in page has been sent.
    pub code_sent: bool,
}

impl Job {
    /// The latest line it printed worth showing, if any.
    pub fn last_line(&self) -> Option<&str> {
        self.lines
            .iter()
            .rev()
            .map(|l| status_line(l))
            .find(|l| !l.is_empty() && !l.starts_with("http"))
    }
}

/// An installer's line as the panel shows it: without the arrows and
/// bullets it starts with ("==> ", "-> ", "• ").
pub fn status_line(l: &str) -> &str {
    l.trim().trim_start_matches(|c: char| !c.is_alphanumeric()).trim()
}

impl Drop for Job {
    fn drop(&mut self) {
        // Cancelled, or the panel closed with it running: don't leave it.
        if let Ok(None) = self.child.try_wait() {
            // (What `script` runs is in a session of its own, which outlives
            // `script` itself: it's ended first.)
            #[cfg(unix)]
            let _ = Command::new("pkill")
                .args(["-TERM", "-P", &self.child.id().to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[derive(Default)]
pub struct Setup {
    pub step: Step,
    /// Whether each agent is signed in (by [`AgentKind::index`]): `None`
    /// until asked, `Some(false)` only when it plainly says it isn't.
    pub signed_in: [Option<bool>; AgentKind::COUNT],
    asking: [Option<Receiver<Option<bool>>>; AgentKind::COUNT],
    /// An install just finished: look for the program again.
    pub installed_now: bool,
    /// A code from the sign-in page, being typed or pasted.
    pub code: String,
}

/// What came of polling a running job.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Running,
    Installed,
    SignedIn,
    Failed,
}

impl Setup {
    pub fn busy(&self) -> bool {
        matches!(self.step, Step::Installing(_) | Step::SigningIn(_))
    }

    /// Install `kind` with its maker's installer.
    pub fn install(&mut self, kind: AgentKind, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        self.step = match start(installer(kind), kind, false, ctx) {
            Ok(job) => Step::Installing(job),
            Err(e) => Step::Failed {
                kind,
                install: true,
                why: format!("Couldn't start the installer: {e}"),
            },
        };
    }

    /// Sign in to `kind` (its program at `program`, run with `path` as its
    /// PATH, as the agent is): opens the browser.
    pub fn sign_in(
        &mut self,
        kind: AgentKind,
        program: &Path,
        path: Option<String>,
        ctx: &egui::Context,
    ) {
        if self.busy() {
            return;
        }
        let mut cmd = match kind {
            AgentKind::Antigravity => agy_sign_in(program),
            _ => Command::new(program),
        };
        if let Some(p) = path {
            // (An npm install is a Node script: Node has to be found.)
            cmd.env("PATH", p);
        }
        match kind {
            AgentKind::Codex => {
                cmd.arg("login");
            }
            AgentKind::Claude => {
                cmd.args(["auth", "login"]);
            }
            AgentKind::Antigravity => {}
        };
        let started = if kind == AgentKind::Antigravity && cfg!(windows) {
            start_in_window(cmd, kind)
        } else {
            start(cmd, kind, true, ctx)
        };
        self.step = match started {
            Ok(job) => Step::SigningIn(job),
            Err(e) => Step::Failed {
                kind,
                install: false,
                why: format!("Couldn't start {}: {e}", kind.name()),
            },
        };
    }

    /// Stop what's running.
    pub fn cancel(&mut self) {
        self.step = Step::Idle;
    }

    /// Send the code from the sign-in page to Claude's `auth login` (or
    /// Antigravity's first prompt).
    pub fn send_code(&mut self, code: &str) {
        if let Step::SigningIn(job) = &mut self.step
            && let Some(stdin) = job.stdin.as_mut()
        {
            let _ = writeln!(stdin, "{}", code.trim());
            let _ = stdin.flush();
            job.code_sent = true;
        }
    }

    /// Read what the running job printed, and see whether it's done.
    pub fn poll(&mut self, ctx: &egui::Context) -> Outcome {
        let (job, install) = match &mut self.step {
            Step::Installing(j) => (j, true),
            Step::SigningIn(j) => (j, false),
            _ => return Outcome::Running,
        };
        let mut result = None;
        while let Ok(line) = job.rx.try_recv() {
            if job.url.is_none()
                && let Some(u) = sign_in_url(&line)
            {
                // (Antigravity only prints it: the panel opens it.)
                if job.kind == AgentKind::Antigravity {
                    crate::agent::open_web(ctx, &u);
                }
                job.url = Some(u);
            }
            if job.kind == AgentKind::Antigravity
                && let Some(r) = agy_result(&line)
            {
                result = Some(r);
            }
            job.lines.push(line);
            if job.lines.len() > 40 {
                job.lines.remove(0);
            }
        }
        // Antigravity's result says how signing in went: it's done then,
        // whether or not it has exited.
        if let Some(r) = result {
            let kind = job.kind;
            if r.is_ok() {
                self.step = Step::Idle;
                self.signed_in[kind.index()] = Some(true);
                return Outcome::SignedIn;
            }
            let why = r.err().unwrap_or_default();
            // (Its result says "failed or timed out" either way: the line
            // before it says which.)
            let said = |w: &str| job.lines.iter().any(|l| l.contains(w));
            self.step = Step::Failed {
                kind,
                install: false,
                why: if said("token exchange") || said("invalid_grant") {
                    "Google didn't accept that code. Click Sign In to try again, and paste the new code it shows.".into()
                } else if why.contains("timed out") && !job.code_sent {
                    "Signing in took longer than a minute, so Antigravity gave up. Click Sign In to try again.".into()
                } else {
                    format!("Signing in didn't finish: {why}.")
                },
            };
            return Outcome::Failed;
        }
        let status = match job.child.try_wait() {
            Ok(None) => {
                // (Its output wakes the panel; this catches it ending quietly.)
                ctx.request_repaint_after(Duration::from_millis(300));
                return Outcome::Running;
            }
            Ok(Some(s)) => s,
            Err(e) => {
                self.step = Step::Failed {
                    kind: job.kind,
                    install,
                    why: e.to_string(),
                };
                return Outcome::Failed;
            }
        };
        let kind = job.kind;
        if status.success() {
            self.step = Step::Idle;
            if install {
                self.installed_now = true;
                // A fresh install has nobody signed in yet.
                self.signed_in[kind.index()] = None;
                Outcome::Installed
            } else {
                self.signed_in[kind.index()] = Some(true);
                Outcome::SignedIn
            }
        } else {
            let tail: Vec<&str> = job
                .lines
                .iter()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect();
            let tail = tail[tail.len().saturating_sub(3)..].join(" ");
            let why = if install {
                format!(
                    "The {} installer stopped{}.",
                    kind.name(),
                    if tail.is_empty() {
                        String::new()
                    } else {
                        format!(": {tail}")
                    }
                )
            } else {
                sign_in_failure(&job.lines)
            };
            self.step = Step::Failed { kind, install, why };
            Outcome::Failed
        }
    }

    /// Whether `kind` (its program at `program`) needs signing in: asked
    /// once, off the UI thread; `false` until known.
    pub fn needs_sign_in(
        &mut self,
        kind: AgentKind,
        program: &Path,
        path: Option<String>,
        ctx: &egui::Context,
    ) -> bool {
        let i = kind.index();
        if let Some(rx) = &self.asking[i]
            && let Ok(answer) = rx.try_recv()
        {
            // (Unknown counts as signed in.)
            self.signed_in[i] = Some(answer.unwrap_or(true));
            self.asking[i] = None;
        }
        if let Some(s) = self.signed_in[i] {
            return !s;
        }
        // Tests set `signed_in` themselves rather than run programs.
        if cfg!(test) {
            return false;
        }
        if self.asking[i].is_none() {
            let (tx, rx) = mpsc::channel();
            let program = program.to_path_buf();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(signed_in(kind, &program, path.as_deref()));
                ctx.request_repaint();
            });
            self.asking[i] = Some(rx);
        }
        false
    }

    /// Ask again whether `kind` is signed in (after a turn that failed).
    pub fn recheck(&mut self, kind: AgentKind) {
        self.signed_in[kind.index()] = None;
        self.asking[kind.index()] = None;
    }
}

/// The installer for `kind`, run with nothing to answer and no window.
fn installer(kind: AgentKind) -> Command {
    let mut cmd = if cfg!(windows) {
        let url = match kind {
            AgentKind::Codex => "https://chatgpt.com/codex/install.ps1",
            AgentKind::Claude => "https://claude.ai/install.ps1",
            AgentKind::Antigravity => "https://antigravity.google/cli/install.ps1",
        };
        let mut c = Command::new("powershell.exe");
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!("irm {url} | iex"),
        ]);
        c
    } else {
        let (url, shell) = match kind {
            AgentKind::Codex => ("https://chatgpt.com/codex/install.sh", "sh"),
            AgentKind::Claude => ("https://claude.ai/install.sh", "bash"),
            AgentKind::Antigravity => ("https://antigravity.google/cli/install.sh", "bash"),
        };
        let mut c = Command::new("/bin/sh");
        // (Downloaded whole first: in a pipe, a download that fails would
        // leave the shell nothing to run, and that counts as working.)
        c.args([
            "-c",
            &format!("s=$(curl -fsSL {url}) || exit $?; printf '%s\\n' \"$s\" | {shell}"),
        ]);
        // (A Mac app's PATH is short; curl and bash are in these.)
        let path = std::env::var("PATH").unwrap_or_default();
        c.env("PATH", format!("/usr/bin:/bin:/usr/sbin:/sbin:{path}"));
        c
    };
    // Codex's asks about things like replacing another copy; take its defaults.
    cmd.env("CODEX_NON_INTERACTIVE", "1");
    cmd
}

/// What signs Antigravity in: a first prompt, with no tools to use (the
/// same refusals as the panel's runs) in a folder of its own. On a Mac or
/// Linux it's given a terminal (`script`'s): without one it won't ask.
fn agy_sign_in(program: &Path) -> Command {
    let args = [
        "-p",
        "Reply with OK.",
        "--output-format",
        "stream-json",
        "--sandbox",
        "--settings",
        r#"{"agentMode":"","permissions":{"deny":["command(*)","write_file(*)","read_url(*)","execute_url(*)","download_url(*)"]}}"#,
    ];
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new(program);
        c.args(args);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("/usr/bin/script");
        c.args(["-q", "/dev/null"]).arg(program).args(args);
        c
    } else {
        // (util-linux's `script` takes the command as one line.)
        let line: Vec<String> = std::iter::once(program.to_string_lossy().into_owned())
            .chain(args.iter().map(|a| a.to_string()))
            .map(|a| format!("'{}'", a.replace('\'', r"'\''")))
            .collect();
        let mut c = Command::new("script");
        c.args(["-q", "-e", "-c", &line.join(" "), "/dev/null"]);
        c
    };
    let dir = std::env::temp_dir().join("zigzag-agent-sign-in");
    let _ = std::fs::create_dir_all(&dir);
    cmd.current_dir(dir);
    cmd
}

/// What a line of Antigravity's output says about its turn, if it's the
/// result: `Ok` for a success, else why not.
pub fn agy_result(line: &str) -> Option<Result<(), String>> {
    let start = line.find('{')?;
    let v: serde_json::Value = serde_json::from_str(line[start..].trim()).ok()?;
    if v["event"] != "result" {
        return None;
    }
    let r = &v["result"];
    if r["status"] == "SUCCESS" {
        Some(Ok(()))
    } else {
        Some(Err(r["error"]
            .as_str()
            .or(r["status"].as_str())
            .unwrap_or("it stopped")
            .to_string()))
    }
}

/// Start `cmd` in a console window of its own (Windows), for the person to
/// use: what it prints is there, not read here.
fn start_in_window(mut cmd: Command, kind: AgentKind) -> std::io::Result<Job> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        cmd.creation_flags(CREATE_NEW_CONSOLE);
    }
    let child = cmd.spawn()?;
    let (_tx, rx) = mpsc::channel();
    Ok(Job {
        kind,
        child,
        stdin: None,
        rx,
        lines: Vec::new(),
        url: None,
        code_sent: false,
    })
}

/// Start `cmd` in the background, its output read line by line.
fn start(
    mut cmd: Command,
    kind: AgentKind,
    with_input: bool,
    ctx: &egui::Context,
) -> std::io::Result<Job> {
    cmd.stdin(if with_input {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn()?;
    let (tx, rx) = mpsc::channel();
    if let Some(out) = child.stdout.take() {
        read_lines(out, tx.clone(), ctx.clone());
    }
    if let Some(err) = child.stderr.take() {
        read_lines(err, tx, ctx.clone());
    }
    let stdin = child.stdin.take();
    Ok(Job {
        kind,
        child,
        stdin,
        rx,
        lines: Vec::new(),
        url: None,
        code_sent: false,
    })
}

/// Send each line `from` prints (a prompt without a newline counts too),
/// with its terminal colours taken out.
fn read_lines(from: impl Read + Send + 'static, tx: Sender<String>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let mut r = BufReader::new(from);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match r.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let text = plain(&String::from_utf8_lossy(&buf));
                    for line in text.split('\r') {
                        if tx.send(line.trim_end().to_string()).is_err() {
                            return;
                        }
                    }
                    ctx.request_repaint();
                }
            }
        }
    });
}

/// `s` without terminal escape codes (colours, cursor moves).
pub fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // Up to the final letter.
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() || d == '~' {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// What to tell the person when signing in ended without it: in plain
/// words for what's known (the keychain), else the program's last words,
/// without the sign-in address and prompts it printed along the way.
pub fn sign_in_failure(lines: &[String]) -> String {
    let all = lines.join(" ").to_lowercase();
    if all.contains("keychain") {
        return "You signed in, but your Mac's keychain wouldn't save it. Unlock the keychain \
(Keychain Access, or restart your Mac), then click Sign In again."
            .into();
    }
    let said: Vec<String> = lines
        .iter()
        .map(|l| {
            // (Its words, less any address.)
            let words: Vec<&str> = l.split_whitespace().filter(|w| !w.contains("://")).collect();
            words.join(" ").trim_end_matches('>').trim().to_string()
        })
        .filter(|l| {
            let lower = l.to_lowercase();
            // (The prompts it printed while waiting.)
            !l.is_empty() && !lower.starts_with("paste") && !(lower.starts_with("visit") && lower.ends_with(':'))
        })
        .collect();
    let tail = said[said.len().saturating_sub(2)..].join(" ");
    if tail.is_empty() {
        "Signing in didn't finish. Click Sign In to try again.".into()
    } else {
        format!("Signing in didn't finish: {tail}")
    }
}

/// The sign-in page's address in a line of output: its first web address
/// that isn't the program's own local one.
pub fn sign_in_url(line: &str) -> Option<String> {
    line.split_whitespace()
        .filter_map(|w| w.find("https://").map(|i| &w[i..]))
        .map(|w| w.trim_end_matches(['.', ',', ')', '"', '\'']))
        .find(|w| w.len() > 12)
        .map(str::to_string)
}

/// Whether `kind` (at `program`) is signed in: `Some(false)` only when it
/// plainly says it isn't; `None` when that can't be told.
fn signed_in(kind: AgentKind, program: &PathBuf, path: Option<&str>) -> Option<bool> {
    let mut cmd = Command::new(program);
    if let Some(p) = path {
        cmd.env("PATH", p);
    }
    match kind {
        AgentKind::Codex => cmd.args(["login", "status"]),
        AgentKind::Claude => cmd.args(["auth", "status", "--json"]),
        AgentKind::Antigravity => cmd.arg("models"),
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok()?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    status_says(kind, out.status.success(), &text)
}

/// What a status command's output says (see [`signed_in`]).
pub fn status_says(kind: AgentKind, ok: bool, text: &str) -> Option<bool> {
    match kind {
        AgentKind::Codex => {
            if text.contains("Not logged in") {
                Some(false)
            } else if ok && text.contains("Logged in") {
                Some(true)
            } else {
                None
            }
        }
        AgentKind::Claude => {
            let start = text.find('{')?;
            let end = text.rfind('}')?;
            let v: serde_json::Value = serde_json::from_str(&text[start..=end]).ok()?;
            v["loggedIn"].as_bool()
        }
        AgentKind::Antigravity => {
            if text.contains("Please sign in") || text.contains("not logged in") {
                Some(false)
            } else if ok {
                Some(true)
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installers_last_line_reads_plainly() {
        assert_eq!(status_line("==> Installing standalone package"), "Installing standalone package");
        assert_eq!(status_line("  -> Downloading 2.1.330 "), "Downloading 2.1.330");
        assert_eq!(status_line("• Done"), "Done");
        assert_eq!(status_line("Codex 0.9 installed"), "Codex 0.9 installed");
        assert_eq!(status_line("==>"), "");
    }

    #[test]
    fn a_failed_sign_in_says_why_without_the_address() {
        let lines = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Claude Code, signed in but unable to keep it.
        let why = sign_in_failure(&lines(&[
            "Opening browser to sign in…",
            "https://claude.ai/oauth/authorize?code=true&client_id=x",
            "Couldn't log in: Couldn't save your login. If your Mac's keychain is locked, unlock it and log in again.",
            "Paste code here if prompted >",
        ]));
        assert!(why.contains("keychain") && !why.contains("://"), "{why}");
        // Anything else: its last words, the address and prompt left out.
        let why = sign_in_failure(&lines(&[
            "Visit this page to sign in:",
            "https://example.com/login?x=1",
            "Error: the server said no",
            "Paste code here if prompted >",
        ]));
        assert_eq!(why, "Signing in didn't finish: Error: the server said no");
        assert!(sign_in_failure(&[]).contains("try again"));
    }

    #[test]
    fn reads_whether_each_agent_is_signed_in() {
        assert_eq!(
            status_says(AgentKind::Codex, true, "Logged in using ChatGPT\n"),
            Some(true)
        );
        assert_eq!(
            status_says(AgentKind::Codex, false, "Not logged in\n"),
            Some(false)
        );
        // Anything else can't be told: the panel doesn't stand in the way.
        assert_eq!(
            status_says(AgentKind::Codex, false, "error: unknown command 'login'"),
            None
        );
        assert_eq!(
            status_says(
                AgentKind::Claude,
                true,
                "{\n  \"loggedIn\": false,\n  \"authMethod\": \"none\"\n}\n"
            ),
            Some(false)
        );
        assert_eq!(
            status_says(AgentKind::Claude, true, "{\"loggedIn\": true}"),
            Some(true)
        );
        assert_eq!(
            status_says(AgentKind::Claude, false, "error: unknown option '--json'"),
            None
        );
    }

    #[test]
    fn reads_whether_antigravity_is_signed_in() {
        // `agy models`, signed out (exit 1).
        assert_eq!(
            status_says(
                AgentKind::Antigravity,
                false,
                "Fetching available models...\nError: Please sign in to view available models. Launch the CLI without arguments to sign in.\n"
            ),
            Some(false)
        );
        assert_eq!(
            status_says(AgentKind::Antigravity, true, "Fetching available models...\ngemini-4-pro\n"),
            Some(true)
        );
        assert_eq!(
            status_says(AgentKind::Antigravity, false, "flags provided but not defined"),
            None
        );
    }

    #[test]
    fn reads_antigravitys_result() {
        assert_eq!(
            agy_result(r#"{"event":"result","result":{"conversation_id":"c1","status":"SUCCESS","response":"OK"}}"#),
            Some(Ok(()))
        );
        assert_eq!(
            agy_result(r#"{"event":"result","result":{"conversation_id":"","status":"ERROR","response":"","error":"authentication failed or timed out"}}"#),
            Some(Err("authentication failed or timed out".into()))
        );
        assert_eq!(agy_result("Waiting for authentication (timeout 60s)..."), None);
        assert_eq!(
            agy_result(r#"{"event":"step_update","step_update":{"step_index":1}}"#),
            None
        );
    }

    #[test]
    fn each_installer_is_its_makers() {
        let line = |kind| {
            let c = installer(kind);
            c.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let agy = line(AgentKind::Antigravity);
        if cfg!(windows) {
            assert!(agy.contains("irm https://antigravity.google/cli/install.ps1 | iex"), "{agy}");
        } else {
            assert!(agy.contains("curl -fsSL https://antigravity.google/cli/install.sh") && agy.ends_with("| bash"), "{agy}");
        }
        assert!(line(AgentKind::Claude).contains("claude.ai/install"));
        assert!(line(AgentKind::Codex).contains("chatgpt.com/codex/install"));
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_signs_in_from_a_terminal_of_its_own() {
        let c = agy_sign_in(Path::new("/home/pat/.local/bin/agy"));
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let all = format!("{} {}", c.get_program().to_string_lossy(), args.join(" "));
        assert!(all.contains("script"), "{all}");
        assert!(all.contains("/home/pat/.local/bin/agy"), "{all}");
        assert!(all.contains("stream-json") && all.contains("--sandbox"), "{all}");
        assert!(all.contains("command(*)"), "no shell: {all}");
        assert!(!all.contains("dangerously"), "{all}");
    }

    #[test]
    fn finds_the_sign_in_page_in_the_output() {
        assert_eq!(
            sign_in_url(
                "If the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&x=1"
            ),
            Some("https://claude.com/cai/oauth/authorize?code=true&x=1".into())
        );
        assert_eq!(
            sign_in_url("https://auth.openai.com/oauth/authorize?response_type=code&client_id=app"),
            Some("https://auth.openai.com/oauth/authorize?response_type=code&client_id=app".into())
        );
        assert_eq!(
            sign_in_url("Starting local login server on http://localhost:1455."),
            None
        );
        assert_eq!(plain("\u{1b}[32m✔ Installed\u{1b}[0m"), "✔ Installed");
    }

    /// A stand-in for an installer or a sign-in, run as one would be.
    #[cfg(unix)]
    fn run(script: &str, with_input: bool, input: Option<&str>) -> (Outcome, Setup) {
        let ctx = egui::Context::default();
        let mut s = Setup::default();
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", script]);
        let job = start(cmd, AgentKind::Claude, with_input, &ctx).unwrap();
        s.step = if with_input {
            Step::SigningIn(job)
        } else {
            Step::Installing(job)
        };
        if let Some(code) = input {
            s.send_code(code);
        }
        for _ in 0..200 {
            let o = s.poll(&ctx);
            if o != Outcome::Running {
                return (o, s);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("it never finished");
    }

    #[cfg(unix)]
    #[test]
    fn an_install_that_works_and_one_that_doesnt() {
        let (o, s) = run("echo downloading; echo done", false, None);
        assert_eq!(o, Outcome::Installed);
        assert!(s.installed_now);
        let (o, s) = run(
            "echo 'Downloading…'; echo 'curl: (6) Could not resolve host' >&2; exit 6",
            false,
            None,
        );
        assert_eq!(o, Outcome::Failed);
        match &s.step {
            Step::Failed { install: true, why, .. } => {
                assert!(why.contains("Could not resolve host"), "{why}")
            }
            _ => panic!("should say why"),
        }
    }

    /// The installer's own command line, with a download that fails: the
    /// install fails (in a pipe, the shell would run nothing and call it done).
    #[cfg(unix)]
    #[test]
    fn a_download_that_fails_is_an_install_that_failed() {
        let line = installer(AgentKind::Codex)
            .get_args()
            .nth(1)
            .map(|a| a.to_string_lossy().into_owned())
            .unwrap();
        // (A stand-in for curl with no network.)
        let line = line.replace("curl -fsSL https://chatgpt.com/codex/install.sh", "sh -c 'echo \"curl: (6) Could not resolve host\" >&2; exit 6'");
        assert!(line.contains("exit 6"), "{line}");
        let (o, s) = run(&line, false, None);
        assert_eq!(o, Outcome::Failed);
        assert!(matches!(&s.step, Step::Failed { install: true, why, .. } if why.contains("Could not resolve host")));
    }

    #[cfg(unix)]
    #[test]
    fn signing_in_with_a_pasted_code() {
        // Like `claude auth login` when the page shows a code: it reads it.
        let (o, s) = run(
            "echo 'visit: https://claude.com/cai/oauth/authorize?code=true'; printf 'Paste code here if prompted > '; read c; [ \"$c\" = abc123 ]",
            true,
            Some("  abc123 "),
        );
        assert_eq!(o, Outcome::SignedIn);
        assert_eq!(s.signed_in[AgentKind::Claude.index()], Some(true));
    }

    /// A stand-in for Antigravity signing in: the page's address, then it
    /// reads the code and gives its result (and, as it can, doesn't exit).
    #[cfg(unix)]
    fn agy_sign_in_with(code: &str) -> (Outcome, Setup) {
        let ctx = egui::Context::default();
        let mut s = Setup::default();
        let mut cmd = Command::new("/bin/sh");
        cmd.args([
            "-c",
            "echo 'Authentication required. Please visit the URL to log in:'; \
             echo '  https://accounts.google.com/o/oauth2/auth?access_type=offline&client_id=x'; \
             printf 'Or, paste the authorization code here and press Enter:\n'; read c; \
             if [ \"$c\" = good ]; then echo '{\"event\":\"result\",\"result\":{\"conversation_id\":\"c1\",\"status\":\"SUCCESS\",\"response\":\"OK\"}}'; \
             else echo 'Error: authentication failed: token exchange failed: oauth2: \"invalid_grant\"'; \
             echo '{\"event\":\"result\",\"result\":{\"conversation_id\":\"\",\"status\":\"ERROR\",\"error\":\"authentication failed or timed out\"}}'; fi; \
             sleep 30",
        ]);
        s.step = Step::SigningIn(start(cmd, AgentKind::Antigravity, true, &ctx).unwrap());
        let t = std::time::Instant::now();
        loop {
            let o = s.poll(&ctx);
            if o != Outcome::Running {
                return (o, s);
            }
            if let Step::SigningIn(j) = &s.step
                && j.url.is_some()
                && !j.code_sent
            {
                // The page was opened for the person.
                let opened = ctx.output(|o| {
                    o.commands.iter().any(|c| {
                        matches!(c, egui::OutputCommand::OpenUrl(u) if u.url.starts_with("https://accounts.google.com/"))
                    })
                });
                assert!(opened, "opens the page");
                s.send_code(code);
            }
            assert!(t.elapsed() < Duration::from_secs(10), "never finished");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_signs_in_with_a_pasted_code() {
        let (o, s) = agy_sign_in_with("good");
        assert_eq!(o, Outcome::SignedIn, "done at its result, running or not");
        assert_eq!(s.signed_in[AgentKind::Antigravity.index()], Some(true));
        let (o, s) = agy_sign_in_with("bad");
        assert_eq!(o, Outcome::Failed);
        match &s.step {
            Step::Failed { install: false, why, .. } => assert!(why.contains("didn't accept that code"), "{why}"),
            _ => panic!("says why"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_stops_it() {
        let ctx = egui::Context::default();
        let mut s = Setup::default();
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "sleep 30"]);
        s.step = Step::SigningIn(start(cmd, AgentKind::Codex, true, &ctx).unwrap());
        let pid = match &s.step {
            Step::SigningIn(j) => j.child.id(),
            _ => unreachable!(),
        };
        s.cancel();
        assert!(matches!(s.step, Step::Idle));
        // (The process is gone: kill -0 fails.)
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .unwrap()
            .success();
        assert!(!alive, "the sign-in was stopped");
    }
}

/// The real thing, on this computer: each maker's installer into a home of
/// its own, then the start of signing in. Downloads both programs, so it's
/// run by hand:
/// `ZIGZAG_REAL_INSTALL=1 cargo test --features gui --bin zigzag-studio real_install -- --ignored --nocapture`
#[cfg(all(test, unix))]
mod real {
    use super::*;

    #[test]
    #[ignore]
    fn real_install_and_sign_in() {
        if std::env::var_os("ZIGZAG_REAL_INSTALL").is_none() {
            return;
        }
        let home = std::env::temp_dir().join(format!("zigzag-real-install-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        // (The installers and the programs find their home here.)
        // SAFETY: this test runs alone (`--ignored`, by hand).
        unsafe { std::env::set_var("HOME", &home) };
        let ctx = egui::Context::default();
        for kind in AgentKind::ALL {
            let mut s = Setup::default();
            s.install(kind, &ctx);
            let t = std::time::Instant::now();
            let outcome = loop {
                let o = s.poll(&ctx);
                if o != Outcome::Running {
                    break o;
                }
                assert!(
                    t.elapsed() < Duration::from_secs(600),
                    "{kind:?} took too long"
                );
                std::thread::sleep(Duration::from_millis(200));
            };
            if let Step::Failed { why, .. } = &s.step {
                panic!("{kind:?}: {why}");
            }
            assert_eq!(outcome, Outcome::Installed, "{kind:?}");
            let program =
                crate::agent::locate_for_test(kind, None).expect("found where ZigZag looks");
            eprintln!("{kind:?} installed at {}", program.display());
            assert!(program.starts_with(&home), "{}", program.display());
            assert_eq!(
                signed_in(kind, &program, None),
                Some(false),
                "{kind:?}: nobody's signed in yet"
            );
        }
        // Signing in to Claude Code: its page's address comes through, and
        // it waits for the browser. (Codex opens a real browser; not here.)
        let fake = home.join("browser");
        std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
        Command::new("chmod")
            .args(["+x"])
            .arg(&fake)
            .status()
            .unwrap();
        unsafe { std::env::set_var("BROWSER", &fake) };
        let claude = crate::agent::locate_for_test(AgentKind::Claude, None).unwrap();
        let mut s = Setup::default();
        s.sign_in(AgentKind::Claude, &claude, None, &ctx);
        let t = std::time::Instant::now();
        loop {
            assert_eq!(
                s.poll(&ctx),
                Outcome::Running,
                "still waiting for the browser"
            );
            if let Step::SigningIn(j) = &s.step
                && j.url.is_some()
            {
                eprintln!("sign-in page: {}…", &j.url.as_ref().unwrap()[..60]);
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(30), "no sign-in page");
            std::thread::sleep(Duration::from_millis(100));
        }
        s.cancel();
        // Signing in to Antigravity: its page's address comes through (and
        // the panel would open it), then it waits for the code.
        let agy = crate::agent::locate_for_test(AgentKind::Antigravity, None).unwrap();
        let mut s = Setup::default();
        s.sign_in(AgentKind::Antigravity, &agy, None, &ctx);
        let t = std::time::Instant::now();
        loop {
            assert_eq!(s.poll(&ctx), Outcome::Running, "waiting for the code");
            if let Step::SigningIn(j) = &s.step
                && let Some(url) = &j.url
            {
                eprintln!("Antigravity's sign-in page: {}…", &url[..60]);
                assert!(url.starts_with("https://accounts.google.com/"), "{url}");
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(30), "no sign-in page");
            std::thread::sleep(Duration::from_millis(100));
        }
        let pid = match &s.step {
            Step::SigningIn(j) => j.child.id(),
            _ => unreachable!(),
        };
        s.cancel();
        std::thread::sleep(Duration::from_millis(500));
        // (Nothing of it is left running: `script` and `agy` both gone.)
        let left = Command::new("pgrep")
            .args(["-f", &agy.to_string_lossy()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        assert!(left.is_empty(), "still running after {pid}: {left}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
