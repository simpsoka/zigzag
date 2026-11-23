//! Prompt an AI agent from inside the app (like Paper's in-app agent): the
//! Claude Code or Codex CLI runs in the background, connected to this
//! studio's MCP server, so its tool calls edit the open design live. It
//! lives in the Agent panel (docked, floating or tabbed like any panel); the
//! transcript shows what it said and each step, with a spinner on the one
//! running, and the canvas marks the objects it's working on.

use crate::app::Studio;
use crate::icons::{self, Icon};
use crate::theme::{pal_ui, space};
use eframe::egui::{self, Color32, Key, Rect, Sense, Stroke, Vec2};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The platform the studio runs on, for install steps and wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Os {
    Mac,
    Windows,
    /// Linux and other Unixes.
    Linux,
}

impl Os {
    const CURRENT: Os = if cfg!(target_os = "macos") {
        Os::Mac
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    };

    /// Where to run a command: the Mac's Terminal app, or any terminal.
    fn terminal(self) -> &'static str {
        match self {
            Os::Mac => "Terminal",
            Os::Windows | Os::Linux => "a terminal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AgentKind {
    Claude,
    #[default]
    Codex,
    /// Google's Antigravity CLI (`agy`).
    Antigravity,
}

impl AgentKind {
    /// Every one, in menu order.
    pub const ALL: [AgentKind; 3] = [AgentKind::Codex, AgentKind::Claude, AgentKind::Antigravity];
    /// How many there are (the size of the arrays kept for each).
    pub const COUNT: usize = 3;

    pub(crate) fn index(self) -> usize {
        self as usize
    }

    /// How to install its command-line tool on this computer: (how, command)
    /// pairs, the first recommended, then how to sign in.
    pub fn install_steps(self) -> (&'static [(&'static str, &'static str)], &'static str) {
        self.install_steps_on(Os::CURRENT)
    }

    /// How to install its command-line tool on `os`.
    fn install_steps_on(self, os: Os) -> (&'static [(&'static str, &'static str)], &'static str) {
        match (os, self) {
            (Os::Mac, AgentKind::Codex) => (
                &[
                    (
                        "Recommended",
                        "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
                    ),
                    ("Or with Homebrew", "brew install --cask codex"),
                ],
                "Then run `codex` once in Terminal and choose Sign in with ChatGPT.",
            ),
            (Os::Mac, AgentKind::Claude) => (
                &[
                    (
                        "Recommended",
                        "curl -fsSL https://claude.ai/install.sh | bash",
                    ),
                    ("Or with Homebrew", "brew install --cask claude-code"),
                ],
                "Then run `claude` once in Terminal to sign in.",
            ),
            (Os::Windows, AgentKind::Codex) => (
                &[
                    (
                        "Recommended (PowerShell)",
                        "powershell -ExecutionPolicy ByPass -c \"irm https://chatgpt.com/codex/install.ps1 | iex\"",
                    ),
                    ("Or with npm", "npm install -g @openai/codex"),
                ],
                "Then run `codex` once in a terminal and choose Sign in with ChatGPT.",
            ),
            (Os::Linux, AgentKind::Codex) => (
                &[
                    (
                        "Recommended",
                        "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
                    ),
                    ("Or with npm", "npm install -g @openai/codex"),
                ],
                "Then run `codex` once in a terminal and choose Sign in with ChatGPT.",
            ),
            (Os::Windows, AgentKind::Claude) => (
                &[
                    (
                        "Recommended (PowerShell)",
                        "irm https://claude.ai/install.ps1 | iex",
                    ),
                    ("Or with WinGet", "winget install Anthropic.ClaudeCode"),
                ],
                "Then run `claude` once in a terminal to sign in.",
            ),
            (Os::Linux, AgentKind::Claude) => (
                &[
                    (
                        "Recommended",
                        "curl -fsSL https://claude.ai/install.sh | bash",
                    ),
                    ("Or with npm", "npm install -g @anthropic-ai/claude-code"),
                ],
                "Then run `claude` once in a terminal to sign in.",
            ),
            (Os::Mac, AgentKind::Antigravity) => (
                &[(
                    "Recommended",
                    "curl -fsSL https://antigravity.google/cli/install.sh | bash",
                )],
                "Then run `agy` once in Terminal to sign in with your Google account.",
            ),
            (Os::Linux, AgentKind::Antigravity) => (
                &[(
                    "Recommended",
                    "curl -fsSL https://antigravity.google/cli/install.sh | bash",
                )],
                "Then run `agy` once in a terminal to sign in with your Google account.",
            ),
            (Os::Windows, AgentKind::Antigravity) => (
                &[(
                    "Recommended (PowerShell)",
                    "irm https://antigravity.google/cli/install.ps1 | iex",
                )],
                "Then run `agy` once in a terminal to sign in with your Google account.",
            ),
        }
    }

    /// Said in the transcript when a prompt is sent but its tool is missing.
    fn not_installed_message(self, os: Os) -> String {
        let (ways, sign_in) = self.install_steps_on(os);
        format!(
            "{} isn't installed (no `{}` command found). Click Install {} below, or install it in {} with `{}`{}. {sign_in} Then try again.",
            self.name(),
            self.program(),
            self.name(),
            os.terminal(),
            ways[0].1,
            ways.get(1)
                .map(|w| format!(" (or `{}`)", w.1))
                .unwrap_or_default(),
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "Codex",
            AgentKind::Antigravity => "Antigravity",
        }
    }
    /// Its maker's mark, for the panel's setup.
    pub fn brand(self) -> crate::brands::Brand {
        match self {
            AgentKind::Claude => crate::brands::Brand::Anthropic,
            AgentKind::Codex => crate::brands::Brand::OpenAi,
            AgentKind::Antigravity => crate::brands::Brand::Google,
        }
    }
    /// Who makes it, for "…'s installer".
    fn maker(self) -> &'static str {
        match self {
            AgentKind::Claude => "Anthropic",
            AgentKind::Codex => "OpenAI",
            AgentKind::Antigravity => "Google",
        }
    }
    /// The account it signs in with.
    fn account(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude",
            AgentKind::Codex => "ChatGPT",
            AgentKind::Antigravity => "Google",
        }
    }
    fn program(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Antigravity => "agy",
        }
    }
    /// The agent a key ([`AgentKind::key`]) names.
    pub fn from_key(k: &str) -> Option<AgentKind> {
        AgentKind::ALL.into_iter().find(|a| a.key() == k)
    }

    pub fn key(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Antigravity => "antigravity",
        }
    }

    /// Its sign-in page can show a code to paste back into the program
    /// (where the panel can pass it on: Antigravity signs in from a window
    /// of its own on Windows).
    pub fn pastes_code(self) -> bool {
        match self {
            AgentKind::Claude => true,
            AgentKind::Antigravity => !cfg!(windows),
            AgentKind::Codex => false,
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
    /// The notes pinned on the canvas that went with the prompt above.
    Notes(Vec<String>),
    /// What a slash command did (or why it couldn't), in a quiet line.
    Info(String),
    /// Images that went with the prompt above (kept files).
    Images(Vec<PathBuf>),
    /// Optional changes the agent offered (`offer_choices`): ticked and
    /// applied by the person. `applied`: what was, once it's been (an
    /// earlier chat's card comes back closed, with none).
    Choices {
        title: String,
        /// What the person is asked ("Which of these should I apply?").
        question: String,
        choices: Vec<Choice>,
        applied: Option<Vec<String>>,
    },
}

/// One of the changes on a card of choices.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    /// What it gains and what it costs.
    pub detail: String,
    /// The tool calls that make it ([{tool, arguments}]).
    pub steps: Value,
    pub picked: bool,
}

/// Something the agent process reported.
enum Event {
    Line(String),
    Stderr(String),
    /// Everything it printed has been read.
    Eof,
    Exit(Option<i32>),
}

struct Run {
    child: Arc<Mutex<Child>>,
    rx: Receiver<Event>,
    stderr: Vec<String>,
    /// How it ended, and when that was heard (its last lines may still be
    /// on their way).
    exit: Option<(Option<i32>, Instant)>,
    /// Its output has been read to the end.
    eof: bool,
    /// Which agent it is (the one in use may be changed while it runs,
    /// from another document).
    kind: AgentKind,
    /// It was started to carry on an earlier session.
    resumed: bool,
    /// What Antigravity has said so far, in pieces (see [`AgyStream`]).
    agy: AgyStream,
    /// When its final result came in, and whether that was a success: an
    /// Antigravity that doesn't exit after it is stopped (see `poll`).
    result_at: Option<(Instant, bool)>,
}

/// An agent's words for "there's no such session to carry on".
fn session_gone(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    let about = ["session", "conversation", "thread", "rollout", "resume"]
        .iter()
        .any(|w| m.contains(w));
    let missing = [
        "not found",
        "no conversation found",
        "no rollout",
        "does not exist",
        "no such",
        "could not find",
        "couldn't find",
    ]
    .iter()
    .any(|w| m.contains(w));
    about && missing
}

/// Ideas for a new design, for an empty chat to start from.
const NEW_IDEAS: &[&str] = &[
    "A round camp badge with a pine tree",
    "A cat curled up asleep, for a pocket",
    "A wildflower bouquet tied with a ribbon",
    "A mountain sunset patch with a stitched border",
    "My initials in a monogram with a laurel wreath",
    "A bee on a honeycomb",
    "A retro surf patch: “Catch a Wave”",
    "A little whale for a baby's onesie",
    "A coffee cup with steam swirls",
    "A crescent moon with stars",
    "A cactus in a terracotta pot",
    "A mushroom cluster with tiny leaves",
    "A lemon slice with leaves",
    "A paper airplane with a dashed trail",
    "A strawberry with a smiling face",
    "A lighthouse on rocks, for a hat",
    "A dog's name in a bone-shaped badge",
    "A rainbow over two clouds",
    "A fox sitting in tall grass",
    "A bicycle with a basket of flowers",
];

/// Ideas for a change to the design that's open.
const CHANGE_IDEAS: &[&str] = &[
    "Make the lettering satin",
    "Simplify this for a 4x4 hoop",
    "Cut down the thread changes",
    "Add a satin border around it",
    "Make it sew faster",
    "Try a warmer color palette",
    "Fewer trims, please",
    "Make it work on a stretchy T-shirt",
    "Make it half the size",
    "Check it for problems before I sew",
];

/// Three ideas from `seed`, none twice: for a design that's open, two new
/// designs and a change to it; for an empty one, three new designs.
fn ideas(seed: u64, has_design: bool) -> Vec<&'static str> {
    // (splitmix64: a few good numbers from one.)
    let mut x = seed;
    let mut next = move || {
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let mut pick = |from: &[&'static str], n: usize, out: &mut Vec<&'static str>| {
        let mut left: Vec<&'static str> = from.to_vec();
        for _ in 0..n.min(left.len()) {
            let k = (next() % left.len() as u64) as usize;
            out.push(left.swap_remove(k));
        }
    };
    let mut out = Vec::with_capacity(3);
    if has_design {
        pick(NEW_IDEAS, 2, &mut out);
        pick(CHANGE_IDEAS, 1, &mut out);
    } else {
        pick(NEW_IDEAS, 3, &mut out);
    }
    out
}

/// A seed that's different each time it's asked for (the standard
/// library's hasher is keyed at random).
fn fresh_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    h.finish()
}

/// How long Antigravity has to exit once it has given its result.
const AGY_LINGER: Duration = Duration::from_secs(3);

/// How long to wait for the last of its output once it has ended (something
/// it started may be holding the pipe open).
const LAST_WORDS: Duration = Duration::from_secs(2);

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
    /// The runs of steps opened out in the transcript (by the log index of
    /// their first step); the rest show as one line each.
    pub open_steps: std::collections::HashSet<usize>,
    /// The user's login-shell PATH (GUI apps on macOS don't get it).
    path: Option<Option<String>>,
    pub(crate) config: Option<PathBuf>,
    /// Where each agent's program is (by [`AgentKind::index`]): `None`
    /// until looked up, then `Some(None)` if it isn't installed.
    pub found: [Option<Option<PathBuf>>; AgentKind::COUNT],
    /// Installing it or signing in to it from the panel (see `agent_setup`).
    pub setup: crate::agent_setup::Setup,
    /// The Setup page chosen from the agent menu (it also shows by itself
    /// while the chosen agent isn't installed).
    pub on_setup: bool,
    /// The first look for the programs has settled which agent the panel
    /// opens on (see `open_on_an_installed_agent`).
    settled: bool,
    /// Why the studio's MCP server isn't running, when it isn't: what a
    /// prompt sent without it says.
    pub mcp_off: Option<String>,
    /// Images being made ready to attach, each for the prompt of the
    /// document (a recovery number) it was dropped or pasted in.
    pub(crate) attaching: Vec<(u32, Receiver<Result<PathBuf, String>>)>,
    /// The document its calls go to (a recovery number), once it has
    /// opened or switched to one itself while working.
    pub follow: Option<u32>,
    /// Which ideas an empty chat shows (see [`ideas`]): chosen when it's
    /// first shown, kept until the chat has something in it.
    pub ideas_seed: Option<u64>,
    /// Stop was pressed (or the chat cleared) since the last frame: what
    /// was waiting to go to the agent next doesn't.
    pub stopped: bool,
    /// Messages written while it worked, sent together when its turn ends
    /// (or put back in the prompt box, if it's stopped).
    pub queued: Vec<String>,
    /// It was working last frame (this chat's), to tell when a turn ends.
    was_running: bool,
    /// A turn ended while the panel was put away: the tool strip's Agent
    /// button shows a dot until it's opened.
    pub unread: bool,
    /// A lookup running off the UI thread: (login PATH, what was found).
    lookup: Option<Receiver<(Option<String>, [Option<PathBuf>; AgentKind::COUNT])>>,
    /// The model chosen for each agent (by [`AgentKind::index`]); `None`
    /// is the agent's own default.
    pub model: [Option<String>; AgentKind::COUNT],
    /// How hard each agent thinks; `None` is its own default.
    pub effort: [Option<String>; AgentKind::COUNT],
    /// Codex's models, as `codex debug models` lists them (once looked up).
    /// What Codex says about itself (its models, the person's tool
    /// servers), looked up off the UI thread once it's found.
    codex_info: Option<CodexInfo>,
    codex_rx: Option<Receiver<CodexInfo>>,
    /// The slash command menu: the row picked, the prompt it was for, and
    /// the prompt it was closed on (Esc) until that changes.
    pub menu_sel: usize,
    pub menu_for: String,
    pub menu_closed: Option<String>,
    /// The prompt of the turn running (what it makes is kept with it).
    pub turn_prompt: Option<String>,
    /// The agent `session` is with (a chat is continued only by that one).
    session_kind: Option<AgentKind>,
    /// Kept chats (see `history`).
    pub store: crate::history::Store,
    /// The id the document's chats are kept under, once known.
    pub doc_key: Option<String>,
    /// The kept chat this one is, and how many of its lines are written.
    pub hist_id: Option<String>,
    pub hist_saved: usize,
    /// The session as last written to the index.
    pub hist_session: Option<String>,
    /// Its document's kept chats have been looked for.
    pub hist_checked: bool,
    /// The document's last chat, too old to come back by itself: offered.
    pub earlier: Option<crate::history::Meta>,
    /// Its turns before the last few are shown (else folded into a line).
    pub show_earlier: bool,
    /// Images attached to the prompt being written (kept files).
    pub attach: Vec<PathBuf>,
    /// The images going with the prompt being sent (for its command line).
    sending_images: Vec<PathBuf>,
    /// A prompt sent before what starting needs was looked up: it goes
    /// once that's in (shown as working meanwhile).
    deferred: Option<Deferred>,
    /// Where the panel was last frame (an image dropped there is attached).
    pub panel_rect: Option<Rect>,
}

/// A prompt waiting to start (see `AgentState::deferred`).
struct Deferred {
    addr: std::net::SocketAddr,
    text: String,
    images: Vec<PathBuf>,
    ctx: egui::Context,
}

/// One document's conversation with the agent, parked with its tab while
/// another is in front (see [`AgentState::swap_chat`]): what was said, the
/// session to continue, a prompt being written, and a run still going.
#[derive(Default)]
pub struct Chat {
    prompt: String,
    log: Vec<Entry>,
    session: Option<String>,
    session_kind: Option<AgentKind>,
    run: Option<Run>,
    started: Option<Instant>,
    work: Work,
    calls: std::collections::HashMap<String, usize>,
    open_steps: std::collections::HashSet<usize>,
    turn_prompt: Option<String>,
    deferred: Option<Deferred>,
    doc_key: Option<String>,
    hist_id: Option<String>,
    hist_saved: usize,
    hist_session: Option<String>,
    hist_checked: bool,
    earlier: Option<crate::history::Meta>,
    show_earlier: bool,
    attach: Vec<PathBuf>,
    queued: Vec<String>,
    was_running: bool,
}

impl Chat {
    /// An image attached while its document wasn't in front.
    pub(crate) fn attach_later(&mut self, path: PathBuf) {
        self.attach.push(path);
    }

    /// The kept chat it's showing, if it is one.
    pub fn kept_id(&self) -> Option<&str> {
        self.hist_id.as_deref()
    }

    /// History was cleared: it isn't a kept chat any more. What's on
    /// screen stays but isn't written again; what's said after is kept.
    pub fn forget_kept(&mut self) {
        self.hist_id = None;
        self.hist_saved = self.log.len();
        self.earlier = None;
    }

    /// Its agent is working (or about to).
    pub fn running(&self) -> bool {
        self.run.is_some() || self.deferred.is_some()
    }
}

/// A model an agent can run on.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelInfo {
    /// What its command line is given.
    pub id: String,
    pub name: String,
    pub about: String,
    /// The efforts it takes, least first.
    pub efforts: Vec<String>,
}

/// Claude Code's efforts (`--effort`).
const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// The efforts any Codex model takes (what's offered while its models,
/// and theirs, aren't known).
const CODEX_EFFORTS: [&str; 3] = ["low", "medium", "high"];
/// Antigravity's efforts (`agy --effort`).
const AGY_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Claude Code's model aliases: each the latest of its family.
const CLAUDE_MODELS: [(&str, &str, &str); 4] = [
    ("fable", "Fable", "The latest Fable model"),
    ("opus", "Opus", "The latest Opus model"),
    ("sonnet", "Sonnet", "The latest Sonnet model"),
    ("haiku", "Haiku", "The latest Haiku model: the quickest"),
];

/// What Codex says about itself.
#[derive(Clone, Debug, Default)]
struct CodexInfo {
    models: Vec<ModelInfo>,
    /// The tool (MCP) servers it would start, from the person's own setup:
    /// switched off for the panel's runs.
    servers: Vec<String>,
}

/// The tool servers in `codex mcp list --json`, the ones switched on,
/// by names that can be given on its command line.
pub fn parse_codex_servers(json: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    v.as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["enabled"].as_bool() != Some(false))
        .filter_map(|s| s["name"].as_str())
        .filter(|n| {
            !n.is_empty()
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
        .map(str::to_string)
        .collect()
}

/// The models in `codex debug models`'s JSON, those it lists for people.
pub fn parse_codex_models(json: &str) -> Vec<ModelInfo> {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"].as_str().is_none_or(|x| x == "list"))
        .filter_map(|m| {
            let id = m["slug"].as_str()?.to_string();
            Some(ModelInfo {
                name: m["display_name"].as_str().unwrap_or(&id).to_string(),
                about: m["description"].as_str().unwrap_or("").to_string(),
                efforts: m["supported_reasoning_levels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e["effort"].as_str().map(str::to_string))
                    .collect(),
                id,
            })
        })
        .collect()
}

/// A prompt with images, as a line of Claude Code's streaming input: the
/// text, then each image (PNG) inline.
pub fn claude_message(text: &str, images: &[PathBuf]) -> String {
    use base64::Engine;
    let mut content = vec![serde_json::json!({ "type": "text", "text": text })];
    for p in images {
        if let Ok(bytes) = std::fs::read(p) {
            content.push(serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/png",
                    "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                },
            }));
        }
    }
    let msg = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": content },
    });
    format!("{msg}\n")
}

/// End an agent's run and everything it started. The command it's run as
/// may only launch the program that does the work (npm's `codex` is a Node
/// script that starts Codex itself): ending just that would leave the work
/// going, still connected to the studio and still holding its chat. So the
/// run has a process group of its own (see `send_with`), and the whole
/// group is asked to end, then made to a moment later. On Windows its
/// process tree is ended.
fn end_run(child: &mut Child) {
    let pid = child.id();
    #[cfg(unix)]
    {
        fn signal(group: &str, sig: &str) {
            let _ = Command::new("kill")
                .args([sig, "--", group])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let group = format!("-{pid}");
        signal(&group, "-TERM");
        let _ = child.kill();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            signal(&group, "-KILL");
        });
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x0800_0000;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, child.kill());
    }
}

/// A card of choices (see [`Entry::Choices`]): the question asked, then a
/// numbered row for each change (what it gains and costs under it),
/// divided by lines. A click on a row is the choice, made at once (not
/// while the agent works). Once made (or from an earlier chat), the one
/// chosen keeps its mark and the rest fade. True when one is clicked (it's
/// the one `picked`).
fn choices_card(ui: &mut egui::Ui, e: &mut Entry, busy: bool) -> bool {
    let Entry::Choices {
        title,
        question,
        choices,
        applied,
    } = e
    else {
        return false;
    };
    let p = pal_ui(ui);
    let open = applied.is_none();
    let mut apply = false;
    // One change on offer (and the way out): a question, and a button for
    // each answer.
    let keep = |c: &Choice| c.steps.as_array().is_none_or(|s| s.is_empty());
    if choices.len() == 2 && keep(&choices[1]) && !keep(&choices[0]) {
        egui::Frame::new()
            .fill(p.hover)
            .stroke(Stroke::new(1.0, p.separator))
            .corner_radius(10)
            .inner_margin(egui::Margin::same(space::M as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let heading = if question.is_empty() {
                    title.as_str()
                } else {
                    question.as_str()
                };
                clipped(ui, heading, PANEL_PT, p.text, 2);
                if !choices[0].detail.is_empty() {
                    ui.add_space(2.0);
                    clipped(ui, &choices[0].detail, PANEL_PT - 1.0, p.secondary, 2);
                }
                ui.add_space(space::S);
                match applied {
                    Some(done) if done.is_empty() => {
                        quiet(ui, "Offered earlier.");
                    }
                    Some(done) if done.first() == Some(&choices[1].label) => {
                        quiet(ui, "Kept it as it is.");
                    }
                    Some(done) => {
                        quiet(ui, format!("Done: {}.", done.join(", ")));
                    }
                    None => {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = space::S;
                            let button =
                                |ui: &mut egui::Ui, label: &str, fill: Color32, ink: Color32| {
                                    let b = ui.add_enabled(
                                        !busy,
                                        egui::Button::new(
                                            egui::RichText::new(label).size(PANEL_PT).color(ink),
                                        )
                                        .fill(fill)
                                        .corner_radius(8)
                                        .min_size(Vec2::new(0.0, 30.0)),
                                    );
                                    b.on_disabled_hover_text("Once the agent is done").clicked()
                                };
                            let yes = choices[0].label.clone();
                            if button(ui, &yes, p.text, p.dock) {
                                choices[0].picked = true;
                                apply = true;
                            }
                            if button(ui, "Not now", p.dock, p.text) {
                                choices[1].picked = true;
                                apply = true;
                            }
                        });
                    }
                }
            });
        return apply;
    }
    egui::Frame::new()
        .fill(p.hover)
        .stroke(Stroke::new(1.0, p.separator))
        .corner_radius(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            let w = ui.available_width();
            // The question, as the heading.
            egui::Frame::new()
                .inner_margin(egui::Margin::same(space::M as i8))
                .show(ui, |ui| {
                    ui.set_width(w - 2.0 * space::M);
                    let heading = if question.is_empty() {
                        title.as_str()
                    } else {
                        question.as_str()
                    };
                    clipped(ui, heading, PANEL_PT, p.text, 2);
                });
            // A row for each change.
            let done: Vec<String> = applied.clone().unwrap_or_default();
            for (k, c) in choices.iter_mut().enumerate() {
                let line = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover()).0;
                ui.painter().hline(
                    line.x_range(),
                    line.center().y,
                    Stroke::new(1.0, p.separator),
                );
                let marked = if open {
                    c.picked
                } else {
                    done.contains(&c.label)
                };
                let faded = !open && !marked;
                let bg = ui.painter().add(egui::Shape::Noop);
                let row = ui
                    .scope_builder(
                        egui::UiBuilder::new().sense(if open && !busy {
                            Sense::click()
                        } else {
                            Sense::hover()
                        }),
                        |ui| {
                            egui::Frame::new()
                                .inner_margin(egui::Margin {
                                    left: (space::M + BADGE + 12.0) as i8,
                                    right: (space::M + 22.0) as i8,
                                    top: (space::S + 3.0) as i8,
                                    bottom: (space::S + 3.0) as i8,
                                })
                                .show(ui, |ui| {
                                    ui.set_width(w - 2.0 * space::M - BADGE - 12.0 - 22.0);
                                    let ink =
                                        |c: Color32| if faded { c.gamma_multiply(0.5) } else { c };
                                    clipped(ui, &c.label, PANEL_PT, ink(p.text), 1);
                                    if !c.detail.is_empty() {
                                        ui.add_space(2.0);
                                        clipped(ui, &c.detail, PANEL_PT - 1.0, ink(p.secondary), 2);
                                    }
                                });
                        },
                    )
                    .response;
                row.widget_info(|| {
                    egui::WidgetInfo::selected(egui::Role::Button, open, marked, &c.label)
                });
                let r = row.rect;
                if marked || (open && !busy && row.hovered()) {
                    let fill = if marked {
                        p.accent.gamma_multiply(0.18)
                    } else {
                        p.dock.gamma_multiply(0.6)
                    };
                    ui.painter().set(bg, egui::Shape::rect_filled(r, 0.0, fill));
                }
                // Its number, in a badge: filled when it's chosen.
                let badge = Rect::from_min_size(
                    egui::pos2(r.left() + space::M, r.center().y - BADGE / 2.0),
                    Vec2::splat(BADGE),
                );
                let (fill, ink) = if marked && !faded {
                    (p.accent, Color32::WHITE)
                } else if faded {
                    (p.dock, p.secondary.gamma_multiply(0.6))
                } else {
                    (p.dock, p.secondary)
                };
                ui.painter().rect_filled(badge, 6.0, fill);
                ui.painter().text(
                    badge.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{}", k + 1),
                    egui::FontId::proportional(PANEL_PT - 1.0),
                    ink,
                );
                if marked {
                    icons::paint(
                        ui.painter(),
                        Rect::from_center_size(
                            egui::pos2(r.right() - space::M - 6.0, r.center().y),
                            Vec2::splat(13.0),
                        ),
                        Icon::Check,
                        if faded { p.secondary } else { p.accent },
                    );
                }
                // A click is the choice: made at once.
                if open && busy {
                    row.on_hover_text("Once the agent is done");
                } else if open
                    && row
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                {
                    c.picked = true;
                    apply = true;
                }
            }
        });
    apply
}

