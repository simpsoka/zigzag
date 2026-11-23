//! File › Share…: a page for the design at zigzag.diy/share/:id, with its
//! picture, stitch player and downloads (the share page, served by the
//! zigzag-front Worker from what ZigZag's server keeps).
//!
//! What the person chooses, as the server knows it (zigzag-server's
//! `storage.ts` › Share page):
//!
//! - **Who can see it.** Public (anyone, and search engines), a link that
//!   expires (a signed link to the page, up to a week), or private (only
//!   its owner; every link stops working).
//! - **License**: what others may do with it, CC BY 4.0 unless changed.
//! - **Editable**: whether others get the design itself, or only its
//!   picture and machine files. On unless turned off.
//!
//! Sharing needs no account. The first share makes a guest on the server
//! and keeps its secret in ZigZag's settings folder (`shares.json`, only
//! this user may read it): whoever has the secret can change and delete
//! what it shared, so it never leaves this computer except to the server,
//! and the dialog says that a share can only be managed from here.
//!
//! A share uploads three things: the design as ZigZag saves it (its
//! source, with every setting), its picture, and its stitch plan
//! (`formats::plan`), from which the server writes machine files byte for
//! byte as ZigZag would. Each design's share is remembered by the design's
//! own id (the one its chats are kept under), so sharing it again updates
//! the same page and link.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

use eframe::egui::{self, Vec2};
use serde_json::{Value, json};

use crate::app::Studio;
use crate::icons::Icon;
use crate::theme::space;

/// ZigZag's server. `ZIGZAG_SERVER` points the app at another one (the
/// dev deployment, or one running locally).
pub const SERVER: &str = "https://api.zigzag.diy";

pub(crate) fn server() -> String {
    std::env::var("ZIGZAG_SERVER")
        .ok()
        .filter(|s| {
            // (This computer only: not `http://localhost.example.com`.)
            let local = |host: &str| {
                s.strip_prefix(host)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
            };
            s.starts_with("https://") || local("http://localhost") || local("http://127.0.0.1")
        })
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_else(|| SERVER.into())
}

/// Who can see a shared design.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Anyone with the page's address; search engines list it.
    Public,
    /// Only with a link that stops working after a while.
    Link,
    /// Nobody but its owner: every link stops working.
    Private,
}

impl Access {
    fn key(self) -> &'static str {
        match self {
            Access::Public => "public",
            Access::Link => "link",
            Access::Private => "private",
        }
    }
    fn from_key(k: &str) -> Option<Access> {
        Some(match k {
            "public" => Access::Public,
            "link" => Access::Link,
            "private" => Access::Private,
            _ => return None,
        })
    }
}

/// A link that lasts "forever": a hundred years, as long as the server
/// lets a link to the share page last (it can still be revoked at once).
pub const FOREVER: u32 = u32::MAX;

/// How long a link lasts, longest first: forever unless chosen otherwise.
/// (0 is an hour.)
pub const LINK_DAYS: [(u32, &str); 6] = [
    (FOREVER, "forever"),
    (365, "for 1 year"),
    (30, "for 1 month"),
    (7, "for 1 week"),
    (1, "for 1 day"),
    (0, "for 1 hour"),
];

fn link_seconds(days: u32) -> u64 {
    match days {
        0 => 3600,
        FOREVER => 100 * 365 * 86_400,
        d => d as u64 * 86_400,
    }
}

/// The licenses the server takes (`LICENSES` in its `config.ts`): id,
/// name, and what it lets others do. The first is the default.
pub const LICENSES: [(&str, &str, &str); 6] = [
    (
        "CC-BY-4.0",
        "CC BY 4.0",
        "Others may sew, change and share it, crediting you.",
    ),
    (
        "CC-BY-SA-4.0",
        "CC BY-SA 4.0",
        "As CC BY, and what they make from it is shared the same way.",
    ),
    ("CC-BY-NC-4.0", "CC BY-NC 4.0", "As CC BY, but not to sell."),
    (
        "CC-BY-NC-SA-4.0",
        "CC BY-NC-SA 4.0",
        "Not to sell, and shared the same way.",
    ),
    (
        "CC0-1.0",
        "CC0 (public domain)",
        "Anyone may do anything with it, no credit needed.",
    ),
    (
        "all-rights-reserved",
        "All rights reserved",
        "Others may look and sew it for themselves, nothing more.",
    ),
];

fn license_name(id: &str) -> &str {
    LICENSES.iter().find(|l| l.0 == id).map_or(id, |l| l.1)
}

/// What the person chose in the dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub name: String,
    pub access: Access,
    /// How long a link lasts (`LINK_DAYS`; 0 is an hour).
    pub days: u32,
    pub license: String,
    pub editable: bool,
}

impl Settings {
    fn new(name: &str) -> Settings {
        Settings {
            name: name.to_string(),
            access: Access::Public,
            days: FOREVER,
            license: LICENSES[0].0.into(),
            editable: true,
        }
    }

    /// A new share's settings: how the last one was shared (`Prefs::share_defaults`).
    pub fn with_defaults(name: &str, d: Option<&Value>) -> Settings {
        let mut s = Settings::new(name);
        let Some(d) = d else { return s };
        if let Some(a) = d["access"]
            .as_str()
            .and_then(Access::from_key)
            .filter(|a| *a != Access::Private)
        {
            s.access = a;
        }
        if let Some(n) = d["days"]
            .as_u64()
            .filter(|n| LINK_DAYS.iter().any(|l| l.0 as u64 == *n))
        {
            s.days = n as u32;
        }
        if let Some(l) = d["license"]
            .as_str()
            .filter(|l| LICENSES.iter().any(|x| x.0 == *l))
        {
            s.license = l.to_string();
        }
        if let Some(e) = d["editable"].as_bool() {
            s.editable = e;
        }
        s
    }

    /// What's kept as the next share's defaults.
    fn defaults(&self) -> Value {
        json!({ "access": self.access.key(), "days": self.days, "license": self.license, "editable": self.editable })
    }
}

/// A design shared from this computer.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    /// The design's id in ZigZag (`chats_id`), what it's remembered by.
    pub design: String,
    /// Its file on the server.
    pub asset: String,
    pub settings: Settings,
    /// The page's address (None while private).
    pub url: Option<String>,
    /// When a link stops working (ms since 1970), for a link.
    pub expires: Option<u64>,
    /// A fingerprint of the design as uploaded: has it changed since?
    pub uploaded: u64,
    /// The server took it down after reports.
    pub hidden: bool,
    /// Shared from an account (its email), not this computer's guest:
    /// only that account can change it.
    pub by: Option<String>,
    /// The file it was shared from, once it has one: a copy of that
    /// file (Save As) is another design, with a page of its own.
    pub file: Option<String>,
}

/// `shares.json`: the guest's secret and what it shared.
#[derive(Default, Debug)]
pub struct Store {
    path: Option<PathBuf>,
    pub guest: Option<String>,
    pub shares: Vec<Record>,
    /// Shares in the file that this version can't read (written by a
    /// newer one, say): kept as they are, and written back.
    kept: Vec<Value>,
}

impl Store {
    pub fn in_config() -> Store {
        // (Tests never touch this computer's shares.)
        if cfg!(test) {
            return Store::default();
        }
        match crate::dock::config_file("shares.json") {
            Some(p) => Store::at(p),
            None => Store::default(),
        }
    }

    pub fn at(path: PathBuf) -> Store {
        let text = std::fs::read_to_string(&path).ok();
        let v: Value = text
            .as_deref()
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        // A file that can't be read as one is kept aside, not written
        // over: it has the guest's secret, the only way to its shares.
        if v.is_null() && text.is_some_and(|t| !t.trim().is_empty()) {
            let _ = std::fs::rename(&path, path.with_extension("json.damaged"));
        }
        let s = |r: &Value, k: &str| r[k].as_str().map(String::from);
        let mut kept = Vec::new();
        let shares = v["shares"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| {
                let read = (|| Some(Record {
                    design: s(r, "design")?,
                    asset: s(r, "asset")?,
                    settings: Settings {
                        name: s(r, "name").unwrap_or_default(),
                        access: Access::from_key(r["access"].as_str()?)?,
                        days: r["days"]
                            .as_u64()
                            .map_or(FOREVER, |d| d.min(FOREVER as u64) as u32),
                        license: s(r, "license").unwrap_or_else(|| LICENSES[0].0.into()),
                        editable: r["editable"].as_bool().unwrap_or(true),
                    },
                    url: s(r, "url"),
                    expires: r["expires"].as_u64(),
                    uploaded: r["uploaded"].as_u64().unwrap_or(0),
                    hidden: r["hidden"].as_bool().unwrap_or(false),
                    by: s(r, "by"),
                    file: s(r, "file"),
                }))();
                if read.is_none() {
                    kept.push(r.clone());
                }
                read
            })
            .collect();
        Store {
            guest: s(&v, "guest"),
            shares,
            kept,
            path: Some(path),
        }
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        let mut shares: Vec<Value> = self
            .shares
            .iter()
            .map(|r| {
                json!({
                    "design": r.design, "asset": r.asset, "name": r.settings.name,
                    "access": r.settings.access.key(), "days": r.settings.days,
                    "license": r.settings.license, "editable": r.settings.editable,
                    "url": r.url, "expires": r.expires, "uploaded": r.uploaded,
                    "hidden": r.hidden, "by": r.by, "file": r.file,
                })
            })
            .collect();
        shares.extend(self.kept.iter().cloned());
        let v = json!({ "guest": self.guest, "shares": shares });
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Only this user may read it: the secret is the guest.
        let _ = crate::session::write_private(
            path,
            &serde_json::to_string_pretty(&v).unwrap_or_default(),
        );
    }

    pub fn find(&self, design: &str) -> Option<&Record> {
        self.shares.iter().find(|r| r.design == design)
    }

    fn put(&mut self, r: Record) {
        self.shares.retain(|o| o.design != r.design);
        self.shares.push(r);
    }
}

