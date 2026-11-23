//! Settings that follow you between computers, while signed in
//! (zigzag-server's `account-settings.ts`): your machines and their hoops,
//! the machine new documents start with, your thread palettes, the thread
//! chart, how you last shared a design, and the last lettering font. What
//! describes this computer (appearance, interface size, the GPU, the agent's
//! server and port, folded panels…) stays here.
//!
//! Each setting has a key and a time: when it last changed on this computer.
//! The server keeps, per key, the newest change from any computer. So:
//!
//! - **Pull** on signing in, at launch, and when the window comes to the
//!   front (at most once a minute): a key newer there is applied here.
//! - **Push** a key as soon as it changes here (looked for every few
//!   seconds by comparing each key's value with the last one seen), with
//!   the time it changed.
//!
//! Nothing is pushed before the first pull, so a fresh install doesn't
//! write its defaults over what you have. What a key was last seen as, and
//! when it changed, are kept in `sync.json` in the settings folder: written
//! down once the server has it, so a change that didn't get there (offline,
//! or the app quit first) is still a change the next time.
//!
//! A key changed here since it was last in step isn't pulled over: this
//! computer's is the newer, and goes up. The first time a computer syncs,
//! the machines and palettes it already has are kept beside the account's.
//! When the server can't be reached, it's tried again after longer and
//! longer waits.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::app::Studio;
use crate::share::{Api, Cred};

/// The keys that follow you.
pub const KEYS: [&str; 6] = [
    "machines",
    "machines.current",
    "palettes",
    "threads.chart",
    "share.defaults",
    "lettering.font",
];