/// Text wrapped to the room there is, `rows` lines at most (cut with an
/// ellipsis, the whole of it on hover): a card of choices keeps short.
fn clipped(ui: &mut egui::Ui, text: &str, size: f32, color: Color32, rows: usize) {
    let mut job = egui::text::LayoutJob::simple(
        text.to_string(),
        egui::FontId::proportional(size),
        color,
        ui.available_width(),
    );
    job.wrap.max_rows = rows;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    let galley = ui.painter().layout_job(job);
    let cut = galley.elided;
    let r = ui.add(egui::Label::new(galley).selectable(false));
    if cut {
        r.on_hover_text(text);
    }
}

/// A choice's number badge, points square.
const BADGE: f32 = 22.0;

/// The turns a chat shows; earlier ones fold into a line.
const SHOWN_TURNS: usize = 3;

/// Extra instructions for the agent, on top of the MCP server's own.
const BRIEF: &str = "You are the design agent inside ZigZag, a machine-embroidery editor. \
The person is watching the canvas while you work. Make the design they ask for on the open document \
using the ZigZag MCP tools (get_design first; add_svg for artwork you write as SVG, add_text for lettering, \
transform_objects to size and place, set_object for stitch types and thread colors). Keep everything inside the hoop. \
Draw with as much detail as the subject needs and the size allows: thread sets minimums, not a style. \
A filled detail needs to be at least about 1.5 mm across and a satin column 1 to 7 mm wide; finer lines \
(features, creases, outlines) are thin strokes, which sew as running stitch. A gradient sews as one thread color, \
so shade with separate shapes in a few related colors instead. Build a drawing in layers as an illustrator would: \
base shapes, then shading and highlights laid on top, then lines; trace curves with real bezier curves, not a few \
straight points. If a subject needs more detail than its size can hold, make it bigger rather than leave the detail out. \
Sew it as an embroiderer would: keep underlay and the fabric's usual spacing on everything meant to cover (don't turn \
underlay off or open up the rows to save stitches: the cloth shows through and puckers); give each fill its own direction, \
along its length where it has one and at least 30° from the pieces it touches and the ground it sits on, never everything at \
the default 0°; keep a background or patch ground to one plain fill, which may be lighter (up to twice the usual spacing) but \
keeps its underlay, with nothing sewn across it that isn't part of the picture; sew a patch's satin border last, 3 to 5 mm \
wide; keep lettering at least 5 mm tall (6.5 mm on a cap); and leave Sew over fill off unless the person asks for it: a trim \
costs less than a line of thread across finished stitching. Trim settings already on when you start (Sew over fill, \
Connect, Blend) are the person's: never turn them off to clear notes or tidy the stitching, since each saves trims and \
taking it away can double them; if their lines look like a problem, offer it as a choice with what it costs in trims. \
Pass terse: true on tools that change the design (the short answer names what was added and any new problems; get_design has the rest when you need it). \
After making changes, call render_preview to look at the stitches, and with artwork true to see your drawing itself; \
zoom in with region or object to check the details, refine what looks crude. \
Before you finish, call review_design: it reviews the design as an embroiderer would (thread changes, trims and what \
causes them, pieces too small to read, threads that won't show on the fabric or can't be told apart) and shows it at \
about its real size. Fix what it finds that you can without taking out shading, highlights or features: try the stitch \
types it suggests (a narrow band as satin) and the fill angles it suggests (rows along a piece's length for flow, \
touching pieces set apart for dimension), keeping each change that reads better close up. Review again, \
and say in your summary what it costs to sew (threads, trims). Ask the person to choose only when there really are different ways to go or a trade-off that's theirs to weigh (fewer stops to cut the thread against some thread showing, two looks that are both fine): most replies need no choice, and when one way is clearly better and costs nothing they'd see, just do it. An optional change worth their while is always offered, never only mentioned in your summary (not \"optional X could …; I left it unchanged\"): one such change is offered as one choice, which shows as a question with a button for it and one for Not now. Then, at most one question in a reply, it goes to them with offer_choices: your question and a few options, each with its exact steps; they click one and it's made (an option to do them all, and one to keep it as it is, are added for you). Keep it short, so it fits the panel: up to four options, each a few words with one short sentence under it. Write it so a beginner understands the impact: each option in everyday words, and what they'd see on the fabric and what it means for sewing (\"the machine stops to cut the thread 27 times instead of 89, so it sews faster, but a few short lines of thread will show on the trunk\"), no setting names or unexplained jargon. Don't make those changes yourself, and don't only describe them. \
When they ask to clean up, refine, improve or add detail to something already there, work from what's there: \
look at it close up first (render_preview with artwork and object), keep what's good about its shapes and improve them \
(edit_path to reshape, set_object for its stitching) and add layers on top (shading, highlights, fur or feather lines, \
small features); never replace it with a simpler version. Compare the artwork after with before: it should end with \
at least as much detail as it began with, unless they asked for it simpler. \
When they ask for options, alternatives or a few versions, make them with start_variants and save_variant: \
the person sees them side by side with the original and picks one (show_comparison shows them again, or puts them away). \
Don't save or export unless asked. Finish with one or two sentences on what you made.";

/// For an agent that can generate images (Codex): draw pictures by
/// generating one and tracing it, which often beats hand-written SVG.
const IMAGE_BRIEF: &str = "For a pictorial subject (an animal, a person or character, a scene, a detailed object), generate an image and trace it rather than writing its SVG yourself: traced shapes are far more natural and detailed than hand-written ones. That goes for a picture already in the design that should be more detailed or more natural too: generate a new image that matches it (its pose, proportions and colors, from a close look at it), trace it at the old one's size and place, and delete the old one once the new one is in. Make the image for sewing, not for the screen. Every color is a thread change and every separate piece of a color is a jump the machine trims, so first read the design's threads (get_design) and ask for the picture in those exact colors, giving their hex values; add a new color only where the subject can't do without it, and then a shade near one already in the design where you can (a tan beside a cream, not a new brown). Shading and highlights are wanted: they're what makes a picture look drawn rather than cut out. Ask for them as solid shapes in darker and lighter shades of its base colors (a design thread a shade off where there is one), such as a shadow down a flank, a lighter muzzle or the far legs in a darker tone, each a clear shape of its own rather than a gradient, mottling or many small flecks. Ask for a flat illustration in solid colors, at most 6 to 8 in all, no gradients, textures, cast shadows, outlines in a color of their own or fine hatching; each color in clear shapes, not scattered specks; bold shapes with clean edges; the subject alone on a plain white background. Then call trace_image with the image's absolute path, palette \"design\" (so its colors become the design's threads), threads for the machine's thread chart (for any new color), lines true if it has line details, and width_mm, x_mm and y_mm for its size and place. Its answer gives new_colors and the trims before and after: recolor a piece in a color that isn't needed to the design thread it's nearest (set_object color). To bring trims down, delete only stray bits and specks too small to sew; never take out shading, highlights or features to save trims: a few more trims for a picture that reads well is the right trade. Change the document's connect_mm or blend_mm only as review_design's what_ifs say (never over_sewn on your own): one that saves trims without showing (shows false) you set, and say so; one whose thread shows is the person's to choose, so offer it with offer_choices, saying what it saves and what shows. Then look at it close up with render_preview's artwork and region, and fix what the trace got wrong (stray bits, colors, stitch types). Write SVG yourself for logos, badges, lettering and simple geometric shapes, where exact shapes matter.";

impl AgentKind {
    /// The brief for this agent: what it can do on top of the tools.
    fn brief(self) -> String {
        match self {
            AgentKind::Claude | AgentKind::Antigravity => BRIEF.to_string(),
            AgentKind::Codex => format!("{BRIEF} {IMAGE_BRIEF}"),
        }
    }
}

/// PATH from the login shell, so `claude` and `codex` (and node) are found
/// even when the app is opened from Finder.
fn login_path() -> Option<String> {
    // Windows apps get the user's PATH already, and there's no login shell.
    if cfg!(windows) {
        return None;
    }
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

/// Held by a test that points `ZIGZAG_CLAUDE` at a stand-in, for as long as
/// it's set: the environment is the whole process's, and tests run side by
/// side.
#[cfg(test)]
pub(crate) static AGENT_ENV: Mutex<()> = Mutex::new(());

/// [`locate`], for the setup module's test that installs for real.
#[cfg(test)]
pub(crate) fn locate_for_test(kind: AgentKind, login: Option<&str>) -> Option<PathBuf> {
    locate(kind, login)
}

/// The usual places the agents' installers put them on `os`, after the
/// PATH: under `home`, and on Windows under %APPDATA% and %LOCALAPPDATA%.
fn install_dirs(os: Os, home: &str, appdata: Option<&str>, local: Option<&str>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        ".local/bin",
        ".claude/local",
        ".npm-global/bin",
        ".bun/bin",
        ".cargo/bin",
    ]
    .iter()
    .map(|d| PathBuf::from(home).join(d))
    .collect();
    if os == Os::Windows {
        if let Some(appdata) = appdata {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
        if let Some(local) = local {
            // Where Codex's and Antigravity's own installers put them.
            dirs.push(PathBuf::from(local).join(r"Programs\OpenAI\Codex\bin"));
            dirs.push(PathBuf::from(local).join(r"agy\bin"));
        }
    } else {
        dirs.push("/opt/homebrew/bin".into());
        dirs.push("/usr/local/bin".into());
    }
    dirs
}

/// Where `kind`'s program is: $ZIGZAG_<NAME> (for tests, if it's there), the login PATH,
/// this process's PATH, then the usual install places.
fn locate(kind: AgentKind, login: Option<&str>) -> Option<PathBuf> {
    let env = format!("ZIGZAG_{}", kind.key().to_uppercase());
    // (Not there yet counts as not installed: a test copy watching for an
    // install to land there.)
    if let Some(p) = std::env::var_os(env) {
        let p = PathBuf::from(p);
        return p.exists().then_some(p);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for p in [login.map(str::to_string), std::env::var("PATH").ok()]
        .into_iter()
        .flatten()
    {
        dirs.extend(std::env::split_paths(&p));
    }
    dirs.extend(install_dirs(
        Os::CURRENT,
        &home,
        std::env::var("APPDATA").ok().as_deref(),
        std::env::var("LOCALAPPDATA").ok().as_deref(),
    ));
    // On Windows a program has an extension: a bare `claude` there is the
    // npm shim for sh, which Windows can't run (os error 193). Prefer the
    // native .exe anywhere on the PATH, then npm's .cmd launcher.
    let names: Vec<String> = if cfg!(windows) {
        [".exe", ".cmd"]
            .iter()
            .map(|ext| format!("{}{ext}", kind.program()))
            .collect()
    } else {
        vec![kind.program().to_string()]
    };
    names
        .iter()
        .flat_map(|n| dirs.iter().map(move |d| d.join(n)))
        .find(|p| p.is_file())
}

impl AgentState {
    /// The agent last used, remembered between launches.
    pub fn load() -> AgentState {
        Self::load_from(crate::dock::config_file("agent.json"))
    }

    /// As `load`, from `config` (agent.json, or another file for tests).
    fn load_from(config: Option<PathBuf>) -> AgentState {
        let v = config
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or(Value::Null);
        let saved = v["agent"].as_str().and_then(AgentKind::from_key);
        let kind = saved.unwrap_or_default();
        let per = |key: &str| {
            AgentKind::ALL.iter().fold(Default::default(), |mut out: [Option<String>; AgentKind::COUNT], k| {
                out[k.index()] = v[key][k.key()].as_str().map(str::to_string);
                out
            })
        };
        AgentState {
            kind,
            // The first time (no agent chosen yet): Setup, with the guide.
            on_setup: saved.is_none(),
            config,
            model: per("model"),
            effort: per("effort"),
            store: crate::history::Store::usual(),
            ..Default::default()
        }
    }

    fn save(&self) {
        let Some(f) = &self.config else { return };
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let per = |v: &[Option<String>; AgentKind::COUNT]| {
            let mut m = serde_json::Map::new();
            for k in AgentKind::ALL {
                if let Some(x) = &v[k.index()] {
                    m.insert(k.key().into(), Value::String(x.clone()));
                }
            }
            Value::Object(m)
        };
        let _ = std::fs::write(
            f,
            serde_json::json!({
                "agent": self.kind.key(),
                "model": per(&self.model),
                "effort": per(&self.effort),
            })
            .to_string(),
        );
    }

    /// The model chosen for the agent in use (`None`: its default).
    pub fn model(&self) -> Option<&str> {
        self.model[self.kind.index()].as_deref()
    }

    /// The effort chosen for the agent in use (`None`: its default).
    pub fn effort(&self) -> Option<&str> {
        self.effort[self.kind.index()].as_deref()
    }

    /// Choose the model for the agent in use (from the next message;
    /// remembered). An effort it doesn't take goes back to the default.
    /// What to say about it, if anything.
    pub fn set_model(&mut self, model: Option<String>, ctx: &egui::Context) -> Option<String> {
        self.model[self.kind.index()] = model;
        let mut said = None;
        if let Some(e) = self.effort().map(str::to_string)
            && !self.efforts(ctx).contains(&e)
        {
            self.effort[self.kind.index()] = None;
            said = Some(format!(
                "This model doesn't take effort {e}: back to its default."
            ));
        }
        self.save();
        said
    }

    /// Choose the effort for the agent in use (remembered).
    pub fn set_effort(&mut self, effort: Option<String>) {
        self.effort[self.kind.index()] = effort;
        self.save();
    }

    /// The models the agent in use can run on. Codex's are looked up from
    /// it (off the UI thread): none until that's done, or if it can't say.
    pub fn models(&mut self, ctx: &egui::Context) -> Vec<ModelInfo> {
        match self.kind {
            AgentKind::Claude => CLAUDE_MODELS
                .iter()
                .map(|&(id, name, about)| ModelInfo {
                    id: id.into(),
                    name: name.into(),
                    about: about.into(),
                    efforts: CLAUDE_EFFORTS.map(str::to_string).to_vec(),
                })
                .collect(),
            AgentKind::Codex => {
                self.codex_lookup(Some(ctx));
                self.codex_info
                    .as_ref()
                    .map(|i| i.models.clone())
                    .unwrap_or_default()
            }
            // (`agy models` lists them only once signed in, in a form not
            // yet seen: its default, or one named with /model.)
            AgentKind::Antigravity => Vec::new(),
        }
    }

    /// Look Codex's models and tool servers up (once, off the UI thread),
    /// and take the answer if it's come.
    fn codex_lookup(&mut self, ctx: Option<&egui::Context>) {
        if let Some(rx) = &self.codex_rx
            && let Ok(info) = rx.try_recv()
        {
            self.codex_info = Some(info);
            self.codex_rx = None;
        }
        if self.codex_info.is_some() || self.codex_rx.is_some() {
            return;
        }
        let Some(program) = self.find(AgentKind::Codex) else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let path = self.shell_path();
        let ctx = ctx.cloned();
        std::thread::spawn(move || {
            let run = |args: &[&str]| {
                let mut cmd = Command::new(&program);
                cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null());
                if let Some(p) = &path {
                    cmd.env("PATH", p);
                }
                cmd.output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                    .unwrap_or_default()
            };
            // (The two side by side, each most of a second: models, then
            // servers, as runs see them, plugins off: a plugin's own server
            // named on the command line reads as a broken entry in the
            // person's config, and Codex won't start.)
            let servers = std::thread::scope(|sc| {
                let servers = sc.spawn(|| {
                    run(&[
                        "-c",
                        "features.plugins=false",
                        "-c",
                        "features.apps=false",
                        "mcp",
                        "list",
                        "--json",
                    ])
                });
                let models = run(&["debug", "models"]);
                (models, servers.join().unwrap_or_default())
            });
            let info = CodexInfo {
                models: parse_codex_models(&servers.0),
                // (Listed as runs see them, plugins off: a plugin's own
                // server named on the command line reads as a broken
                // entry in the person's config, and Codex won't start.)
                servers: parse_codex_servers(&servers.1),
            };
            let _ = tx.send(info);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        self.codex_rx = Some(rx);
    }

    /// The person's own Codex tool servers, to switch off for a run:
    /// waited for (briefly) if the lookup hasn't answered yet.
    fn codex_servers(&mut self) -> Vec<String> {
        self.codex_lookup(None);
        if self.codex_info.is_none()
            && let Some(rx) = self.codex_rx.take()
        {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(info) => self.codex_info = Some(info),
                Err(_) => self.codex_rx = Some(rx),
            }
        }
        self.codex_info
            .as_ref()
            .map(|i| i.servers.clone())
            .unwrap_or_default()
    }

    /// The efforts the agent in use takes, on its chosen model (or on any
    /// of them, when it's on its default).
    pub fn efforts(&mut self, ctx: &egui::Context) -> Vec<String> {
        let models = self.models(ctx);
        let chosen = self.model().map(str::to_string);
        let mut out: Vec<String> = Vec::new();
        for m in models.iter().filter(|m| {
            chosen
                .as_ref()
                .is_none_or(|c| c.eq_ignore_ascii_case(&m.id))
        }) {
            for e in &m.efforts {
                if !out.contains(e) {
                    out.push(e.clone());
                }
            }
        }
        if out.is_empty() {
            // (Not known yet, or a model named in full: the levels every
            // model of that agent takes.)
            out = match self.kind {
                AgentKind::Claude => CLAUDE_EFFORTS.map(str::to_string).to_vec(),
                AgentKind::Codex => CODEX_EFFORTS.map(str::to_string).to_vec(),
                AgentKind::Antigravity => AGY_EFFORTS.map(str::to_string).to_vec(),
            };
        }
        out
    }

    pub fn running(&self) -> bool {
        self.run.is_some() || self.deferred.is_some()
    }

    /// Swap the conversation in front with a parked one (another
    /// document's): the agent, its settings and the panel stay.
    pub fn swap_chat(&mut self, c: &mut Chat) {
        use std::mem::swap;
        swap(&mut self.prompt, &mut c.prompt);
        swap(&mut self.log, &mut c.log);
        swap(&mut self.session, &mut c.session);
        swap(&mut self.session_kind, &mut c.session_kind);
        swap(&mut self.run, &mut c.run);
        swap(&mut self.started, &mut c.started);
        swap(&mut self.work, &mut c.work);
        swap(&mut self.calls, &mut c.calls);
        swap(&mut self.open_steps, &mut c.open_steps);
        swap(&mut self.turn_prompt, &mut c.turn_prompt);
        swap(&mut self.deferred, &mut c.deferred);
        swap(&mut self.doc_key, &mut c.doc_key);
        swap(&mut self.hist_id, &mut c.hist_id);
        swap(&mut self.hist_saved, &mut c.hist_saved);
        swap(&mut self.hist_session, &mut c.hist_session);
        swap(&mut self.hist_checked, &mut c.hist_checked);
        swap(&mut self.earlier, &mut c.earlier);
        swap(&mut self.show_earlier, &mut c.show_earlier);
        swap(&mut self.attach, &mut c.attach);
        swap(&mut self.queued, &mut c.queued);
        swap(&mut self.was_running, &mut c.was_running);
    }

    /// Put what was queued back in the prompt box, before anything being
    /// written there now: it isn't sent.
    pub fn unqueue(&mut self) {
        if self.queued.is_empty() {
            return;
        }
        let mut text = std::mem::take(&mut self.queued).join("\n\n");
        if !self.prompt.trim().is_empty() {
            text.push_str("\n\n");
            text.push_str(&self.prompt);
        }
        self.prompt = text;
    }

    /// History was cleared: see [`Chat::forget_kept`].
    pub fn forget_kept(&mut self) {
        self.hist_id = None;
        self.hist_saved = self.log.len();
        self.earlier = None;
    }

    /// Continue `session` (an earlier chat's), with the agent it was with.
    pub fn resume(&mut self, session: Option<String>, kind: AgentKind) {
        self.session = session;
        self.session_kind = Some(kind);
    }

    /// The document in front is another now (one opened into this tab):
    /// its kept chats are looked for afresh.
    pub fn forget_document(&mut self) {
        self.doc_key = None;
        self.hist_checked = false;
        self.earlier = None;
    }

    /// The agent the session being carried on is with.
    pub(crate) fn session_kind(&self) -> Option<AgentKind> {
        self.session_kind
    }

    /// Stop the agent working in a document that isn't in front (its
    /// chat says so when it comes forward).
    pub fn stop_parked(&mut self, c: &mut Chat) {
        let stopped = self.stopped;
        self.swap_chat(c);
        self.stop();
        self.swap_chat(c);
        // (Not the chat in front's Stop.)
        self.stopped = stopped;
    }

    /// As if a prompt were running (a process that waits; `stop` ends it):
    /// for tests, and capture mode's pictures of the panel at work.
    pub fn start_fake_run(&mut self) {
        // (Where there's no `sleep` to run, Windows say, nothing starts.)
        let Ok(child) = Command::new("sleep").arg("30").spawn() else {
            return;
        };
        let (_tx, rx) = mpsc::channel();
        self.run = Some(Run {
            child: Arc::new(Mutex::new(child)),
            rx,
            stderr: Vec::new(),
            exit: None,
            eof: false,
            kind: self.kind,
            resumed: false,
            agy: AgyStream::default(),
            result_at: None,
        });
    }

    /// What it's doing now: the step of a tool call still running, else thinking.
    pub fn status(&self) -> &'static str {
        match self.log.last() {
            Some(Entry::Tool(name, _, None)) => activity(name),
            _ => "Thinking",
        }
    }

    pub fn stop(&mut self) {
        self.unqueue();
        if self.deferred.take().is_some() {
            self.log.push(Entry::Error("Stopped.".into()));
        }
        if let Some(r) = self.run.take() {
            if let Ok(mut c) = r.child.lock() {
                end_run(&mut c);
            }
            self.settle(false);
            self.log.push(Entry::Error("Stopped.".into()));
        }
        self.stopped = true;
    }

    /// End the turn running now to send something else at once (a queued
    /// message's Send Now): like [`Self::stop`], but what's still queued
    /// stays queued, and the notes waiting for it stay too.
    pub fn interrupt(&mut self) {
        if let Some(r) = self.run.take() {
            if let Ok(mut c) = r.child.lock() {
                end_run(&mut c);
            }
            self.settle(false);
            self.log.push(Entry::Info("Stopped to send your message now.".into()));
        }
    }

    /// The run is over: a step still marked as running isn't (it would
    /// spin for good). `ok`: how the run ended.
    fn settle(&mut self, ok: bool) {
        for e in &mut self.log {
            if let Entry::Tool(_, _, state @ None) = e {
                *state = Some(ok);
            }
        }
    }

    /// Use `kind` in the panel (a new conversation if it's another agent;
    /// remembered, so the panel opens on it next time).
    pub fn set_kind(&mut self, kind: AgentKind) {
        self.on_setup = false;
        if kind != self.kind {
            self.new_chat();
            self.kind = kind;
        }
        // (Saved even when it's the one already set: choosing the default
        // agent is a choice, and the panel opens on it next time, not Setup.)
        self.save();
    }

    pub fn new_chat(&mut self) {
        self.stop();
        self.log.clear();
        self.calls.clear();
        self.open_steps.clear();
        self.session = None;
        self.hist_id = None;
        self.hist_saved = 0;
        self.earlier = None;
        self.show_earlier = false;
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
            self.open_on_an_installed_agent();
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
                let mut found: [Option<PathBuf>; AgentKind::COUNT] = Default::default();
                for k in AgentKind::ALL {
                    found[k.index()] = locate(k, login.as_deref());
                }
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
        self.found = Default::default();
        self.lookup = None;
    }

    /// Once, when ZigZag first knows which agents are here: the panel opens
    /// on the agent last used (saved in agent.json), or, when that one isn't
    /// installed, the first that is (and remembers it). Setup shows only
    /// when none is. (Only then: an agent picked later shows its own setup.)
    pub(crate) fn open_on_an_installed_agent(&mut self) {
        if std::mem::replace(&mut self.settled, true) || self.on_setup || self.setup.busy() {
            return;
        }
        if !matches!(self.found[self.kind.index()], Some(None)) {
            return;
        }
        if let Some(k) = AgentKind::ALL
            .into_iter()
            .find(|k| matches!(self.found[k.index()], Some(Some(_))))
        {
            // (Not `set_kind`: a chat restored from last time stays. Its
            // session was the other agent's, so a send starts afresh.)
            self.kind = k;
            self.save();
        }
    }

    /// Whether the panel shows the Setup page ("Agent: Setup"): chosen
    /// from the menu, or the chosen agent isn't here (or is being
    /// installed, or its install failed).
    pub fn setup_page(&self) -> bool {
        use crate::agent_setup::Step;
        self.on_setup
            || matches!(self.found[self.kind.index()], Some(None))
            || matches!(&self.setup.step, Step::Installing(j) if j.kind == self.kind)
            || matches!(&self.setup.step, Step::Failed { kind, install: true, .. } if *kind == self.kind)
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
                    .args(["--append-system-prompt", &AgentKind::Claude.brief()]);
                if let Some(s) = &self.session {
                    cmd.args(["--resume", s]);
                }
                if let Some(m) = self.model() {
                    cmd.args(["--model", m]);
                }
                if let Some(e) = self.effort() {
                    cmd.args(["--effort", e]);
                }
                if !self.sending_images.is_empty() {
                    cmd.args(["--input-format", "stream-json"]);
                }
            }
            AgentKind::Codex => {
                cmd.arg("exec");
                if let Some(s) = &self.session {
                    cmd.args(["resume", s]);
                }
                cmd.args(["--json", "--skip-git-repo-check"]);
                // (Joined to their option, so none is taken for the prompt.)
                for p in &self.sending_images {
                    cmd.arg(format!("--image={}", p.display()));
                }
                // Only ZigZag's tools, as Claude Code is given: none of the
                // person's own plugins, apps or tool servers (a browser, a
                // JS runner…), which go off looking for things on the web
                // and fail in ways that read as ZigZag's. (A setting this
                // Codex doesn't know is ignored.)
                for f in ["plugins", "apps", "browser_use", "computer_use"] {
                    cmd.arg("-c").arg(format!("features.{f}=false"));
                }
                for name in self.codex_servers() {
                    if name != "zigzag" {
                        cmd.arg("-c")
                            .arg(format!("mcp_servers.{name}.enabled=false"));
                    }
                }
                if let Some(m) = self.model() {
                    cmd.args(["-m", m]);
                }
                if let Some(e) = self.effort() {
                    cmd.arg("-c").arg(format!(
                        "model_reasoning_effort={}",
                        Value::String(e.into())
                    ));
                }
                // Codex starts MCP servers as commands: the stdio bridge.
                match bridge {
                    Some(b) => {
                        cmd.arg("-c").arg(format!(
                            "mcp_servers.zigzag.command={}",
                            Value::String(b.to_string_lossy().into())
                        ));
                        cmd.arg("-c").arg(format!(
                            "mcp_servers.zigzag.args=[\"--port\", \"{port}\", \"--panel\"]"
                        ));
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
            AgentKind::Antigravity => {
                // Its tools and brief come from files in its folder (below):
                // it has no flags for them.
                cmd.args(["--output-format", "stream-json", "--sandbox"]);
                let home = std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .unwrap_or_default();
                cmd.args(["--settings", &agy_settings(&home, &self.sending_images).to_string()]);
                if let Some(s) = &self.session {
                    cmd.args(["--conversation", s]);
                }
                if let Some(m) = self.model() {
                    cmd.args(["--model", m]);
                }
                if let Some(e) = self.effort() {
                    cmd.args(["--effort", e]);
                }
                // (The prompt follows it, in `launch`: it's taken from the
                // command line, not stdin.)
                cmd.arg("-p");
            }
        }
        if let Some(p) = self.shell_path() {
            cmd.env("PATH", p);
        }
        // A quiet folder of its own, away from your files.
        let dir = self.work_dir();
        let _ = std::fs::create_dir_all(&dir);
        if self.kind == AgentKind::Antigravity {
            write_agy_workspace(&dir, mcp_url, &self.kind.brief());
        }
        cmd.current_dir(dir);
        cmd
    }

    /// The folder the agent runs in: one of ZigZag's own, beside its
    /// settings. Antigravity has one inside it, for the files that set
    /// it up (Codex would read its AGENTS.md too).
    fn work_dir(&self) -> PathBuf {
        let base = self
            .config
            .as_ref()
            .and_then(|c| c.parent())
            .map(|d| d.join("agent"));
        match (self.kind, base) {
            (AgentKind::Antigravity, Some(b)) => b.join("antigravity"),
            (AgentKind::Antigravity, None) => std::env::temp_dir().join("zigzag-agent-antigravity"),
            (_, Some(b)) => b,
            (_, None) => std::env::temp_dir(),
        }
    }

    /// Send the prompt. `mcp`: the studio's MCP endpoint (if it's running).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn send(&mut self, mcp: Option<std::net::SocketAddr>, ctx: &egui::Context) {
        self.send_with(mcp, ctx, Vec::new(), "");
    }

    /// Send the prompt, with `notes` (the canvas notes going with it, as
    /// the transcript shows them) and `extra` (what the agent is told
    /// about them, and anything else it needs, after the prompt).
    pub fn send_with(
        &mut self,
        mcp: Option<std::net::SocketAddr>,
        ctx: &egui::Context,
        notes: Vec<String>,
        extra: &str,
    ) {
        let text = self.prompt.trim().to_string();
        if (text.is_empty() && extra.is_empty() && self.attach.is_empty()) || self.running() {
            return;
        }
        if !text.is_empty() {
            self.log.push(Entry::You(text.clone()));
        }
        let images = std::mem::take(&mut self.attach);
        if !images.is_empty() {
            self.log.push(Entry::Images(images.clone()));
        }
        if !notes.is_empty() {
            self.log.push(Entry::Notes(notes));
        }
        let mut text = match (text.is_empty(), extra.is_empty()) {
            (_, true) => text,
            (true, false) => extra.to_string(),
            (false, false) => format!("{text}\n\n{extra}"),
        };
        // The images are seen, and named: trace_image takes a file.
        if !images.is_empty() {
            let files: Vec<String> = images.iter().map(|p| p.display().to_string()).collect();
            text = format!(
                "{text}{}The person attached {} (also saved as {}): you can see {}, and trace_image can trace {} into stitches.",
                if text.is_empty() { "" } else { "\n\n" },
                if images.len() == 1 {
                    "an image".to_string()
                } else {
                    format!("{} images", images.len())
                },
                files.join(", "),
                if images.len() == 1 { "it" } else { "them" },
                if images.len() == 1 { "it" } else { "one" },
            );
        }
        let Some(addr) = mcp else {
            self.log.push(Entry::Error(format!(
                "The agent works through the studio's MCP server, which {}.",
                self.mcp_off.as_deref().unwrap_or("isn't running")
            )));
            return;
        };
        // A chat begun with the other agent (another document's, say): this
        // one can't continue that session, so it starts its own.
        if self.session_kind != Some(self.kind) {
            self.session = None;
            self.session_kind = Some(self.kind);
        }
        // Where its program is and (Codex) the person's tool servers are
        // looked up off the UI thread (see `warm_up`): until they're in, the
        // send waits, shown as working, and goes the moment they are.
        self.prompt.clear();
        self.started = Some(Instant::now());
        if self.ready_to_start(ctx) {
            self.launch(addr, text, images, ctx);
        } else {
            self.deferred = Some(Deferred {
                addr,
                text,
                images,
                ctx: ctx.clone(),
            });
        }
    }

    /// Look up, off the UI thread, what starting an agent needs: the login
    /// PATH and where each agent's program is, then Codex's models and the
    /// person's tool servers. Cheap once known; called every frame.
    pub fn warm_up(&mut self, ctx: &egui::Context) {
        let _ = self.installed(self.kind, ctx);
        if self.found[AgentKind::Codex.index()]
            .as_ref()
            .is_some_and(|f| f.is_some())
        {
            self.codex_lookup(Some(ctx));
        }
    }

    /// Everything starting the agent in use needs is known (or known not
    /// to be there).
    fn ready_to_start(&mut self, ctx: &egui::Context) -> bool {
        self.warm_up(ctx);
        let Some(found) = &self.found[self.kind.index()] else {
            return false;
        };
        self.kind != AgentKind::Codex || found.is_none() || self.codex_info.is_some()
    }

    /// Start the agent on `text` (its prompt, ready to send), `images` with it.
    fn launch(
        &mut self,
        addr: std::net::SocketAddr,
        text: String,
        images: Vec<PathBuf>,
        ctx: &egui::Context,
    ) {
        self.sending_images = images;
        let Some(program) = self.find(self.kind) else {
            self.log
                .push(Entry::Error(self.kind.not_installed_message(Os::CURRENT)));
            return;
        };
        let bridge = std::env::current_exe()
            .ok()
            .and_then(|e| {
                e.parent()
                    .map(|d| d.join(format!("zigzag-mcp{}", std::env::consts::EXE_SUFFIX)))
            })
            .filter(|p| p.is_file());
        // (`panel`: these calls are this panel's agent's.)
        let url = format!("http://{addr}/mcp?panel=1");
        let mut cmd = self.command(program, &url, bridge, addr.port());
        // The prompt goes in on stdin (no shell quoting to worry about),
        // except to Antigravity, which takes it on its command line.
        let first = self.session.is_none();
        let prompt = if self.kind == AgentKind::Codex && first {
            format!("{}\n\n{text}", self.kind.brief())
        } else {
            text
        };
        // (Claude is given images in a message of its streaming input.)
        let payload = match self.kind {
            AgentKind::Claude if !self.sending_images.is_empty() => {
                claude_message(&prompt, &self.sending_images)
            }
            AgentKind::Antigravity => {
                let arg = agy_prompt(&prompt, &self.sending_images);
                // (Windows takes a command line of about 32,000 characters.)
                if cfg!(windows) && arg.len() > 30_000 {
                    self.sending_images.clear();
                    self.log.push(Entry::Error(
                        "That prompt is too long to give Antigravity on Windows. Shorten it, or use another agent.".into(),
                    ));
                    return;
                }
                cmd.arg(arg);
                String::new()
            }
            _ => prompt,
        };
        self.sending_images.clear();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // A group of its own, so stopping it stops what it starts too.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
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
        // (Off this thread: with pictures it's megabytes, and the program
        // reads it when it's ready to.)
        if let Some(mut stdin) = child.stdin.take() {
            std::thread::spawn(move || {
                let _ = stdin.write_all(payload.as_bytes());
            });
        }
        let (tx, rx) = mpsc::channel();
        let mut eof = true;
        if let Some(out) = child.stdout.take() {
            eof = false;
            let tx = tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let _ = tx.send(Event::Line(line));
                    ctx.request_repaint();
                }
                let _ = tx.send(Event::Eof);
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
        self.run = Some(Run {
            child,
            rx,
            stderr: Vec::new(),
            exit: None,
            eof,
            kind: self.kind,
            resumed: self.session.is_some(),
            agy: AgyStream::new(),
            result_at: None,
        });
    }

    /// Take in what the agent printed since last frame.
    pub fn poll(&mut self) {
        if let Some(d) = self.deferred.take() {
            let ctx = d.ctx.clone();
            if self.ready_to_start(&ctx) {
                self.launch(d.addr, d.text, d.images, &ctx);
            } else {
                self.deferred = Some(d);
            }
        }
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
                Event::Eof => run.eof = true,
                Event::Exit(code) => run.exit = Some((code, Instant::now())),
            }
        }
        let (kind, resumed) = (run.kind, run.resumed);
        // Antigravity sometimes never exits after its result: a few seconds
        // on, it's stopped, and the turn ends as the result said.
        if let Some((at, ok)) = run.result_at
            && run.exit.is_none()
            && at.elapsed() > AGY_LINGER
        {
            if let Ok(mut c) = run.child.lock() {
                end_run(&mut c);
            }
            run.exit = Some((Some(if ok { 0 } else { 1 }), Instant::now()));
            run.eof = true;
        }
        // It has ended once its last lines are in too: the process can be
        // gone before what it printed has been read.
        let ended = run
            .exit
            .filter(|(_, at)| run.eof || at.elapsed() > LAST_WORDS)
            .map(|(code, _)| code);
        let mut agy = std::mem::take(&mut run.agy);
        let mut result = None;
        for l in lines {
            let Ok(v) = serde_json::from_str::<Value>(&l) else {
                continue;
            };
            let entries = match kind {
                AgentKind::Claude => claude_event(&v, &mut self.session),
                AgentKind::Codex => codex_event(&v, &mut self.session),
                AgentKind::Antigravity => {
                    if v["event"] == "result" {
                        result = Some(agy_succeeded(&v));
                    }
                    agy_event(&v, &mut self.session, &mut agy)
                }
            };
            for (id, e) in entries {
                self.push(id, e);
            }
        }
        if let Some(run) = &mut self.run {
            run.agy = agy;
            // (The panel looks again every tenth of a second while it works.)
            if let Some(ok) = result {
                run.result_at.get_or_insert((Instant::now(), ok));
            }
        }
        if let Some(code) = ended {
            let stderr = self.run.take().map(|r| r.stderr).unwrap_or_default();
            let finished = matches!(self.log.last(), Some(Entry::Done(_) | Entry::Error(_)));
            self.settle(code == Some(0));
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
                        format!("{} stopped (exit code {code:?}).", kind.name())
                    } else {
                        tail
                    })
                });
            }
            // The session it was to carry on is gone (cleared away by the
            // agent's own program, or made on another computer): every
            // prompt would fail the same way. The next starts a new one.
            let since: Vec<&Entry> = self
                .log
                .iter()
                .rev()
                .take_while(|e| !matches!(e, Entry::You(_) | Entry::Notes(_) | Entry::Images(_)))
                .collect();
            let did_nothing = !since
                .iter()
                .any(|e| matches!(e, Entry::Said(_) | Entry::Tool(..)));
            let gone = since
                .iter()
                .any(|e| matches!(e, Entry::Error(m) if session_gone(m)));
            if resumed && code != Some(0) && did_nothing && gone {
                self.session = None;
                self.session_kind = None;
                self.log.push(Entry::Info(format!(
                    "{} can't pick up the earlier session (it's no longer there). Send your message again to go on in a new one; it won't remember what was said before.",
                    kind.name()
                )));
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
                .take_while(|x| !matches!(x, Entry::You(_) | Entry::Notes(_)))
                .any(|x| matches!(x, Entry::Error(n) if n == m))
        {
            return;
        }
        self.log.push(e);
    }
}

