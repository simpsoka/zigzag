//! Settings › Account: signing in to zigzag.diy with an email address and
//! a code (zigzag-server's `email.ts`). No password: the first code for an
//! address makes its account, so signing in and signing up are one step.
//!
//! Why have one: without it, what's shared from this computer can only be
//! changed from here (see `share`). Signed in, shares belong to the
//! account; signing in also brings along what this computer shared before.
//!
//! What's kept is `account.json` in the settings folder, only this user
//! may read it: the address, and the long random token that renews this
//! computer's session (only its hash is on the server). Signing out
//! forgets it here and there.

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;
use serde_json::{Value, json};

use crate::app::Studio;
use crate::share::{Api, Cred};

/// Signed in on this computer.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub email: String,
    /// Renews the session (`POST /v1/session {device}`).
    pub device: String,
}

/// `account.json`.
#[derive(Debug, Default)]
pub struct AccountFile {
    path: Option<PathBuf>,
    pub account: Option<Account>,
}

impl AccountFile {
    pub fn in_config() -> AccountFile {
        // (Tests never read this computer's sign-in.)
        if cfg!(test) {
            return AccountFile::default();
        }
        match crate::dock::config_file("account.json") {
            Some(p) => AccountFile::at(p),
            None => AccountFile::default(),
        }
    }