/// A fingerprint of the design's text (FNV-1a): enough to tell it changed.
pub fn fingerprint(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

// ── Talking to the server ──────────────────────────────────────────────────

/// What a call to the server said when it refused (or never answered).
#[derive(Debug)]
pub struct Refused {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl Refused {
    pub(crate) fn say(&self) -> String {
        if self.status == 0 {
            format!("Couldn't reach zigzag.diy ({}).", self.message)
        } else {
            self.message.clone()
        }
    }
}

pub struct Api {
    pub base: String,
    token: Option<String>,
}

impl Api {
    pub fn new(base: &str) -> Api {
        Api {
            base: base.trim_end_matches('/').into(),
            token: None,
        }
    }

    /// One call: JSON back, or why not. The session token and the address
    /// go to curl on its standard input (`--config -`), not its command
    /// line, where other users' programs could read them.
    pub(crate) fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<(&str, &[u8])>,
        headers: &[(&str, &str)],
    ) -> Result<Value, Refused> {
        let gone = |m: String| Refused {
            status: 0,
            code: "network".into(),
            message: m,
        };
        let mut config = String::new();
        // (As curl reads a quoted value: a line break in one would start
        // another option.)
        let quote = |s: &str| {
            s.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t")
        };
        config.push_str(&format!(
            "url = \"{}\"\n",
            quote(&format!("{}{}", self.base, path))
        ));
        if let Some(t) = &self.token {
            config.push_str(&format!(
                "header = \"Authorization: Bearer {}\"\n",
                quote(t)
            ));
        }
        for (k, v) in headers {
            config.push_str(&format!("header = \"{}: {}\"\n", quote(k), quote(v)));
        }
        // The body from a file only this user can read (stdin has the config).
        let body_file = match body {
            Some((ctype, bytes)) => {
                config.push_str(&format!("header = \"Content-Type: {}\"\n", quote(ctype)));
                let f = temp_file();
                write_private_bytes(&f, bytes).map_err(|e| gone(e.to_string()))?;
                config.push_str(&format!(
                    "data-binary = \"@{}\"\n",
                    quote(&f.to_string_lossy())
                ));
                Some(f)
            }
            None => None,
        };
        // Two minutes, and longer for a big upload on a slow line (as if
        // at 20 kB a second).
        let max_time = (120 + body.map_or(0, |b| b.1.len()) / 20_000).to_string();
        let mut cmd = Command::new("curl");
        cmd.args(["--silent", "--show-error", "--request", method])
            .args([
                "--proto",
                "=https,http",
                "--connect-timeout",
                "20",
                "--max-time",
                &max_time,
                // (An answer is a little JSON: never more than this.)
                "--max-filesize",
                "16000000",
            ])
            .args([
                "--user-agent",
                concat!("ZigZag/", env!("CARGO_PKG_VERSION")),
            ])
            .args(["--write-out", "\n%{http_code}", "--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let result = (|| {
            let mut child = cmd.spawn().map_err(|e| gone(format!("curl: {e}")))?;
            {
                use std::io::Write;
                let mut stdin = child.stdin.take().ok_or_else(|| gone("curl".into()))?;
                stdin
                    .write_all(config.as_bytes())
                    .map_err(|e| gone(e.to_string()))?;
            }
            let out = child.wait_with_output().map_err(|e| gone(e.to_string()))?;
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            let (reply, code) = text.rsplit_once('\n').unwrap_or(("", text.as_str()));
            let status: u16 = code.trim().parse().unwrap_or(0);
            if status == 0 {
                let why = String::from_utf8_lossy(&out.stderr)
                    .trim()
                    .trim_start_matches("curl: ")
                    .to_string();
                return Err(gone(if why.is_empty() {
                    "no answer".into()
                } else {
                    why
                }));
            }
            let v: Value = serde_json::from_str(reply).unwrap_or(Value::Null);
            if (200..300).contains(&status) {
                Ok(v)
            } else {
                Err(Refused {
                    status,
                    code: v["error"]["code"].as_str().unwrap_or("").into(),
                    message: v["error"]["message"]
                        .as_str()
                        .map(String::from)
                        .unwrap_or_else(|| format!("zigzag.diy answered {status}.")),
                })
            }
        })();
        if let Some(f) = body_file {
            let _ = std::fs::remove_file(f);
        }
        result
    }

    pub(crate) fn json(&self, method: &str, path: &str, v: &Value) -> Result<Value, Refused> {
        self.call(
            method,
            path,
            Some(("application/json", v.to_string().as_bytes())),
            &[],
        )
    }

    /// The session it has, to use again; and one from before, to use.
    pub(crate) fn session(&self) -> Option<String> {
        self.token.clone()
    }
    pub(crate) fn set_session(&mut self, token: Option<String>) {
        self.token = token;
    }

    /// A session: for this computer's account (its device token), for the
    /// guest whose secret this is, or for a new guest (then its secret is
    /// handed to `keep` before anything else happens).
    pub(crate) fn sign_in(&mut self, cred: &Cred, keep: &dyn Fn(String)) -> Result<(), Refused> {
        let v = match cred {
            Cred::Device(device) => {
                self.json("POST", "/v1/session", &json!({ "device": device }))?
            }
            Cred::Guest(Some(secret)) => {
                self.json("POST", "/v1/session", &json!({ "guest": secret }))?
            }
            Cred::Guest(None) => {
                let v = self.json("POST", "/v1/guest", &json!({}))?;
                if let Some(s) = v["secret"].as_str() {
                    keep(s.to_string());
                }
                v
            }
        };
        self.token = v["token"].as_str().map(String::from);
        if self.token.is_none() {
            return Err(Refused {
                status: 500,
                code: "session".into(),
                message: "zigzag.diy didn't sign this computer in.".into(),
            });
        }
        Ok(())
    }
}

/// Who the app is to the server: this computer's account, or its guest.
#[derive(Clone, Debug)]
pub enum Cred {
    /// Signed in (Settings › Account): the device token that renews it.
    Device(String),
    /// Not signed in: the guest's secret, or none yet (one is made).
    Guest(Option<String>),
}

fn temp_file() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "zigzag-share-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

fn write_private_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)?.write_all(bytes)
}

/// What an upload is made from: the design as saved and, when it has
/// stitches, its plan, the cloth's color and what it's exported with.
struct UploadParts {
    svg: String,
    stitched: Option<(
        zigzag::design::Design,
        [u8; 3],
        zigzag::formats::ExportOptions,
    )>,
}

impl UploadParts {
    /// The upload: the picture drawn and the plan written (slow for a
    /// big design, so on the job's thread).
    fn make(self) -> Upload {
        let (picture, plan) = match &self.stitched {
            Some((design, cloth, export)) => {
                let opt = zigzag::formats::preview::PreviewOptions {
                    hoop_mm: None,
                    background: *cloth,
                    show_jumps: false,
                    ..Default::default()
                };
                (
                    zigzag::formats::preview::render_png(design, &opt, 1200).ok(),
                    Some(zigzag::formats::plan::write(design, export)),
                )
            }
            None => (None, None),
        };
        Upload {
            svg: self.svg,
            picture,
            plan,
        }
    }
}

/// What's uploaded: the design and what's made from it.
pub struct Upload {
    /// The design as ZigZag saves it.
    pub svg: String,
    /// Its picture (PNG), when it has stitches.
    pub picture: Option<Vec<u8>>,
    /// Its stitch plan (`formats::plan`), when it has stitches.
    pub plan: Option<Vec<u8>>,
}

/// What a job asks of the server.
pub enum Job {
    /// Share the design (again): upload it when it's new or changed, and
    /// make the server's settings these.
    Share {
        design: String,
        settings: Settings,
        upload: Option<Upload>,
        fingerprint: u64,
        /// Shared before: its record.
        before: Option<Record>,
    },
    /// Take it off the server altogether.
    Delete(Record),
}

/// What a job sends back.
pub enum Msg {
    /// A new guest's secret: keep it now, whatever happens next.
    Guest(String),
    Shared(Record, Note),
    Deleted(String),
    Failed(String),
}

/// Something to say with a finished share.
#[derive(Debug, PartialEq)]
pub enum Note {
    None,
    /// The old file wasn't on the server any more: this is a new page.
    NewPage,
}

/// Run a job against the server at `base`, as `cred`.
pub fn run(base: &str, cred: Cred, job: Job, send: &dyn Fn(Msg)) {
    let mut api = Api::new(base);
    let keep = |s: String| send(Msg::Guest(s));
    if let Err(e) = api.sign_in(&cred, &keep) {
        let why = if e.code == "device" {
            "You've been signed out. Sign in again in Settings › Account to change your shares."
                .to_string()
        } else if e.code == "guest" {
            "This computer's shares aren't known to zigzag.diy any more, so they can't be changed. Delete shares.json in ZigZag's settings folder to share as someone new.".to_string()
        } else {
            e.say()
        };
        return send(Msg::Failed(why));
    }
    match job {
        Job::Share {
            design,
            settings,
            upload,
            fingerprint,
            before,
        } => {
            match share(
                &api,
                &design,
                &settings,
                upload.as_ref(),
                fingerprint,
                before.as_ref(),
            ) {
                Ok(out) => send(Msg::Shared(out.0, out.1)),
                Err(e) => send(Msg::Failed(e.say())),
            }
        }
        Job::Delete(r) => match api.call("DELETE", &format!("/v1/assets/{}", r.asset), None, &[]) {
            Ok(_) => send(Msg::Deleted(r.design)),
            Err(e) if e.status == 404 => send(Msg::Deleted(r.design)),
            Err(e) => send(Msg::Failed(e.say())),
        },
    }
}

fn share(
    api: &Api,
    design: &str,
    s: &Settings,
    upload: Option<&Upload>,
    fingerprint: u64,
    before: Option<&Record>,
) -> Result<(Record, Note), Refused> {
    let mut made = None;
    let out = share_to(api, design, s, upload, fingerprint, before, &mut made);
    // A file made for it and then not finished (an upload failed) isn't
    // left on the server, where nothing here would remember it.
    if out.is_err()
        && let Some(id) = made
    {
        let _ = api.call("DELETE", &format!("/v1/assets/{id}"), None, &[]);
    }
    out
}

fn share_to(
    api: &Api,
    design: &str,
    s: &Settings,
    upload: Option<&Upload>,
    fingerprint: u64,
    before: Option<&Record>,
    made: &mut Option<String>,
) -> Result<(Record, Note), Refused> {
    let name = if s.name.trim().is_empty() {
        "Design"
    } else {
        s.name.trim()
    };
    let mut note = Note::None;
    // The file: the one shared before, if it's still there.
    let mut asset = before.map(|b| b.asset.clone());
    let mut fresh = false;
    if let Some(id) = &asset
        && let Err(e) = api.call("GET", &format!("/v1/assets/{id}/versions"), None, &[])
    {
        if e.status != 404 {
            return Err(e);
        }
        asset = None;
        note = Note::NewPage;
    }
    let id = match asset {
        Some(id) => id,
        None => {
            fresh = true;
            let v = api.json(
                "POST",
                "/v1/assets",
                &json!({ "name": name, "format": "svg" }),
            )?;
            let id = v["id"].as_str().map(String::from).ok_or_else(|| Refused {
                status: 500,
                code: "id".into(),
                message: "zigzag.diy didn't make a file for it.".into(),
            })?;
            *made = Some(id.clone());
            id
        }
    };
    let changed = fresh || before.is_none_or(|b| b.uploaded != fingerprint);
    if changed {
        let Some(u) = upload else {
            return Err(Refused {
                status: 0,
                code: "upload".into(),
                message: "nothing to upload".into(),
            });
        };
        let v = api.call(
            "PUT",
            &format!("/v1/assets/{id}"),
            Some(("image/svg+xml", u.svg.as_bytes())),
            &[],
        )?;
        let etag = v["etag"].as_str().unwrap_or("").to_string();
        if let Some(png) = &u.picture {
            api.call(
                "PUT",
                &format!("/v1/assets/{id}/preview"),
                Some(("image/png", png)),
                &[],
            )?;
        }
        if let Some(plan) = &u.plan {
            api.call(
                "PUT",
                &format!("/v1/assets/{id}/plan"),
                Some(("application/octet-stream", plan)),
                &[("X-Design-Etag", &etag)],
            )?;
        }
    }
    // A link is a signed link to the page, so the file itself is private.
    let access = if s.access == Access::Public {
        "public"
    } else {
        "private"
    };
    let v = api.json(
        "PATCH",
        &format!("/v1/assets/{id}"),
        &json!({ "name": name, "access": access, "license": s.license, "editable": s.editable }),
    )?;
    let hidden = v["hidden"].as_bool().unwrap_or(false);
    let (url, expires) = match s.access {
        Access::Public => (v["shareUrl"].as_str().map(String::from), None),
        Access::Link => {
            // The link already sent keeps working, unless it's run out or
            // it's to last a different time.
            let now = now_ms();
            match before.filter(|b| {
                !fresh
                    && b.settings.access == Access::Link
                    && b.settings.days == s.days
                    && b.url.is_some()
                    && b.expires.is_some_and(|e| e > now + 60_000)
            }) {
                Some(b) => (b.url.clone(), b.expires),
                None => {
                    let v = api.json(
                        "POST",
                        &format!("/v1/assets/{id}/signed"),
                        &json!({ "page": true, "expiresIn": link_seconds(s.days) }),
                    )?;
                    (v["url"].as_str().map(String::from), v["expires"].as_u64())
                }
            }
        }
        Access::Private => {
            api.call("POST", &format!("/v1/assets/{id}/signed/revoke"), None, &[])?;
            (None, None)
        }
    };
    let mut settings = s.clone();
    settings.name = name.to_string();
    Ok((
        Record {
            design: design.to_string(),
            asset: id,
            settings,
            url,
            expires,
            uploaded: fingerprint,
            hidden,
            by: None,
            file: None,
        },
        note,
    ))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// "Expires in 6 days": how long a link has left.
fn time_left(expires: u64, now: u64) -> String {
    // (A link made to last "forever" runs a hundred years.)
    if expires > now + 50 * 365 * 86_400_000 {
        return "Never expires".into();
    }
    if expires <= now {
        return "Expired".into();
    }
    let h = (expires - now) / 3_600_000;
    if h >= 48 {
        format!("Expires in {} days", h / 24)
    } else if h >= 1 {
        format!("Expires in {h} hour{}", if h == 1 { "" } else { "s" })
    } else {
        "Expires within the hour".into()
    }
}

// ── The panel ──────────────────────────────────────────────────────────────

/// The panel's two tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Share,
    Export,
}