pub(crate) fn short(v: &Value) -> String {
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

pub(crate) fn tool_name(n: &str) -> String {
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

/// What to say when one of Codex's tool calls (to MCP server `server`)
/// failed with `message`.
fn codex_tool_error(server: &str, tool: &str, message: &str) -> String {
    // Only ZigZag's tools are approved here: another server's failure (a
    // tool of Codex's own, from the person's plugins) is said as it is,
    // its first line, under its server's name. (Its words may well mention
    // cancelling without being about approval.)
    if !server.is_empty() && server != "zigzag" {
        let first = message
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("");
        let first: String = first.chars().take(200).collect();
        return if first.is_empty() {
            format!("{server} {tool} failed.")
        } else {
            format!("{server} {tool}: {first}")
        };
    }
    // A tool's whole answer (batch's: its error and the steps before it):
    // the error is what's said.
    let parsed = serde_json::from_str::<Value>(message).ok();
    let message = parsed
        .as_ref()
        .and_then(|v| v["error"].as_str())
        .unwrap_or(message);
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

/// What to say when a Codex turn failed with `message`.
fn codex_failure(message: &str) -> String {
    if message.contains("active writer") {
        // (Its chat is held by a run of it that's still going.)
        return "Codex is still finishing an earlier run of this chat, so it can't carry on yet. Wait a moment and send again, or start a new chat with /new.".into();
    }
    message.to_string()
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
                        // (A call that didn't get through has an error; one
                        // the tool itself refused has the tool's own words,
                        // as its result.)
                        let said: Vec<&str> = it["result"]["content"]
                            .as_array()
                            .map(|c| c.iter().filter_map(|x| x["text"].as_str()).collect())
                            .unwrap_or_default();
                        let said = said.join("\n");
                        let m = it["error"]["message"]
                            .as_str()
                            .map(str::trim)
                            .filter(|m| !m.is_empty())
                            .unwrap_or(said.trim());
                        let server = it["server"].as_str().unwrap_or("");
                        out.push((None, Entry::Error(codex_tool_error(server, &tool, m))));
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
            Entry::Error(codex_failure(
                v["error"]["message"].as_str().unwrap_or("The turn failed."),
            )),
        )),
        Some("error") => {
            let m = v["message"].as_str().unwrap_or("");
            if !m.starts_with("Reconnecting") {
                out.push((None, Entry::Error(codex_failure(m))));
            }
        }
        _ => {}
    }
    out
}

/// Antigravity's run settings (`--settings`, never saved): only ZigZag's
/// tools. Its shell, file writes and the web are refused outright; reading
/// a file is let through only for the images going with the prompt and
/// for the descriptions it keeps of ZigZag's tools (under `home`). Anything
/// else it would ask about, and run like this there's nobody to ask, so
/// it's refused too. `agentMode` "" is its default mode: `auto` (its usual
/// for `-p`) would approve things by itself.
fn agy_settings(home: &str, images: &[PathBuf]) -> Value {
    let mut allow = vec![
        "mcp(zigzag/*)".to_string(),
        format!(
            "read_file({})",
            PathBuf::from(home)
                .join(".gemini/antigravity-cli/mcp/zigzag")
                .display()
        ),
    ];
    for p in images {
        allow.push(format!("read_file({})", p.display()));
    }
    serde_json::json!({
        "agentMode": "",
        "permissions": {
            "allow": allow,
            "deny": [
                "command(*)",
                "write_file(*)",
                "read_url(*)",
                "execute_url(*)",
                "download_url(*)",
            ],
        },
    })
}

/// Set up Antigravity's folder for a run: its MCP servers (just ZigZag's,
/// at `mcp_url`: the port can change from one launch to the next) in
/// `.agents/mcp_config.json`, and ZigZag's brief as the folder's
/// `AGENTS.md`, which it reads as standing instructions. It has no flags
/// for either. ZigZag's tools are loaded up front ("eager"), so it calls
/// them by name rather than looking each one up first.
fn write_agy_workspace(dir: &std::path::Path, mcp_url: &str, brief: &str) {
    let tools: serde_json::Map<String, Value> = crate::tools::tool_specs()
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["name"].as_str())
        .map(|n| (n.to_string(), serde_json::json!({ "eager": true })))
        .collect();
    let config = serde_json::json!({
        "mcpServers": { "zigzag": { "serverUrl": mcp_url, "tools": tools } }
    });
    let agents = dir.join(".agents");
    let _ = std::fs::create_dir_all(&agents);
    let _ = std::fs::write(
        agents.join("mcp_config.json"),
        serde_json::to_string_pretty(&config).unwrap_or_default(),
    );
    let _ = std::fs::write(dir.join("AGENTS.md"), format!("{brief}\n"));
}

/// What Antigravity has said so far in a run: its words come in pieces.
#[derive(Default)]
struct AgyStream {
    /// The words of each step still being said (by step).
    text: std::collections::BTreeMap<i64, String>,
    /// The tool steps already shown (a step can be reported more than once).
    started: std::collections::HashSet<i64>,
    /// Something was said this turn (else the result's response is).
    said: bool,
    /// Its tool calls' ids are this run's own: its steps are counted per
    /// chat, and an earlier turn's are in the transcript too.
    run: u64,
}

impl AgyStream {
    fn new() -> AgyStream {
        AgyStream {
            run: fresh_seed(),
            ..Default::default()
        }
    }

    /// The words of `step` (all of them, for `None`), said.
    fn flush(&mut self, step: Option<i64>, out: &mut Vec<(Option<String>, Entry)>) {
        let steps: Vec<i64> = match step {
            Some(k) => vec![k],
            None => self.text.keys().copied().collect(),
        };
        for k in steps {
            if let Some(t) = self.text.remove(&k) {
                let t = t.trim();
                if !t.is_empty() {
                    out.push((None, Entry::Said(t.to_string())));
                    self.said = true;
                }
            }
        }
    }
}

/// A tool's name as Antigravity gives it, as ZigZag's (`mcp_zigzag_add_svg`
/// is `add_svg`).
fn agy_tool_name(n: &str) -> String {
    tool_name(n.trim_start_matches("mcp_zigzag_"))
}

/// Whether Antigravity's result line says the turn worked.
fn agy_succeeded(v: &Value) -> bool {
    let r = if v["result"].is_object() { &v["result"] } else { v };
    r["status"].as_str() == Some("SUCCESS")
}