    pub fn at(path: PathBuf) -> AccountFile {
        let v: Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        let account = match (v["email"].as_str(), v["device"].as_str()) {
            (Some(e), Some(d)) => Some(Account {
                email: e.to_string(),
                device: d.to_string(),
            }),
            _ => None,
        };
        AccountFile {
            path: Some(path),
            account,
        }
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        match &self.account {
            Some(a) => {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                // Only this user may read it: the token is the sign-in.
                let v = json!({ "email": a.email, "device": a.device });
                let _ = crate::session::write_private(path, &v.to_string());
            }
            None => {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Where signing in is.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Step {
    /// Typing the address.
    #[default]
    Email,
    /// A code went to this address: typing it.
    Code(String),
}

/// Who you are on zigzag.diy (`GET /v1/me`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Me {
    /// Unique, chosen by you (a random one to start: "dreamy-applique-575").
    pub screen_name: String,
    /// Shown on your shared designs, if you give one.
    pub name: String,
}

/// Whether a screen name can be had (`GET /v1/screen-names/:name`).
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub name: String,
    pub available: bool,
    /// Why not, when it can't be had.
    pub reason: Option<String>,
}

/// What a job against the server says back.
pub enum Msg {
    CodeSent(String),
    SignedIn {
        account: Account,
        claimed: u64,
        created: bool,
    },
    Me(Me),
    Checked(Check),
    Saved(Me),
    SignedOut,
    /// This computer's guest was asked for again (`claim`): true when
    /// that's settled (its shares are the account's, or it's gone).
    Claimed(bool),
    Failed(String),
}

/// The Account section: what's typed, and what's under way.
#[derive(Default)]
pub struct AccountUi {
    pub file: Option<AccountFile>,
    pub step: Step,
    pub email: String,
    pub code: String,
    pub busy: Option<mpsc::Receiver<Msg>>,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// The server, when not `share::server()` (tests).
    pub base: Option<String>,
    /// Who you are there, once asked.
    pub me: Option<Me>,
    /// This computer's guest has been asked for this launch (see
    /// `take_account_news`).
    pub claim_asked: bool,
    /// Changing the screen name: this is what's typed.
    pub editing: bool,
    pub screen_name: String,
    /// Put the cursor in the screen name field (it was just opened).
    pub focus_name: bool,
    /// Just made: say what the screen name is.
    pub welcome: bool,
}

impl AccountUi {
    pub fn file(&mut self) -> &mut AccountFile {
        self.file.get_or_insert_with(AccountFile::in_config)
    }

    /// Signed in on this computer, as whom.
    pub fn signed_in(&mut self) -> Option<Account> {
        self.file().account.clone()
    }
}

/// Ask for a code for `email`.
pub fn start(base: &str, email: &str) -> Msg {
    let api = Api::new(base);
    match api.json("POST", "/v1/email/start", &json!({ "email": email })) {
        Ok(_) => Msg::CodeSent(email.trim().to_string()),
        Err(e) => Msg::Failed(e.say()),
    }
}

/// Trade a code for this computer's sign-in, bringing the guest's shares.
pub fn verify(base: &str, email: &str, code: &str, guest: Option<&str>) -> Msg {
    let api = Api::new(base);
    let mut b = json!({ "email": email, "code": code });
    if let Some(g) = guest {
        b["guest"] = json!(g);
    }
    match api.json("POST", "/v1/email/verify", &b) {
        Ok(v) => match (v["user"]["email"].as_str(), v["device"].as_str()) {
            (Some(e), Some(d)) => Msg::SignedIn {
                account: Account {
                    email: e.to_string(),
                    device: d.to_string(),
                },
                claimed: v["claimed"].as_u64().unwrap_or(0),
                created: v["created"].as_bool().unwrap_or(false),
            },
            _ => Msg::Failed("zigzag.diy didn't sign this computer in.".into()),
        },
        Err(e) => Msg::Failed(e.say()),
    }
}

/// Bring this computer's guest's shares into the account, when signing
/// in didn't (the server couldn't just then).
pub fn claim(base: &str, device: &str, guest: &str) -> Msg {
    let Ok(api) = session(base, device) else {
        return Msg::Claimed(false);
    };
    match api.json("POST", "/v1/claim", &json!({ "guest": guest })) {
        Ok(_) => Msg::Claimed(true),
        // (A guest the server doesn't know: nothing left to bring.)
        Err(e) => Msg::Claimed(e.status == 404),
    }
}

fn me_from(v: &Value) -> Me {
    Me {
        screen_name: v["user"]["screenName"].as_str().unwrap_or("").to_string(),
        name: v["user"]["name"].as_str().unwrap_or("").to_string(),
    }
}

/// A session for this computer's account.
pub(crate) fn session(base: &str, device: &str) -> Result<Api, String> {
    let mut api = Api::new(base);
    api.sign_in(&Cred::Device(device.to_string()), &|_| {})
        .map_err(|e| e.say())?;
    Ok(api)
}

/// Who you are there.
pub fn fetch_me(base: &str, device: &str) -> Msg {
    match session(base, device)
        .and_then(|api| api.call("GET", "/v1/me", None, &[]).map_err(|e| e.say()))
    {
        Ok(v) => Msg::Me(me_from(&v)),
        Err(e) => Msg::Failed(e),
    }
}

/// Can this screen name be had?
pub fn check_name(base: &str, device: &str, name: &str) -> Msg {
    let path = format!("/v1/screen-names/{}", encode(name));
    match session(base, device)
        .and_then(|api| api.call("GET", &path, None, &[]).map_err(|e| e.say()))
    {
        Ok(v) => Msg::Checked(Check {
            name: name.to_string(),
            available: v["valid"].as_bool().unwrap_or(false)
                && v["available"].as_bool().unwrap_or(false),
            reason: v["reason"].as_str().map(String::from),
        }),
        Err(e) => Msg::Failed(e),
    }
}

/// Take a new screen name: checked first, so a taken or invalid one says
/// why without trying.
pub fn change_name(base: &str, device: &str, screen_name: &str) -> Msg {
    if let Msg::Checked(c) = check_name(base, device, screen_name)
        && !c.available
    {
        return Msg::Failed(
            c.reason
                .unwrap_or_else(|| "That screen name is taken.".into()),
        );
    }
    let body = json!({ "screenName": screen_name });
    match session(base, device)
        .and_then(|api| api.json("PATCH", "/v1/me", &body).map_err(|e| e.say()))
    {
        Ok(v) => Msg::Saved(me_from(&v)),
        Err(e) => Msg::Failed(e),
    }
}

/// A path segment, percent-encoded.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Sign this computer out on the server (its token stops working). Signed
/// out here whatever the server says.
pub fn sign_out(base: &str, device: &str) -> Msg {
    let mut api = Api::new(base);
    if api
        .sign_in(&Cred::Device(device.to_string()), &|_| {})
        .is_ok()
    {
        let _ = api.json("POST", "/v1/signout", &json!({}));
    }
    Msg::SignedOut
}

impl Studio {
    fn account_job(&mut self, job: impl FnOnce(&str) -> Msg + Send + 'static) {
        let base = self
            .account
            .base
            .clone()
            .unwrap_or_else(crate::share::server);
        let (tx, rx) = mpsc::channel();
        let ctx = self.egui_ctx.clone();
        let _ = std::thread::Builder::new()
            .name("zigzag-account".into())
            .spawn(move || {
                let _ = tx.send(job(&base));
                if let Some(c) = &ctx {
                    c.request_repaint();
                }
            });
        self.account.busy = Some(rx);
        self.account.error = None;
        self.account.notice = None;
    }

    /// The server's answer to a job under way. (Every frame, not only
    /// while Settings shows the account: signing in is kept even if
    /// Settings was closed before the answer came.)
    pub(crate) fn take_account_news(&mut self) {
        // Once a launch, and after signing in: a guest of this computer's
        // whose shares aren't the account's yet is asked for.
        if self.account.busy.is_none() && !std::mem::replace(&mut self.account.claim_asked, true) {
            let guest = {
                let store = self.share.store();
                store
                    .guest
                    .clone()
                    .filter(|_| store.shares.iter().any(|r| r.by.is_none()))
            };
            if let (Some(guest), Some(a)) = (guest, self.account.signed_in()) {
                // (Quietly: what Settings says stays.)
                let said = (self.account.error.take(), self.account.notice.take());
                self.account_job(move |base| claim(base, &a.device, &guest));
                (self.account.error, self.account.notice) = said;
            }
        }
        let Some(rx) = &self.account.busy else { return };
        let msg = match rx.try_recv() {
            Ok(m) => m,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.account.busy = None;
                return;
            }
        };
        self.account.busy = None;
        match msg {
            Msg::CodeSent(email) => {
                self.account.step = Step::Code(email);
                self.account.code.clear();
            }
            Msg::SignedIn {
                account,
                claimed,
                created,
            } => {
                self.account.welcome = created;
                self.account.me = None;
                let email = account.email.clone();
                let f = self.account.file();
                f.account = Some(account);
                f.save();
                // What this computer shared is the account's now (the
                // server moved it, and the guest is gone).
                let store = self.share.store();
                if claimed > 0 || store.shares.iter().all(|r| r.by.is_some()) {
                    store.guest = None;
                    for r in store.shares.iter_mut().filter(|r| r.by.is_none()) {
                        r.by = Some(email.clone());
                    }
                    store.save();
                } else {
                    // Shares of the guest's that didn't come (none were
                    // moved): they stay the guest's here too, and it's
                    // asked for again (`claim`), not written down as the
                    // account's while the server has them as the guest's.
                    self.account.claim_asked = false;
                }
                self.account.step = Step::Email;
                self.account.code.clear();
                self.account.notice = Some(match claimed {
                    0 => "Signed in.".into(),
                    1 => "Signed in. The design shared from this computer is in your account now."
                        .into(),
                    n => format!(
                        "Signed in. The {n} designs shared from this computer are in your account now."
                    ),
                });
                self.status = format!("Signed in as {email}.");
                // Bring this account's settings here (then send ours up).
                self.sync.pulled = false;
                self.sync.want_pull = true;
            }
            Msg::Me(me) | Msg::Saved(me) => {
                let saved = self.account.me.is_some();
                // (Not over a name being typed, unless it was just saved.)
                if !self.account.editing || saved || self.account.screen_name.is_empty() {
                    self.account.screen_name = me.screen_name.clone();
                }
                self.account.editing &= !saved;
                if saved && !self.account.welcome {
                    self.account.notice = Some("Saved.".into());
                }
                self.account.me = Some(me);
            }
            Msg::Checked(_) => {}
            Msg::Claimed(settled) => {
                if settled && let Some(a) = self.account.signed_in() {
                    let store = self.share.store();
                    store.guest = None;
                    for r in store.shares.iter_mut().filter(|r| r.by.is_none()) {
                        r.by = Some(a.email.clone());
                    }
                    store.save();
                }
            }
            Msg::SignedOut => {
                self.account.me = None;
                let f = self.account.file();
                f.account = None;
                f.save();
                // The next account starts from what's on this computer.
                self.sync.file().clear();
                self.sync.pulled = false;
                self.account.notice = Some("Signed out.".into());
                self.status = "Signed out of zigzag.diy.".into();
            }
            Msg::Failed(why) => self.account.error = Some(why),
        }
    }

    /// Settings › Account.
    pub(crate) fn settings_account(&mut self, ui: &mut egui::Ui) {
        use crate::settings::{group, note, row};
        self.take_account_news();
        let busy = self.account.busy.is_some();
        group(ui, "Account", true);
        if let Some(a) = self.account.signed_in() {
            // Who you are there: asked for once.
            if self.account.me.is_none() && !busy && self.account.error.is_none() {
                let device = a.device.clone();
                self.account_job(move |base| fetch_me(base, &device));
            }
            row(ui, "Signed in as", |ui, _| {
                ui.label(&a.email);
            });
            // (A server without screen names yet: nothing to show.)
            if let Some(me) = self
                .account
                .me
                .clone()
                .filter(|m| !m.screen_name.is_empty())
            {
                row(ui, "Screen name", |ui, lid| {
                    if self.account.editing {
                        let r = ui
                            .add_enabled(
                                !busy,
                                egui::TextEdit::singleline(&mut self.account.screen_name)
                                    .desired_width(200.0),
                            )
                            .labelled_by(lid);
                        if !r.has_focus() && !r.lost_focus() && !busy && self.account.focus_name {
                            r.request_focus();
                            self.account.focus_name = false;
                        }
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let sn = self.account.screen_name.trim().to_string();
                        let ok = !sn.is_empty() && sn != me.screen_name && !busy;
                        if (ui.add_enabled(ok, egui::Button::new("Save")).clicked() || enter) && ok
                        {
                            let device = a.device.clone();
                            self.account.welcome = false;
                            self.account_job(move |base| change_name(base, &device, &sn));
                        }
                        if ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked() {
                            self.account.editing = false;
                            self.account.screen_name = me.screen_name.clone();
                            self.account.error = None;
                        }
                    } else {
                        ui.label(&me.screen_name);
                        if ui.add_enabled(!busy, egui::Button::new("Change")).clicked() {
                            self.account.editing = true;
                            self.account.focus_name = true;
                            self.account.screen_name = me.screen_name.clone();
                            self.account.error = None;
                            self.account.notice = None;
                        }
                    }
                });
                note(
                    ui,
                    if self.account.welcome {
                        "We picked a screen name for you; change it to any that's free. It's how others see you on your shared designs."
                    } else {
                        "How others see you on your shared designs."
                    },
                );
            }
            row(ui, "", |ui, _| {
                if ui
                    .add_enabled(!busy, egui::Button::new("Sign Out"))
                    .clicked()
                {
                    let device = a.device.clone();
                    self.account_job(move |base| sign_out(base, &device));
                }
            });
        } else {
            match self.account.step.clone() {
                Step::Email => {
                    let mut send = false;
                    row(ui, "Email", |ui, lid| {
                        let r = ui
                            .add_enabled(
                                !busy,
                                egui::TextEdit::singleline(&mut self.account.email)
                                    .hint_text("you@example.com")
                                    .desired_width(220.0),
                            )
                            .labelled_by(lid);
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let ok = self.account.email.contains('@') && !busy;
                        send = (ui.add_enabled(ok, egui::Button::new("Sign In")).clicked()
                            || enter)
                            && ok;
                    });
                    note(
                        ui,
                        "Sign in to change your shared designs from any computer. We'll email you a code; no password.",
                    );
                    if send {
                        let email = self.account.email.trim().to_string();
                        self.account_job(move |base| start(base, &email));
                    }
                }
                Step::Code(email) => {
                    let mut go = false;
                    row(ui, "Code", |ui, lid| {
                        let r = ui
                            .add_enabled(
                                !busy,
                                egui::TextEdit::singleline(&mut self.account.code)
                                    .hint_text("123456")
                                    .char_limit(7)
                                    .desired_width(90.0),
                            )
                            .labelled_by(lid);
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let digits = self
                            .account
                            .code
                            .chars()
                            .filter(char::is_ascii_digit)
                            .count();
                        let ok = digits == 6 && !busy;
                        go = (ui.add_enabled(ok, egui::Button::new("Continue")).clicked() || enter)
                            && ok;
                        if ui
                            .add_enabled(!busy, egui::Button::new("Use Another Email"))
                            .clicked()
                        {
                            self.account.step = Step::Email;
                            self.account.error = None;
                        }
                    });
                    note(
                        ui,
                        &format!("We sent a 6-digit code to {email}. It works for 10 minutes."),
                    );
                    if go {
                        let code: String = self
                            .account
                            .code
                            .chars()
                            .filter(char::is_ascii_digit)
                            .collect();
                        let guest = self.share.store().guest.clone();
                        self.account_job(move |base| verify(base, &email, &code, guest.as_deref()));
                    }
                }
            }
        }
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                crate::theme::caption(ui, "Talking to zigzag.diy…");
            });
        } else if let Some(e) = &self.account.error {
            let c = crate::theme::pal_ui(ui).error;
            ui.colored_label(c, e);
        } else if let Some(n) = &self.account.notice {
            crate::theme::caption(ui, n.clone());
        }
    }
}