/// The Share and Export panel (under the toolbar's button), and what
/// it's doing.
#[derive(Default)]
pub struct ShareUi {
    /// The panel is up, for the design with this id.
    pub open: Option<String>,
    pub tab: Tab,
    /// The settings as chosen: what a share uses.
    pub settings: Option<Settings>,
    /// The design's fingerprint when last looked at (the panel opening, a
    /// share starting or ending).
    pub now: u64,
    pub store: Option<Store>,
    /// Working: the server's answers come here.
    pub busy: Option<mpsc::Receiver<Msg>>,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// Delete was chosen: asking to be sure.
    pub confirm_delete: bool,
    /// Copy the link once the share under way is done.
    pub copy_when_done: bool,
    /// The name, being edited (the pencil).
    pub renaming: Option<String>,
    /// The server, when not `server()` (tests).
    pub base: Option<String>,
    /// Make a new link even if the old one still works.
    pub fresh_link: bool,
    /// Opened from File › Share…: sharing only (no Export tab), in the
    /// middle of the window.
    pub from_menu: bool,
    /// For a screenshot: open this setting's menu ("who", "license",
    /// "downloads"; see `capture`).
    pub peek: Option<&'static str>,
    /// The share in front was made from this account, not signed in here.
    pub locked: Option<String>,
    /// The toolbar button the panel hangs from.
    pub anchor: Option<egui::Rect>,
    /// The frame it opened in (that click doesn't close it again).
    opened: u64,
    /// The account the job under way runs as (its email; None: this
    /// computer's guest): whose the share it makes is.
    busy_as: Option<String>,
    /// The file the design was in when the job under way began.
    busy_file: Option<String>,
}

impl ShareUi {
    pub fn store(&mut self) -> &mut Store {
        self.store.get_or_insert_with(Store::in_config)
    }
}

/// One option of a menu: its name, and under it (in the same row, so
/// the grey isn't taken for another option) what it means; a check by
/// the one that's chosen.
pub(crate) fn choice(ui: &mut egui::Ui, on: bool, title: &str, what: &str) -> bool {
    choice_w(ui, on, title, what, MENU_W)
}

/// [`choice`], in a narrow menu.
fn choice_narrow(ui: &mut egui::Ui, on: bool, title: &str, what: &str) -> bool {
    choice_w(ui, on, title, what, 200.0)
}

fn choice_w(ui: &mut egui::Ui, on: bool, title: &str, what: &str, w: f32) -> bool {
    let p = crate::theme::pal_ui(ui);
    let text_x = 30.0;
    let title_g =
        ui.painter()
            .layout_no_wrap(title.to_string(), egui::FontId::proportional(14.0), p.text);
    let what_g = (!what.is_empty()).then(|| {
        ui.painter().layout(
            what.to_string(),
            egui::FontId::proportional(12.0),
            p.secondary,
            w - text_x - 10.0,
        )
    });
    let h = 8.0 + title_g.size().y + what_g.as_ref().map_or(0.0, |g| 2.0 + g.size().y) + 8.0;
    let (rect, r) = ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::click());
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), on, title));
    if r.hovered() {
        ui.painter().rect_filled(rect, 6.0, p.hover);
    }
    if on {
        let at = egui::Rect::from_center_size(
            egui::pos2(
                rect.left() + 15.0,
                rect.top() + 8.0 + title_g.size().y / 2.0,
            ),
            Vec2::splat(13.0),
        );
        crate::icons::paint(ui.painter(), at, Icon::Check, p.accent);
    }
    let mut y = rect.top() + 8.0;
    let th = title_g.size().y;
    ui.painter()
        .galley(egui::pos2(rect.left() + text_x, y), title_g, p.text);
    y += th + 2.0;
    if let Some(g) = what_g {
        ui.painter()
            .galley(egui::pos2(rect.left() + text_x, y), g, p.secondary);
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// How wide the share settings' menus are.
const MENU_W: f32 = 300.0;

/// A line between a menu's options.
pub(crate) fn divider(ui: &mut egui::Ui) {
    ui.add_space(2.0);
    ui.separator();
    ui.add_space(2.0);
}

/// A menu of options, with room around them.
pub(crate) fn options_menu(ui: &mut egui::Ui) {
    ui.set_width(MENU_W);
    ui.spacing_mut().item_spacing.y = 2.0;
}

/// One row of the Export list: the format in a chip (all chips one
/// width, so the words line up), then what it's for (`dim`: the brands).
pub(crate) fn format_row(
    ui: &mut egui::Ui,
    format: &str,
    text: &str,
    dim: bool,
    enabled: bool,
) -> egui::Response {
    let p = crate::theme::pal_ui(ui);
    let w = ui.available_width();
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, r) = ui.allocate_exact_size(Vec2::new(w, 32.0), sense);
    let label = format!("{format} {text}");
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &label));
    if enabled && r.hovered() {
        ui.painter().rect_filled(rect, 6.0, p.hover);
    }
    let fade = |c: egui::Color32| if enabled { c } else { c.gamma_multiply(0.5) };
    let chip = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 8.0, rect.center().y - 10.0),
        Vec2::new(46.0, 20.0),
    );
    let chip_fill = if p.dark {
        egui::Color32::from_gray(64)
    } else {
        egui::Color32::from_gray(222)
    };
    ui.painter().rect_filled(chip, 4.0, fade(chip_fill));
    ui.painter().text(
        chip.center(),
        egui::Align2::CENTER_CENTER,
        format,
        egui::FontId::monospace(11.5),
        fade(p.text),
    );
    let g = ui.painter().layout(
        text.to_string(),
        egui::FontId::proportional(13.5),
        fade(if dim { p.secondary } else { p.text }),
        rect.right() - chip.right() - 20.0,
    );
    let at = egui::pos2(chip.right() + 12.0, rect.center().y - g.size().y / 2.0);
    ui.painter().galley(at, g, p.text);
    if enabled {
        r.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        r
    }
}

/// The panel's blue button: an icon, then its words.
fn accent_button(ui: &mut egui::Ui, icon: Icon, text: &str, enabled: bool) -> egui::Response {
    let p = crate::theme::pal_ui(ui);
    let font = egui::FontId::proportional(14.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, egui::Color32::WHITE);
    let (pad, icon_w, gap) = (12.0, 14.0, 6.0);
    let size = Vec2::new(pad * 2.0 + icon_w + gap + galley.size().x, 28.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, r) = ui.allocate_exact_size(size, sense);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, text));
    let fill = if !enabled {
        p.accent.gamma_multiply(0.45)
    } else if r.is_pointer_button_down_on() {
        p.accent.gamma_multiply(0.8)
    } else if r.hovered() {
        p.accent.gamma_multiply(0.9)
    } else {
        p.accent
    };
    ui.painter().rect_filled(rect, 6.0, fill);
    let at = egui::Rect::from_center_size(
        egui::pos2(rect.left() + pad + icon_w / 2.0, rect.center().y),
        Vec2::splat(icon_w),
    );
    crate::icons::paint(ui.painter(), at, icon, egui::Color32::WHITE);
    let text_at = egui::pos2(at.right() + gap, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(text_at, galley, egui::Color32::WHITE);
    if enabled {
        r.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        r
    }
}

/// Open a value's menu for a screenshot (see `ShareUi::peek`).
fn peek(ui: &egui::Ui, r: &egui::Response, open: bool) {
    let id = egui::Popup::default_response_id(r);
    if open && !egui::Popup::is_id_open(ui.ctx(), id) {
        egui::Popup::open_id(ui.ctx(), id);
    }
}

/// Underlined text to click (Update, New link).
fn underlined(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let p = crate::theme::pal_ui(ui);
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(12.0)
                .color(p.text)
                .underline(),
        )
        .sense(egui::Sense::click()),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A setting's value: underlined, with a chevron (a menu) or a pencil
/// (rename) beside it, so it reads as something to click. `what` names
/// it for screen readers ("License: CC BY 4.0").
fn value_link(ui: &mut egui::Ui, text: &str, icon: Icon, what: &str) -> egui::Response {
    let p = crate::theme::pal_ui(ui);
    let row = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(
                egui::RichText::new(text)
                    .size(13.0)
                    .color(p.text)
                    .underline(),
            );
            let (r, _) = ui.allocate_exact_size(Vec2::splat(13.0), egui::Sense::hover());
            crate::icons::paint(ui.painter(), r, icon, p.secondary);
        })
        .response;
    let r = ui
        .interact(row.rect, ui.scope_id().with(what), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let enabled = ui.is_enabled();
    r.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, enabled, format!("{what}: {text}"))
    });
    r
}

impl Studio {
    /// The toolbar's Share and Export button: the panel, or away with it.
    pub(crate) fn toggle_share_panel(&mut self) {
        if self.share.open.is_some() && !self.share.from_menu {
            self.share.open = None;
        } else {
            self.open_share();
            self.share.from_menu = false;
        }
    }

    /// What the design in front's share is remembered by: its id
    /// (`chats_id`), which a copy of its file keeps (Save As, or a copy
    /// made in the Finder: its chats come along). So a share also
    /// remembers its file, and a design whose id is shared from another
    /// file that's still there is a copy, with a share (and a page) of
    /// its own, under the id and its own file. A file that was moved or
    /// renamed (the one remembered is gone) is still the same design.
    pub(crate) fn share_id(&mut self) -> String {
        let id = self.chats_id();
        let path = self.doc.path.as_ref().map(|p| p.display().to_string());
        let copies = format!("{id}@");
        let store = self.share.store();
        let same = |r: &Record| r.design == id || r.design.starts_with(&copies);
        // This file's own.
        if let Some(r) = store.shares.iter().find(|r| same(r) && r.file == path) {
            return r.design.clone();
        }
        // One whose file isn't known yet (shared before it was saved, or
        // by an older ZigZag) or isn't there any more: this is it, here now.
        if let Some(r) = store.shares.iter_mut().find(|r| {
            same(r)
                && r.file
                    .as_ref()
                    .is_none_or(|f| !std::path::Path::new(f).exists())
        }) {
            let design = r.design.clone();
            if path.is_some() {
                r.file = path;
                store.save();
            }
            return design;
        }
        if !store.shares.iter().any(same) {
            return id;
        }
        // A copy of a shared design.
        match path {
            Some(p) => format!("{id}@{:x}", fingerprint(&p)),
            None => format!("{id}@unsaved"),
        }
    }

    /// File › Share…: the panel, on its Share tab.
    pub(crate) fn open_share(&mut self) {
        let id = self.share_id();
        let now = self.doc.to_svg().map(|t| fingerprint(&t)).unwrap_or(0);
        let before = self.share.store().find(&id).cloned();
        self.share.settings = Some(match &before {
            Some(r) => r.settings.clone(),
            None => Settings::with_defaults(&self.doc.name, self.prefs.share_defaults.as_ref()),
        });
        self.share.now = now;
        self.share.tab = Tab::Share;
        // (The toolbar's button says otherwise straight after.)
        self.share.from_menu = true;
        self.share.error = None;
        self.share.notice = None;
        self.share.confirm_delete = false;
        self.share.renaming = None;
        self.share.opened = self
            .egui_ctx
            .as_ref()
            .map_or(0, |c| c.cumulative_frame_nr());
        self.share.open = Some(id);
    }

    /// What's uploaded for the design in front. (All of it at once:
    /// a share makes the picture and the plan off the UI thread, see
    /// `apply_share`.)
    #[cfg(test)]
    fn share_upload(&mut self) -> Result<Upload, String> {
        self.plan_now();
        let svg = self
            .doc
            .to_svg()
            .map_err(|e| format!("Couldn't write the design: {e:#}"))?;
        Ok(self.upload_parts(svg).make())
    }

    /// The design in front as saved (`svg`), with what its picture and
    /// plan are made from.
    fn upload_parts(&self, svg: String) -> UploadParts {
        UploadParts {
            svg,
            stitched: (!self.design.blocks.is_empty()).then(|| {
                (
                    self.design.clone(),
                    self.doc.settings.cloth,
                    self.doc.settings.export_options(),
                )
            }),
        }
    }