/// Antigravity's stream-json events (`init`, `step_update`, `result`, each
/// with its details under its own name).
fn agy_event(
    v: &Value,
    session: &mut Option<String>,
    st: &mut AgyStream,
) -> Vec<(Option<String>, Entry)> {
    let mut out = Vec::new();
    let Some(event) = v["event"].as_str() else {
        return out;
    };
    let u = if v[event].is_object() { &v[event] } else { v };
    if let Some(c) = u["conversation_id"].as_str().filter(|c| !c.is_empty()) {
        *session = Some(c.to_string());
    }
    match event {
        "step_update" => {
            let step = u["step_index"].as_i64().unwrap_or(-1);
            let done = u["state"] == "DONE";
            match u["step_type"].as_str() {
                Some("agent_response") => {
                    if let Some(t) = u["text_delta"].as_str() {
                        st.text.entry(step).or_default().push_str(t);
                    }
                    if done {
                        st.flush(Some(step), &mut out);
                    }
                }
                Some("tool") => {
                    // What was said before it comes first.
                    st.flush(None, &mut out);
                    let info = &u["tool_info"];
                    let raw = info["name"]
                        .as_str()
                        .or(u["tool_name"].as_str())
                        .unwrap_or("tool");
                    let params = &info["parameters"];
                    // A tool reached through its MCP caller: the one called.
                    let inner = ["tool_name", "toolName", "tool"]
                        .iter()
                        .find_map(|k| params[*k].as_str());
                    let (name, args) = match inner {
                        Some(t) if raw == "call_mcp_tool" => (
                            agy_tool_name(t),
                            ["arguments", "args", "input"]
                                .iter()
                                .map(|k| &params[*k])
                                .find(|a| !a.is_null())
                                .unwrap_or(&Value::Null),
                        ),
                        _ => (agy_tool_name(raw), params),
                    };
                    let id = Some(format!("agy-{}-{step}", st.run));
                    let args = if args.is_null() { String::new() } else { short(args) };
                    if !done {
                        if st.started.insert(step) {
                            out.push((id, Entry::Tool(name, args, None)));
                        }
                    } else {
                        let error = &info["error"];
                        let ok = error.is_null()
                            || error.as_object().is_some_and(|e| e.is_empty());
                        out.push((id, Entry::Tool(name.clone(), args, Some(ok))));
                        if !ok {
                            let m = error["message"]
                                .as_str()
                                .or(error.as_str())
                                .map(str::trim)
                                .unwrap_or("");
                            out.push((
                                None,
                                Entry::Error(if m.is_empty() {
                                    format!("{name} failed.")
                                } else {
                                    format!("{name}: {m}")
                                }),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        "result" => {
            st.flush(None, &mut out);
            if !st.said
                && let Some(r) = u["response"].as_str().map(str::trim).filter(|r| !r.is_empty())
            {
                out.push((None, Entry::Said(r.to_string())));
            }
            st.said = false;
            if agy_succeeded(v) {
                let turns = u["num_turns"].as_u64().unwrap_or(0);
                let secs = u["duration_seconds"].as_f64().unwrap_or(0.0);
                out.push((None, Entry::Done(format!("{turns} turns · {secs:.0} s"))));
            } else {
                let msg = u["error"]
                    .as_str()
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .map(agy_failure)
                    .unwrap_or_else(|| {
                        format!(
                            "Stopped: {}",
                            u["status"].as_str().unwrap_or("error").to_ascii_lowercase()
                        )
                    });
                out.push((None, Entry::Error(msg)));
            }
        }
        _ => {}
    }
    out
}

/// The prompt as Antigravity is given it, on its command line: the images
/// going with it named (it's let read just those files, and has no other
/// way to be handed them), and never starting with a dash, which could be
/// taken for an option.
fn agy_prompt(text: &str, images: &[PathBuf]) -> String {
    let mut out = String::new();
    if text.starts_with('-') {
        out.push(' ');
    }
    out.push_str(text);
    if !images.is_empty() {
        out.push_str(if images.len() == 1 {
            "\n\nAn image is attached to this message. Read it from this file before answering:"
        } else {
            "\n\nImages are attached to this message. Read them from these files before answering:"
        });
        for p in images {
            out.push_str(&format!("\n{}", p.display()));
        }
    }
    out
}

/// What to say when an Antigravity turn failed with `message`.
fn agy_failure(message: &str) -> String {
    if message.contains("authentication") {
        return "Antigravity isn't signed in. Sign in to it with your Google account, then send again.".into();
    }
    message.to_string()
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
    /// Show these as being worked on, and keep them showing (for pictures
    /// of the app: there's no agent behind it).
    pub(crate) fn pose(&mut self, uids: &[usize]) {
        self.note("set_object", uids);
        self.last = Some(Instant::now() + Duration::from_secs(3600));
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

/// The objects whose bounds box what's being worked on. One object is
/// boxed by itself (an element's fill and outline together). Several are
/// boxed by what holds them all, exactly as selecting that would be: the
/// innermost group they're all in, or else the whole design. So there's
/// one box, never a box round every piece.
fn boxed(shapes: &[zigzag::svg::Shape], uids: &[usize]) -> Vec<usize> {
    let uids: std::collections::HashSet<usize> = uids.iter().copied().collect();
    let mine: Vec<&zigzag::svg::Shape> = shapes.iter().filter(|s| uids.contains(&s.uid)).collect();
    let Some(first) = mine.first() else {
        return Vec::new();
    };
    if mine.iter().all(|s| s.id == first.id) {
        return mine.iter().map(|s| s.uid).collect();
    }
    // The groups every one of them is in (outermost first): the last is
    // the innermost that holds them all.
    let shared = first
        .groups
        .iter()
        .enumerate()
        .take_while(|(k, g)| mine.iter().all(|s| s.groups.get(*k) == Some(*g)))
        .last()
        .map(|(_, g)| g);
    // (What's hidden isn't part of what there is to see, or to select.)
    let held: Vec<usize> = shapes
        .iter()
        .filter(|s| !s.params.hidden() && shared.is_none_or(|g| s.groups.contains(g)))
        .map(|s| s.uid)
        .collect();
    if held.is_empty() {
        mine.iter().map(|s| s.uid).collect()
    } else {
        held
    }
}

/// What a tool call is doing, in a word or two.
pub fn activity(tool: &str) -> &'static str {
    match tool {
        "add_svg" => "Drawing",
        "add_text" | "edit_text" | "list_fonts" => "Lettering",
        "set_object" | "group_colors" => "Restyling",
        "transform_objects" | "move_object" | "align_objects" => "Placing",
        "combine_shapes" => "Combining shapes",
        "rename_object" => "Naming",
        "simplify" | "clean_up" => "Tidying",
        "open_file" | "new_document" | "switch_document" | "list_documents" => "Opening",
        "list_machines" | "machine_status" => "Checking the machine",
        "send_to_skitch" => "Sending to the machine",
        "redo" => "Redoing",
        "duplicate_objects" => "Duplicating",
        "delete_object" => "Removing",
        "round_corners" => "Rounding corners",
        "break_into_columns" | "create_outlines" => "Breaking into parts",
        "get_path" => "Reading",
        "edit_path" => "Reshaping",
        "add_outline" => "Outlining",
        "simplify_design" => "Simplifying",
        "trace_image" => "Tracing",
        "render_preview" => "Looking",
        "check_design" | "measure_stitching" | "get_cuts" => "Checking",
        "offer_choices" => "Offering choices",
        "review_design" => "Reviewing",
        "get_design" | "get_svg" => "Reading",
        "select" => "Selecting",
        "set_document" => "Setting up",
        "batch" => "Making changes",
        "undo" => "Undoing",
        "get_annotations" => "Reading notes",
        "resolve_note" => "Resolving a note",
        "start_variants" => "Setting up versions",
        "save_variant" => "Keeping a version",
        "show_comparison" => "Comparing",
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
pub(crate) fn fingerprint(s: &zigzag::svg::Shape) -> u64 {
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
        self.recall_chat();
        self.agent.poll();
        // Find the agents' programs before they're wanted: the login shell
        // can take a moment, and a first send shouldn't wait on it.
        // (Not in tests: a login shell for every one of them.)
        #[cfg(not(test))]
        {
            let kind = self.agent.kind;
            let _ = self.agent.installed(kind, ctx);
        }
        // Installing or signing in from the panel: done yet?
        if self.agent.setup.busy()
            && self.agent.setup.poll(ctx) == crate::agent_setup::Outcome::Installed
        {
            self.agent.check_again();
        }
        if std::mem::take(&mut self.agent.stopped) && self.review.queued == Some(self.session.rec) {
            self.review.queued = None;
        }
        if self.take_attached() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        // (Where its calls go once it has changed documents itself: only
        // while it works.)
        if self.agent_tab().is_none() {
            self.agent.follow = None;
        }
        self.keep_chat();
        // An agent working on a document that isn't in front: its chat goes
        // on there, and its turn ends on that document.
        if let Some(i) = self.agent_tab().filter(|&i| i != self.tabs.active) {
            self.agent.swap_chat(&mut self.tabs.list[i].chat);
            self.agent.poll();
            self.keep_chat();
            let done = !self.agent.running();
            self.agent.swap_chat(&mut self.tabs.list[i].chat);
            if done {
                // (And a note that was waiting for it goes, to it.)
                self.visit(i, |s| {
                    s.end_turn();
                    s.send_queued(ctx);
                    s.send_queued_notes(ctx);
                });
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        if !self.agent.running() {
            let ended = std::mem::take(&mut self.agent.was_running);
            if ended && !self.agent_showing() {
                self.agent.unread = true;
            }
            // A turn that failed may have failed for want of signing in: ask again.
            if ended && matches!(self.agent.log.last(), Some(Entry::Error(m)) if m != "Stopped.") {
                let kind = self.agent.kind;
                self.agent.setup.recheck(kind);
            }
            self.end_turn();
            self.send_queued(ctx);
            self.send_queued_notes(ctx);
        }
        self.agent.was_running = self.agent.running();
        if self.agent.running() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Its turn is over: what was written meanwhile goes now, as one
    /// message (with the notes pinned for it, as a send would take them),
    /// and anything being written in the box stays there.
    fn send_queued(&mut self, ctx: &egui::Context) {
        if self.agent.queued.is_empty() || self.agent.running() {
            return;
        }
        let draft = std::mem::take(&mut self.agent.prompt);
        self.agent.prompt = std::mem::take(&mut self.agent.queued).join("\n\n");
        self.agent_send(ctx);
        if self.agent.running() {
            self.agent.prompt = draft;
        } else {
            // (It couldn't go: it's in the box to send again.)
            if !draft.trim().is_empty() {
                self.agent.prompt.push_str("\n\n");
                self.agent.prompt.push_str(&draft);
            }
        }
    }

    /// A queued message's Send Now: the turn running ends, and that
    /// message goes at once, in the same chat; the rest stay queued, for
    /// when its turn is done.
    fn send_queued_now(&mut self, k: usize, ctx: &egui::Context) {
        if k >= self.agent.queued.len() {
            return;
        }
        let text = self.agent.queued.remove(k);
        // (Waiting to start, not started: it just goes first.)
        if self.agent.deferred.is_some() {
            self.agent.queued.insert(0, text);
            return;
        }
        let rest = std::mem::take(&mut self.agent.queued);
        if self.agent.running() {
            self.agent.interrupt();
            self.end_turn();
        }
        let draft = std::mem::replace(&mut self.agent.prompt, text);
        self.agent_send(ctx);
        if self.agent.running() {
            self.agent.prompt = draft;
        } else if !draft.trim().is_empty() {
            // (It couldn't go: it's in the box to send again.)
            self.agent.prompt.push_str("\n\n");
            self.agent.prompt.push_str(&draft);
        }
        self.agent.queued.extend(rest);
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
        let result = self.run_tool_caught(name, args);
        // What it added, or else what it changed. (A new document isn't a
        // piece of the design being worked on.)
        let added: Vec<usize> = self
            .doc
            .shapes
            .iter()
            .filter(|s| !before.contains_key(&s.uid))
            .map(|s| s.uid)
            .collect();
        // What the panel's agent made: kept with the prompt it was given.
        // (Not what an undo or redo brings back: that was made before.)
        if !matches!(name, "open_file" | "undo" | "redo") && !added.is_empty() {
            self.keep_prompt(&added);
        }
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
        self.agent_changed();
        result
    }

    /// Keep the running turn's prompt with the objects `uids` it made
    /// (unless that's turned off in Settings). Part of the tool call's
    /// own undo step. (The calls that come while the panel's agent works
    /// are taken to be its: they don't say who sent them.)
    fn keep_prompt(&mut self, uids: &[usize]) {
        if !self.prefs.agent_prompts || !self.agent.running() {
            return;
        }
        let Some(prompt) = self.agent.turn_prompt.clone() else {
            return;
        };
        let idx: Vec<usize> = self
            .doc
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| uids.contains(&s.uid))
            .map(|(i, _)| i)
            .collect();
        if self.doc.set_prompt(&idx, &prompt).is_ok() {
            self.stable = self.doc.clone();
            self.modified = true;
        }
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
        // One box: round the object being worked on, or round what holds
        // them all when there are several (see `boxed`). It sits where the
        // selection's box would, so the two are one box, not one inside
        // the other. What it has moved on from fades the same way:
        // whatever went at one moment, together.
        let touches = &self.agent.work.touches;
        let mut sets: Vec<Option<Instant>> = touches.iter().map(|t| t.gone).collect();
        sets.sort();
        sets.dedup();
        for gone in sets {
            let mut k = 0.0f32;
            let mut uids = Vec::new();
            for t in touches.iter().filter(|t| t.gone == gone) {
                uids.push(t.uid);
                let fade_in = ((now - t.at).as_secs_f32() / 0.2).min(1.0);
                let fade_out =
                    gone.map_or(1.0, |g| 1.0 - (now - g).as_secs_f32() / FADE.as_secs_f32());
                k = k.max((fade_in * fade_out).clamp(0.0, 1.0));
            }
            let mut b = zigzag::geom::Bounds::empty();
            let held: std::collections::HashSet<usize> =
                boxed(&self.doc.shapes, &uids).into_iter().collect();
            for s in self.doc.shapes.iter().filter(|s| held.contains(&s.uid)) {
                let sb = crate::app::shape_bounds(s);
                if !sb.is_empty() {
                    b.add(sb.min);
                    b.add(sb.max);
                }
            }
            if b.is_empty() {
                continue;
            }
            // (The selection box's own margin: `selection_screen_rect`.)
            let r = Rect::from_two_pos(
                self.view.to_screen(canvas, b.min),
                self.view.to_screen(canvas, b.max),
            )
            .expand(4.0);
            working(painter, r, k, time, p.accent);
            if gone.is_none() {
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
            egui::pos2(header.right() - 82.0, header.center().y),
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
    /// dock (it sits at the bottom), or of a floating panel's fixed height
    /// (queued messages and all, it stays the same size).
    fn agent_transcript_h(&self) -> f32 {
        let h = match self.dock_ui.pinned_h {
            Some(h) if !self.dock_ui.in_window => h,
            _ => self
                .dock
                .agent_float_h
                .min(self.dock_ui.window_max.unwrap_or(f32::INFINITY)),
        };
        let pad = crate::theme::pad().sum().y;
        (h - crate::dock::HEADER_H - pad - self.agent.chrome_h).max(120.0)
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

    /// The issues popover's Fix: the design's issues to the agent, as a
    /// message in its chat (the panel opened to show it), with the objects
    /// they're about selected for it to start on. A message being written
    /// there is kept for after, and so are the notes pinned on the canvas.
    pub fn fix_issues_with_agent(&mut self, ctx: &egui::Context) {
        if self.issues.is_empty() || self.agent.running() {
            return;
        }
        let mut lines = Vec::new();
        let mut objects = Vec::new();
        for (k, issue) in self.issues.iter().enumerate() {
            let what = match issue.shape {
                Some(i) => {
                    if !objects.contains(&i) {
                        objects.push(i);
                    }
                    self.names.get(i).cloned().unwrap_or_default()
                }
                None => "The design".to_string(),
            };
            lines.push(format!("{}. {what}: {}", k + 1, issue.message));
        }
        let prompt = format!(
            "Fix these problems the studio found in the design (its Issues list):\n\n{}\n\nChange only what each one needs, and keep the design looking as it does.",
            lines.join("\n")
        );
        if !objects.is_empty() {
            self.set_selection(&objects);
        }
        let draft = std::mem::replace(&mut self.agent.prompt, prompt);
        self.ask_agent();
        // (On their own: notes pinned on the canvas wait for a message of
        // the user's.)
        self.agent_send_with(ctx, false);
        // What was being written goes back in the box, sent or not (an
        // agent that didn't start says so in the chat).
        self.agent.prompt = draft;
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
        // (Where it is, for an image dropped on it next frame.)
        self.agent.panel_rect = Some(ui.max_rect());
        // What starting the agent needs, looked up while the panel's open,
        // before a prompt is sent.
        self.agent.warm_up(ui.ctx());
        let p = pal_ui(ui);
        let h = self.agent_transcript_h();
        let mut send = false;
        let top = ui.cursor().top();

        let review = &self.review;
        let here = self.session.rec;
        let mut act: Option<crate::review::TurnAction> = None;
        let mut open_earlier: Option<String> = None;
        let mut apply_choices: Option<usize> = None;
        let busy = self.agent.running();
        let has_design = !self.doc.shapes.is_empty();
        let a = &mut self.agent;
        // (A new chat gets new ideas.)
        if !a.log.is_empty() {
            a.ideas_seed = None;
        }
        // (Setup fits the panel as it is: less room above it.)
        let top_pad = if a.setup_page() && !a.running() { space::S } else { space::M };
        // The conversation: plain, on the panel itself.
        let transcript_h = egui::Frame::new()
            // (Room round the conversation, and between its lines.)
            .inner_margin(egui::Margin {
                left: space::XS as i8,
                right: space::XS as i8,
                top: top_pad as i8,
                bottom: space::M as i8,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let inner_h = h - top_pad - space::M;
                // (Setup shows its card even over a chat: picking the agent
                // again brings the chat back.)
                if (a.log.is_empty() || a.setup_page()) && !a.running() {
                    let (r, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), inner_h),
                        Sense::hover(),
                    );
                    // Its program isn't on this computer: how to install it
                    // instead. Installed but not signed in: how to sign in.
                    let kind = a.kind;
                    let ctx = ui.ctx().clone();
                    let _ = a.installed(kind, &ctx);
                    let card = if a.setup_page() {
                        Some(true)
                    } else if setting_up(a, false) || signed_out(a, &ctx) {
                        Some(false)
                    } else {
                        None
                    };
                    if let Some(install) = card {
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(r)
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                        );
                        // (From the top, like the conversation.)
                        // (Scrolls when the terminal steps are open: they don't fit.)
                        egui::ScrollArea::vertical()
                            .id_salt("agent-setup-card")
                            // (A press is a click on what it holds, not a drag.)
                            .scroll_source(egui::containers::scroll_area::ScrollSource { scroll_bar: true, mouse_wheel: true, ..egui::containers::scroll_area::ScrollSource::NONE })
                            .max_height(child.available_height())
                            .auto_shrink([false, true])
                            .show(&mut child, |ui| {
                                ui.set_width(ui.available_width() - space::S);
                                if install {
                                    install_steps(ui, a);
                                } else {
                                    sign_in_steps(ui, a);
                                }
                            });
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
                            .size(PANEL_PT)
                            .color(p.secondary),
                        )
                        .halign(egui::Align::Center)
                        .wrap(),
                    );
                    child.add_space(space::M);
                    // A little air between the ideas, and room inside each.
                    child.spacing_mut().item_spacing.y = space::S;
                    child.spacing_mut().button_padding = Vec2::new(12.0, 6.0);
                    let seed = *a.ideas_seed.get_or_insert_with(fresh_seed);
                    for idea in ideas(seed, has_design) {
                        if child
                            .add(egui::Button::new(egui::RichText::new(idea).size(PANEL_PT)).corner_radius(10))
                            .clicked()
                        {
                            a.prompt = idea.to_string();
                            a.focus = true;
                        }
                    }
                    // This design's last chat, too old to have come back by
                    // itself.
                    if let Some(m) = &a.earlier {
                        child.add_space(space::S);
                        let title: String = m.title.chars().take(40).collect();
                        if child
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(format!("Earlier chat · {} · “{title}”", m.ago()))
                                        .size(PANEL_PT)
                                        .color(p.secondary),
                                )
                                .frame(false),
                            )
                            .on_hover_text("Open it, to read it or carry on")
                            .clicked()
                        {
                            open_earlier = Some(m.id.clone());
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
                        // (Room between the conversation's lines.)
                        ui.spacing_mut().item_spacing.y = FEED_GAP;
                        let time = ui.input(|i| i.time);
                        // The turns before the last few fold into a line,
                        // and aren't laid out until it's opened.
                        let mut k = 0;
                        let prompts: Vec<usize> = a
                            .log
                            .iter()
                            .enumerate()
                            .filter(|(_, e)| matches!(e, Entry::You(_)))
                            .map(|(i, _)| i)
                            .collect();
                        if !a.show_earlier && prompts.len() > SHOWN_TURNS {
                            let folded = prompts.len() - SHOWN_TURNS;
                            k = prompts[folded];
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(format!(
                                            "{folded} earlier turn{}",
                                            if folded == 1 { "" } else { "s" }
                                        ))
                                        .size(PANEL_PT)
                                        .color(p.secondary),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text("Show the whole chat")
                                .clicked()
                            {
                                a.show_earlier = true;
                            }
                        }
                        while k < a.log.len() {
                            // Steps one after another are one line, opened
                            // out with a click (one alone is just itself).
                            let run = a.log[k..]
                                .iter()
                                .take_while(|e| matches!(e, Entry::Tool(..)))
                                .count();
                            if run >= 1 {
                                let open = a.open_steps.contains(&k);
                                if steps(ui, k, &a.log[k..k + run], open, time) {
                                    if open {
                                        a.open_steps.remove(&k);
                                    } else {
                                        a.open_steps.insert(k);
                                    }
                                }
                                if open {
                                    opened(ui, |ui| {
                                        for e in &a.log[k..k + run] {
                                            entry(ui, e, time);
                                        }
                                    });
                                }
                                k += run;
                            } else if matches!(a.log[k], Entry::Choices { .. }) {
                                if choices_card(ui, &mut a.log[k], busy) {
                                    apply_choices = Some(k);
                                }
                                k += 1;
                            } else {
                                // A turn that ended here: its before, after and
                                // side by side, on its Done line (or under how
                                // it ended, if it didn't finish).
                                let ended = review.turn_ending_at(k, here);
                                let done = match &a.log[k] {
                                    Entry::Done(t) => Some(t),
                                    _ => None,
                                };
                                match (ended, done) {
                                    (Some(t), done) => {
                                        if done.is_none() {
                                            entry(ui, &a.log[k], time);
                                        }
                                        // Done, and at the right end of its line,
                                        // the turn's view buttons.
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = space::S;
                                            if let Some(text) = done {
                                                done_line(ui, text);
                                            }
                                            let w = crate::review::turn_controls_width(t);
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.allocate_ui_with_layout(
                                                        Vec2::new(w, crate::review::BUTTON),
                                                        egui::Layout::left_to_right(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            if let Some(x) =
                                                                crate::review::turn_controls(
                                                                    ui, review, t, here,
                                                                )
                                                            {
                                                                act = Some(x);
                                                            }
                                                        },
                                                    );
                                                },
                                            );
                                        });
                                    }
                                    (None, _) => entry(ui, &a.log[k], time),
                                }
                                k += 1;
                            }
                        }
                        // Between steps: thinking (a step running shows its own spinner).
                        if a.running() && !matches!(a.log.last(), Some(Entry::Tool(_, _, None))) {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                                spinner(ui.painter(), r.center(), 4.5, p.accent, time);
                                let secs = a.started.map_or(0, |t| t.elapsed().as_secs());
                                quiet(ui, format!("{}… {secs} s", a.status()));
                            });
                        }
                    });
            })
            .response
            .rect
            .height();
        // The notes pinned for the next prompt, and the ways to add more.
        let has_notes = self.review.pending(self.session.rec) > 0;
        let annotating = self.review.annotating;
        let mut annotate = false;
        // What's waiting for its turn to end, just above the prompt.
        if !self.agent.queued.is_empty() {
            if let Some(k) = queued_box(ui, &mut self.agent.queued, &p) {
                self.send_queued_now(k, ui.ctx());
            }
            ui.add_space(space::S);
        }
        // Just a divider between the conversation and the prompt (none on
        // Setup, which has no prompt).
        if !(self.agent.setup_page() && !self.agent.running()) {
            let (line, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
            ui.painter().hline(
                line.x_range(),
                line.center().y,
                Stroke::new(1.0, p.separator),
            );
        }
        // The comments pinned on the canvas, as an attachment to the prompt.
        let notes_act = self.notes_list(ui);
        // The prompt: one line (it grows with Shift-Enter), Send beside it.
        let field_id = ui.make_persistent_id("agent-prompt");
        // A slash command being typed: its menu, and the menu's keys
        // (taken before the field sees them).
        let prompt = self.agent.prompt.clone();
        let rows = if ui.memory(|m| m.has_focus(field_id))
            && self.agent.menu_closed.as_deref() != Some(prompt.as_str())
        {
            crate::slash::rows(self, &prompt, ui.ctx())
        } else {
            Vec::new()
        };
        let mut pick = self.slash_keys(ui, &rows);
        // An image pasted into the prompt goes with it.
        let pasted: Vec<std::sync::Arc<egui::ColorImage>> = if ui.memory(|m| m.has_focus(field_id))
        {
            ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::PasteImage(img) => Some(img.clone()),
                        _ => None,
                    })
                    .collect()
            })
        } else {
            Vec::new()
        };
        for img in pasted {
            if !self.attach_pasted(&img) {
                self.status = "Couldn't attach that image.".into();
            }
        }
        let mut field_rect = None;
        let a = &mut self.agent;
        let busy_now = a.running();
        // Setup has no agent to write to: no prompt.
        let setup_view = a.setup_page() && !busy_now;
        // (Above it, its own space; below, the panel's padding is enough.)
        let input = if setup_view {
            Rect::NOTHING
        } else {
            egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 0,
                right: 0,
                top: space::S as i8,
                bottom: 0,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // A chat that stopped for want of the program, or of signing
                // in: how to put that right, above where they'd write next.
                if !a.log.is_empty() && !busy_now {
                    let ctx = ui.ctx().clone();
                    // (Not installed, or installing: the Setup page has the
                    // panel then, and there's no prompt to stand above.)
                    if setting_up(a, false) || signed_out(a, &ctx) {
                        egui::Frame::new()
                            .fill(p.control)
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::same(space::M as i8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                // (Room left for the conversation above it.)
                                egui::ScrollArea::vertical()
                                    .id_salt("agent-setup-banner")
                            // (A press is a click on what it holds, not a drag.)
                            .scroll_source(egui::containers::scroll_area::ScrollSource { scroll_bar: true, mouse_wheel: true, ..egui::containers::scroll_area::ScrollSource::NONE })
                                    .max_height(320.0)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| sign_in_steps(ui, a));
                            });
                        ui.add_space(space::S);
                    }
                }
                // The images going with it, each with a × to take it off.
                if !a.attach.is_empty() {
                    let mut gone = None;
                    ui.horizontal(|ui| {
                        ui.add_space(space::S);
                        ui.spacing_mut().item_spacing.x = space::S;
                        for (k, path) in a.attach.iter().enumerate() {
                            let r = crate::attach::show(ui, path, crate::attach::THUMB);
                            let x = Rect::from_center_size(
                                r.rect.right_top() + Vec2::new(-5.0, 5.0),
                                Vec2::splat(15.0),
                            );
                            let xr = ui.interact(x, ui.make_persistent_id(("attach-x", k)), Sense::click());
                            xr.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Button, true, "Remove Image")
                            });
                            ui.painter().circle_filled(
                                x.center(),
                                7.5,
                                if xr.hovered() { p.text } else { p.secondary },
                            );
                            icons::paint(ui.painter(), x.shrink(4.0), Icon::Close, p.dock);
                            if xr.on_hover_text("Remove image").clicked() {
                                gone = Some(k);
                            }
                        }
                    });
                    if let Some(k) = gone {
                        // (The copy made to send goes with it.)
                        let gone = a.attach.remove(k);
                        let _ = std::fs::remove_file(gone);
                    }
                    ui.add_space(space::XS);
                }
                ui.horizontal(|ui| {
                    let button_w = crate::review::BUTTON;
                    let button_h = crate::review::BUTTON;
                    ui.add_space(space::S);
                    // (Room for Annotate and Send at the right.)
                    let field_w = ui.available_width() - 2.0 * button_w - 2.0 * space::S;
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
                            .font(egui::FontId::proportional(crate::review::FIELD_PT))
                            // (One hint, working or not: a longer one wrapped
                            // to two lines, and the box jumped when typed in.)
                            .hint_text(
                                egui::RichText::new("Describe a design, or a change…")
                                    .color(crate::review::hint_color(p)),
                            )
                            // (Enter sends, below; Shift-Enter is the
                            // field's own new line, at the caret.)
                            .return_key(egui::KeyboardShortcut::new(
                                egui::Modifiers::SHIFT,
                                Key::Enter,
                            )),
                    );
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Prompt")
                    });
                    field_rect = Some(r.rect);
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
                    let ok = !a.prompt.trim().is_empty() || has_notes || !a.attach.is_empty();
                    let running = a.running();
                    // Beside Send: pin comments on the canvas.
                    if crate::review::annotate_button(ui, annotating)
                        .on_hover_text("Annotate: click the canvas to pin a comment for the agent, or drag a box round an area (N)")
                        .clicked()
                    {
                        annotate = true;
                    }
                    if crate::review::send_button(ui, ok, running).clicked() {
                        // (A Stop in the first moment of a run is the second
                        // click of a Send, not a change of mind.)
                        let just_sent = a
                            .started
                            .is_some_and(|t| t.elapsed() < Duration::from_millis(900));
                        if running && !just_sent {
                            a.stop();
                        } else if !running {
                            send = true;
                        }
                    }
                });
            })
            .response
            .rect
        };
        // Everything but the transcript, to size it next frame.
        let chrome_h = (ui.cursor().top() - top - transcript_h).max(0.0);
        // (The transcript is sized from this: when the input area grows or
        // shrinks, draw the frame again at once rather than show one with
        // the old size, which jolts as you type.)
        if (chrome_h - a.chrome_h).abs() > 0.5 {
            ui.ctx().request_discard("the Agent panel's input height");
        }
        a.chrome_h = chrome_h;
        // (Out of the top of the input area, as wide as it.)
        if field_rect.is_some()
            && !rows.is_empty()
            && let Some(k) = crate::slash::menu(ui, &rows, self.agent.menu_sel, input)
        {
            pick = Some((k, true));
        }
        if let Some((k, run)) = pick {
            self.slash_pick(&rows[k], run, ui.ctx());
            // The caret at the end of what was filled in.
            if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), field_id) {
                let end = egui::text::CCursor::new(self.agent.prompt.chars().count());
                st.cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(end)));
                st.store(ui.ctx(), field_id);
            }
        }
        if annotate {
            self.set_annotating(!annotating);
        }
        if let Some(x) = notes_act {
            self.notes_action(x);
        }
        if let Some(x) = act {
            self.turn_action(x);
        }
        if let Some(id) = open_earlier {
            self.open_chat(&id);
        }
        if let Some(k) = apply_choices {
            self.apply_choices(k);
        }
        if send {
            self.agent_send(ui.ctx());
            // Drawn again at once: the button is Stop and the spinner turns
            // now, not when something else wakes the window.
            ui.ctx().request_repaint();
        }
    }

    /// Send the prompt. What's selected is what the agent starts on; the
    /// notes pinned on the canvas go with it; and the document as it is
    /// now is kept, to compare with what the agent makes of it.
    pub fn agent_send(&mut self, ctx: &egui::Context) {
        self.agent_send_with(ctx, true);
    }

    /// The names of the selected objects (the first few dozen), as the
    /// agent is told them.
    pub(crate) fn selected_names(&self) -> Vec<String> {
        self.sel_indices()
            .iter()
            .filter_map(|&i| self.names.get(i).cloned())
            .take(40)
            .collect()
    }

    /// Send the prompt, with the canvas's notes or (`notes` false) on its
    /// own, the notes left open for a later message.
    fn agent_send_with(&mut self, ctx: &egui::Context, notes: bool) {
        let here = self.session.rec;
        let has_notes = notes && self.review.pending(here) > 0;
        // A slash command runs here (it isn't sent), even while the agent works.
        if self.agent.prompt.trim_start().starts_with('/') && Studio::is_slash(&self.agent.prompt) {
            let text = self.agent.prompt.clone();
            self.run_slash(&text, ctx);
            return;
        }
        // The agent works on one document at a time: the prompt waits.
        if let Some(i) = self.agent_tab().filter(|&i| i != self.tabs.active) {
            let name = self.tab_infos()[i].name.clone();
            self.agent.log.push(Entry::Info(format!(
                "{} is working on {name}: it works on one document at a time. Send this when it's done, or type /stop to stop it.",
                self.agent.kind.name()
            )));
            return;
        }
        // Written while it works: it goes when the turn is done.
        if self.agent.running() {
            let text = self.agent.prompt.trim().to_string();
            if !text.is_empty() {
                self.agent.queued.push(text);
                self.agent.prompt.clear();
            }
            return;
        }
        if self.agent.prompt.trim().is_empty() && !has_notes && self.agent.attach.is_empty() {
            return;
        }
        let addr = self.mcp.as_ref().map(|m| m.addr);
        self.agent.mcp_off = addr.is_none().then(|| match (&self.mcp_ui.start_error, self.mcp_ui.port) {
            (Some(e), Some(port)) => format!(
                "couldn't start on port {port} ({e}): another copy of ZigZag may be using it. Quit that one, or pick another port in Settings and reopen ZigZag"
            ),
            (Some(e), None) => format!("couldn't start ({e})"),
            (None, _) => "is off (in Settings, or started with --no-mcp)".to_string(),
        });
        let (shown, mut extra) = if notes {
            self.notes_for_prompt()
        } else {
            (Vec::new(), String::new())
        };
        // What's selected is what "this" means: the agent can't see the
        // canvas's selection otherwise.
        // What they chose from the versions it made, if they did.
        if let Some((_, chose)) = self.review.chose.clone().filter(|(tab, _)| *tab == here) {
            if !extra.is_empty() {
                extra.push_str("\n\n");
            }
            extra.push_str(&format!(
                "From the versions you made, the person chose {chose}: the design is that one now."
            ));
        }
        let selected = self.selected_names();
        if !selected.is_empty() {
            if !extra.is_empty() {
                extra.push_str("\n\n");
            }
            extra.push_str(&format!(
                "Selected on the canvas (what \"this\" or \"these\" means): {}.",
                selected.join(", ")
            ));
        }
        let prompt = self.agent.prompt.trim().to_string();
        let start = self.agent.log.len();
        self.agent.send_with(addr, ctx, shown, &extra);
        self.keep_chat();
        if self.agent.running() {
            self.review.chose = self.review.chose.take().filter(|(tab, _)| *tab != here);
            self.agent.turn_prompt = (!prompt.is_empty()).then(|| prompt.clone());
            if notes {
                self.begin_turn(prompt, start);
            } else {
                self.begin_turn_without_notes(prompt, start);
            }
            let sel: Vec<usize> = self
                .sel_indices()
                .iter()
                .filter_map(|&i| self.doc.shapes.get(i).map(|s| s.uid))
                .collect();
            self.agent.work.note("get_design", &sel);
        }
    }
}