/// How often to look for changes to push.
const LOOK_EVERY: Duration = Duration::from_secs(3);
/// Pulls when the window comes to the front, at most this often.
const PULL_EVERY: Duration = Duration::from_secs(60);
/// Looks anyway this often, saved or not (a change that wasn't saved
/// through `touched` still goes up).
const LOOK_ANYWAY: Duration = Duration::from_secs(60);
/// How many times a synced setting's file has been saved: the look for
/// changes waits for one (it writes every setting out to compare).
static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A file with synced settings in it was saved (machines, palettes, the
/// preferences): there may be something to push.
pub fn touched() {
    SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// The longest wait before trying a server that didn't answer again.
const RETRY_AT_MOST: Duration = Duration::from_secs(300);

/// `sync.json`: per key, when it last changed here and what it was.
#[derive(Debug, Default)]
pub struct SyncFile {
    path: Option<PathBuf>,
    /// When each key last changed (here, or as pulled), ms since 1970.
    pub times: BTreeMap<String, u64>,
    /// A fingerprint of each key's value as the server last had it (pushed
    /// or pulled).
    pub seen: BTreeMap<String, u64>,
}

impl SyncFile {
    pub fn in_config() -> SyncFile {
        if cfg!(test) {
            return SyncFile::default();
        }
        match crate::dock::config_file("sync.json") {
            Some(p) => SyncFile::at(p),
            None => SyncFile::default(),
        }
    }

    pub fn at(path: PathBuf) -> SyncFile {
        let v: Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        let map = |k: &str| -> BTreeMap<String, u64> {
            v[k].as_object()
                .into_iter()
                .flatten()
                .filter_map(|(k, n)| Some((k.clone(), n.as_u64()?)))
                .collect()
        };
        SyncFile {
            times: map("times"),
            seen: map("seen"),
            path: Some(path),
        }
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let v = json!({ "times": self.times, "seen": self.seen });
        let _ = std::fs::write(path, v.to_string());
    }

    /// Forget it all (signed out: the next account starts fresh).
    pub fn clear(&mut self) {
        self.times.clear();
        self.seen.clear();
        self.save();
    }
}

/// What a job against the server says back.
pub enum Msg {
    /// The account's settings and their times.
    Pulled {
        settings: Value,
        times: Value,
    },
    Pushed,
    Failed,
}

/// Keeping settings in step with the account.
#[derive(Default)]
pub struct Sync {
    pub file: Option<SyncFile>,
    /// A job under way: its answer, and the session it ended with.
    pub busy: Option<mpsc::Receiver<(Msg, Option<String>)>>,
    /// The session the last job had, and whose (its email): the next
    /// job uses it, signing in again only if it's refused.
    pub session: Option<(String, String)>,
    /// How many saves there had been at the last look (None: look
    /// whatever), and when that was.
    pub looked_at: Option<u64>,
    pub last_full: Option<Instant>,
    /// Pulled at least once since signing in (or launch): pushing may start.
    pub pulled: bool,
    pub last_look: Option<Instant>,
    pub last_pull: Option<Instant>,
    /// Pull at the next chance (signed in, launched, came to the front).
    pub want_pull: bool,
    /// The window was in front last frame.
    pub focused: bool,
    /// The server, when not `share::server()` (tests).
    pub base: Option<String>,
    /// The keys of a push under way, each with its value's fingerprint and
    /// the time it changed: written down when the server has them.
    pub pushing: Vec<(String, u64, u64)>,
    /// The job under way is a pull (asked for again if it fails).
    pub pulling: bool,
    /// Whose account the job under way is for: its answer is dropped if
    /// that's no longer who's signed in.
    pub busy_for: Option<String>,
    /// Jobs that failed in a row, and when to try the server again.
    pub fails: u32,
    pub retry_at: Option<Instant>,
}

impl Sync {
    pub fn file(&mut self) -> &mut SyncFile {
        self.file.get_or_insert_with(SyncFile::in_config)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn print(v: &Value) -> u64 {
    crate::share::fingerprint(&v.to_string())
}

impl Studio {
    /// A synced setting's value here.
    pub(crate) fn sync_value(&self, key: &str) -> Value {
        match key {
            "machines" => self.machines.mine_json(),
            "machines.current" => json!(self.machines.current),
            "palettes" => self.palettes.to_json(),
            "threads.chart" => json!(match self.prefs.thread_chart {
                crate::threads::Brand::Brother => "brother",
                crate::threads::Brand::Janome => "janome",
            }),
            "share.defaults" => self.prefs.share_defaults.clone().unwrap_or(Value::Null),
            "lettering.font" => json!(self.prefs.last_font),
            _ => Value::Null,
        }
    }

    /// Take a synced setting from another computer, and keep it.
    pub(crate) fn apply_sync_value(&mut self, key: &str, v: &Value) {
        match key {
            "machines" => {
                self.machines.set_mine_json(v);
                self.machines.save();
            }
            "machines.current" => {
                if let Some(id) = v.as_str().filter(|id| self.machines.find(id).is_some()) {
                    self.machines.current = Some(id.to_string());
                    self.machines.save();
                }
            }
            "palettes" => {
                self.palettes.set_from_json(v);
                self.palettes.save();
            }
            "threads.chart" => {
                let b = if v.as_str() == Some("janome") {
                    crate::threads::Brand::Janome
                } else {
                    crate::threads::Brand::Brother
                };
                self.set_thread_chart(b);
            }
            "share.defaults" => {
                self.prefs.share_defaults = v.is_object().then(|| v.clone());
                self.prefs.save();
            }
            "lettering.font" => {
                self.prefs.last_font = v.as_str().map(String::from);
                self.prefs.save();
            }
            _ => {}
        }
    }

    fn sync_job(&mut self, job: impl Fn(&Api) -> Result<Msg, ()> + Send + 'static) {
        let Some(a) = self.account.signed_in() else {
            return;
        };
        let base = self.sync.base.clone().unwrap_or_else(crate::share::server);
        let (tx, rx) = mpsc::channel();
        let ctx = self.egui_ctx.clone();
        let device = a.device.clone();
        let session = self
            .sync
            .session
            .as_ref()
            .filter(|(who, _)| *who == a.email)
            .map(|(_, token)| token.clone());
        let _ = std::thread::Builder::new()
            .name("zigzag-sync".into())
            .spawn(move || {
                let mut api = Api::new(&base);
                // With the session from last time; signed in again if
                // there's none, or it's no longer taken.
                let mut out = Err(());
                if session.is_some() {
                    api.set_session(session);
                    out = job(&api);
                }
                if out.is_err() {
                    api.set_session(None);
                    if api.sign_in(&Cred::Device(device), &|_| {}).is_ok() {
                        out = job(&api);
                    }
                }
                let _ = tx.send((out.unwrap_or(Msg::Failed), api.session()));
                if let Some(c) = &ctx {
                    c.request_repaint();
                }
            });
        self.sync.busy = Some(rx);
        self.sync.busy_for = Some(a.email);
    }

    /// Pull the account's settings now.
    pub(crate) fn sync_pull(&mut self) {
        self.sync.want_pull = false;
        self.sync.last_pull = Some(Instant::now());
        self.sync.pulling = true;
        self.sync_job(|api| {
            let v = api
                .call("GET", "/v1/me/settings", None, &[])
                .map_err(|_| ())?;
            Ok(Msg::Pulled {
                settings: v["settings"].clone(),
                times: v["times"].clone(),
            })
        });
    }

    /// Apply what was pulled: each key newer there than here.
    fn take_pulled(&mut self, settings: &Value, times: &Value) {
        for key in KEYS {
            let Some(t) = times[key].as_u64() else {
                continue;
            };
            let f = self.sync.file();
            let here = f.times.get(key).copied().unwrap_or(0);
            let seen = f.seen.get(key).copied();
            if t <= here {
                continue;
            }
            // Changed here since it was last in step (an edit the look
            // hasn't come to yet, or one that never got to the server):
            // this computer's is the newer, and goes up at the next look.
            if seen.is_some_and(|s| s != print(&self.sync_value(key))) {
                continue;
            }
            let v = settings.get(key).cloned().unwrap_or(Value::Null);
            // (A list that isn't one would empty this computer's.)
            let list = matches!(key, "machines" | "palettes");
            if list && !v[key].is_array() {
                continue;
            }
            // The first time this computer syncs, what it already has is
            // kept beside the account's (and goes up with it).
            let mine = (list && seen.is_none())
                .then(|| (self.machines.mine.clone(), self.palettes.mine.clone()));
            self.apply_sync_value(key, &v);
            let pulled = print(&self.sync_value(key));
            if let Some((machines, palettes)) = mine {
                if key == "machines" {
                    for m in machines {
                        if self.machines.find(&m.id).is_none() {
                            self.machines.mine.push(m);
                        }
                    }
                    self.machines.save();
                } else {
                    for p in palettes {
                        if self.palettes.mine.iter().all(|q| q.name != p.name) {
                            self.palettes.mine.push(p);
                        }
                    }
                    self.palettes.save();
                }
            }
            let f = self.sync.file();
            f.times.insert(key.to_string(), t);
            f.seen.insert(key.to_string(), pulled);
        }
        self.sync.file().save();
        self.sync.pulled = true;
        // (What's only here goes up at the next look.)
        self.sync.looked_at = None;
    }

    /// The server answered: no more waiting before the next job.
    fn sync_answered(&mut self) {
        self.sync.busy = None;
        self.sync.pulling = false;
        self.sync.fails = 0;
        self.sync.retry_at = None;
    }

    /// The job under way came to nothing: what it was for is still to do,
    /// after a wait that doubles each time (a server that's down, or no
    /// network, isn't asked every few seconds).
    fn sync_failed(&mut self) {
        self.sync.busy = None;
        self.sync.pushing.clear();
        self.sync.looked_at = None;
        if std::mem::take(&mut self.sync.pulling) {
            self.sync.want_pull = true;
        }
        self.sync.fails = (self.sync.fails + 1).min(16);
        let wait = (LOOK_EVERY * 2u32.pow(self.sync.fails.min(8))).min(RETRY_AT_MOST);
        self.sync.retry_at = Some(Instant::now() + wait);
    }

    /// Each frame: take a job's answer, pull when it's wanted, and push
    /// what changed here.
    pub(crate) fn sync_tick(&mut self, ctx: &eframe::egui::Context) {
        // (A job for an account since signed out of, or changed for
        // another: its answer isn't this account's.)
        let who = self.account.signed_in().map(|a| a.email);
        if self.sync.busy.is_some() && self.sync.busy_for != who {
            self.sync.busy = None;
            self.sync.pulling = false;
            self.sync.pushing.clear();
        }
        if let Some(rx) = &self.sync.busy {
            let got = rx.try_recv();
            if let (Ok((_, session)), Some(who)) = (&got, &who) {
                self.sync.session = session.clone().map(|t| (who.clone(), t));
            }
            match got.map(|(m, _)| m) {
                Ok(Msg::Pulled { settings, times }) => {
                    self.sync_answered();
                    self.take_pulled(&settings, &times);
                }
                Ok(Msg::Pushed) => {
                    self.sync_answered();
                    // The server has them: now they're in step.
                    let sent = std::mem::take(&mut self.sync.pushing);
                    let f = self.sync.file();
                    for (k, p, t) in sent {
                        f.seen.insert(k.clone(), p);
                        f.times.insert(k, t);
                    }
                    f.save();
                }
                Ok(Msg::Failed) | Err(mpsc::TryRecvError::Disconnected) => self.sync_failed(),
                Err(mpsc::TryRecvError::Empty) => return,
            }
        }
        if self.account.signed_in().is_none() {
            self.sync.pulled = false;
            return;
        }
        // Came to the front: pull (now and then).
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused
            && !self.sync.focused
            && self.sync.last_pull.is_none_or(|t| t.elapsed() > PULL_EVERY)
        {
            self.sync.want_pull = true;
        }
        self.sync.focused = focused;
        if !self.sync.pulled && self.sync.last_pull.is_none() {
            self.sync.want_pull = true;
        }
        if self.sync.retry_at.is_some_and(|t| Instant::now() < t) {
            return;
        }
        if self.sync.want_pull {
            self.sync_pull();
            return;
        }
        if !self.sync.pulled
            || self
                .sync
                .last_look
                .is_some_and(|t| t.elapsed() < LOOK_EVERY)
        {
            return;
        }
        self.sync.last_look = Some(Instant::now());
        // Nothing saved since the last look: nothing to compare (now and
        // then anyway).
        let saves = SAVES.load(std::sync::atomic::Ordering::Relaxed);
        if self.sync.looked_at == Some(saves)
            && self.sync.last_full.is_some_and(|t| t.elapsed() < LOOK_ANYWAY)
        {
            return;
        }
        self.sync.looked_at = Some(saves);
        self.sync.last_full = Some(Instant::now());
        // What changed here since it was last seen goes up, with its time.
        let now = now_ms();
        let mut changes = serde_json::Map::new();
        let mut pushing = Vec::new();
        for key in KEYS {
            let v = self.sync_value(key);
            let p = print(&v);
            if self.sync.file().seen.get(key) == Some(&p) {
                continue;
            }
            pushing.push((key.to_string(), p, now));
            changes.insert(key.to_string(), json!({ "value": v, "t": now }));
        }
        if changes.is_empty() {
            return;
        }
        self.sync.pushing = pushing;
        let body = json!({ "changes": changes });
        self.sync_job(move |api| {
            api.json("PATCH", "/v1/me/settings", &body)
                .map(|_| Msg::Pushed)
                .map_err(|_| ())
        });
    }
}