    /// Start a job on its own thread; `make` puts it together there
    /// (what takes a while, like drawing the picture).
    fn start_share_job(&mut self, make: impl FnOnce() -> Job + Send + 'static) {
        // Signed in: as the account; else as this computer's guest.
        let (cred, who) = match self.account.signed_in() {
            Some(a) => (Cred::Device(a.device), Some(a.email)),
            None => (Cred::Guest(self.share.store().guest.clone()), None),
        };
        let (tx, rx) = mpsc::channel();
        let ctx = self.egui_ctx.clone();
        let base = self.share.base.clone().unwrap_or_else(server);
        let _ = std::thread::Builder::new()
            .name("zigzag-share".into())
            .spawn(move || {
                run(&base, cred, make(), &|m| {
                    let _ = tx.send(m);
                    if let Some(c) = &ctx {
                        c.request_repaint();
                    }
                });
            });
        self.share.busy = Some(rx);
        self.share.busy_as = who;
        self.share.busy_file = self.doc.path.as_ref().map(|p| p.display().to_string());
        self.share.error = None;
        self.share.notice = None;
    }

    /// Share the design with settings `s` now (the panel's choices take
    /// effect as they're made), uploading it if it's new or changed.
    fn apply_share(&mut self, design: &str, s: &Settings, before: Option<&Record>) {
        // The design as it is now (not as it was when the panel opened):
        // what's compared, uploaded and remembered are the same thing.
        self.plan_now();
        let svg = match self.doc.to_svg() {
            Ok(svg) => svg,
            Err(e) => {
                self.share.error = Some(format!("Couldn't write the design: {e:#}"));
                self.share_came_to_nothing(design);
                return;
            }
        };
        let now = fingerprint(&svg);
        self.share.now = now;
        let unchanged = before.is_some_and(|b| b.uploaded == now);
        let parts = (!unchanged).then(|| self.upload_parts(svg));
        let (design, settings, before) = (design.to_string(), s.clone(), before.cloned());
        self.start_share_job(move || Job::Share {
            design,
            settings,
            upload: parts.map(UploadParts::make),
            fingerprint: now,
            before,
        });
    }

    /// A share didn't happen: the panel goes back to how the design is
    /// shared (so a choice that failed can be made again, and doesn't go
    /// along with the next), and nothing waits to be copied.
    fn share_came_to_nothing(&mut self, design: &str) {
        self.share.copy_when_done = false;
        if self.share.open.as_deref() == Some(design)
            && let Some(r) = self.share.store().find(design).cloned()
        {
            self.share.settings = Some(r.settings);
        }
    }