/// The most queued messages listed at once (the rest are counted).
const QUEUED_SHOWN: usize = 3;

/// The messages waiting for the agent's turn to end: a darker box along
/// the top of the prompt, as wide as it, one line each (a × to take it
/// back on the one under the pointer), divided by hairlines; the first
/// few, then how many more. Beside the ×, a send arrow (Send Now): the
/// row it's clicked on, if it is.
fn queued_box(ui: &mut egui::Ui, queued: &mut Vec<String>, p: &crate::theme::Pal) -> Option<usize> {
    const ROW_H: f32 = 30.0;
    const MORE_H: f32 = 26.0;
    const X: f32 = 16.0;
    let shown = queued.len().min(QUEUED_SHOWN);
    let more = queued.len() - shown;
    let h = shown as f32 * ROW_H + if more > 0 { MORE_H } else { 0.0 } + 2.0 * space::XS;
    // As wide as the prompt row: from under its text to the Send button.
    let (outer, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    let r = Rect::from_min_max(egui::pos2(outer.left() + space::S, outer.top()), outer.max);
    let painter = ui.painter().clone();
    // The dividers' and the outline's colour: darker than the box itself.
    let groove = {
        let k = if p.dark { 0.55 } else { 0.85 };
        let c = p.desk;
        Color32::from_rgb(
            (c.r() as f32 * k) as u8,
            (c.g() as f32 * k) as u8,
            (c.b() as f32 * k) as u8,
        )
    };
    painter.rect(r, 8.0, p.desk, Stroke::new(1.0, groove), egui::StrokeKind::Inside);
    // The text's column, and the ×'s.
    let left = r.left() + space::M;
    let right = r.right() - space::S;
    let line = |y: f32| {
        painter.hline(r.x_range().shrink(1.0), y, Stroke::new(1.0, groove));
    };
    let mut gone = None;
    let mut now = None;
    let mut y = r.top() + space::XS;
    for (k, text) in queued.iter().enumerate().take(shown) {
        let row = Rect::from_min_max(egui::pos2(left, y), egui::pos2(right, y + ROW_H));
        // A hairline between each and the one above it.
        if k > 0 {
            line(row.top());
        }
        let x = Rect::from_center_size(
            egui::pos2(right - X / 2.0, row.center().y),
            Vec2::splat(X),
        );
        // Send Now, with room between it and the ×.
        let send = x.translate(Vec2::new(-(X + space::M), 0.0));
        let label = Rect::from_min_max(row.min, egui::pos2(send.left() - space::S, row.max.y));
        // (In a child of its own: placed by the parent, it would move the
        // parent's cursor back up to it.)
        let mut cell = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(label)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        cell.add(
            egui::Label::new(
                egui::RichText::new(text.split_whitespace().collect::<Vec<_>>().join(" "))
                    .size(PANEL_PT)
                    .color(p.text),
            )
            .truncate(),
        )
        .on_hover_text(text.as_str());
        // Its × only while the pointer's on it (the whole row, edge to edge).
        let over = ui.rect_contains_pointer(Rect::from_x_y_ranges(r.x_range(), row.y_range()));
        let xr = ui.interact(x, ui.make_persistent_id(("queued-x", k)), Sense::click());
        xr.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, "Remove Queued Message")
        });
        if over {
            painter.circle_filled(
                x.center(),
                X / 2.0,
                if xr.hovered() { p.text } else { p.secondary },
            );
            icons::paint(&painter, x.shrink(4.5), Icon::Close, p.desk);
        }
        if over && xr.on_hover_text("Don't send this").clicked() {
            gone = Some(k);
        }
        let sr = ui.interact(send, ui.make_persistent_id(("queued-send", k)), Sense::click());
        sr.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Send Queued Message Now"));
        if over {
            painter.circle_filled(
                send.center(),
                X / 2.0,
                if sr.hovered() { p.text } else { p.secondary },
            );
            icons::paint(&painter, send.shrink(3.5), Icon::ArrowUp, p.desk);
        }
        if over && sr.on_hover_text("Send now: stops what it's doing").clicked() {
            now = Some(k);
        }
        y += ROW_H;
    }
    if more > 0 {
        line(y);
        painter.text(
            egui::pos2(left, y + MORE_H / 2.0),
            egui::Align2::LEFT_CENTER,
            format!("{more} more"),
            egui::FontId::proportional(PANEL_PT - 1.0),
            p.tertiary,
        );
    }
    ui.advance_cursor_after_rect(outer);
    if let Some(k) = gone {
        queued.remove(k);
        return None;
    }
    now
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
            you_bubble(ui, |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(t).size(PANEL_PT).color(p.text)).wrap(),
                );
            });
        }
        Entry::Said(t) => {
            ui.add(egui::Label::new(egui::RichText::new(t).size(PANEL_PT).color(p.text)).wrap());
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
                    .size(PANEL_PT)
                    .color(if state.is_none() {
                        p.secondary
                    } else {
                        p.tertiary
                    }),
                );
                let dim = crate::review::hint_color(p);
                ui.label(
                    egui::RichText::new(name)
                        .size(PANEL_PT)
                        .monospace()
                        .color(dim),
                );
                if !args.is_empty() {
                    ui.add(
                        egui::Label::new(egui::RichText::new(args).size(PANEL_PT).color(dim))
                            .truncate(),
                    )
                    .on_hover_text(args);
                }
            });
        }
        // The comments pinned on the canvas that went with it: each one a
        // message like the person's own, a comment's icon at its right.
        Entry::Notes(notes) => {
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = space::XS;
                for n in notes {
                    you_bubble(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = space::S;
                            let icon = crate::review::ICON - 2.0;
                            let w = ui.available_width() - icon - space::S;
                            ui.allocate_ui_with_layout(
                                Vec2::new(w, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_min_width(w);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(n).size(PANEL_PT).color(p.text),
                                        )
                                        .wrap(),
                                    );
                                },
                            );
                            // At the right: the canvas comment's icon, outlined.
                            let line = ui.text_style_height(&egui::TextStyle::Body);
                            let (r, _) =
                                ui.allocate_exact_size(Vec2::new(icon, line), Sense::hover());
                            icons::paint(
                                ui.painter(),
                                Rect::from_center_size(r.center(), Vec2::splat(icon)),
                                Icon::Comment,
                                p.text,
                            );
                        });
                    });
                }
            });
        }
        Entry::Error(t) => crate::theme::callout(ui, p.warning, Icon::Warning, t),
        Entry::Done(t) => done_line(ui, t),
        // A choice that was made reads like the steps the agent made: a dot.
        Entry::Info(t) if t.starts_with("Applied ") => {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::XS;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, p.accent);
                ui.add(
                    egui::Label::new(egui::RichText::new(t).size(PANEL_PT).color(p.text)).wrap(),
                );
            });
        }
        Entry::Info(t) => {
            quiet(ui, t.as_str());
        }
        // (Drawn as a card, with its boxes, where the chat is shown: see
        // `choices_card`. Anywhere else, its title.)
        Entry::Choices { title, .. } => {
            quiet(ui, title.as_str());
        }
        // Under the prompt they went with, at its side.
        Entry::Images(paths) => {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                ui.spacing_mut().item_spacing.x = space::XS;
                for p in paths.iter().rev() {
                    crate::attach::show(ui, p, 120.0);
                }
            });
        }
    }
}

/// The Agent panel's one text size: what matters more is brighter, not bigger.
const PANEL_PT: f32 = crate::review::PANEL_PT;

/// A quieter line in the Agent panel (the panel's size, a dimmer color).
fn quiet(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    let p = pal_ui(ui);
    ui.add(
        egui::Label::new(
            egui::RichText::new(text.into())
                .size(PANEL_PT)
                .color(p.tertiary),
        )
        .wrap(),
    )
}

/// Between the lines of the conversation.
const FEED_GAP: f32 = 16.0;

/// The person's message: a soft blue box the panel's width.
fn you_bubble(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let p = pal_ui(ui);
    egui::Frame::new()
        .fill(p.accent.gamma_multiply(0.18))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(space::M as i8, space::S as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// "Done · 6 turns · 8 s", dim (nothing, if there's nothing to say).
fn done_line(ui: &mut egui::Ui, t: &str) {
    if !t.is_empty() {
        let p = pal_ui(ui);
        ui.label(
            egui::RichText::new(format!("Done · {t}"))
                .size(PANEL_PT)
                .color(crate::review::hint_color(p)),
        );
    }
}

/// Steps one after another, as one line (a click opens it out): how they went, and
/// what they were (`Checking ×12 · Reading ×7`); the one running, while
/// one is. `first`: the first step's place in the log (the line's id).
/// Returns whether it was clicked (to open it out, or fold it away).
fn steps(ui: &mut egui::Ui, first: usize, run: &[Entry], open: bool, time: f64) -> bool {
    let p = pal_ui(ui);
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    let mut failed = 0;
    let mut current = None;
    for e in run {
        if let Entry::Tool(name, _, state) = e {
            let what = activity(name);
            match counts.iter_mut().find(|c| c.0 == what) {
                Some(c) => c.1 += 1,
                None => counts.push((what, 1)),
            }
            match state {
                Some(false) => failed += 1,
                None => current = Some(name.as_str()),
                _ => {}
            }
        }
    }
    let mut what: Vec<String> = counts
        .iter()
        .map(|(w, n)| {
            if *n > 1 {
                format!("{w} ×{n}")
            } else {
                w.to_string()
            }
        })
        .collect();
    if failed > 0 {
        what.push(format!("{failed} failed"));
    }
    let n = run.len();
    let count = if n == 1 {
        "1 step".to_string()
    } else {
        format!("{n} steps")
    };
    let row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::XS;
        // How they went: a spinner while one runs, a warning if one failed,
        // else a dot.
        let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
        match current {
            Some(_) => spinner(ui.painter(), r.center(), 4.0, p.accent, time),
            None if failed > 0 => icons::paint(ui.painter(), r, Icon::Warning, p.warning),
            None => {
                ui.painter()
                    .circle_filled(r.center(), 3.5, p.accent.gamma_multiply(0.7));
            }
        }
        let (title, rest) = match current {
            Some(name) => (
                format!("{}…", activity(name)),
                format!("{count} · {}", what.join(" · ")),
            ),
            None => (count.clone(), what.join(" · ")),
        };
        // (Quieter than what was said: the steps are how, not what.)
        ui.label(
            egui::RichText::new(title)
                .size(PANEL_PT)
                .color(if current.is_some() {
                    p.secondary
                } else {
                    p.tertiary
                }),
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(rest)
                    .size(PANEL_PT)
                    .color(crate::review::hint_color(p)),
            )
            .truncate(),
        );
    });
    let resp = ui
        .interact(
            row.response.rect,
            ui.make_persistent_id(("agent-steps", first)),
            Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            true,
            format!(
                "{} {count}: {}",
                if open { "Hide" } else { "Show" },
                what.join(" · ")
            ),
        )
    });
    if resp.hovered() {
        ui.painter().rect_filled(
            row.response.rect.expand2(Vec2::new(3.0, 1.0)),
            4.0,
            p.text.gamma_multiply(0.05),
        );
    }
    resp.clicked()
}

/// A run of steps opened out: each one, set in under its line.
fn opened(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let p = pal_ui(ui);
    let r = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 18,
            right: 0,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = space::XS;
            add(ui);
        })
        .response
        .rect;
    ui.painter()
        .vline(r.left() + 6.0, r.y_range(), Stroke::new(1.0, p.separator));
}

/// The agent's command-line tool isn't installed: the commands to install
/// it (with Copy buttons), how to sign in, Check Again, and the other agent
/// if that one is installed.
/// Whether the panel is installing (`install`) or signing in to the agent
/// it runs, or that just failed.
fn setting_up(a: &AgentState, install: bool) -> bool {
    use crate::agent_setup::Step;
    match &a.setup.step {
        Step::Installing(j) => install && j.kind == a.kind,
        Step::SigningIn(j) => !install && j.kind == a.kind,
        Step::Failed { kind, install: i, .. } => *i == install && *kind == a.kind,
        Step::Idle => false,
    }
}

/// The agent is installed and plainly not signed in (asked once, off the
/// UI thread).
fn signed_out(a: &mut AgentState, ctx: &egui::Context) -> bool {
    let kind = a.kind;
    match a.found[kind.index()].clone() {
        Some(Some(program)) => {
            let path = a.path.clone().flatten();
            a.setup.needs_sign_in(kind, &program, path, ctx)
        }
        _ => false,
    }
}

/// The getting-started guide on the website.
const GUIDE_URL: &str = "https://zigzag.diy/guides/ai-assistant/";

/// A spinner and a line beside it.
fn working_line(ui: &mut egui::Ui, text: &str) {
    let p = pal_ui(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
        spinner(ui.painter(), r.center(), 5.5, p.accent, ui.input(|i| i.time));
        ui.label(egui::RichText::new(text).size(PANEL_PT).color(p.text));
    });
}

/// A setup card's title.
fn setup_title(ui: &mut egui::Ui, text: &str) {
    let p = pal_ui(ui);
    ui.label(egui::RichText::new(text).size(PANEL_PT + 1.0).strong().color(p.text));
}

/// Body text under a setup card's title: the panel's size, one step dimmer.
fn setup_text(ui: &mut egui::Ui, text: impl Into<String>) {
    let p = pal_ui(ui);
    ui.add(egui::Label::new(egui::RichText::new(text.into()).size(PANEL_PT).color(p.secondary)).wrap());
}

/// Open a web page in the browser. (This build's eframe has no opener of
/// its own: egui's `open_url` would go nowhere. In tests it's only noted,
/// in `OPENED`, so a test can see it and no browser opens.)
pub(crate) fn open_web(ctx: &egui::Context, url: &str) {
    #[cfg(test)]
    {
        ctx.open_url(egui::OpenUrl::new_tab(url));
        OPENED.lock().unwrap().push(url.to_string());
    }
    #[cfg(not(test))]
    {
        let _ = ctx;
        crate::sew::open_url(url);
    }
}

/// The pages tests have asked to open.
#[cfg(test)]
pub(crate) static OPENED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A link that opens in the browser.
fn web_link(ui: &mut egui::Ui, text: &str, url: &str) {
    let p = pal_ui(ui);
    let r = ui.add(
        egui::Label::new(egui::RichText::new(text).size(PANEL_PT).color(p.accent))
            // (Selectable text would take the click.)
            .selectable(false)
            .sense(Sense::click()),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, text));
    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(url);
    if r.clicked() {
        open_web(ui.ctx(), url);
    }
}

/// The beginner's guide, as a row of its own: which assistant to pick,
/// what it costs and how to start. The whole row opens it.
fn guide_row(ui: &mut egui::Ui) {
    let p = pal_ui(ui);
    let inner = egui::Frame::new()
        .stroke(Stroke::new(1.0, p.accent))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(space::M as i8, space::S as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::S + 2.0;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Book, p.accent);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    // (Not selectable: a click anywhere is the row's.)
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("New to AI assistants?")
                                .size(PANEL_PT)
                                .strong()
                                .color(p.text),
                        )
                        .selectable(false),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("Read the guide to get started")
                                .size(PANEL_PT)
                                .color(p.text),
                        )
                        .wrap()
                        .selectable(false),
                    );
                });
            });
        });
    let r = ui.interact(inner.response.rect, ui.make_persistent_id("guide-row"), Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(inner.response.rect, 8.0, p.hover);
    }
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, "Read the guide"));
    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(GUIDE_URL);
    if r.clicked() {
        open_web(ui.ctx(), GUIDE_URL);
    }
}

/// No agent to run yet: each one ZigZag works with, to install (or use, if
/// it's here already), with how to do it in a terminal folded away.
fn install_steps(ui: &mut egui::Ui, a: &mut AgentState) {
    use crate::agent_setup::Step;
    let p = pal_ui(ui);
    let ctx = ui.ctx().clone();
    ui.spacing_mut().item_spacing.y = space::S;
    setup_title(ui, "Set up an AI assistant");
    setup_text(
        ui,
        "It works on your design while you watch.",
    );
    guide_row(ui);
    // The services, one row each, in a box like a list.
    let mut choose = None;
    egui::Frame::new()
        .fill(p.control)
        .stroke(Stroke::new(1.0, p.control_border))
        .corner_radius(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            for (n, kind) in AgentKind::ALL.into_iter().enumerate() {
                if n > 0 {
                    let y = ui.cursor().top();
                    let x = ui.max_rect().x_range();
                    ui.painter().hline(x, y, Stroke::new(1.0, p.control_border));
                }
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(space::M as i8, (space::S + 2.0) as i8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if let Some(k) = service_row(ui, a, kind, &ctx) {
                            choose = Some(k);
                        }
                    });
            }
        });
    if let Some(k) = choose {
        a.set_kind(k);
    }
    // What went wrong with the last install, under the list.
    if let Step::Failed { install: true, why, .. } = &a.setup.step {
        crate::theme::callout(ui, p.warning, Icon::Warning, why);
    } else if a.setup.installed_now && a.installed(a.kind, &ctx) == Some(false) {
        crate::theme::callout(
            ui,
            p.warning,
            Icon::Warning,
            &format!(
                "The installer finished, but ZigZag can't find `{}` yet. Quit ZigZag and open it again.",
                a.kind.program()
            ),
        );
    }
    // Doing it by hand: each one's commands.
    egui::CollapsingHeader::new(
        egui::RichText::new("Install from a terminal instead")
            .size(PANEL_PT)
            .color(p.tertiary),
    )
    .id_salt("install-by-hand")
    .default_open(false)
    .show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = space::S;
        for kind in AgentKind::ALL {
            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::S;
                crate::brands::show(ui, kind.brand(), 14.0, p.text);
                ui.label(egui::RichText::new(kind.name()).size(PANEL_PT).strong().color(p.text));
            });
            let (ways, sign_in) = kind.install_steps();
            for (how, command) in ways {
                crate::mcp_chip::copy_block(ui, how, command);
            }
            quiet(ui, sign_in);
        }
        ui.add_space(space::XS);
        if card_button(ui, "Check Again")
            .on_hover_text("Look for them again, after installing one")
            .clicked()
        {
            a.check_again();
        }
    });
}

/// One service in the setup list: its mark, its name and who makes it, and
/// what can be done with it here. Returns the agent to switch to.
fn service_row(ui: &mut egui::Ui, a: &mut AgentState, kind: AgentKind, ctx: &egui::Context) -> Option<AgentKind> {
    use crate::agent_setup::Step;
    let p = pal_ui(ui);
    let installing = matches!(&a.setup.step, Step::Installing(j) if j.kind == kind);
    let here = a.found[kind.index()].as_ref().is_some_and(|f| f.is_some());
    let mut choose = None;
    // The button first, from the right, so the words get only what's left
    // (and end in … rather than running under it).
    // (A row's height: the name and the line under it.)
    let row_h = 34.0;
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), row_h),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
        ui.set_height(row_h);
        ui.spacing_mut().item_spacing.x = space::S + 2.0;
        if installing {
            installing_button(ui, "Install");
        } else if here {
            if card_button(ui, "Use").on_hover_text(format!("Use {} in this panel", kind.name())).clicked() {
                choose = Some(kind);
            }
        } else if crate::dialog::primary(ui, "Install", !a.setup.busy())
            .on_hover_text(format!("Install {} with {}'s own installer", kind.name(), kind.maker()))
            .clicked()
        {
            a.setup.installed_now = false;
            a.setup.install(kind, ctx);
            // (The one being installed is the one the panel will run.)
            choose = Some(kind);
        }
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = space::S + 2.0;
            crate::brands::show(ui, kind.brand(), 22.0, p.text);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.add(
                    egui::Label::new(egui::RichText::new(kind.name()).size(PANEL_PT).strong().color(p.text))
                        .truncate(),
                );
                let sub = match &a.setup.step {
                    Step::Installing(j) if j.kind == kind => j
                        .last_line()
                        .unwrap_or("Downloading. This takes a minute or two.")
                        .to_string(),
                    _ if here => "Installed".to_string(),
                    _ => format!("{} · {} account", kind.maker(), kind.account()),
                };
                ui.add(egui::Label::new(egui::RichText::new(sub).size(PANEL_PT - 1.0).color(p.tertiary)).truncate());
            });
        });
        },
    );
    choose
}