    /// The server's answers to a job under way.
    fn take_share_news(&mut self) {
        let Some(rx) = &self.share.busy else { return };
        let msgs: Vec<Msg> = rx.try_iter().collect();
        let done = msgs.iter().any(|m| !matches!(m, Msg::Guest(_)));
        let disconnected =
            msgs.is_empty() && matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected));
        let mut failed = false;
        for m in msgs {
            match m {
                Msg::Guest(secret) => {
                    let s = self.share.store();
                    s.guest = Some(secret);
                    s.save();
                }
                Msg::Shared(r, note) => {
                    let first = self.share.store().find(&r.design).is_none();
                    let copy = std::mem::take(&mut self.share.copy_when_done);
                    if copy && let (Some(url), Some(ctx)) = (&r.url, &self.egui_ctx) {
                        ctx.copy_text(url.clone());
                    }
                    self.share.notice = match note {
                        Note::NewPage => {
                            Some("Its old page was gone, so this is a new one.".into())
                        }
                        Note::None if first && copy && r.url.is_some() => {
                            Some("Shared, and the link is copied.".into())
                        }
                        Note::None => None,
                    };
                    self.status = match &r.url {
                        Some(u) => format!("Shared {} at {u}", r.settings.name),
                        None => format!("{} isn't shared now.", r.settings.name),
                    };
                    // (The panel may be another design's by now.)
                    if self.share.open.as_deref() == Some(r.design.as_str()) {
                        self.share.now = r.uploaded;
                        self.share.settings = Some(r.settings.clone());
                    }
                    let mut r = r;
                    // Whose it is: who the job ran as (not who's signed
                    // in now, which may have changed meanwhile).
                    r.by = self.share.busy_as.clone();
                    // And the file it's of (as known before, if it has
                    // none yet).
                    r.file = self.share.busy_file.clone().or_else(|| {
                        self.share.store().find(&r.design).and_then(|b| b.file.clone())
                    });
                    // The next share starts the way this one went (not private).
                    if r.settings.access != Access::Private {
                        let d = Some(r.settings.defaults());
                        if self.prefs.share_defaults != d {
                            self.prefs.share_defaults = d;
                            self.prefs.save();
                        }
                    }
                    let s = self.share.store();
                    s.put(r);
                    s.save();
                }
                Msg::Deleted(design) => {
                    let s = self.share.store();
                    s.shares.retain(|r| r.design != design.as_str());
                    s.save();
                    self.status = "Deleted from zigzag.diy.".into();
                    let name = self.doc.name.clone();
                    if self.share.open.as_deref() == Some(design.as_str())
                        && let Some(st) = &mut self.share.settings
                    {
                        *st = Settings::new(&name);
                    }
                    self.share.notice = Some("Deleted from zigzag.diy.".into());
                }
                Msg::Failed(why) => {
                    self.share.error = Some(why);
                    failed = true;
                }
            }
        }
        if failed || disconnected {
            if let Some(design) = self.share.open.clone() {
                self.share_came_to_nothing(&design);
            }
            self.share.copy_when_done = false;
        }
        if done || disconnected {
            self.share.busy = None;
        }
    }

    /// The panel's foot. Not shared: Share, which approves the settings
    /// above. Shared: the link with Copy, where Share was, and under it an
    /// icon and what's true now, with Update after edits, a new link once
    /// one expires, or what went wrong.
    #[allow(clippy::too_many_arguments)]
    fn share_foot(
        &mut self,
        ui: &mut egui::Ui,
        before: Option<&Record>,
        changed: bool,
        expired: bool,
        s: &mut Settings,
        apply: &mut bool,
        delete: &mut bool,
        copy: &mut Option<String>,
    ) {
        let p = crate::theme::pal_ui(ui);
        let busy = self.share.busy.is_some();
        // The link, and a button to copy it (the same height).
        if !self.share.confirm_delete
            && let Some(u) = before
                .filter(|b| !b.hidden && !expired)
                .and_then(|b| b.url.clone())
        {
            ui.horizontal(|ui| {
                let w = ui.available_width() - 64.0;
                let mut shown = u.clone();
                ui.add_sized(
                    Vec2::new(w, 28.0),
                    egui::TextEdit::singleline(&mut shown)
                        .font(egui::FontId::proportional(13.0))
                        .align(egui::Align2::LEFT_CENTER),
                )
                .on_hover_text("The design's page");
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new("Copy").min_size(Vec2::new(56.0, 28.0)),
                    )
                    .clicked()
                {
                    *copy = Some(u.clone());
                }
            });
            ui.add_space(space::XS);
        }
        ui.horizontal(|ui| {
            ui.set_min_height(28.0);
            if self.share.confirm_delete {
                ui.label("Delete it from zigzag.diy?");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // (Not while a share is under way: the two would cross.)
                    if ui.add_enabled(!busy, egui::Button::new("Delete")).clicked() {
                        *delete = true;
                    }
                    if ui.button("Keep").clicked() {
                        self.share.confirm_delete = false;
                    }
                });
                return;
            }
            let Some(b) = before else {
                // Not shared: Get Share Link.
                if let Some(e) = &self.share.error {
                    ui.add(
                        egui::Label::new(egui::RichText::new(e).color(p.error).size(12.0)).wrap(),
                    );
                } else if busy {
                    ui.spinner();
                    crate::theme::caption(ui, "Sharing…");
                } else {
                    crate::theme::caption(ui, "Not shared yet.");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if accent_button(
                        ui,
                        Icon::Link,
                        "Get Share Link",
                        !busy && !self.doc.shapes.is_empty(),
                    )
                    .on_hover_text("Put it on zigzag.diy as set above, and copy its address")
                    .clicked()
                    {
                        self.share.copy_when_done = true;
                        *apply = true;
                    }
                });
                return;
            };
            if let Some(by) = &self.share.locked {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
                crate::icons::paint(ui.painter(), r, Icon::Lock, p.secondary);
                crate::theme::caption(
                    ui,
                    format!(
                        "Shared from {by}. Sign in as them in Settings › Account to change it."
                    ),
                );
                return;
            }
            // Shared: an icon and the state.
            let (icon, state) = match b.settings.access {
                _ if b.hidden => (
                    Icon::Warning,
                    "Taken down after reports: others can't see it".to_string(),
                ),
                Access::Public => (
                    Icon::Globe,
                    "Public: anyone can find and open it".to_string(),
                ),
                Access::Link if expired => (Icon::Link, "The link has expired".to_string()),
                Access::Link => (
                    Icon::Link,
                    format!(
                        "Only with the link · {}",
                        b.expires.map_or(String::new(), |e| time_left(e, now_ms()))
                    ),
                ),
                Access::Private => (
                    Icon::Lock,
                    "Private: its page and links are off".to_string(),
                ),
            };
            if busy {
                ui.spinner();
                crate::theme::caption(ui, "Saving…");
                return;
            }
            let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
            crate::icons::paint(
                ui.painter(),
                r,
                icon,
                if b.hidden { p.warning } else { p.secondary },
            );
            if let Some(e) = &self.share.error {
                ui.add(egui::Label::new(egui::RichText::new(e).color(p.error).size(12.0)).wrap());
                return;
            }
            if let Some(n) = &self.share.notice {
                crate::theme::caption(ui, n.clone());
            } else {
                crate::theme::caption(ui, state);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if changed && b.settings.access != Access::Private {
                    if underlined(ui, "Update")
                        .on_hover_text("Changed since it was shared: put this version up")
                        .clicked()
                    {
                        *apply = true;
                    }
                } else if expired && underlined(ui, "New link").clicked() {
                    s.access = Access::Link;
                    self.share.fresh_link = true;
                    *apply = true;
                }
            });
        });
    }

    pub fn share_window(&mut self, ctx: &egui::Context) {
        self.take_share_news();
        let Some(design) = self.share.open.clone() else {
            return;
        };
        // The panel is one design's: another in front (a tab changed, a
        // file opened) and it's gone, so nothing of this design goes to
        // that one's page.
        if design != self.share_id() {
            self.share.open = None;
            return;
        }
        let Some(mut s) = self.share.settings.take() else {
            return;
        };
        // (Escape ends a rename, not the panel as well.)
        let was_renaming = self.share.renaming.is_some();
        let before = self.share.store().find(&design).cloned();
        // Shared from an account that isn't signed in here: only it can change it.
        let me = self.account.signed_in().map(|a| a.email);
        let locked = before
            .as_ref()
            .and_then(|b| b.by.clone())
            .filter(|by| me.as_ref() != Some(by));
        self.share.locked = locked.clone();
        let busy = self.share.busy.is_some() || locked.is_some();
        let changed = before
            .as_ref()
            .is_some_and(|b| b.uploaded != self.share.now);
        // What's chosen here takes effect at once.
        let mut apply = false;
        let mut close = false;
        let mut copy: Option<String> = None;
        let mut delete = false;
        let anchor = self.share.anchor;
        let mut area = egui::Area::new(egui::Id::unique("share-panel"))
            .order(egui::Order::Foreground)
            .constrain(true);
        let menu = self.share.from_menu;
        area = match anchor {
            // From the File menu: in the middle of the window. (Its top is
            // fixed, half a shared panel's height above the middle, so it
            // doesn't jump when the link appears under it.)
            Some(_) if menu => {
                let c = ctx.content_rect().center();
                area.pivot(egui::Align2::CENTER_TOP)
                    .fixed_pos(egui::pos2(c.x, (c.y - 170.0).max(60.0)))
            }
            Some(r) => area
                .pivot(egui::Align2::RIGHT_TOP)
                .fixed_pos(egui::pos2(r.right() + 6.0, r.bottom() + 6.0)),
            None => area
                .pivot(egui::Align2::CENTER_TOP)
                .fixed_pos(egui::pos2(ctx.content_rect().center().x, 90.0)),
        };
        let shown = area.show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::same(16))
                .corner_radius(12.0)
                .show(ui, |ui| {
                    ui.set_width(380.0);
                    // Share or Export; More and close.
                    ui.horizontal(|ui| {
                        if menu {
                            ui.label(egui::RichText::new("Share").size(15.0));
                        } else {
                            crate::theme::segmented(
                                ui,
                                "share-tab",
                                &mut self.share.tab,
                                &[(Tab::Share, "Share"), (Tab::Export, "Export")],
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::theme::icon_button(ui, Icon::Close, "Close", false, 22.0).clicked() {
                                close = true;
                            }
                            if self.share.tab == Tab::Share {
                            let more = crate::theme::icon_button(ui, Icon::More, "More", false, 22.0);
                            egui::Popup::menu(&more).show(|ui| {
                                ui.set_max_width(260.0);
                                if ui
                                    .add_enabled(before.is_some() && !busy, egui::Button::new("Delete from zigzag.diy…"))
                                    .clicked()
                                {
                                    self.share.confirm_delete = true;
                                }
                                ui.separator();
                                crate::theme::caption(
                                    ui,
                                    match &me {
                                        Some(e) => format!("Shared from your account, {e}."),
                                        None => "Shared without an account, so it can be changed only from this computer. Sign in in Settings › Account to change it anywhere.".into(),
                                    },
                                );
                            });
                            }
                        });
                    });
                    ui.add_space(space::M);
                    if self.share.tab == Tab::Export {
                        if self.export_list(ui) {
                            close = true;
                        }
                        return;
                    }

                    let expired = before.as_ref().is_some_and(|b| {
                        b.settings.access == Access::Link && b.expires.is_none_or(|e| e <= now_ms() + 60_000)
                    });

                    // Everything else: labelled, the value underlined (a
                    // menu, or the pencil to rename). The defaults are good.
                    ui.add_space(space::S);
                    ui.add_enabled_ui(!busy, |ui| {
                        crate::theme::form(ui, "share-more", |fm| {
                            fm.row("Name", |ui, lid| {
                                if let Some(name) = &mut self.share.renaming {
                                    let r = ui.add(egui::TextEdit::singleline(name).desired_width(200.0)).labelled_by(lid);
                                    if !r.has_focus() && !r.lost_focus() {
                                        r.request_focus();
                                    }
                                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                        self.share.renaming = None;
                                    } else if r.lost_focus() {
                                        let n = name.trim().to_string();
                                        self.share.renaming = None;
                                        if !n.is_empty() && n != s.name {
                                            s.name = n;
                                            apply = before.is_some();
                                        }
                                    }
                                } else if value_link(ui, &s.name, Icon::Pencil, "Name").clicked() {
                                    self.share.renaming = Some(s.name.clone());
                                }
                            });
                            fm.row("Who can see it", |ui, _| {
                                // (Before it's shared, the choice waits for Share.)
                                let shared = before.is_some();
                                let now = before.as_ref().map(|b| b.settings.access).unwrap_or(s.access);
                                let label = match now {
                                    Access::Public => "Anyone",
                                    Access::Link => "Anyone with the link",
                                    Access::Private => "Only you",
                                };
                                let who = value_link(ui, label, Icon::ChevronDown, "Who can see it");
                                peek(ui, &who, self.share.peek == Some("who"));
                                // A link: how long, on the same line.
                                if s.access == Access::Link || now == Access::Link {
                                    let len = LINK_DAYS.iter().find(|l| l.0 == s.days).map_or("forever", |l| l.1);
                                    let how_long = value_link(ui, len, Icon::ChevronDown, "Link works");
                                    peek(ui, &how_long, self.share.peek == Some("length"));
                                    egui::Popup::menu(&how_long).show(|ui| {
                                        ui.set_width(200.0);
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        for (k, (d, len)) in LINK_DAYS.into_iter().enumerate() {
                                            let title = match len.strip_prefix("for ") {
                                                Some(t) => t.to_string(),
                                                None => "Forever".to_string(),
                                            };
                                            let what = if k == 0 { "Recommended" } else { "" };
                                            if choice_narrow(ui, s.days == d, &title, what) && s.days != d {
                                                s.days = d;
                                                s.access = Access::Link;
                                                // (A new link, for the new length.)
                                                self.share.fresh_link = shared;
                                                apply = shared;
                                            }
                                        }
                                    });
                                }
                                egui::Popup::menu(&who).show(|ui| {
                                    options_menu(ui);
                                    if choice(ui, now == Access::Public, "Anyone", "Anyone can find it, and search engines list it.") {
                                        s.access = Access::Public;
                                        apply = shared && now != Access::Public;
                                    }
                                    divider(ui);
                                    if choice(ui, now == Access::Link, "Anyone with the link", "Only people you send it to. Forever, unless you choose a time.") {
                                        s.access = Access::Link;
                                        apply = shared && now != Access::Link;
                                    }
                                    if shared {
                                        divider(ui);
                                        if choice(ui, now == Access::Private, "Only you", "Its page and links stop working.") {
                                            s.access = Access::Private;
                                            apply = now != Access::Private;
                                        }
                                    }
                                });
                            });
                            fm.row("License", |ui, _| {
                                let lic = value_link(ui, license_name(&s.license), Icon::ChevronDown, "License")
                                    .on_hover_text("What others may do with it");
                                peek(ui, &lic, self.share.peek == Some("license"));
                                egui::Popup::menu(&lic).show(|ui| {
                                    options_menu(ui);
                                    for (k, (id, name, what)) in LICENSES.into_iter().enumerate() {
                                        if k > 0 {
                                            divider(ui);
                                        }
                                        let what = if k == 0 { format!("Recommended. {what}") } else { what.to_string() };
                                        if choice(ui, s.license == id, name, &what) && s.license != id {
                                            s.license = id.to_string();
                                            apply = before.is_some();
                                        }
                                    }
                                });
                            });
                            fm.row("Downloads", |ui, _| {
                                let dl = value_link(
                                    ui,
                                    if s.editable { "Editable design and machine files" } else { "Machine files only" },
                                    Icon::ChevronDown,
                                    "Downloads",
                                )
                                .on_hover_text("What others can download");
                                peek(ui, &dl, self.share.peek == Some("downloads"));
                                egui::Popup::menu(&dl).show(|ui| {
                                    options_menu(ui);
                                    if choice(
                                        ui,
                                        s.editable,
                                        "Editable design and machine files",
                                        "Recommended. Others can open it in ZigZag and change it, as well as sew it.",
                                    ) && !s.editable
                                    {
                                        s.editable = true;
                                        apply = before.is_some();
                                    }
                                    divider(ui);
                                    if choice(ui, !s.editable, "Machine files only", "Others can sew it, not change it.")
                                        && s.editable
                                    {
                                        s.editable = false;
                                        apply = before.is_some();
                                    }
                                });
                            });
                        });
                    });

                    // The foot: Share, or how it's shared.
                    ui.add_space(space::S);
                    ui.separator();
                    ui.add_space(space::S);
                    self.share_foot(ui, before.as_ref(), changed, expired, &mut s, &mut apply, &mut delete, &mut copy);
                });
        });
        if let Some(u) = copy {
            ctx.copy_text(u);
            self.share.notice = Some("Link copied.".into());
            self.share.error = None;
        }
        if delete && !busy {
            self.share.confirm_delete = false;
            if let Some(b) = before.clone() {
                self.start_share_job(move || Job::Delete(b));
            }
        }
        if apply && !busy {
            // (Private before it's shared is nothing to do.)
            if before.is_some() || s.access != Access::Private {
                // A new link asked for: as if the old one had run out.
                let mut before = before.clone();
                if std::mem::take(&mut self.share.fresh_link)
                    && let Some(b) = &mut before
                {
                    b.expires = None;
                }
                self.apply_share(&design, &s, before.as_ref());
            }
        }
        self.share.settings = Some(s);
        // Away with a click elsewhere (not on the button that opened it,
        // nor in one of its own menus), or Escape.
        let outside = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_some_and(|at| {
                    !shown.response.rect.contains(at) && !anchor.is_some_and(|a| a.contains(at))
                })
        });
        let escape = !was_renaming
            && self.share.renaming.is_none()
            && ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let menus = egui::Popup::is_any_open(ctx);
        if close
            || escape && !menus
            || outside && !menus && ctx.cumulative_frame_nr() > self.share.opened + 1
        {
            self.share.open = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A pretend zigzag-server: the routes sharing uses, with what was
    /// asked of it written down.
    struct Fake {
        base: String,
        log: Arc<Mutex<Vec<String>>>,
        state: Arc<Mutex<Value>>,
    }

    fn fake() -> Fake {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!(
            "http://127.0.0.1:{}",
            server.server_addr().to_ip().unwrap().port()
        );
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let state = Arc::new(Mutex::new(json!({ "assets": {}, "settings_doc": {} })));
        let (l, st) = (log.clone(), state.clone());
        std::thread::spawn(move || {
            for mut r in server.incoming_requests() {
                let mut body = Vec::new();
                let _ = std::io::Read::read_to_end(r.as_reader(), &mut body);
                let auth = r
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .map(|h| h.value.to_string())
                    .unwrap_or_default();
                let method = r.method().to_string();
                let url = r.url().to_string();
                l.lock().unwrap().push(format!("{method} {url}"));
                let b: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let mut s = st.lock().unwrap();
                let parts: Vec<&str> = url.trim_start_matches('/').split('/').collect();
                let (status, reply) = match (method.as_str(), parts.as_slice()) {
                    ("POST", ["v1", "guest"]) => {
                        (201, json!({ "secret": "s".repeat(43), "token": "tok" }))
                    }
                    ("POST", ["v1", "session"]) if b["guest"] == "s".repeat(43) => {
                        (200, json!({ "token": "tok" }))
                    }
                    // Email sign-in (zigzag-server's email.ts): the code is 123456.
                    ("POST", ["v1", "session"]) if b["device"] == "device-token" => {
                        (200, json!({ "token": "tok" }))
                    }
                    ("POST", ["v1", "email", "start"]) => (200, json!({ "sent": true })),
                    ("POST", ["v1", "email", "verify"]) if b["code"] == "123456" => {
                        // The guest's files are the account's now.
                        let claimed = if b["guest"] == "s".repeat(43) {
                            s["claimed"] = json!(true);
                            s["assets"].as_object().unwrap().len()
                        } else {
                            0
                        };
                        (
                            200,
                            json!({
                                "user": { "email": b["email"] },
                                "device": "device-token",
                                "claimed": claimed,
                                "token": "tok",
                            }),
                        )
                    }
                    ("POST", ["v1", "email", "verify"]) => (
                        401,
                        json!({ "error": { "code": "code", "message": "That code isn't right." } }),
                    ),
                    // Screen names (zigzag-server's names.ts): "taken-name" is someone's.
                    ("GET", ["v1", "me"]) => {
                        let sn = s["screen_name"]
                            .as_str()
                            .unwrap_or("dreamy-applique-575")
                            .to_string();
                        (
                            200,
                            json!({ "user": { "screenName": sn, "name": s["display_name"].clone() } }),
                        )
                    }
                    ("GET", ["v1", "screen-names", n]) => {
                        let n = n.to_string();
                        let valid = n.len() >= 3;
                        let available = valid && n != "taken-name";
                        let reason = if !valid {
                            Some("Too short")
                        } else if !available {
                            Some("Taken")
                        } else {
                            None
                        };
                        (
                            200,
                            json!({ "name": n, "valid": valid, "available": available, "reason": reason }),
                        )
                    }
                    ("PATCH", ["v1", "me"]) => {
                        if b["screenName"] == "taken-name" {
                            (
                                409,
                                json!({ "error": { "code": "taken", "message": "Taken." } }),
                            )
                        } else {
                            if let Some(n) = b["screenName"].as_str() {
                                s["screen_name"] = json!(n);
                            }
                            if let Some(n) = b["name"].as_str() {
                                s["display_name"] = json!(n);
                            }
                            (
                                200,
                                json!({ "user": { "screenName": s["screen_name"].clone(), "name": s["display_name"].clone() } }),
                            )
                        }
                    }
                    // Settings that follow you (account-settings.ts): newest per key wins.
                    ("GET", ["v1", "me", "settings"]) => {
                        let doc = s["settings_doc"].clone();
                        let mut settings = serde_json::Map::new();
                        let mut times = serde_json::Map::new();
                        for (k, e) in doc.as_object().into_iter().flatten() {
                            times.insert(k.clone(), e["t"].clone());
                            if !e["v"].is_null() {
                                settings.insert(k.clone(), e["v"].clone());
                            }
                        }
                        (
                            200,
                            json!({ "settings": settings, "times": times, "updated": 1 }),
                        )
                    }
                    ("PATCH", ["v1", "me", "settings"]) => {
                        for (k, c) in b["changes"].as_object().into_iter().flatten() {
                            let t = c["t"].as_u64().unwrap_or(0);
                            if s["settings_doc"][k]["t"].as_u64().unwrap_or(0) < t {
                                s["settings_doc"][k] = json!({ "v": c["value"].clone(), "t": t });
                            }
                        }
                        (200, json!({}))
                    }
                    ("POST", ["v1", "signout"]) => {
                        s["signed_out"] = json!(true);
                        (200, json!({ "ok": true }))
                    }
                    ("POST", ["v1", "session"]) => (
                        401,
                        json!({ "error": { "code": "guest", "message": "unknown" } }),
                    ),
                    _ if auth != "Bearer tok" => (
                        401,
                        json!({ "error": { "code": "auth", "message": "Sign in." } }),
                    ),
                    ("POST", ["v1", "assets"]) => {
                        let id = format!("a{}", s["assets"].as_object().unwrap().len() + 1);
                        s["assets"][&id] = json!({ "access": "private", "etag": null });
                        (201, json!({ "id": id }))
                    }
                    (_, ["v1", "assets", id, ..]) if s["assets"][*id].is_null() => (
                        404,
                        json!({ "error": { "code": "not_found", "message": "Not found." } }),
                    ),
                    ("GET", ["v1", "assets", _, "versions"]) => (200, json!({ "versions": [] })),
                    ("PUT", ["v1", "assets", id]) => {
                        s["assets"][*id]["etag"] = json!("e1");
                        s["assets"][*id]["svg"] = json!(String::from_utf8_lossy(&body));
                        (200, json!({ "etag": "e1" }))
                    }
                    ("PUT", ["v1", "assets", id, "preview"]) => {
                        s["assets"][*id]["preview"] = json!(body.starts_with(b"\x89PNG"));
                        (200, json!({}))
                    }
                    ("PUT", ["v1", "assets", id, "plan"]) => {
                        let ok = r
                            .headers()
                            .iter()
                            .any(|h| h.field.equiv("X-Design-Etag") && h.value.as_str() == "e1");
                        s["assets"][*id]["plan"] = json!(ok && body.starts_with(b"ZZPLAN"));
                        (200, json!({}))
                    }
                    ("PATCH", ["v1", "assets", id]) => {
                        let a = &mut s["assets"][*id];
                        for k in ["name", "access", "license", "editable"] {
                            a[k] = b[k].clone();
                        }
                        let url = (b["access"] == "public")
                            .then(|| format!("https://zigzag.diy/share/{id}"));
                        (200, json!({ "shareUrl": url, "hidden": false }))
                    }
                    ("POST", ["v1", "assets", id, "signed"]) => {
                        let n = s["assets"][*id]["signed"].as_u64().unwrap_or(0) + 1;
                        s["assets"][*id]["signed"] = json!(n);
                        let exp = now_ms() + b["expiresIn"].as_u64().unwrap() * 1000;
                        (
                            201,
                            json!({ "url": format!("https://zigzag.diy/share/{id}?s={n}"), "expires": exp }),
                        )
                    }
                    ("POST", ["v1", "assets", id, "signed", "revoke"]) => {
                        s["assets"][*id]["revoked"] = json!(true);
                        (200, json!({ "ok": true }))
                    }
                    ("DELETE", ["v1", "assets", id]) => {
                        s["assets"].as_object_mut().unwrap().remove(*id);
                        (200, json!({ "ok": true }))
                    }
                    _ => (
                        404,
                        json!({ "error": { "code": "not_found", "message": "Not found." } }),
                    ),
                };
                drop(s);
                let _ = r.respond(
                    tiny_http::Response::from_string(reply.to_string()).with_status_code(status),
                );
            }
        });
        Fake { base, log, state }
    }

    fn go(f: &Fake, guest: Option<String>, job: Job) -> Vec<Msg> {
        let out = Mutex::new(Vec::new());
        run(&f.base, Cred::Guest(guest), job, &|m| {
            out.lock().unwrap().push(m)
        });
        out.into_inner().unwrap()
    }

    fn upload() -> Upload {
        Upload {
            svg: "<svg/>".into(),
            picture: Some(b"\x89PNG....".to_vec()),
            plan: Some(b"ZZPLAN..".to_vec()),
        }
    }

    fn shared(msgs: Vec<Msg>) -> (Option<String>, Record, Note) {
        let mut guest = None;
        for m in msgs {
            match m {
                Msg::Guest(s) => guest = Some(s),
                Msg::Shared(r, n) => return (guest, r, n),
                Msg::Failed(why) => panic!("{why}"),
                Msg::Deleted(_) => panic!("deleted"),
            }
        }
        panic!("no answer")
    }

    #[test]
    fn a_first_share_makes_a_guest_and_a_public_page() {
        let f = fake();
        let s = Settings::new("Bee");
        let job = Job::Share {
            design: "d1".into(),
            settings: s,
            upload: Some(upload()),
            fingerprint: 7,
            before: None,
        };
        let (guest, r, note) = shared(go(&f, None, job));
        assert_eq!(guest.as_deref(), Some("s".repeat(43).as_str()));
        assert_eq!(note, Note::None);
        assert_eq!(r.url.as_deref(), Some("https://zigzag.diy/share/a1"));
        assert_eq!((r.asset.as_str(), r.uploaded), ("a1", 7));
        let st = f.state.lock().unwrap();
        let a = &st["assets"]["a1"];
        assert_eq!(a["svg"], "<svg/>");
        assert_eq!(
            (a["preview"].clone(), a["plan"].clone()),
            (json!(true), json!(true))
        );
        assert_eq!(a["access"], "public");
        assert_eq!(a["license"], "CC-BY-4.0");
        assert_eq!(a["editable"], true);
    }

    #[test]
    fn every_setting_reaches_the_server() {
        let f = fake();
        let secret = Some("s".repeat(43));
        let mut s = Settings::new("Bee");
        let job = Job::Share {
            design: "d".into(),
            settings: s.clone(),
            upload: Some(upload()),
            fingerprint: 1,
            before: None,
        };
        let (_, r, _) = shared(go(&f, secret.clone(), job));

        // A link for three days, no editable design, another license.
        s.access = Access::Link;
        s.days = 3;
        s.editable = false;
        s.license = "CC-BY-NC-4.0".into();
        let job = Job::Share {
            design: "d".into(),
            settings: s.clone(),
            upload: None,
            fingerprint: 1,
            before: Some(r),
        };
        let (_, r, _) = shared(go(&f, secret.clone(), job));
        assert_eq!(r.url.as_deref(), Some("https://zigzag.diy/share/a1?s=1"));
        let left = r.expires.unwrap() - now_ms();
        assert!(
            (3 * 86_400_000 - 60_000..=3 * 86_400_000).contains(&left),
            "{left}"
        );
        {
            let st = f.state.lock().unwrap();
            let a = &st["assets"]["a1"];
            assert_eq!(
                a["access"], "private",
                "a link is a signed link to a private file"
            );
            assert_eq!(
                (a["editable"].clone(), a["license"].clone()),
                (json!(false), json!("CC-BY-NC-4.0"))
            );
        }
        // Nothing changed was uploaded again.
        assert_eq!(
            f.log
                .lock()
                .unwrap()
                .iter()
                .filter(|l| l.starts_with("PUT"))
                .count(),
            3
        );

        // The name changed: the link already sent keeps working.
        s.name = "Bumble".into();
        let job = Job::Share {
            design: "d".into(),
            settings: s.clone(),
            upload: None,
            fingerprint: 1,
            before: Some(r),
        };
        let (_, r, _) = shared(go(&f, secret.clone(), job));
        assert_eq!(r.url.as_deref(), Some("https://zigzag.diy/share/a1?s=1"));
        assert_eq!(f.state.lock().unwrap()["assets"]["a1"]["name"], "Bumble");

        // Private: no page, every link revoked.
        s.access = Access::Private;
        let job = Job::Share {
            design: "d".into(),
            settings: s,
            upload: None,
            fingerprint: 1,
            before: Some(r),
        };
        let (_, r, _) = shared(go(&f, secret, job));
        assert_eq!(r.url, None);
        assert_eq!(f.state.lock().unwrap()["assets"]["a1"]["revoked"], true);
    }

    #[test]
    fn a_changed_design_is_uploaded_again_to_the_same_page() {
        let f = fake();
        let secret = Some("s".repeat(43));
        let s = Settings::new("Bee");
        let job = Job::Share {
            design: "d".into(),
            settings: s.clone(),
            upload: Some(upload()),
            fingerprint: 1,
            before: None,
        };
        let (_, r, _) = shared(go(&f, secret.clone(), job));
        let mut u = upload();
        u.svg = "<svg id='2'/>".into();
        let job = Job::Share {
            design: "d".into(),
            settings: s,
            upload: Some(u),
            fingerprint: 2,
            before: Some(r),
        };
        let (_, r, _) = shared(go(&f, secret, job));
        assert_eq!((r.asset.as_str(), r.uploaded), ("a1", 2));
        assert_eq!(
            f.state.lock().unwrap()["assets"]["a1"]["svg"],
            "<svg id='2'/>"
        );
    }

    #[test]
    fn a_page_gone_from_the_server_is_made_again() {
        let f = fake();
        let secret = Some("s".repeat(43));
        let before = Record {
            design: "d".into(),
            asset: "gone".into(),
            settings: Settings::new("Bee"),
            url: Some("https://zigzag.diy/share/gone".into()),
            expires: None,
            uploaded: 1,
            hidden: false,
            by: None,
            file: None,
        };
        let job = Job::Share {
            design: "d".into(),
            settings: Settings::new("Bee"),
            upload: Some(upload()),
            fingerprint: 1,
            before: Some(before),
        };
        let (_, r, note) = shared(go(&f, secret, job));
        assert_eq!(note, Note::NewPage);
        assert_eq!(r.asset, "a1");
        assert_eq!(f.state.lock().unwrap()["assets"]["a1"]["svg"], "<svg/>");
    }

    #[test]
    fn deleting_takes_it_off_the_server() {
        let f = fake();
        let secret = Some("s".repeat(43));
        let job = Job::Share {
            design: "d".into(),
            settings: Settings::new("Bee"),
            upload: Some(upload()),
            fingerprint: 1,
            before: None,
        };
        let (_, r, _) = shared(go(&f, secret.clone(), job));
        let msgs = go(&f, secret, Job::Delete(r));
        assert!(matches!(msgs.as_slice(), [Msg::Deleted(d)] if d == "d"));
        assert!(f.state.lock().unwrap()["assets"]["a1"].is_null());
    }

    #[test]
    fn a_forgotten_guest_says_so() {
        let f = fake();
        let job = Job::Share {
            design: "d".into(),
            settings: Settings::new("Bee"),
            upload: Some(upload()),
            fingerprint: 1,
            before: None,
        };
        let msgs = go(&f, Some("x".repeat(43)), job);
        assert!(
            matches!(msgs.as_slice(), [Msg::Failed(w)] if w.contains("only") || w.contains("aren't known"))
        );
    }

    #[test]
    fn no_server_says_so() {
        let job = Job::Share {
            design: "d".into(),
            settings: Settings::new("Bee"),
            upload: Some(upload()),
            fingerprint: 1,
            before: None,
        };
        let out = Mutex::new(Vec::new());
        run("http://127.0.0.1:9", Cred::Guest(None), job, &|m| {
            out.lock().unwrap().push(m)
        });
        let msgs = out.into_inner().unwrap();
        assert!(
            matches!(msgs.as_slice(), [Msg::Failed(w)] if w.starts_with("Couldn't reach zigzag.diy"))
        );
    }

    #[test]
    fn the_store_keeps_the_secret_and_every_setting() {
        let dir = std::env::temp_dir().join(format!("zigzag-share-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("shares.json");
        let mut s = Store::at(path.clone());
        s.guest = Some("secret".into());
        let mut settings = Settings::new("Bee");
        settings.access = Access::Link;
        settings.days = 3;
        settings.editable = false;
        settings.license = "CC0-1.0".into();
        let r = Record {
            design: "d".into(),
            asset: "a".into(),
            settings,
            url: Some("https://zigzag.diy/share/a?s=1".into()),
            expires: Some(5),
            uploaded: u64::MAX - 3,
            hidden: true,
            by: None,
            file: None,
        };
        s.put(r.clone());
        s.save();
        let back = Store::at(path.clone());
        assert_eq!(back.guest.as_deref(), Some("secret"));
        assert_eq!(back.find("d"), Some(&r));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "only this user may read the secret");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn how_long_a_link_lasts() {
        let day = 86_400_000;
        assert_eq!(time_left(10 * day, 3 * day), "Expires in 7 days");
        assert_eq!(time_left(day + 3_600_000, 0), "Expires in 25 hours");
        assert_eq!(time_left(3_600_000, 0), "Expires in 1 hour");
        assert_eq!(time_left(5, 10), "Expired");
        assert_eq!(time_left(100 * 365 * day, 0), "Never expires");
    }

    #[test]
    fn licenses_match_the_servers() {
        let ids: Vec<&str> = LICENSES.iter().map(|l| l.0).collect();
        assert_eq!(ids[0], "CC-BY-4.0", "the default");
        assert_eq!(ids.len(), 6);
    }

    /// Against a real zigzag-server (`wrangler dev`, say):
    /// `ZIGZAG_SERVER=http://localhost:8799 cargo test --features gui -- --ignored live_share`.
    /// Shares the badge from the app, as File › Share… does, and reads its
    /// page's details, design and a machine file back.
    #[test]
    #[ignore]
    fn live_share() {
        let base = std::env::var("ZIGZAG_SERVER").expect("ZIGZAG_SERVER");
        let badge = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/badge.svg");
        let mut studio = Studio::with_file(Some(badge), None);
        let upload = studio.share_upload().unwrap();
        assert!(upload.picture.is_some() && upload.plan.is_some());
        let svg = upload.svg.clone();
        let mut s = Settings::new("Badge");
        let job = Job::Share {
            design: "live".into(),
            settings: s.clone(),
            upload: Some(upload),
            fingerprint: 1,
            before: None,
        };
        let out = Mutex::new(Vec::new());
        run(&base, Cred::Guest(None), job, &|m| {
            out.lock().unwrap().push(m)
        });
        let (guest, r, _) = shared(out.into_inner().unwrap());
        let url = r.url.clone().expect("a public page");
        eprintln!("shared at {url}");
        let api = Api::new(&base);
        let d = api
            .call("GET", &format!("/v1/share/{}", r.asset), None, &[])
            .unwrap();
        assert_eq!(d["name"], "Badge");
        assert_eq!(d["license"]["id"], "CC-BY-4.0");
        assert_eq!(d["editable"], true);
        assert!(d["plan"]["stitches"].as_u64().unwrap() > 100, "{d}");
        assert!(d["urls"]["preview"].is_string());
        let fetch = |u: &str| Command::new("curl").args(["-sf", u]).output().unwrap();
        let design = fetch(d["urls"]["design"].as_str().unwrap());
        assert_eq!(String::from_utf8_lossy(&design.stdout), svg);
        let pes = fetch(d["urls"]["exports"]["pes"].as_str().unwrap());
        let ours = zigzag::formats::write_format_with(
            &studio.design,
            "pes",
            &studio.doc.settings.export_options(),
        )
        .unwrap();
        assert_eq!(
            pes.stdout, ours,
            "the server's PES is byte for byte the app's"
        );

        // A link for a day, no editable design.
        s.access = Access::Link;
        s.days = 1;
        s.editable = false;
        let job = Job::Share {
            design: "live".into(),
            settings: s,
            upload: None,
            fingerprint: 1,
            before: Some(r.clone()),
        };
        let out = Mutex::new(Vec::new());
        run(&base, Cred::Guest(guest.clone()), job, &|m| {
            out.lock().unwrap().push(m)
        });
        let (_, r2, _) = shared(out.into_inner().unwrap());
        let link = r2.url.clone().unwrap();
        assert!(link.contains("&s="), "{link}");
        let public = api.call("GET", &format!("/v1/share/{}", r.asset), None, &[]);
        assert_eq!(public.unwrap_err().status, 404, "not public any more");
        let q = &link[link.find('?').unwrap()..];
        let d = api
            .call("GET", &format!("/v1/share/{}{q}", r.asset), None, &[])
            .unwrap();
        assert_eq!(
            (d["editable"].clone(), d["urls"]["design"].clone()),
            (json!(false), Value::Null)
        );

        let out = Mutex::new(Vec::new());
        run(&base, Cred::Guest(guest), Job::Delete(r2), &|m| {
            out.lock().unwrap().push(m)
        });
        assert!(matches!(
            out.into_inner().unwrap().as_slice(),
            [Msg::Deleted(_)]
        ));
    }

    /// The panel, clicked through as someone would: Share, then a link
    /// instead, then the license, against the pretend server.
    #[test]
    fn the_panel_shares_and_changes_settings_as_theyre_chosen() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        let f = fake();
        let dir = std::env::temp_dir().join(format!("zigzag-share-panel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let badge = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/badge.svg");
        let mut studio = Studio::with_file(Some(badge), None);
        studio.share.store = Some(Store::at(dir.join("shares.json")));
        studio.share.base = Some(f.base.clone());
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), studio);
        h.run_steps(2);
        let wait = |h: &mut Harness<'_, Studio>| {
            for _ in 0..200 {
                h.run_steps(1);
                if h.state().share.busy.is_none() {
                    h.run_steps(2);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("the server never answered");
        };

        // Opening it shares nothing: a choice made now waits for Share.
        h.state_mut().open_share();
        h.run_steps(2);
        h.get_by_label("Downloads: Editable design and machine files")
            .click();
        h.run_steps(2);
        h.get_by_label("Machine files only").click();
        h.run_steps(2);
        assert!(h.state().share.busy.is_none());
        assert!(
            f.log.lock().unwrap().is_empty(),
            "nothing sent before Share"
        );
        assert!(h.query_by_label("Copy").is_none());

        // Share: anyone (the default), and that choice.
        h.get_by_label("Get Share Link").click();
        h.run_steps(1);
        wait(&mut h);
        assert_eq!(h.state().share.error, None);
        let r = h.state_mut().share.store().shares[0].clone();
        assert_eq!(r.settings.access, Access::Public);
        assert_eq!(r.url.as_deref(), Some("https://zigzag.diy/share/a1"));
        assert!(h.query_by_label("Who can see it: Anyone").is_some());
        assert!(h.query_by_label("Copy").is_some());
        {
            let st = f.state.lock().unwrap();
            let a = &st["assets"]["a1"];
            assert_eq!(
                (
                    a["access"].clone(),
                    a["plan"].clone(),
                    a["license"].clone(),
                    a["editable"].clone()
                ),
                (
                    json!("public"),
                    json!(true),
                    json!("CC-BY-4.0"),
                    json!(false)
                )
            );
        }

        // A link instead: forever, unless a time is chosen.
        h.get_by_label("Who can see it: Anyone").click();
        h.run_steps(2);
        h.get_by_label("Anyone with the link").click();
        h.run_steps(1);
        wait(&mut h);
        let r = h.state_mut().share.store().shares[0].clone();
        assert_eq!(
            (r.settings.access, r.settings.days),
            (Access::Link, FOREVER)
        );
        assert!(r.expires.unwrap() > now_ms() + 99 * 365 * 86_400_000);
        assert!(r.url.unwrap().contains("?s=1"));
        assert!(
            h.query_by_label("Who can see it: Anyone with the link")
                .is_some()
        );
        // A month: a new link, from the second dropdown on the same line.
        h.get_by_label("Link works: forever").click();
        h.run_steps(2);
        h.get_by_label("1 month").click();
        h.run_steps(1);
        wait(&mut h);
        let r = h.state_mut().share.store().shares[0].clone();
        assert_eq!(r.settings.days, 30);
        assert!(r.url.unwrap().contains("?s=2"));
        assert!(h.query_by_label("Link works: for 1 month").is_some());

        // The license.
        h.get_by_label("License: CC BY 4.0").click();
        h.run_steps(2);
        h.get_by_label("CC0 (public domain)").click();
        h.run_steps(1);
        wait(&mut h);
        assert_eq!(
            f.state.lock().unwrap()["assets"]["a1"]["license"],
            "CC0-1.0"
        );

        // The name, with the pencil.
        h.get_by_label("Name: badge").click();
        h.run_steps(2);
        // (The field has the focus: type into it.)
        h.event(egui::Event::Text("2".into()));
        h.run_steps(1);
        h.key_press(egui::Key::Enter);
        h.run_steps(1);
        wait(&mut h);
        assert_eq!(f.state.lock().unwrap()["assets"]["a1"]["name"], "badge2");
        assert_eq!(
            f.log
                .lock()
                .unwrap()
                .iter()
                .filter(|l| l.starts_with("PUT /v1/assets/a1"))
                .count(),
            3,
            "uploaded once"
        );

        // A change that doesn't reach the server isn't left looking made:
        // the panel shows how it's shared, and the choice can be made again.
        let base = h.state_mut().share.base.replace("http://127.0.0.1:9".into());
        h.get_by_label("License: CC0 (public domain)").click();
        h.run_steps(2);
        h.get_by_label("CC BY 4.0").click();
        h.run_steps(1);
        wait(&mut h);
        assert!(h.state().share.error.is_some());
        assert_eq!(
            h.state().share.settings.as_ref().unwrap().license,
            "CC0-1.0"
        );
        assert!(h.query_by_label("License: CC0 (public domain)").is_some());
        h.state_mut().share.base = base;

        // Escape ends a rename, and leaves the panel up.
        h.get_by_label("Name: badge2").click();
        h.run_steps(2);
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert!(h.state().share.renaming.is_none());
        assert!(h.state().share.open.is_some());

        // A copy of its file (Save As) keeps its id but is another design:
        // not shared yet, with a page of its own to come. And the panel,
        // which was the original's, goes when the copy is in front.
        let shared = h.state_mut().share_id();
        let original = h.state().doc.path.clone();
        assert_eq!(
            h.state_mut().share.store().shares[0].file,
            original.as_ref().map(|p| p.display().to_string())
        );
        h.state_mut().doc.path = Some(dir.join("copy.svg"));
        let copy = h.state_mut().share_id();
        assert_ne!(copy, shared);
        assert!(h.state_mut().share.store().find(&copy).is_none());
        h.run_steps(2);
        assert!(h.state().share.open.is_none());
        // The original is still its own.
        h.state_mut().doc.path = original;
        assert_eq!(h.state_mut().share_id(), shared);
        // A file that was moved (the one remembered is gone) is the same
        // design, and is remembered where it is now.
        h.state_mut().share.store().shares[0].file =
            Some(dir.join("gone.svg").display().to_string());
        h.state_mut().doc.path = Some(dir.join("moved.svg"));
        assert_eq!(h.state_mut().share_id(), shared);
        assert_eq!(
            h.state_mut().share.store().shares[0].file,
            Some(dir.join("moved.svg").display().to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Settings › Account, clicked through: an address, the code, signed
    /// in; what this computer shared goes with it, and shares from then
    /// on are the account's. Signed out, those can't be changed here.
    #[test]
    fn signing_in_brings_this_computers_shares_and_signing_out_locks_them() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        let f = fake();
        let dir = std::env::temp_dir().join(format!("zigzag-account-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let badge = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/badge.svg");
        let mut studio = Studio::with_file(Some(badge), None);
        studio.share.store = Some(Store::at(dir.join("shares.json")));
        studio.share.base = Some(f.base.clone());
        studio.account.file = Some(crate::account::AccountFile::at(dir.join("account.json")));
        studio.account.base = Some(f.base.clone());
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), studio);
        h.run_steps(2);
        let settle = |h: &mut Harness<'_, Studio>| {
            for _ in 0..200 {
                h.run_steps(1);
                if h.state().share.busy.is_none() && h.state().account.busy.is_none() {
                    h.run_steps(2);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("the server never answered");
        };

        // Shared first, as this computer's guest.
        h.state_mut().open_share();
        h.run_steps(2);
        h.get_by_label("Get Share Link").click();
        settle(&mut h);
        assert_eq!(h.state_mut().share.store().shares[0].by, None);
        h.state_mut().share.open = None;

        // Settings › Account: the address, then the code.
        h.state_mut()
            .open_settings(crate::settings::Section::Account);
        h.run_steps(2);
        h.state_mut().account.email = "kelly@example.com".into();
        h.run_steps(1);
        h.get_by_label("Sign In").click();
        settle(&mut h);
        assert!(
            matches!(&h.state().account.step, crate::account::Step::Code(e) if e == "kelly@example.com")
        );
        h.state_mut().account.code = "123 456".into();
        h.run_steps(1);
        h.get_by_label("Continue").click();
        settle(&mut h);
        assert_eq!(h.state().account.error, None);
        let a = h.state_mut().account.signed_in().unwrap();
        assert_eq!(
            (a.email.as_str(), a.device.as_str()),
            ("kelly@example.com", "device-token")
        );
        assert_eq!(
            f.state.lock().unwrap()["claimed"],
            true,
            "the guest's secret went along"
        );
        let store = h.state_mut().share.store();
        assert_eq!(store.guest, None, "the guest is gone");
        assert_eq!(store.shares[0].by.as_deref(), Some("kelly@example.com"));
        assert!(h.query_by_label("Sign Out").is_some());
        // Kept: a new launch is signed in too.
        let back = crate::account::AccountFile::at(dir.join("account.json"));
        assert_eq!(back.account, Some(a));
        h.state_mut().settings_ui.open = false;

        // Signed in, a change goes as the account.
        h.state_mut().open_share();
        h.run_steps(2);
        h.get_by_label("License: CC BY 4.0").click();
        h.run_steps(2);
        h.get_by_label("CC0 (public domain)").click();
        settle(&mut h);
        assert_eq!(h.state().share.error, None);
        assert_eq!(
            f.state.lock().unwrap()["assets"]["a1"]["license"],
            "CC0-1.0"
        );
        h.state_mut().share.open = None;

        // Signed out: the share is the account's, so it can't be changed here.
        h.state_mut()
            .open_settings(crate::settings::Section::Account);
        h.run_steps(2);
        h.get_by_label("Sign Out").click();
        settle(&mut h);
        assert!(h.state_mut().account.signed_in().is_none());
        assert_eq!(f.state.lock().unwrap()["signed_out"], true);
        assert!(!dir.join("account.json").exists());
        h.state_mut().settings_ui.open = false;
        h.state_mut().open_share();
        h.run_steps(2);
        assert_eq!(h.state().share.locked.as_deref(), Some("kelly@example.com"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Studio signed in to the pretend server, its files in `dir`.
    fn signed_in_studio(f: &Fake, dir: &std::path::Path) -> Studio {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("account.json"),
            json!({ "email": "kelly@example.com", "device": "device-token" }).to_string(),
        )
        .unwrap();
        let mut studio = Studio::with_file(None, None);
        studio.share.store = Some(Store::at(dir.join("shares.json")));
        studio.share.base = Some(f.base.clone());
        studio.account.file = Some(crate::account::AccountFile::at(dir.join("account.json")));
        studio.account.base = Some(f.base.clone());
        studio.sync.file = Some(crate::sync::SyncFile::at(dir.join("sync.json")));
        studio.sync.base = Some(f.base.clone());
        studio
    }

    fn settle(h: &mut egui_kittest::Harness<'_, Studio>) {
        for _ in 0..300 {
            h.run_steps(1);
            let s = h.state();
            if s.account.busy.is_none() && s.sync.busy.is_none() {
                h.run_steps(2);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the server never answered");
    }

    /// Signed in, Settings › Account shows the screen name made for you,
    /// and Change opens it to type a new one: a taken one says so, a free
    /// one is taken.
    #[test]
    fn screen_names_are_checked_and_saved() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        let f = fake();
        let dir = std::env::temp_dir().join(format!("zigzag-names-{}", std::process::id()));
        let mut studio = signed_in_studio(&f, &dir);
        studio.open_settings(crate::settings::Section::Account);
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), studio);
        settle(&mut h);
        assert!(
            h.query_by_label("dreamy-applique-575").is_some(),
            "shown, not in a field"
        );
        assert!(!h.state().account.editing);

        // Change: a field; a taken name says so and nothing is saved.
        h.get_by_label("Change").click();
        h.run_steps(2);
        assert!(h.state().account.editing);
        h.state_mut().account.screen_name = "taken-name".into();
        h.run_steps(1);
        h.get_by_label("Save").click();
        settle(&mut h);
        assert_eq!(h.state().account.error.as_deref(), Some("Taken"));
        assert!(h.state().account.editing);
        assert!(f.state.lock().unwrap()["screen_name"].is_null());

        // A free one: taken, and shown again as text.
        h.state_mut().account.screen_name = "kelly-stitches".into();
        h.run_steps(1);
        h.get_by_label("Save").click();
        settle(&mut h);
        assert_eq!(f.state.lock().unwrap()["screen_name"], "kelly-stitches");
        assert!(!h.state().account.editing);
        assert!(h.query_by_label("kelly-stitches").is_some());

        // Cancel puts it back.
        h.get_by_label("Change").click();
        h.run_steps(2);
        h.state_mut().account.screen_name = "something-else".into();
        h.get_by_label("Cancel").click();
        h.run_steps(2);
        assert!(!h.state().account.editing);
        assert_eq!(h.state().account.screen_name, "kelly-stitches");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Signed in: what's newer on the account comes here; a change here
    /// goes up with its time; something older there doesn't overwrite it.
    #[test]
    fn settings_follow_the_account_newest_wins() {
        use egui_kittest::Harness;
        let f = fake();
        let dir = std::env::temp_dir().join(format!("zigzag-sync-{}", std::process::id()));
        {
            let mut st = f.state.lock().unwrap();
            st["settings_doc"] = json!({
                "threads.chart": { "v": "janome", "t": 1000 },
                "palettes": { "v": { "palettes": [{ "name": "Spring", "colors": ["#ff0000", "#00ff00"] }] }, "t": 1000 },
                "lettering.font": { "v": "Old Font", "t": 1000 },
            });
        }
        let mut studio = signed_in_studio(&f, &dir);
        // Changed here after the account's copy (time 1000): ours stays.
        studio.prefs.last_font = Some("Newer Font".into());
        {
            let sf = studio.sync.file();
            sf.times.insert("lettering.font".into(), 5000);
        }
        let mut h = Harness::builder()
            .with_size(Vec2::new(1200.0, 800.0))
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), studio);
        settle(&mut h);
        assert!(h.state().sync.pulled);
        assert_eq!(h.state().prefs.thread_chart, crate::threads::Brand::Janome);
        assert_eq!(h.state().view_tools.brand, crate::threads::Brand::Janome);
        assert_eq!(h.state().palettes.mine.len(), 1);
        assert_eq!(h.state().palettes.mine[0].name, "Spring");
        assert_eq!(
            h.state().prefs.last_font.as_deref(),
            Some("Newer Font"),
            "older there, kept here"
        );

        // The first look sends up what's only here, or newer here.
        h.state_mut().sync.last_look = None;
        settle(&mut h);
        {
            let st = f.state.lock().unwrap();
            assert_eq!(st["settings_doc"]["lettering.font"]["v"], "Newer Font");
            assert!(
                st["settings_doc"]["machines"].is_object(),
                "machines went up"
            );
            assert_eq!(
                st["settings_doc"]["palettes"]["t"], 1000,
                "unchanged here: not sent back"
            );
        }

        // A change here goes up with its time.
        h.state_mut()
            .set_thread_chart(crate::threads::Brand::Brother);
        h.state_mut().sync.last_look = None;
        settle(&mut h);
        {
            let st = f.state.lock().unwrap();
            assert_eq!(st["settings_doc"]["threads.chart"]["v"], "brother");
            assert!(st["settings_doc"]["threads.chart"]["t"].as_u64().unwrap() > 1000);
        }
        // Kept: what was seen and when, for the next launch.
        let back = crate::sync::SyncFile::at(dir.join("sync.json"));
        assert!(back.times["threads.chart"] > 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A change that didn't get to the server (offline) is still a change:
    /// tried again after a wait, and after a restart.
    #[test]
    fn a_setting_that_didnt_go_up_goes_up_later() {
        use egui_kittest::Harness;
        let f = fake();
        let dir = std::env::temp_dir().join(format!("zigzag-sync-later-{}", std::process::id()));
        let studio = signed_in_studio(&f, &dir);
        let mut h = Harness::builder()
            .with_size(Vec2::new(1200.0, 800.0))
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), studio);
        settle(&mut h);
        h.state_mut().sync.last_look = None;
        settle(&mut h);
        assert!(h.state().sync.retry_at.is_none());

        // No server: the change stays here, and the next try waits.
        h.state_mut().sync.base = Some("http://127.0.0.1:9".into());
        h.state_mut()
            .set_thread_chart(crate::threads::Brand::Janome);
        h.state_mut().sync.last_look = None;
        settle(&mut h);
        assert!(h.state().sync.retry_at.is_some(), "waits before trying again");
        assert_ne!(
            f.state.lock().unwrap()["settings_doc"]["threads.chart"]["v"],
            "janome"
        );

        // Quit and launched again, the server back: it goes up.
        h.state_mut().sync.file = Some(crate::sync::SyncFile::at(dir.join("sync.json")));
        h.state_mut().sync.base = Some(f.base.clone());
        h.state_mut().sync.retry_at = None;
        h.state_mut().sync.last_look = None;
        settle(&mut h);
        assert_eq!(
            f.state.lock().unwrap()["settings_doc"]["threads.chart"]["v"],
            "janome"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pull that fails is asked for again (and pushing isn't held up
    /// for the rest of the session).
    #[test]
    fn a_pull_that_fails_is_tried_again() {
        use egui_kittest::Harness;
        let f = fake();
        let dir = std::env::temp_dir().join(format!("zigzag-sync-pull-{}", std::process::id()));
        let mut studio = signed_in_studio(&f, &dir);
        studio.sync.base = Some("http://127.0.0.1:9".into());
        let mut h = Harness::builder()
            .with_size(Vec2::new(1200.0, 800.0))
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), studio);
        settle(&mut h);
        assert!(!h.state().sync.pulled);
        assert!(h.state().sync.want_pull && h.state().sync.retry_at.is_some());
        h.state_mut().sync.base = Some(f.base.clone());
        h.state_mut().sync.retry_at = None;
        settle(&mut h);
        assert!(h.state().sync.pulled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The first time a computer syncs, its own palettes stay beside the
    /// account's (and go up); after that, an edit here that a pull hasn't
    /// seen yet isn't overwritten by something older on the account.
    #[test]
    fn a_pull_keeps_whats_only_here() {
        use egui_kittest::Harness;
        let f = fake();
        let dir = std::env::temp_dir().join(format!("zigzag-sync-keep-{}", std::process::id()));
        f.state.lock().unwrap()["settings_doc"] = json!({
            "palettes": { "v": { "palettes": [{ "name": "Spring", "colors": ["#ff0000"] }] }, "t": 1000 },
            "lettering.font": { "v": "Old Font", "t": 1000 },
        });
        let mut studio = signed_in_studio(&f, &dir);
        studio.palettes.mine.push(crate::palettes::UserPalette {
            name: "Mine".into(),
            colors: vec![[0, 0, 255]],
        });
        let mut h = Harness::builder()
            .with_size(Vec2::new(1200.0, 800.0))
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), studio);
        settle(&mut h);
        let names = |s: &Studio| -> Vec<String> {
            s.palettes.mine.iter().map(|p| p.name.clone()).collect()
        };
        assert_eq!(names(h.state()), ["Spring", "Mine"]);
        h.state_mut().sync.last_look = None;
        settle(&mut h);
        assert_eq!(
            f.state.lock().unwrap()["settings_doc"]["palettes"]["v"]["palettes"]
                .as_array()
                .map(Vec::len),
            Some(2),
            "both went up"
        );

        // Changed here, then a pull of something newer than the last sync
        // but made before this edit: ours stays, and goes up.
        assert_eq!(h.state().prefs.last_font.as_deref(), Some("Old Font"));
        h.state_mut().prefs.last_font = Some("Mine Font".into());
        f.state.lock().unwrap()["settings_doc"]["lettering.font"] =
            json!({ "v": "Other Font", "t": 2000 });
        h.state_mut().sync.want_pull = true;
        settle(&mut h);
        assert_eq!(h.state().prefs.last_font.as_deref(), Some("Mine Font"));
        h.state_mut().sync.last_look = None;
        settle(&mut h);
        assert_eq!(
            f.state.lock().unwrap()["settings_doc"]["lettering.font"]["v"],
            "Mine Font"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