/// The agent is installed but not signed in: a button that signs in (in
/// the browser), and what to do while that's open.
fn sign_in_steps(ui: &mut egui::Ui, a: &mut AgentState) {
    use crate::agent_setup::Step;
    let p = pal_ui(ui);
    let kind = a.kind;
    let ctx = ui.ctx().clone();
    ui.spacing_mut().item_spacing.y = space::S;
    let waiting = matches!(&a.setup.step, Step::SigningIn(job) if job.kind == kind);
    // While it waits: where things stand, in a bar across the top.
    if let Step::SigningIn(job) = &a.setup.step
        && job.kind == kind
    {
        let in_window = kind == AgentKind::Antigravity && cfg!(windows);
        status_bar(
            ui,
            if job.code_sent {
                "Checking the code…"
            } else if in_window {
                "Waiting for you in the sign-in window…"
            } else {
                "Waiting for you in your browser…"
            },
        );
        ui.add_space(space::S);
    }
    // Its mark and what this is, the mark centred on the title.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        crate::brands::show(ui, kind.brand(), 20.0, p.text);
        setup_title(ui, &format!("Sign in to {}", kind.name()));
    });
    if waiting && let Step::SigningIn(job) = &a.setup.step {
        let url = job.url.clone();
        let code_sent = job.code_sent;
        let in_window = kind == AgentKind::Antigravity && cfg!(windows);
        setup_text(
            ui,
            match kind {
                // (It waits a minute, then gives up.)
                AgentKind::Antigravity if in_window => "Sign in with your Google account in that window, within a minute.".to_string(),
                AgentKind::Antigravity => "Sign in with your Google account, then paste the code here within a minute.".to_string(),
                _ => "Sign in, then come back here.".to_string(),
            },
        );
        // The page can show a code to paste back.
        if kind.pastes_code() && !code_sent {
            ui.add_space(space::XS);
            let hint = if kind == AgentKind::Antigravity {
                "Paste the code"
            } else {
                "Paste a code, if the page shows one"
            };
            if code_field(ui, &mut a.setup.code, hint) {
                let code = std::mem::take(&mut a.setup.code);
                a.setup.send_code(&code);
            }
        }
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::M + space::XS;
            if let Some(url) = url
                && text_link(ui, "Open sign-in page", p.accent)
                    .on_hover_text("If your browser didn't open, or you closed it")
                    .clicked()
            {
                open_web(&ctx, &url);
            }
            if text_link(ui, "Cancel", p.secondary).clicked() {
                a.setup.cancel();
            }
        });
        return;
    }
    setup_text(
        ui,
        format!(
            "{} is installed. Sign in with your {} account in your browser.",
            kind.name(),
            kind.account()
        ),
    );
    if let Step::Failed { kind: failed, install: false, why } = &a.setup.step
        && *failed == kind
    {
        crate::theme::callout(ui, p.warning, Icon::Warning, why);
    }
    ui.add_space(space::XS);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::M;
        if crate::dialog::primary(ui, "Sign In", !a.setup.busy()).clicked()
            && let Some(Some(program)) = a.found[kind.index()].clone()
        {
            let path = a.path.clone().flatten();
            a.setup.sign_in(kind, &program, path, &ctx);
        }
        web_link(ui, "Help", GUIDE_URL);
    });
}

/// A setup card's status, in a tinted bar across the top: a spinner and
/// what it's waiting for.
fn status_bar(ui: &mut egui::Ui, text: &str) {
    let p = pal_ui(ui);
    let tint = Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), 26);
    egui::Frame::new()
        .fill(tint)
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(space::M as i8, space::S as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            working_line(ui, text);
        });
}

/// A button at work: the size of the one it stands in for (`was`), quiet,
/// with a spinner in its middle. (Nothing to click: it runs to the end.)
fn installing_button(ui: &mut egui::Ui, was: &str) {
    let p = pal_ui(ui);
    let font = egui::FontId::proportional(13.0);
    let w = (ui.painter().layout_no_wrap(was.to_string(), font, p.text).size().x + 32.0).max(72.0);
    let (rect, r) = ui.allocate_exact_size(Vec2::new(w, crate::dialog::BUTTON_H), Sense::hover());
    ui.painter().rect_filled(rect, 6.0, p.control);
    outline(ui, rect);
    spinner(ui.painter(), rect.center(), 6.0, p.accent, ui.input(|i| i.time));
    ui.ctx().request_repaint_after(Duration::from_millis(33));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, "Installing"));
}

/// A lesser action as underlined text, in `color`.
fn text_link(ui: &mut egui::Ui, label: &str, color: Color32) -> egui::Response {
    let r = ui.add(
        egui::Label::new(egui::RichText::new(label).size(PANEL_PT).color(color).underline())
            .selectable(false)
            .sense(Sense::click()),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The card's main action: blue, and a dimmed blue (still plainly the one
/// to press) until it can be.
fn blue_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    if enabled {
        return crate::dialog::primary(ui, label, true);
    }
    let p = pal_ui(ui);
    let font = egui::FontId::proportional(13.0);
    let g = ui.painter().layout_no_wrap(label.to_string(), font, Color32::WHITE);
    let (rect, r) = ui.allocate_exact_size(
        Vec2::new((g.size().x + 32.0).max(72.0), crate::dialog::BUTTON_H),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, 6.0, p.accent.gamma_multiply(0.4));
    ui.painter().galley(
        rect.center() - g.size() / 2.0,
        g,
        Color32::WHITE.gamma_multiply(0.7),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, false, label));
    r
}

/// A dialog's quiet button, outlined: on the panel (not a dialog's own
/// background) its fill alone barely shows in the light theme.
fn card_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let r = crate::dialog::secondary(ui, label);
    outline(ui, r.rect);
    r
}

/// The hairline the panel's fields have, round a button.
fn outline(ui: &egui::Ui, rect: Rect) {
    let p = pal_ui(ui);
    ui.painter().rect_stroke(
        rect,
        6.0,
        Stroke::new(1.0, p.control_border),
        egui::StrokeKind::Inside,
    );
}

/// Where a sign-in code is pasted: as tall as the buttons, the full width,
/// with Send beside it. True when it's sent (Send, or Enter).
fn code_field(ui: &mut egui::Ui, code: &mut String, hint: &str) -> bool {
    let p = pal_ui(ui);
    let ready = !code.trim().is_empty();
    let mut send = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        let send_w = 72.0;
        let r = ui.add(
            egui::TextEdit::singleline(code)
                .hint_text(egui::RichText::new(hint).color(p.tertiary))
                .font(egui::FontId::proportional(13.0))
                .align(egui::Align2::LEFT_CENTER)
                .margin(egui::Margin::symmetric(space::S as i8, 0))
                .min_size(Vec2::new(ui.available_width() - send_w - space::S, crate::dialog::BUTTON_H))
                .desired_width(ui.available_width() - send_w - space::S),
        );
        send = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        send |= blue_button(ui, "Send", ready).clicked();
    });
    send && ready
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_chosen_model_and_effort_go_on_the_command_line() {
        let args = |a: &mut AgentState| -> Vec<String> {
            a.command("x".into(), "http://127.0.0.1:1/mcp", None, 1)
                .get_args()
                .map(|x| x.to_string_lossy().into_owned())
                .collect()
        };
        let mut a = AgentState {
            kind: AgentKind::Claude,
            ..Default::default()
        };
        assert!(
            !args(&mut a)
                .iter()
                .any(|x| x == "--model" || x == "--effort"),
            "defaults: nothing"
        );
        a.model[AgentKind::Claude.index()] = Some("opus".into());
        a.effort[AgentKind::Claude.index()] = Some("high".into());
        let c = args(&mut a).join(" ");
        assert!(
            c.contains("--model opus") && c.contains("--effort high"),
            "{c}"
        );
        a.kind = AgentKind::Codex;
        a.session = Some("t1".into());
        a.model[AgentKind::Codex.index()] = Some("gpt-6.2-astra".into());
        a.effort[AgentKind::Codex.index()] = Some("xhigh".into());
        let c = args(&mut a);
        let joined = c.join(" ");
        assert!(joined.starts_with("exec resume t1"), "{joined}");
        assert!(joined.contains("-m gpt-6.2-astra"), "{joined}");
        assert!(
            joined.contains("model_reasoning_effort=\"xhigh\""),
            "{joined}"
        );
        assert_eq!(
            c.last().map(String::as_str),
            Some("-"),
            "prompt on stdin last"
        );
    }

    #[test]
    fn codex_runs_with_only_zigzags_tools() {
        let json = r#"[{"name":"node_repl","enabled":true},{"name":"computer-use","enabled":false},
            {"name":"cua_repl"},{"name":"zigzag","enabled":true},{"name":"bad name;rm","enabled":true}]"#;
        let servers = parse_codex_servers(json);
        assert_eq!(
            servers,
            ["node_repl", "cua_repl", "zigzag"],
            "on, and safe to name"
        );
        assert!(parse_codex_servers("nope").is_empty());
        let mut a = AgentState {
            kind: AgentKind::Codex,
            codex_info: Some(CodexInfo {
                models: Vec::new(),
                servers,
            }),
            ..Default::default()
        };
        let args: Vec<String> = a
            .command("codex".into(), "http://127.0.0.1:1/mcp", None, 1)
            .get_args()
            .map(|x| x.to_string_lossy().into_owned())
            .collect();
        for want in [
            "features.plugins=false",
            "features.apps=false",
            "mcp_servers.node_repl.enabled=false",
            "mcp_servers.cua_repl.enabled=false",
        ] {
            assert!(args.iter().any(|x| x == want), "{want}: {args:?}");
        }
        assert!(!args.iter().any(|x| x == "mcp_servers.zigzag.enabled=false"));
        assert_eq!(args.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn reads_codex_models_and_leaves_out_the_hidden_ones() {
        let json = r#"{"models":[
            {"slug":"gpt-a","display_name":"GPT-A","description":"Big.","visibility":"list",
             "supported_reasoning_levels":[{"effort":"low"},{"effort":"ultra"}]},
            {"slug":"gpt-hidden","visibility":"hide","supported_reasoning_levels":[]}]}"#;
        let m = parse_codex_models(json);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].id.as_str(), m[0].name.as_str()), ("gpt-a", "GPT-A"));
        assert_eq!(m[0].efforts, ["low", "ultra"]);
        assert!(parse_codex_models("not json").is_empty());
    }

    #[test]
    fn what_the_panels_agent_makes_keeps_its_prompt_unless_turned_off() {
        let mut h = studio();
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10"><rect id="leaf" width="10" height="10" fill="#2a7"/></svg>"##;
        let add = |h: &mut egui_kittest::Harness<'static, Studio>| {
            let r = h
                .state_mut()
                .run_mcp_call("add_svg", &serde_json::json!({ "svg": svg }));
            assert!(!r.is_error);
            h.state().doc.shapes.len() - 1
        };
        // Not while nothing is running (an outside agent, say).
        let i = add(&mut h);
        assert_eq!(h.state().doc.prompt(i), None);
        // While the panel's agent works on a prompt: kept, and read back.
        h.state_mut().agent.turn_prompt = Some("A leaf, \"green\" & <small>\nplease".into());
        h.state_mut().agent.start_fake_run();
        let i = add(&mut h);
        assert_eq!(
            h.state().doc.prompt(i),
            Some("A leaf, \"green\" & <small>\nplease")
        );
        let got = h
            .state_mut()
            .run_mcp_call("get_design", &serde_json::json!({}));
        let text = match &got.content[0] {
            crate::mcp::Content::Text(t) => t.clone(),
            _ => unreachable!(),
        };
        assert!(text.contains("A leaf"), "get_design shows it");
        // Saved with the file, and back on reopening.
        let svg_out = h.state().doc.to_svg().unwrap();
        let back = zigzag::document::Document::from_svg(&svg_out, "back", None).ok();
        assert!(back.is_some_and(|d| {
            (0..d.shapes.len()).any(|k| d.prompt(k).is_some_and(|p| p.ends_with("please")))
        }));
        // One undo takes the object and its prompt together.
        let n = h.state().doc.shapes.len();
        h.state_mut().run_mcp_call("undo", &serde_json::json!({}));
        assert_eq!(h.state().doc.shapes.len(), n - 1);
        // Turned off in Settings: not kept.
        h.state_mut().prefs.agent_prompts = false;
        let i = add(&mut h);
        assert_eq!(h.state().doc.prompt(i), None);
        h.state_mut().agent.stop();
    }

    #[test]
    fn a_long_chat_folds_its_earlier_turns_and_the_clock_lists_chats() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().agent.config = None;
        h.state_mut().dock.reveal(PanelId::Agent);
        for k in 1..=5 {
            let a = &mut h.state_mut().agent;
            a.log.push(Entry::You(format!("prompt {k}")));
            a.log.push(Entry::Said(format!("answer {k}")));
        }
        h.run_steps(4);
        assert!(
            h.query_by_label("2 earlier turns").is_some(),
            "the first two fold"
        );
        assert!(
            h.query_by_label("answer 1").is_none(),
            "and aren't laid out"
        );
        assert!(h.query_by_label("answer 5").is_some());
        h.get_by_label("2 earlier turns").click();
        h.run_steps(3);
        assert!(h.state().agent.show_earlier);
        assert!(h.query_by_label("answer 1").is_some());
        // The clock: no kept chats for this design (none are kept in tests).
        h.get_by_label("Chat History").click();
        h.run_steps(3);
        assert!(
            h.query_by_label("No earlier chats about this design yet.")
                .is_some()
        );
    }

    #[test]
    fn an_image_pasted_or_dropped_goes_with_the_prompt_to_either_agent() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().agent.config = None;
        h.state_mut().agent.kind = AgentKind::Codex;
        h.state_mut().dock.reveal(PanelId::Agent);
        h.state_mut().agent.focus = true;
        h.run_steps(4);
        // ⌘V with an image on the clipboard, the prompt in focus.
        let red = egui::ColorImage::from_rgba_unmultiplied([8, 8], &[220, 30, 40, 255].repeat(64));
        h.event(egui::Event::PasteImage(std::sync::Arc::new(red)));
        h.run_steps(3);
        assert_eq!(h.state().agent.attach.len(), 1, "attached");
        assert!(h.state().agent.attach[0].exists(), "kept as a file");
        let shapes = h.state().doc.shapes.len();
        // Its × takes it off.
        h.get_by_label("Remove Image").click();
        h.run_steps(3);
        assert!(h.state().agent.attach.is_empty());
        // Dropped on the panel: attached, not opened as a design.
        let dir = std::env::temp_dir().join(format!("zz-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("logo.png");
        std::fs::write(
            &file,
            vectorcraft_trace::Raster::from_fn(16, 16, |_, _| [20, 60, 200, 255]).encode_png(),
        )
        .unwrap();
        let over = h.state().agent.panel_rect.unwrap().center();
        h.event(egui::Event::PointerMoved(over));
        /// A file dropped on the window, as the OS hands it over.
        #[derive(Debug)]
        struct Dropped(PathBuf);
        impl egui::DroppedFile for Dropped {
            fn path(&self) -> &std::path::Path {
                &self.0
            }
            fn bytes(&self) -> Result<Vec<u8>, String> {
                std::fs::read(&self.0).map_err(|e| e.to_string())
            }
        }
        h.input_mut()
            .dropped_files
            .push(std::sync::Arc::new(Dropped(file.clone())));
        h.run_steps(3);
        assert_eq!(h.state().agent.attach.len(), 1, "the dropped image");
        assert_eq!(h.state().tab_count(), 1, "no design opened for it");
        assert_eq!(h.state().doc.shapes.len(), shapes);
        let kept = h.state().agent.attach[0].clone();

        // Codex: each image an --image option; the prompt names the file.
        let a = &mut h.state_mut().agent;
        a.sending_images = vec![kept.clone()];
        let args: Vec<String> = a
            .command("codex".into(), "http://127.0.0.1:1/mcp", None, 1)
            .get_args()
            .map(|x| x.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.contains(&format!("--image={}", kept.display())),
            "{args:?}"
        );
        assert_eq!(args.last().map(String::as_str), Some("-"));
        // Claude: its streaming input, the image inline.
        a.kind = AgentKind::Claude;
        let args: Vec<String> = a
            .command("claude".into(), "http://127.0.0.1:1/mcp", None, 1)
            .get_args()
            .map(|x| x.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.windows(2)
                .any(|w| w == ["--input-format", "stream-json"]),
            "{args:?}"
        );
        let msg: Value =
            serde_json::from_str(claude_message("make this", std::slice::from_ref(&kept)).trim())
                .unwrap();
        assert_eq!(msg["message"]["content"][0]["text"], "make this");
        assert_eq!(msg["message"]["content"][1]["type"], "image");
        assert_eq!(
            msg["message"]["content"][1]["source"]["media_type"],
            "image/png"
        );
        a.sending_images.clear();

        // Sent (no agent here to run): the chat shows it under the prompt.
        a.kind = AgentKind::Codex;
        a.found = [Some(None), Some(None), Some(None)];
        a.prompt = "make this into a patch".into();
        a.send(
            Some("127.0.0.1:1".parse().unwrap()),
            &egui::Context::default(),
        );
        assert!(a.attach.is_empty(), "sent with it");
        assert_eq!(a.log[1], Entry::Images(vec![kept.clone()]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_agent_asks_and_a_click_on_an_option_makes_it() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().agent.config = None;
        h.state_mut().dock.reveal(PanelId::Agent);
        h.run_steps(3);
        // Steps that reach outside the document aren't offered; and a
        // question is asked for.
        let bad = h.state_mut().run_mcp_call(
            "offer_choices",
            &json!({ "title": "t", "question": "q?", "choices": [{ "label": "Ship it", "steps": [{ "tool": "export", "arguments": {} }] }] }),
        );
        assert!(bad.is_error);
        let vague = h.state_mut().run_mcp_call(
            "offer_choices",
            &json!({ "choices": [{ "label": "x", "steps": [{ "tool": "set_document", "arguments": { "over_sewn": true } }] }] }),
        );
        assert!(vague.is_error);
        // At most four of the agent's own: a card stays short.
        let many: Vec<Value> = (0..5)
            .map(|k| json!({ "label": format!("Option {k}"), "steps": [{ "tool": "set_document", "arguments": { "connect_mm": k } }] }))
            .collect();
        assert!(
            h.state_mut()
                .run_mcp_call(
                    "offer_choices",
                    &json!({ "title": "t", "question": "q?", "choices": many })
                )
                .is_error
        );
        let offer = json!({ "title": "Fewer trims", "question": "Which should I do?", "choices": [
            { "label": "Travel over fills", "detail": "12 to 7 trims; about 4 mm of thread crosses other colors",
              "steps": [{ "tool": "set_document", "arguments": { "over_sewn": true } }] },
            { "label": "Sun as satin", "detail": "smoother edge",
              "steps": [{ "tool": "set_object", "arguments": { "object": "sun", "kind": "satin" } }] }
        ] });
        assert!(!h.state_mut().run_mcp_call("offer_choices", &offer).is_error);
        h.run_steps(3);
        assert!(
            h.query_by_label("Which should I do?").is_some(),
            "its question"
        );
        assert!(
            h.query_by_label("Keep it as it is").is_some(),
            "always a way out"
        );
        assert!(
            h.query_by_label("Apply").is_none(),
            "no button: a click is the choice"
        );
        let undo = h.state().undo.len();
        // One click on an option makes it, as one undo step.
        h.get_by_label("Travel over fills").click();
        h.run_steps(3);
        assert!(h.state().doc.settings.over_sewn, "made");
        assert_eq!(h.state().undo.len(), undo + 1);
        // The card is closed: another click makes nothing.
        let sun = h.state().names.iter().position(|n| n == "sun").unwrap();
        h.get_by_label("Sun as satin").click();
        h.run_steps(3);
        assert_ne!(
            h.state().doc.shapes[sun].params.kind,
            Some(zigzag::params::Kind::Satin)
        );
        assert_eq!(h.state().undo.len(), undo + 1);
        // Keeping it as it is changes nothing, and closes the card.
        assert!(!h.state_mut().run_mcp_call("offer_choices", &offer).is_error);
        h.run_steps(3);
        let before = h.state().doc.to_svg().unwrap();
        // (The new card's: the first card's is closed.)
        h.get_all_by_label("Keep it as it is")
            .last()
            .unwrap()
            .click();
        h.run_steps(3);
        assert_eq!(h.state().doc.to_svg().unwrap(), before);
        assert!(matches!(
            h.state().agent.log.iter().rev().find(|e| matches!(e, Entry::Choices { .. })),
            Some(Entry::Choices { applied: Some(a), .. }) if a == &["Keep it as it is".to_string()]
        ));
        // Changes that go together get an option to do them all; changes
        // that set the same thing differently don't.
        let both = json!({ "title": "t", "question": "Which?", "choices": [
            { "label": "A", "steps": [{ "tool": "set_document", "arguments": { "connect_mm": 2 } }] },
            { "label": "B", "steps": [{ "tool": "set_object", "arguments": { "object": "sun", "kind": "satin" } }] }
        ] });
        assert!(!h.state_mut().run_mcp_call("offer_choices", &both).is_error);
        let clash = json!({ "title": "t", "question": "Which?", "choices": [
            { "label": "C", "steps": [{ "tool": "set_document", "arguments": { "connect_mm": 2 } }] },
            { "label": "D", "steps": [{ "tool": "set_document", "arguments": { "connect_mm": 3 } }] }
        ] });
        assert!(!h.state_mut().run_mcp_call("offer_choices", &clash).is_error);
        let cards: Vec<Vec<String>> = h
            .state()
            .agent
            .log
            .iter()
            .filter_map(|e| match e {
                Entry::Choices { choices, .. } => {
                    Some(choices.iter().map(|c| c.label.clone()).collect())
                }
                _ => None,
            })
            .collect();
        let n = cards.len();
        assert_eq!(
            cards[n - 2],
            ["A", "B", "Do all of these", "Keep it as it is"]
        );
        assert_eq!(
            cards[n - 1],
            ["C", "D", "Keep it as it is"],
            "no all: they clash"
        );
        assert_eq!(
            cards[0],
            [
                "Travel over fills",
                "Sun as satin",
                "Do all of these",
                "Keep it as it is"
            ]
        );
        // Doing them all makes both, as one undo step.
        h.run_steps(3);
        let undo = h.state().undo.len();
        h.get_all_by_label("Do all of these")
            .last()
            .unwrap()
            .click();
        h.run_steps(3);
        assert_eq!(h.state().doc.settings.connect_mm, 2.0);
        assert_eq!(
            h.state().doc.shapes[sun].params.kind,
            Some(zigzag::params::Kind::Satin)
        );
        assert_eq!(h.state().undo.len(), undo + 1);
        // Kept in history, a card comes back closed.
        let kept = h
            .state()
            .agent
            .log
            .iter()
            .find(|e| matches!(e, Entry::Choices { .. }))
            .cloned()
            .unwrap();
        let back = crate::history::entry_from(&crate::history::entry_json(&kept)).unwrap();
        assert!(matches!(
            back,
            Entry::Choices {
                applied: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn one_change_on_offer_is_a_question_with_two_buttons() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().agent.config = None;
        h.state_mut().dock.reveal(PanelId::Agent);
        h.run_steps(3);
        let one = json!({ "title": "Fewer trims", "question": "Let the thread run over the fills?",
            "choices": [{ "label": "Yes, do it", "detail": "Cuts the thread 7 times instead of 12; short lines show.",
                "steps": [{ "tool": "set_document", "arguments": { "over_sewn": true } }] }] });
        assert!(!h.state_mut().run_mcp_call("offer_choices", &one).is_error);
        h.run_steps(3);
        assert!(h.query_by_label("Not now").is_some());
        assert!(
            h.query_by_label("Do all of these").is_none(),
            "one change: nothing to do all of"
        );
        h.get_by_label("Yes, do it").click();
        h.run_steps(3);
        assert!(h.state().doc.settings.over_sewn);
        assert!(h.query_by_label("Done: Yes, do it.").is_some());
        // Not now: nothing changes.
        let off = json!({ "title": "t", "question": "Stitch across tiny gaps?",
            "choices": [{ "label": "Stitch across", "steps": [{ "tool": "set_document", "arguments": { "connect_mm": 3 } }] }] });
        assert!(!h.state_mut().run_mcp_call("offer_choices", &off).is_error);
        h.run_steps(3);
        h.get_by_label("Not now").click();
        h.run_steps(3);
        assert_eq!(h.state().doc.settings.connect_mm, 0.0);
        assert!(h.query_by_label("Kept it as it is.").is_some());
    }

    #[test]
    fn a_quick_second_click_on_send_doesnt_stop_the_run() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().agent.config = None;
        h.state_mut().dock.reveal(PanelId::Agent);
        h.run_steps(3);
        h.state_mut().agent.start_fake_run();
        h.state_mut().agent.started = Some(Instant::now());
        h.run_steps(2);
        h.get_by_label("Stop").click();
        h.run_steps(2);
        assert!(
            h.state().agent.running(),
            "the click that came with the Send"
        );
        h.state_mut().agent.started = Some(Instant::now() - Duration::from_secs(2));
        h.run_steps(1);
        h.get_by_label("Stop").click();
        h.run_steps(2);
        assert!(!h.state().agent.running(), "a Stop later on stops");
    }

    #[test]
    fn the_prompt_box_keeps_its_height_when_typed_in_while_it_works() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().agent.config = None; // don't save the choice
        h.state_mut().dock.reveal(PanelId::Agent);
        h.state_mut().agent.start_fake_run();
        h.state_mut().agent.focus = true;
        h.run_steps(4);
        let empty = h.get_by_label("Prompt").rect().height();
        let chrome = h.state().agent.chrome_h;
        h.get_by_label("Prompt").type_text("and");
        h.run_steps(2);
        assert_eq!(h.get_by_label("Prompt").rect().height(), empty);
        assert_eq!(h.state().agent.chrome_h, chrome);
        h.state_mut().agent.stop();
    }

    #[test]
    fn return_sends_the_prompt_and_shift_return_makes_a_new_line() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().agent.config = None; // don't save the choice
        h.state_mut().agent.kind = AgentKind::Claude;
        h.state_mut().dock.reveal(PanelId::Agent);
        h.state_mut().agent.focus = true;
        h.run_steps(4);
        h.get_by_label("Prompt").type_text("a star");
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::SHIFT, Key::Enter);
        h.run_steps(2);
        h.get_by_label("Prompt").type_text("in gold");
        h.run_steps(2);
        assert_eq!(h.state().agent.prompt, "a star\nin gold");
        h.key_press(Key::Enter);
        h.run_steps(3);
        // Sent: with no MCP server here, sending says so (and keeps the
        // prompt to send again); Return made no new line.
        assert!(
            matches!(h.state().agent.log.last(), Some(Entry::Error(e)) if e.contains("MCP server")),
            "Return sent it: {:?}",
            h.state().agent.log.last()
        );
        assert_eq!(h.state().agent.prompt, "a star\nin gold");
    }

    #[test]
    fn return_sends_with_an_earlier_chat_in_the_panel() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().agent.config = None; // don't save the choice
        h.state_mut().agent.kind = AgentKind::Codex;
        h.state_mut().dock.reveal(PanelId::Agent);
        // An earlier conversation, ending with a card of choices.
        h.state_mut().agent.log = vec![
            Entry::You("make a patch".into()),
            Entry::Tool("add_svg".into(), "patch".into(), Some(true)),
            Entry::Said("Here's the patch.".into()),
            Entry::Done("1 min".into()),
            Entry::Choices {
                title: "Fewer trims".into(),
                question: "Sew across short gaps?".into(),
                choices: vec![Choice {
                    label: "Connect 3 mm".into(),
                    detail: "Fewer stops.".into(),
                    steps: json!([]),
                    picked: false,
                }],
                applied: None,
            },
        ];
        h.run_steps(4);
        h.get_by_label("Prompt").click();
        h.run_steps(2);
        h.get_by_label("Prompt").type_text("a star");
        h.run_steps(2);
        let before = h.state().agent.log.len();
        h.key_press(Key::Enter);
        h.run_steps(3);
        assert!(
            h.state().agent.log.len() > before,
            "Return did nothing: {:?} / {:?}",
            h.state().agent.prompt,
            h.state().agent.log.last()
        );
    }

    #[test]
    fn typing_a_slash_opens_the_command_menu_and_enter_runs_a_pick() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().agent.config = None; // don't save the choice
        h.state_mut().agent.kind = AgentKind::Claude;
        h.state_mut().dock.reveal(PanelId::Agent);
        h.state_mut().agent.focus = true;
        h.run_steps(4);
        h.get_by_label("Prompt").type_text("/mo");
        h.run_steps(3);
        assert!(
            h.query_by_label("/model name").is_some(),
            "the menu shows /model"
        );
        assert!(
            h.query_by_label("/new").is_none(),
            "narrowed to what's typed"
        );
        // Enter completes it, and the models are listed next.
        h.key_press(Key::Enter);
        h.run_steps(3);
        assert_eq!(h.state().agent.prompt, "/model ");
        assert!(
            h.query_by_label("Opus").is_some(),
            "{:?}",
            h.state().agent.prompt
        );
        // Down to the second model (Default, Fable, Opus…), Enter picks it.
        h.key_press(Key::ArrowDown);
        h.key_press(Key::ArrowDown);
        h.run_steps(2);
        h.key_press(Key::Enter);
        h.run_steps(3);
        assert_eq!(h.state().agent.model(), Some("opus"));
        assert!(h.state().agent.prompt.is_empty());
        assert!(
            !h.state().agent.running(),
            "a command isn't sent to the agent"
        );
    }

    #[test]
    fn only_codex_is_told_to_generate_and_trace_images() {
        // Codex can generate images; Claude can't, and isn't told to.
        let codex = AgentKind::Codex.brief();
        assert!(codex.starts_with(BRIEF) && codex.contains("trace_image"));
        assert_eq!(AgentKind::Claude.brief(), BRIEF);
    }

    #[test]
    fn work_is_boxed_as_the_whole_it_is() {
        use zigzag::geom::P;
        let mut d = zigzag::document::Document::empty();
        let sq = |x: f64| {
            vec![
                P::new(x, 0.0),
                P::new(x + 100.0, 0.0),
                P::new(x + 100.0, 100.0),
                P::new(x, 100.0),
            ]
        };
        for k in 0..5 {
            d.add_drawing(
                &sq(k as f64 * 300.0),
                true,
                [0; 3],
                zigzag::params::Kind::Fill,
            )
            .unwrap();
        }
        let u: Vec<usize> = d.shapes.iter().map(|s| s.uid).collect();
        // One object: itself. Nothing: nothing.
        assert_eq!(boxed(&d.shapes, &u[..1]), vec![u[0]]);
        assert!(boxed(&d.shapes, &[]).is_empty());
        // Several with nothing else in common: the whole design, however
        // few of them (one box, as Select All would draw).
        assert_eq!(boxed(&d.shapes, &[u[0], u[2]]), u);
        assert_eq!(boxed(&d.shapes, &u), u);
        // What's hidden isn't in the whole there is to see.
        d.shapes[4].params.hide = Some(true);
        assert_eq!(boxed(&d.shapes, &u[..2]), u[..4].to_vec());
        d.shapes[4].params.hide = None;
        // Several in one group: that group, all of it. In a group inside
        // it: the inner one. Across the two: the outer.
        for s in &mut d.shapes[..3] {
            s.groups = vec!["head".into()];
        }
        for s in &mut d.shapes[..2] {
            s.groups.push("face".into());
        }
        assert_eq!(boxed(&d.shapes, &u[..2]), u[..2].to_vec());
        assert_eq!(boxed(&d.shapes, &[u[1], u[2]]), u[..3].to_vec());
        assert_eq!(boxed(&d.shapes, &[u[2], u[3]]), u);
        // An element's fill and outline are one object.
        let id = d.shapes[0].id.clone();
        d.shapes[3].id = id;
        assert_eq!(boxed(&d.shapes, &[u[0], u[3]]), vec![u[0], u[3]]);
    }

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
    fn a_tool_that_refuses_a_codex_call_says_why_in_its_own_words() {
        // ZigZag's answer to a call it can't do comes back as the call's
        // result, marked failed, with no error of Codex's own.
        let l = json!({"type":"item.completed","item":{"id":"1","type":"mcp_tool_call","server":"zigzag","tool":"set_object","status":"failed","error":null,
            "result":{"content":[{"type":"text","text":"no object named 'Face_mask'. Objects: Face mask, Mouth"}],"structured_content":null}}});
        let out = codex_event(&l, &mut None);
        assert_eq!(
            out[1].1,
            Entry::Error(
                "set_object: no object named 'Face_mask'. Objects: Face mask, Mouth".into()
            )
        );
        // With nothing said at all, it's still said that it failed.
        let l = json!({"type":"item.completed","item":{"id":"2","type":"mcp_tool_call","server":"zigzag","tool":"set_object","status":"failed"}});
        assert_eq!(
            codex_event(&l, &mut None)[1].1,
            Entry::Error("set_object failed.".into())
        );
    }

    #[test]
    fn a_failed_batch_says_its_error_not_its_whole_answer() {
        let answer = r#"{"error":"Step 3 (edit_path) failed: edit 1: no such point. Nothing was changed.","results":[{"step":0,"tool":"move_object","result":"Sewing order: a → b"}]}"#;
        assert_eq!(
            codex_tool_error("zigzag", "batch", answer),
            "batch: Step 3 (edit_path) failed: edit 1: no such point. Nothing was changed."
        );
        assert_eq!(
            codex_tool_error("zigzag", "set_object", "no object called x"),
            "set_object: no object called x"
        );
    }

    #[test]
    fn another_servers_failure_is_said_as_it_is_not_as_an_approval() {
        // As Codex's browser tool failed in a real run: its help text after
        // the error mentions cancelling, which isn't about approval.
        let mut s = None;
        let l = json!({"type":"item.completed","item":{"id":"1","type":"mcp_tool_call","server":"cua_repl","tool":"js","status":"failed",
            "result":{"content":[{"type":"text","text":"The browser access host is unavailable"},{"type":"text","text":"## Computer Use\nIf the user cancelled, stop."}]}}});
        let out = codex_event(&l, &mut s);
        let said: Vec<&String> = out
            .iter()
            .filter_map(|(_, e)| match e {
                Entry::Error(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(
            said,
            ["cua_repl js: The browser access host is unavailable"]
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

    /// As npm's `codex` does: a launcher that starts the program doing the
    /// work and waits on it. Stopping has to end that one too, or it goes
    /// on (holding its chat, connected to the studio).
    #[cfg(unix)]
    #[test]
    fn stopping_ends_what_the_agent_started_too() {
        let _env = AGENT_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zz-stop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pidfile = dir.join("worker.pid");
        let _ = std::fs::remove_file(&pidfile);
        let fake = dir.join("claude");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nsleep 300 &\necho $! > '{}'\nwait\n",
                pidfile.display()
            ),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        unsafe { std::env::set_var("ZIGZAG_CLAUDE", &fake) };
        let mut a = AgentState {
            kind: AgentKind::Claude,
            path: Some(None),
            ..Default::default()
        };
        a.prompt = "go".into();
        a.send(
            Some("127.0.0.1:1".parse().unwrap()),
            &egui::Context::default(),
        );
        let mut worker = None;
        for _ in 0..150 {
            // (As every frame: a send waiting on its lookups goes.)
            a.poll();
            if let Some(p) = std::fs::read_to_string(&pidfile)
                .ok()
                .filter(|p| !p.trim().is_empty())
            {
                worker = Some(p.trim().to_string());
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let worker = worker.expect("the worker started");
        let alive = |pid: &str| {
            Command::new("kill")
                .args(["-0", pid])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        assert!(alive(&worker));
        a.stop();
        let mut gone = false;
        for _ in 0..200 {
            if !alive(&worker) {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        unsafe { std::env::remove_var("ZIGZAG_CLAUDE") };
        let _ = std::fs::remove_dir_all(&dir);
        assert!(gone, "the program the launcher started ended too");
        assert_eq!(a.log.last(), Some(&Entry::Error("Stopped.".into())));
    }

    #[test]
    fn a_held_codex_chat_is_explained() {
        let mut s = None;
        let l = json!({"type":"error","message":"thread/resume: thread/resume failed: thread 01a1 already has an active writer (code -32600)"});
        let out = codex_event(&l, &mut s);
        assert!(
            matches!(&out[0].1, Entry::Error(m) if m.contains("still finishing") && m.contains("/new")),
            "{out:?}"
        );
    }

    #[test]
    fn a_reply_while_the_panel_is_put_away_is_marked_unread() {
        let mut h = studio();
        h.run_steps(1);
        assert!(!h.state().agent_showing(), "tests start with it put away");
        h.state_mut().agent.start_fake_run();
        h.run_steps(2);
        assert!(h.state().agent.running());
        assert!(!h.state().agent.unread, "not while it works");
        h.state_mut().agent.stop();
        h.run_steps(2);
        assert!(h.state().agent.unread, "its turn ended out of sight");
        // Opened: read.
        h.state_mut().ask_agent();
        h.run_steps(2);
        assert!(h.state().agent_showing());
        assert!(!h.state().agent.unread);
        // A turn that ends in view isn't unread.
        h.state_mut().agent.start_fake_run();
        h.run_steps(2);
        h.state_mut().agent.stop();
        h.run_steps(2);
        assert!(!h.state().agent.unread);
    }

    /// What's written while the agent works waits, and goes as one message
    /// when its turn ends; what's being written then stays in the box.
    #[test]
    fn messages_written_while_it_works_go_when_its_turn_ends() {
        let _env = AGENT_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("tl-queue-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("claude");
        std::fs::write(
            &fake,
            "#!/bin/sh\nread -r p\nsleep 0.4\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\"}'\n\
             echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"num_turns\":1,\"duration_ms\":10}'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        unsafe { std::env::set_var("ZIGZAG_CLAUDE", &fake) };
        let mut h = studio();
        h.state_mut().agent.kind = AgentKind::Claude;
        h.state_mut().agent.config = None;
        h.state_mut().mcp = Some(
            crate::mcp::start(0, crate::tools::tool_specs(), String::new(), || {}).unwrap(),
        );
        let ctx = h.ctx.clone();
        let send = |h: &mut egui_kittest::Harness<'static, Studio>, text: &str| {
            h.state_mut().agent.prompt = text.into();
            h.state_mut().agent_send(&ctx);
        };
        send(&mut h, "make a badge");
        assert!(h.state().agent.running());
        send(&mut h, "in blue");
        send(&mut h, "and bigger");
        assert_eq!(h.state().agent.queued, ["in blue", "and bigger"]);
        assert!(h.state().agent.prompt.is_empty());
        h.state_mut().agent.prompt = "half wri".into();
        let said = |h: &egui_kittest::Harness<'static, Studio>| -> Vec<String> {
            h.state()
                .agent
                .log
                .iter()
                .filter_map(|e| match e {
                    Entry::You(t) => Some(t.clone()),
                    _ => None,
                })
                .collect()
        };
        for _ in 0..300 {
            h.run_steps(1);
            if said(&h).len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(said(&h), ["make a badge", "in blue\n\nand bigger"]);
        assert!(h.state().agent.queued.is_empty());
        assert_eq!(h.state().agent.prompt, "half wri", "the draft stays");
        h.state_mut().agent.stop();
        unsafe { std::env::remove_var("ZIGZAG_CLAUDE") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A queued message's Send Now stops the turn and sends that one at
    /// once, in the same chat; the rest wait for its turn to end.
    #[test]
    fn send_now_on_a_queued_message_stops_the_turn_and_sends_it() {
        use egui_kittest::kittest::Queryable;
        let _env = AGENT_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("tl-send-now-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("claude");
        std::fs::write(
            &fake,
            "#!/bin/sh\nread -r p\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\"}'\nsleep 1\n\
             echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"num_turns\":1,\"duration_ms\":10}'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        unsafe { std::env::set_var("ZIGZAG_CLAUDE", &fake) };
        let mut h = studio();
        h.state_mut().agent.kind = AgentKind::Claude;
        h.state_mut().agent.config = None;
        h.state_mut().dock.reveal(crate::dock::PanelId::Agent);
        h.state_mut().mcp = Some(
            crate::mcp::start(0, crate::tools::tool_specs(), String::new(), || {}).unwrap(),
        );
        let ctx = h.ctx.clone();
        for text in ["make a badge", "in blue", "and bigger"] {
            h.state_mut().agent.prompt = text.into();
            h.state_mut().agent_send(&ctx);
        }
        assert_eq!(h.state().agent.queued, ["in blue", "and bigger"]);
        h.state_mut().agent.prompt = "half wri".into();
        h.run_steps(3);
        // The first row's arrow, under the pointer.
        let at = h.get_all_by_label("Send Queued Message Now").next().unwrap().rect().center();
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(2);
        let said = |h: &egui_kittest::Harness<'static, Studio>| -> Vec<String> {
            h.state()
                .agent
                .log
                .iter()
                .filter_map(|e| match e {
                    Entry::You(t) => Some(t.clone()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(said(&h), ["make a badge", "in blue"]);
        assert!(h.state().agent.running());
        assert_eq!(h.state().agent.queued, ["and bigger"]);
        assert_eq!(h.state().agent.prompt, "half wri", "the draft stays");
        assert!(
            h.state().agent.log.iter().any(|e| matches!(e, Entry::Info(t) if t.starts_with("Stopped to send"))),
            "{:?}",
            h.state().agent.log
        );
        // The rest goes when that turn is done.
        for _ in 0..400 {
            h.run_steps(1);
            if said(&h).len() == 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(said(&h), ["make a badge", "in blue", "and bigger"]);
        h.state_mut().agent.stop();
        unsafe { std::env::remove_var("ZIGZAG_CLAUDE") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whether one of the ideas an empty chat starts from is on screen.
    fn an_idea_shows(h: &egui_kittest::Harness<'static, Studio>) -> bool {
        use egui_kittest::kittest::Queryable;
        NEW_IDEAS
            .iter()
            .chain(CHANGE_IDEAS)
            .any(|i| h.query_by_label(i).is_some())
    }

    /// Three ideas, none twice; with a design open, one of them a change
    /// to it; and not the same three every time.
    #[test]
    fn an_empty_chat_shows_three_ideas_picked_at_random() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0..200u64 {
            let open = ideas(seed, true);
            assert_eq!(open.len(), 3);
            assert_eq!(open.iter().collect::<std::collections::HashSet<_>>().len(), 3);
            assert_eq!(open.iter().filter(|i| CHANGE_IDEAS.contains(i)).count(), 1);
            let empty = ideas(seed, false);
            assert!(empty.iter().all(|i| NEW_IDEAS.contains(i)), "{empty:?}");
            seen.extend(open);
        }
        assert!(seen.len() > 20, "most of them come up: {}", seen.len());
        assert_ne!(ideas(1, true), ideas(2, true));
        assert_ne!(fresh_seed(), fresh_seed());
    }

    /// Stopped, what was waiting comes back to the prompt box, before what
    /// was being written.
    #[test]
    fn a_stop_puts_what_was_queued_back_in_the_box() {
        let mut a = AgentState::default();
        a.start_fake_run();
        a.queued = vec!["first".into(), "second".into()];
        a.prompt = "draft".into();
        a.stop();
        assert!(a.queued.is_empty());
        assert_eq!(a.prompt, "first\n\nsecond\n\ndraft");
    }

    #[test]
    fn runs_an_agent_program_and_streams_its_output() {
        let _env = AGENT_ENV.lock().unwrap_or_else(|e| e.into_inner());
        // A stand-in for `claude`: echoes stream-json and checks its stdin.
        let dir = std::env::temp_dir().join(format!("tl-agent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("claude");
        std::fs::write(
            &fake,
            "#!/bin/sh\nread -r p\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\"}'\n\
             printf '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"you said %s\"}]}}\\n' \"$p\"\n\
             echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"num_turns\":1,\"duration_ms\":10}'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut a = AgentState {
            kind: AgentKind::Claude,
            path: Some(None),
            ..Default::default()
        };
        // Point find() at the fake through a PATH-less lookup.
        let program = fake.clone();
        let mut cmd = a.command(program, "http://127.0.0.1:1/mcp", None, 1);
        let args: Vec<String> = cmd.get_args().map(|s| s.to_string_lossy().into()).collect();
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--tools" && w[1].is_empty())
        );
        // Run it the way send() does.
        let ctx = egui::Context::default();
        a.prompt = "hello".into();
        unsafe { std::env::set_var("ZIGZAG_CLAUDE", &fake) };
        a.send(Some("127.0.0.1:1".parse().unwrap()), &ctx);
        let _ = &mut cmd;
        for _ in 0..100 {
            a.poll();
            if !a.running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!a.running());
        assert_eq!(a.session.as_deref(), Some("s1"));
        assert!(
            a.log.contains(&Entry::Said("you said hello".into())),
            "{:?}",
            a.log
        );
        assert!(matches!(a.log.last(), Some(Entry::Done(_))));
        unsafe { std::env::remove_var("ZIGZAG_CLAUDE") };
        let _ = std::fs::remove_dir_all(&dir);
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
        // As if every agent were installed (the panel's usual start); tests
        // about missing ones say so.
        h.state_mut().agent.found = [
            Some(Some(PathBuf::from("/usr/local/bin/claude"))),
            Some(Some(PathBuf::from("/usr/local/bin/codex"))),
            Some(Some(PathBuf::from("/usr/local/bin/agy"))),
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
            an_idea_shows(&h)
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
        h.state_mut().agent.found = [Some(None), Some(None), Some(None)]; // none installed
        h.state_mut().dock.reveal(PanelId::Agent);
        h.run_steps(5);
        h.get_by_label("Set up an AI assistant");
        assert!(!an_idea_shows(&h), "setup instead of ideas");
        assert_eq!(h.state().panel_name(PanelId::Agent), "Setup", "its own page, no agent's");
        assert!(h.query_by_label("Prompt").is_none(), "no agent to write to");
        // Each service, to install, and the guide.
        assert_eq!(h.query_all_by_label("Install").count(), 3);
        h.get_by_label("OpenAI · ChatGPT account");
        h.get_by_label("Anthropic · Claude account");
        h.get_by_label("Google · Google account");
        h.get_by_label("Read the guide");
        // This platform's commands, folded away until asked for.
        let first = AgentKind::Codex.install_steps().0[0].1;
        assert!(h.query_by_label(first).is_none(), "folded");
        h.get_by_label_contains("Install from a terminal instead").click();
        h.run_steps(5);
        // Every one's commands, not only the chosen agent's.
        for kind in AgentKind::ALL {
            for (_, command) in kind.install_steps().0 {
                assert!(h.query_all_by_label(command).count() >= 1, "{command}");
            }
        }
        h.get_by_label_contains("Sign in with ChatGPT");
        h.get_by_label("Check Again");
        if Os::CURRENT != Os::Mac {
            assert!(h.query_by_label_contains("brew install").is_none(), "no Homebrew");
        }
        // Claude Code installed: it says so, and Use switches to it.
        h.state_mut().agent.found[AgentKind::Claude.index()] =
            Some(Some(PathBuf::from("/usr/local/bin/claude")));
        h.run_steps(3);
        h.get_by_label("Installed");
        h.get_by_label("Use").click();
        h.run_steps(3);
        assert_eq!(h.state().agent.kind, AgentKind::Claude);
        assert!(an_idea_shows(&h), "installed: the usual start");
        assert_eq!(h.state().panel_name(PanelId::Agent), "Claude Code");
        // Setup from the menu, with an agent ready: the page all the same.
        h.state_mut().agent.on_setup = true;
        h.run_steps(3);
        h.get_by_label("Set up an AI assistant");
        assert_eq!(h.state().panel_name(PanelId::Agent), "Setup");
        h.state_mut().agent.set_kind(AgentKind::Claude);
        h.run_steps(3);
        assert!(an_idea_shows(&h), "picking an agent leaves Setup");
        h.get_by_label("Prompt");
        // A chat under way: Setup covers it, and picking the agent brings it back.
        h.state_mut().agent.log.push(Entry::You("make it blue".into()));
        h.state_mut().agent.on_setup = true;
        h.run_steps(3);
        h.get_by_label("Set up an AI assistant");
        assert!(h.query_by_label("make it blue").is_none());
        h.state_mut().agent.set_kind(AgentKind::Claude);
        h.run_steps(3);
        h.get_by_label("make it blue");
    }

    #[test]
    fn the_guide_and_help_open_with_a_real_click() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        // A pointer press and release over each, as a mouse does (not an
        // accessibility click, which goes straight to the widget).
        let opened = |h: &mut egui_kittest::Harness<'_, crate::app::Studio>, label: &str| -> bool {
            let at = h.get_by_label(label).rect().center();
            h.hover_at(at);
            h.step();
            h.drag_at(at);
            h.step();
            h.drop_at(at);
            OPENED.lock().unwrap().clear();
            h.run_steps(3);
            OPENED.lock().unwrap().iter().any(|u| u == GUIDE_URL)
        };
        let mut h = studio();
        h.state_mut().agent.config = None;
        h.state_mut().agent.found = [Some(None), Some(None), Some(None)];
        h.state_mut().agent.on_setup = true;
        h.state_mut().dock.reveal(PanelId::Agent);
        h.run_steps(5);
        assert!(opened(&mut h, "Read the guide"), "the guide row");
        // Installed, signed out: the sign-in card's Help.
        h.state_mut().agent.found[AgentKind::Claude.index()] =
            Some(Some(PathBuf::from("/usr/local/bin/claude")));
        h.state_mut().agent.setup.signed_in[AgentKind::Claude.index()] = Some(false);
        h.state_mut().agent.set_kind(AgentKind::Claude);
        h.run_steps(5);
        assert!(opened(&mut h, "Help"), "Help");
    }

    /// What a look for the programs found: only `here` installed.
    fn found_only(here: &[AgentKind]) -> [Option<Option<PathBuf>>; AgentKind::COUNT] {
        let mut f: [Option<Option<PathBuf>>; AgentKind::COUNT] = [Some(None), Some(None), Some(None)];
        for k in here {
            f[k.index()] = Some(Some(PathBuf::from(format!("/c/{}", k.program()))));
        }
        f
    }

    #[test]
    fn setup_shows_the_first_time_and_after_that_the_agent_chosen() {
        let file = std::env::temp_dir().join(format!("zigzag-first-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        // First time: Setup, even with an agent here already.
        let mut a = AgentState::load_from(Some(file.clone()));
        a.found = found_only(&[AgentKind::Codex]);
        a.open_on_an_installed_agent();
        assert!(a.setup_page(), "first time");
        // Choosing the default agent (Use on Codex) counts: next time, Codex.
        a.set_kind(AgentKind::Codex);
        assert!(!a.setup_page());
        let a = AgentState::load_from(Some(file.clone()));
        assert_eq!(a.kind, AgentKind::Codex);
        assert!(!a.on_setup, "not the first time any more");
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn the_panel_opens_on_the_agent_last_used_or_one_thats_here() {
        let file = std::env::temp_dir().join(format!("zigzag-agent-{}.json", std::process::id()));
        let fresh = |kind: AgentKind, found: [Option<Option<PathBuf>>; AgentKind::COUNT]| AgentState {
            kind,
            found,
            config: Some(file.clone()),
            ..Default::default()
        };
        // The one last used is here: it stays.
        let mut a = fresh(AgentKind::Claude, found_only(&[AgentKind::Codex, AgentKind::Claude]));
        a.open_on_an_installed_agent();
        assert_eq!(a.kind, AgentKind::Claude);
        // It isn't (or nothing was saved, so it's Codex): the first that is,
        // remembered, and a chat restored from last time is kept.
        let mut a = fresh(AgentKind::Codex, found_only(&[AgentKind::Antigravity]));
        a.log.push(Entry::You("make it blue".into()));
        a.open_on_an_installed_agent();
        assert_eq!(a.kind, AgentKind::Antigravity);
        assert!(!a.setup_page());
        assert_eq!(a.log.len(), 1, "the chat stays");
        let saved = std::fs::read_to_string(&file).unwrap();
        assert!(saved.contains(r#""agent":"antigravity""#), "{saved}");
        // Only the first time: an agent picked later shows its own setup.
        a.kind = AgentKind::Codex;
        a.open_on_an_installed_agent();
        assert_eq!(a.kind, AgentKind::Codex);
        assert!(a.setup_page());
        // None here: Setup.
        let mut a = fresh(AgentKind::Codex, found_only(&[]));
        a.open_on_an_installed_agent();
        assert_eq!(a.kind, AgentKind::Codex);
        assert!(a.setup_page());
        // Setup chosen from the menu stays.
        let mut a = fresh(AgentKind::Codex, found_only(&[AgentKind::Claude]));
        a.on_setup = true;
        a.open_on_an_installed_agent();
        assert!(a.setup_page());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn install_steps_fit_the_platform() {
        let commands = |os, kind: AgentKind| -> Vec<&str> {
            kind.install_steps_on(os).0.iter().map(|w| w.1).collect()
        };
        let all_text = |os, kind: AgentKind| {
            let (ways, sign_in) = kind.install_steps_on(os);
            let mut s = kind.not_installed_message(os);
            for (how, command) in ways {
                s += &format!(" {how} {command}");
            }
            s + " " + sign_in
        };
        // A Mac: the makers' installers or Homebrew, and its Terminal app.
        assert_eq!(
            commands(Os::Mac, AgentKind::Codex),
            [
                "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
                "brew install --cask codex"
            ]
        );
        assert_eq!(
            commands(Os::Mac, AgentKind::Claude),
            [
                "curl -fsSL https://claude.ai/install.sh | bash",
                "brew install --cask claude-code"
            ]
        );
        assert!(all_text(Os::Mac, AgentKind::Codex).contains("in Terminal"));
        // The message says the panel can do it.
        assert!(all_text(Os::Mac, AgentKind::Codex).contains("Click Install Codex"));
        // Windows: PowerShell, then npm or WinGet.
        assert_eq!(
            commands(Os::Windows, AgentKind::Codex),
            [
                "powershell -ExecutionPolicy ByPass -c \"irm https://chatgpt.com/codex/install.ps1 | iex\"",
                "npm install -g @openai/codex"
            ]
        );
        assert_eq!(
            commands(Os::Windows, AgentKind::Claude),
            [
                "irm https://claude.ai/install.ps1 | iex",
                "winget install Anthropic.ClaudeCode"
            ]
        );
        // Linux: the shell installers, or npm.
        assert_eq!(
            commands(Os::Linux, AgentKind::Codex),
            [
                "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
                "npm install -g @openai/codex"
            ]
        );
        assert_eq!(
            commands(Os::Linux, AgentKind::Claude),
            [
                "curl -fsSL https://claude.ai/install.sh | bash",
                "npm install -g @anthropic-ai/claude-code"
            ]
        );
        // Antigravity: Google's installer, PowerShell's on Windows.
        assert_eq!(
            commands(Os::Mac, AgentKind::Antigravity),
            ["curl -fsSL https://antigravity.google/cli/install.sh | bash"]
        );
        assert_eq!(
            commands(Os::Linux, AgentKind::Antigravity),
            ["curl -fsSL https://antigravity.google/cli/install.sh | bash"]
        );
        assert_eq!(
            commands(Os::Windows, AgentKind::Antigravity),
            ["irm https://antigravity.google/cli/install.ps1 | iex"]
        );
        assert!(all_text(Os::Mac, AgentKind::Antigravity).contains("no `agy` command found"));
        // None speaks of a Mac, Homebrew or the Terminal app.
        for os in [Os::Windows, Os::Linux] {
            for kind in AgentKind::ALL {
                let text = all_text(os, kind);
                for mac in ["Mac", "brew", "Homebrew", "Terminal"] {
                    assert!(!text.contains(mac), "{os:?} {kind:?}: {text}");
                }
                assert!(text.contains("in a terminal"), "{os:?} {kind:?}: {text}");
            }
        }
    }

    #[test]
    fn the_panel_offers_to_install_and_sign_in() {
        use crate::agent_setup::Step;
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().agent.config = None;
        h.state_mut().agent.kind = AgentKind::Codex;
        h.state_mut().agent.found = [Some(None), Some(None), Some(None)];
        h.state_mut().dock.reveal(PanelId::Agent);
        h.run_steps(5);
        // Not installed: a button that does it.
        h.get_by_label("Set up an AI assistant");
        assert_eq!(h.query_all_by_label("Install").count(), 3);
        // A failed install says why, and can be tried again.
        h.state_mut().agent.setup.step = Step::Failed {
            kind: AgentKind::Codex,
            install: true,
            why: "The Codex installer stopped: curl: (6) Could not resolve host.".into(),
        };
        h.run_steps(3);
        h.get_by_label_contains("Could not resolve host");
        assert_eq!(h.query_all_by_label("Install").count(), 3);
        h.state_mut().agent.setup.step = Step::Idle;
        // Installed but plainly signed out: sign in instead of the ideas.
        let codex = PathBuf::from("/usr/local/bin/codex");
        h.state_mut().agent.found[AgentKind::Codex.index()] = Some(Some(codex));
        h.state_mut().agent.setup.signed_in[AgentKind::Codex.index()] = Some(false);
        h.run_steps(3);
        h.get_by_label("Sign in to Codex");
        h.get_by_label("Sign In");
        h.get_by_label_contains("your ChatGPT account");
        assert!(!an_idea_shows(&h), "signing in first");
        // Signed in: the usual start.
        h.state_mut().agent.setup.signed_in[AgentKind::Codex.index()] = Some(true);
        h.run_steps(3);
        assert!(h.query_by_label("Sign In").is_none());
        assert!(an_idea_shows(&h));
        // A chat that failed for want of signing in: the card is above the prompt.
        h.state_mut().agent.log = vec![
            Entry::You("Make it smaller".into()),
            Entry::Error("Not logged in".into()),
        ];
        h.state_mut().agent.setup.signed_in[AgentKind::Codex.index()] = Some(false);
        h.run_steps(3);
        h.get_by_label("Sign In");
    }

    #[test]
    fn a_turn_that_fails_asks_again_whether_its_signed_in() {
        let mut h = studio();
        let i = AgentKind::Codex.index();
        h.state_mut().agent.kind = AgentKind::Codex;
        h.state_mut().agent.setup.signed_in[i] = Some(true);
        h.state_mut().agent.was_running = true;
        h.state_mut().agent.log = vec![Entry::Error("Not logged in".into())];
        h.run_steps(2);
        assert_eq!(h.state().agent.setup.signed_in[i], None, "asked again");
        // Stopping it isn't a reason to.
        h.state_mut().agent.setup.signed_in[i] = Some(true);
        h.state_mut().agent.was_running = true;
        h.state_mut().agent.log.push(Entry::Error("Stopped.".into()));
        h.run_steps(2);
        assert_eq!(h.state().agent.setup.signed_in[i], Some(true));
    }

    #[test]
    fn check_again_forgets_what_was_found() {
        let mut a = AgentState {
            path: Some(Some("/nowhere".into())),
            found: [Some(None), Some(None), Some(None)],
            ..Default::default()
        };
        a.check_again();
        assert_eq!(a.found, [None, None, None]);
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
    fn codex_keeps_to_the_design_tools() {
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
            args.iter().any(|x| x == "features.plugins=false"),
            "{args:?}"
        );
        // A tool that isn't ours, failing, isn't blamed on ZigZag.
        let v = json!({"type": "item.completed", "item": {"id": "item_1", "type": "mcp_tool_call",
            "server": "node_repl", "tool": "js", "status": "failed",
            "error": {"message": "fetch failed"}}});
        let said = codex_event(&v, &mut None);
        assert!(
            matches!(&said[1].1, Entry::Error(m) if m.contains("node_repl") && m.contains("fetch failed") && !m.contains("ZigZag")),
            "{said:?}"
        );
    }

    #[test]
    fn a_session_thats_gone_isnt_tried_again() {
        assert!(session_gone("No conversation found with session ID: 123"));
        assert!(session_gone("thread/resume failed: rollout not found"));
        assert!(!session_gone("thread 01a1 already has an active writer"));
        assert!(!session_gone("file not found: a.png"));
        let child = Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                &["/C", "exit 0"][..]
            } else {
                &[][..]
            })
            .spawn()
            .unwrap();
        let (tx, rx) = mpsc::channel();
        let mut a = AgentState {
            kind: AgentKind::Claude,
            session: Some("old".into()),
            run: Some(Run {
                child: Arc::new(Mutex::new(child)),
                rx,
                stderr: Vec::new(),
                exit: None,
                eof: true,
                kind: AgentKind::Claude,
                resumed: true,
                agy: AgyStream::default(),
                result_at: None,
            }),
            ..Default::default()
        };
        a.log.push(Entry::You("go on".into()));
        tx.send(Event::Stderr(
            "No conversation found with session ID: old".into(),
        ))
        .unwrap();
        tx.send(Event::Exit(Some(1))).unwrap();
        a.poll();
        assert!(!a.running());
        assert!(a.session.is_none(), "the next prompt starts afresh");
        assert!(
            matches!(a.log.last(), Some(Entry::Info(m)) if m.contains("new one")),
            "{:?}",
            a.log
        );
    }

    #[test]
    fn a_step_cut_short_stops_spinning() {
        let mut a = AgentState::default();
        a.log.push(Entry::You("go".into()));
        a.push(
            Some("1".into()),
            Entry::Tool("add_svg".into(), String::new(), None),
        );
        a.settle(false);
        assert_eq!(
            a.log[1],
            Entry::Tool("add_svg".into(), String::new(), Some(false))
        );
        assert_eq!(a.status(), "Thinking");
    }

    #[test]
    fn the_last_lines_are_read_though_the_program_has_ended() {
        let _env = AGENT_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let mut child = Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                &["/C", "exit 0"][..]
            } else {
                &[][..]
            })
            .spawn()
            .unwrap();
        let _ = child.wait();
        let (tx, rx) = mpsc::channel();
        let mut a = AgentState {
            kind: AgentKind::Claude,
            run: Some(Run {
                child: Arc::new(Mutex::new(child)),
                rx,
                stderr: Vec::new(),
                exit: None,
                eof: false,
                kind: AgentKind::Claude,
                resumed: false,
                agy: AgyStream::default(),
                result_at: None,
            }),
            ..Default::default()
        };
        // It has ended, but what it printed last is still on its way.
        tx.send(Event::Exit(Some(0))).unwrap();
        a.poll();
        assert!(a.running(), "waits for the rest of its output");
        tx.send(Event::Line(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"All done."}]}}"#
                .into(),
        ))
        .unwrap();
        tx.send(Event::Eof).unwrap();
        a.poll();
        assert!(!a.running());
        assert!(
            a.log.contains(&Entry::Said("All done.".into())),
            "{:?}",
            a.log
        );
    }

    #[test]
    fn steps_one_after_another_are_one_line_that_opens() {
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().dock.reveal(PanelId::Agent);
        let a = &mut h.state_mut().agent;
        a.log.push(Entry::You("tidy".into()));
        for (k, t) in [
            "get_path",
            "measure_stitching",
            "measure_stitching",
            "edit_path",
        ]
        .iter()
        .enumerate()
        {
            a.push(
                Some(k.to_string()),
                Entry::Tool((*t).into(), format!("object: {k}"), Some(k != 3)),
            );
        }
        a.log.push(Entry::Said("Done tidying.".into()));
        a.push(
            Some("9".into()),
            Entry::Tool("render_preview".into(), String::new(), Some(true)),
        );
        h.run_steps(4);
        // Four steps: one line, saying what they were.
        h.get_by_label("Show 4 steps: Reading · Checking ×2 · Reshaping · 1 failed");
        assert!(h.query_by_label("object: 1").is_none(), "folded away");
        // One alone is folded away too.
        h.get_by_label("Show 1 step: Looking");
        assert!(h.query_by_label("render_preview").is_none());
        h.get_by_label_contains("Show 4 steps").click();
        h.run_steps(3);
        h.get_by_label("object: 1");
        h.get_by_label_contains("Hide 4 steps").click();
        h.run_steps(3);
        assert!(h.query_by_label("object: 1").is_none());
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

    /// Lines as Antigravity's `--output-format stream-json` writes them.
    fn agy_lines(lines: &[&str]) -> (Vec<Entry>, Option<String>) {
        let mut session = None;
        let mut st = AgyStream::new();
        let mut a = AgentState::default();
        for l in lines {
            let v: Value = serde_json::from_str(l).unwrap();
            for (id, e) in agy_event(&v, &mut session, &mut st) {
                a.push(id, e);
            }
        }
        (a.log, session)
    }

    #[test]
    fn antigravitys_stream_is_read() {
        let (log, session) = agy_lines(&[
            r#"{"event":"init","init":{"cwd":"/tmp/a","tools":["mcp_zigzag_get_design","call_mcp_tool"],"permission_mode":"request-review"}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c-42","step_index":0,"state":"DONE","step_type":"user_input"}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c-42","step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"Let me look "}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c-42","step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"at the design."}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c-42","step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"mcp_zigzag_get_design","tool_info":{"name":"mcp_zigzag_get_design","parameters":{}}}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c-42","step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"mcp_zigzag_get_design","tool_info":{"name":"mcp_zigzag_get_design","parameters":{}}}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c-42","step_index":2,"state":"DONE","step_type":"tool","tool_name":"mcp_zigzag_get_design","duration_seconds":0.4,"tool_info":{"name":"mcp_zigzag_get_design","parameters":{},"output":"{\"objects\":[]}"}}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c-42","step_index":3,"state":"ACTIVE","step_type":"tool","tool_name":"call_mcp_tool","tool_info":{"name":"call_mcp_tool","parameters":{"server_name":"zigzag","tool_name":"set_object","arguments":{"object":1,"stitch":"satin"}}}}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c-42","step_index":3,"state":"DONE","step_type":"tool","tool_name":"call_mcp_tool","tool_info":{"name":"call_mcp_tool","parameters":{"server_name":"zigzag","tool_name":"set_object","arguments":{"object":1,"stitch":"satin"}},"error":{"type":"TOOL_ERROR","message":"no object 1"}}}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c-42","step_index":4,"state":"ACTIVE","step_type":"agent_response","text_delta":"There's nothing to restyle yet."}}"#,
            r#"{"event":"result","result":{"conversation_id":"c-42","status":"SUCCESS","response":"Let me look at the design.There's nothing to restyle yet.","duration_seconds":12.2,"num_turns":1,"usage":{"total_tokens":10}}}"#,
        ]);
        assert_eq!(session.as_deref(), Some("c-42"), "kept to carry on");
        assert_eq!(
            log,
            [
                Entry::Said("Let me look at the design.".into()),
                Entry::Tool("get_design".into(), String::new(), Some(true)),
                Entry::Tool("set_object".into(), "object: 1, stitch: satin".into(), Some(false)),
                Entry::Error("set_object: no object 1".into()),
                Entry::Said("There's nothing to restyle yet.".into()),
                Entry::Done("1 turns · 12 s".into()),
            ]
        );
        // Nothing said along the way: the result's response is.
        let (log, _) = agy_lines(&[
            r#"{"event":"result","result":{"conversation_id":"c-1","status":"SUCCESS","response":"OK","duration_seconds":1,"num_turns":1}}"#,
        ]);
        assert_eq!(log, [Entry::Said("OK".into()), Entry::Done("1 turns · 1 s".into())]);
        // A turn that failed says why; one signed out says to sign in.
        let (log, session) = agy_lines(&[
            r#"{"event":"result","result":{"conversation_id":"","status":"ERROR","response":"","error":"model quota exceeded","duration_seconds":0,"num_turns":0}}"#,
        ]);
        assert_eq!(log, [Entry::Error("model quota exceeded".into())]);
        assert_eq!(session, None, "no conversation to carry on");
        let (log, _) = agy_lines(&[
            r#"{"event":"result","result":{"conversation_id":"","status":"ERROR","response":"","error":"authentication failed or timed out","duration_seconds":0,"num_turns":0,"finished_reason":"error"}}"#,
        ]);
        assert!(matches!(&log[..], [Entry::Error(m)] if m.contains("isn't signed in")), "{log:?}");
        let (log, _) = agy_lines(&[r#"{"event":"result","result":{"status":"CANCELED"}}"#]);
        assert_eq!(log, [Entry::Error("Stopped: canceled".into())]);
    }

    #[test]
    fn antigravity_runs_with_only_zigzags_tools() {
        let dir = std::env::temp_dir().join(format!("zigzag-agy-cmd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let args = |a: &mut AgentState| -> Vec<String> {
            a.command("agy".into(), "http://127.0.0.1:4711/mcp?panel=1", None, 4711)
                .get_args()
                .map(|x| x.to_string_lossy().into_owned())
                .collect()
        };
        let mut a = AgentState {
            kind: AgentKind::Antigravity,
            config: Some(dir.join("agent.json")),
            ..Default::default()
        };
        let c = args(&mut a);
        let joined = c.join(" ");
        assert!(joined.contains("--output-format stream-json"), "{joined}");
        assert!(c.contains(&"--sandbox".to_string()));
        assert!(!joined.contains("dangerously"), "never every tool");
        assert!(!joined.contains("--conversation"), "a new one");
        assert!(!c.contains(&"--mode".to_string()), "not auto");
        assert_eq!(c.last().map(String::as_str), Some("-p"), "the prompt follows");
        let settings: Value = c
            .windows(2)
            .find(|w| w[0] == "--settings")
            .map(|w| serde_json::from_str(&w[1]).unwrap())
            .expect("settings");
        assert_eq!(settings["agentMode"], "", "its default mode, which asks");
        let allow: Vec<&str> = settings["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(allow.contains(&"mcp(zigzag/*)"), "{allow:?}");
        assert!(
            allow.iter().all(|r| r.starts_with("mcp(zigzag/") || r.starts_with("read_file(")),
            "nothing else: {allow:?}"
        );
        let deny = settings["permissions"]["deny"].to_string();
        for d in ["command(*)", "write_file(*)", "read_url(*)", "execute_url(*)", "download_url(*)"] {
            assert!(deny.contains(d), "{d} refused: {deny}");
        }
        // Its folder: ZigZag's server on this launch's port, and the brief.
        let work = dir.join("agent").join("antigravity");
        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(work.join(".agents/mcp_config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            config["mcpServers"]["zigzag"]["serverUrl"],
            "http://127.0.0.1:4711/mcp?panel=1"
        );
        assert_eq!(config["mcpServers"].as_object().unwrap().len(), 1);
        assert_eq!(config["mcpServers"]["zigzag"]["tools"]["add_svg"]["eager"], true);
        let brief = std::fs::read_to_string(work.join("AGENTS.md")).unwrap();
        assert!(brief.starts_with(BRIEF), "the brief");
        // An image going with the prompt can be read, and nothing else.
        a.sending_images = vec![PathBuf::from("/tmp/attached/cat.png")];
        let joined = args(&mut a).join(" ");
        assert!(joined.contains("read_file(/tmp/attached/cat.png)"), "{joined}");
        a.sending_images.clear();
        // The second turn carries on the conversation, with its choices.
        a.session = Some("c-42".into());
        a.model[AgentKind::Antigravity.index()] = Some("gemini-4-pro".into());
        a.effort[AgentKind::Antigravity.index()] = Some("xhigh".into());
        let joined = args(&mut a).join(" ");
        assert!(joined.contains("--conversation c-42"), "{joined}");
        assert!(joined.contains("--model gemini-4-pro"), "{joined}");
        assert!(joined.contains("--effort xhigh"), "{joined}");
        assert!(joined.ends_with(" -p"), "{joined}");
        // A new launch, another port: the file follows.
        let _ = a
            .command("agy".into(), "http://127.0.0.1:5000/mcp?panel=1", None, 5000)
            .get_args()
            .count();
        let config = std::fs::read_to_string(work.join(".agents/mcp_config.json")).unwrap();
        assert!(config.contains("127.0.0.1:5000"), "{config}");
        // Codex's and Claude's folder isn't Antigravity's (Codex reads AGENTS.md).
        a.kind = AgentKind::Codex;
        assert_eq!(a.work_dir(), dir.join("agent"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stand-in for `agy`: it takes the prompt from its command line,
    /// prints its result, then (as the real one sometimes does) doesn't exit.
    #[cfg(unix)]
    #[test]
    fn an_antigravity_that_never_exits_is_stopped_after_its_result() {
        let _env = AGENT_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zigzag-agy-fake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("agy");
        std::fs::write(
            &fake,
            "#!/bin/sh\nfor p; do :; done\n\
             printf '{\"event\":\"step_update\",\"step_update\":{\"conversation_id\":\"c-7\",\"step_index\":1,\"state\":\"DONE\",\"step_type\":\"agent_response\",\"text_delta\":\"you said %s\"}}\\n' \"$p\"\n\
             echo '{\"event\":\"result\",\"result\":{\"conversation_id\":\"c-7\",\"status\":\"SUCCESS\",\"response\":\"\",\"duration_seconds\":2,\"num_turns\":1}}'\n\
             sleep 60\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut a = AgentState {
            kind: AgentKind::Antigravity,
            path: Some(None),
            config: Some(dir.join("agent.json")),
            ..Default::default()
        };
        a.found[AgentKind::Antigravity.index()] = Some(Some(fake.clone()));
        unsafe { std::env::set_var("ZIGZAG_ANTIGRAVITY", &fake) };
        let ctx = egui::Context::default();
        a.prompt = "hello there".into();
        a.send(Some("127.0.0.1:1".parse().unwrap()), &ctx);
        unsafe { std::env::remove_var("ZIGZAG_ANTIGRAVITY") };
        let t = Instant::now();
        while a.running() && t.elapsed() < Duration::from_secs(20) {
            a.poll();
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!a.running(), "stopped once it had answered");
        assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
        assert_eq!(a.session.as_deref(), Some("c-7"));
        assert!(a.log.contains(&Entry::Said("you said hello there".into())), "{:?}", a.log);
        assert_eq!(a.log.last(), Some(&Entry::Done("1 turns · 2 s".into())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_install_doesnt_hold_up_another_agent() {
        use crate::agent_setup::Step;
        let mut a = AgentState {
            kind: AgentKind::Codex,
            found: found_only(&[AgentKind::Claude]),
            ..Default::default()
        };
        a.setup.step = Step::Failed {
            kind: AgentKind::Codex,
            install: true,
            why: "The Codex installer stopped.".into(),
        };
        assert!(a.setup_page(), "Codex isn't here, and its install failed");
        // Claude Code is installed: choosing it leaves Setup.
        a.set_kind(AgentKind::Claude);
        assert!(!a.setup_page());
        assert!(!setting_up(&a, true) && !setting_up(&a, false));
        // A failed sign-in to one agent isn't another's either.
        a.setup.step = Step::Failed {
            kind: AgentKind::Codex,
            install: false,
            why: "Signing in didn't finish.".into(),
        };
        assert!(!setting_up(&a, false), "Claude Code's panel");
        a.kind = AgentKind::Codex;
        assert!(setting_up(&a, false), "Codex's");
    }

    #[test]
    fn antigravitys_prompt_names_its_images_and_is_never_an_option() {
        assert_eq!(agy_prompt("make it blue", &[]), "make it blue");
        assert_eq!(agy_prompt("--help", &[]), " --help", "not taken for an option");
        let one = agy_prompt("trace this", &[PathBuf::from("/tmp/attached/cat.png")]);
        assert!(one.starts_with("trace this\n\nAn image is attached"), "{one}");
        assert!(one.ends_with("\n/tmp/attached/cat.png"), "{one}");
        let two = agy_prompt("these", &[PathBuf::from("/a.png"), PathBuf::from("/b.png")]);
        assert!(two.contains("Images are attached") && two.ends_with("\n/a.png\n/b.png"), "{two}");
    }

    #[test]
    fn antigravity_is_found_where_its_installer_puts_it() {
        let dirs = install_dirs(Os::Windows, r"C:\Users\pat", Some(r"C:\Users\pat\AppData\Roaming"), Some(r"C:\Users\pat\AppData\Local"));
        assert!(dirs.contains(&PathBuf::from(r"C:\Users\pat\AppData\Local").join(r"agy\bin")), "{dirs:?}");
        let dirs = install_dirs(Os::Mac, "/Users/pat", None, None);
        assert!(dirs.contains(&PathBuf::from("/Users/pat/.local/bin")), "{dirs:?}");
        assert!(!dirs.iter().any(|d| d.to_string_lossy().contains("agy")));
        assert_eq!(AgentKind::from_key("antigravity"), Some(AgentKind::Antigravity));
        assert_eq!(AgentKind::Antigravity.program(), "agy");
        assert_eq!(AgentKind::ALL.last(), Some(&AgentKind::Antigravity), "last in the menu");
    }

    #[test]
    fn signing_in_to_antigravity_takes_a_pasted_code() {
        use crate::agent_setup::Step;
        use crate::dock::PanelId;
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().agent.config = None;
        h.state_mut().agent.kind = AgentKind::Antigravity;
        let i = AgentKind::Antigravity.index();
        h.state_mut().agent.found[i] = Some(Some(PathBuf::from("/usr/local/bin/agy")));
        h.state_mut().agent.setup.signed_in[i] = Some(false);
        h.state_mut().dock.reveal(PanelId::Agent);
        h.run_steps(5);
        h.get_by_label("Sign in to Antigravity");
        h.get_by_label_contains("your Google account");
        h.get_by_label("Sign In");
        // Signing in failed for want of time: it says so, and can go again.
        h.state_mut().agent.setup.step = Step::Failed {
            kind: AgentKind::Antigravity,
            install: false,
            why: "Signing in took longer than a minute, so Antigravity gave up. Click Sign In to try again.".into(),
        };
        h.run_steps(3);
        h.get_by_label_contains("longer than a minute");
        h.get_by_label("Sign In");
        // Its page shows a code to paste back (on Windows, into its window).
        assert_eq!(AgentKind::Antigravity.pastes_code(), !cfg!(windows));
        assert!(AgentKind::Claude.pastes_code() && !AgentKind::Codex.pastes_code());
    }
}
