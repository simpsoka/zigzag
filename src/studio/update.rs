//! Updates: ZigZag looks at its latest release on GitHub, says when
//! there's a newer one, and (on a Mac) installs it and opens it again.
//!
//! Nothing about how a release is made changes: it reads the release as
//! `packaging/macos/publish-release.sh` publishes it (the tag `vX.Y.Z`, the
//! notes, `ZigZag-X.Y.Z.dmg` and `SHA256SUMS.txt`). There's no feed to
//! keep and no key to look after.
//!
//! What makes an update safe to install is the same thing that makes the
//! app safe to open: Apple's code signature. The download has to match the
//! release's checksum (that it arrived whole), and the app inside has to be
//! signed, intact, by the same Developer ID team as the copy that's
//! running (that it's ZigZag's). A copy that isn't signed that way (one
//! built from source) can't tell, so it says there's an update and where
//! to get it, and leaves the installing to you.
//!
//! The new app is put beside the old one and the two change places in
//! one step, so there's never half an app: if anything fails before that,
//! the old one is untouched, and if the change itself fails it's put
//! back. Then the old one waits for itself to quit and opens the new.
//!
//! The work is done by the system's own tools (`curl`, `hdiutil`,
//! `codesign`, `ditto`), which is why there's nothing to add to the build:
//! they use the Mac's certificates and proxy settings as a browser would.
//!
//! On Windows and Linux it says there's an update and opens the download:
//! swapping a running program there wants testing on those machines first.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Where the latest release is described (GitHub's API), and where a
/// person downloads it.
const LATEST: &str = "https://api.github.com/repos/simpsoka/zigzag/releases/latest";
pub const DOWNLOAD_PAGE: &str = "https://github.com/simpsoka/zigzag/releases/latest";

pub type Version = [u32; 3];

/// `0.1.3`, or the tag `v0.1.3`. Nothing else: a release that isn't a
/// plain version (a beta) isn't one to update to.
pub fn parse_version(s: &str) -> Option<Version> {
    let mut parts = s.trim().trim_start_matches('v').split('.');
    let mut v = [0u32; 3];
    for slot in &mut v {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    parts.next().is_none().then_some(v)
}

/// The version that's running.
pub fn this_version() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).unwrap_or([0, 0, 0])
}

pub fn version_name(v: Version) -> String {
    format!("{}.{}.{}", v[0], v[1], v[2])
}

/// A file of a release.
#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    /// What's new, as the release says it (Markdown).
    pub notes: String,
    /// Its page on GitHub.
    pub page: String,
    /// The download for this kind of computer, if the release has one
    /// (the Windows one is added a few minutes after the Mac one).
    pub asset: Option<Asset>,
    /// Its checksums file.
    pub sums: Option<String>,
}

/// The release in GitHub's answer, with the download for `os`
/// (`std::env::consts::OS`). `None` for a draft, a prerelease or a tag
/// that isn't a version.
pub fn release_from_json(v: &Value, os: &str) -> Option<Release> {
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return None;
    }
    let version = parse_version(v["tag_name"].as_str()?)?;
    let name = version_name(version);
    let (want, sums) = match os {
        "macos" => (format!("ZigZag-{name}.dmg"), "SHA256SUMS.txt"),
        "windows" => (
            format!("ZigZag-{name}-x86_64-windows.zip"),
            "SHA256SUMS-windows.txt",
        ),
        _ => (String::new(), ""),
    };
    let assets = v["assets"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let find = |name: &str| {
        assets
            .iter()
            .find(|a| a["name"].as_str() == Some(name))
            .and_then(|a| {
                Some(Asset {
                    name: name.to_string(),
                    url: a["browser_download_url"].as_str()?.to_string(),
                    size: a["size"].as_u64().unwrap_or(0),
                })
            })
    };
    Some(Release {
        version,
        notes: v["body"].as_str().unwrap_or("").trim().to_string(),
        page: v["html_url"].as_str().unwrap_or(DOWNLOAD_PAGE).to_string(),
        asset: find(&want),
        sums: find(sums).map(|a| a.url),
    })
}

/// The checksum a `shasum`-style list gives for the file called `name`.
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (sum, file) = line.split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*');
        (file == name && sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| sum.to_ascii_lowercase())
    })
}

/// Where the updater is.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Nothing known yet.
    Idle,
    Checking,
    /// This is the newest there is.
    UpToDate,
    Available(Release),
    Downloading {
        release: Release,
        got: u64,
    },
    Installing(Release),
    /// The new app is in place: quit, and it opens.
    Relaunch,
    /// It couldn't be done. With a release, downloading it by hand is
    /// still the way.
    Failed {
        release: Option<Release>,
        why: String,
    },
}

/// Why an update can't be installed from here, when it's nothing going
/// wrong: said with the release, and the download offered instead.
fn by_hand(why: &str) -> String {
    format!("{why} Download it from GitHub instead.")
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run {:?}: {e}", cmd.get_program()))?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    if out.status.success() {
        Ok(text(&out.stdout))
    } else {
        Err(match text(&out.stderr) {
            e if e.is_empty() => format!("{:?} failed", cmd.get_program()),
            e => e,
        })
    }
}

fn curl() -> Command {
    let mut c = Command::new("curl");
    c.args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--connect-timeout", "20"])
        .args([
            "--user-agent",
            concat!("ZigZag/", env!("CARGO_PKG_VERSION")),
        ]);
    c
}

/// The latest release, asked of `feed`.
fn fetch(feed: &str) -> Result<Release, String> {
    let text = run(curl()
        .args(["--max-time", "30"])
        .args(["--header", "Accept: application/vnd.github+json"])
        .arg(feed))
    .map_err(|e| format!("Couldn't reach GitHub to look for updates ({e})."))?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|_| "GitHub's answer about the latest release couldn't be read.".to_string())?;
    release_from_json(&v, std::env::consts::OS)
        .ok_or_else(|| "The latest release on GitHub isn't one ZigZag can update to.".to_string())
}

/// The `.app` this is running from.
pub fn running_app() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.parent()?.parent()?.parent()?;
    (app.extension().is_some_and(|e| e == "app") && exe.parent()?.ends_with("Contents/MacOS"))
        .then(|| app.to_path_buf())
}

/// The Developer ID team an app is signed by, if it's signed by one.
fn team_of(app: &Path) -> Option<String> {
    let out = Command::new("codesign")
        .args(["--display", "--verbose=2"])
        .arg(app)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .find_map(|l| l.strip_prefix("TeamIdentifier="))
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "not set")
        .map(str::to_string)
}

fn plist(app: &Path, key: &str) -> Option<String> {
    run(Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", &format!("Print :{key}")])
        .arg(app.join("Contents/Info.plist")))
    .ok()
}

/// A folder that's emptied (and an image mounted there let go of) when
/// it's done with, however that comes about.
struct Scratch {
    dir: PathBuf,
    mounted: Option<PathBuf>,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(m) = &self.mounted {
            let _ = Command::new("hdiutil")
                .args(["detach", "-force", "-quiet"])
                .arg(m)
                .stdin(Stdio::null())
                .output();
        }
        // If both installing the new app and restoring the old one failed,
        // this is the only copy left. Keep it at the recovery path reported
        // to the user, while still detaching the downloaded image above.
        if !self.dir.join("ZigZag-old.app").exists() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Download `release`, check it, and put it in the place of `app`.
/// `unsigned`: take an update that isn't signed by a Developer ID team
/// (only ever for a copy of ZigZag that isn't either: a build from
/// source, to try the updater). `got` hears how much has arrived;
/// `cancelled` is asked whether to give up.
///
/// Returns the folder holding the old app, to be deleted once this copy
/// has quit.
pub fn install_mac(
    release: &Release,
    app: &Path,
    unsigned: bool,
    got: &dyn Fn(u64),
    cancelled: &dyn Fn() -> bool,
) -> Result<PathBuf, String> {
    let name = version_name(release.version);
    let (Some(asset), Some(sums)) = (&release.asset, &release.sums) else {
        return Err(by_hand(
            "This release doesn't have a download ZigZag can install by itself.",
        ));
    };
    let shown = app.to_string_lossy();
    if shown.contains("/AppTranslocation/") || shown.starts_with("/Volumes/") {
        return Err(by_hand(
            "ZigZag is running from where it was downloaded, not from Applications, so it can't replace itself. Move it to Applications first, or",
        ));
    }
    let home = app
        .parent()
        .ok_or("ZigZag couldn't tell where it's installed.")?;
    // What this copy is signed by decides what an update must be.
    let team = team_of(app);
    if team.is_none() && !unsigned {
        return Err(by_hand(
            "This copy of ZigZag wasn't signed by its developer (it was built from source), so it can't check that an update is the real thing.",
        ));
    }
    // Beside the app, so the two can change places in one step (and
    // finding out now whether there's permission to). Never overwrite
    // an earlier stage: it may hold the old app after a failed rollback.
    let stage = home.join(format!(".ZigZag-update-{}", std::process::id()));
    std::fs::create_dir(&stage).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            return by_hand(&format!(
                "Files from an earlier update are still at {}. Keep them until ZigZag has been recovered.",
                stage.display()
            ));
        }
        by_hand(&format!(
            "ZigZag can't write to {} ({e}), so it can't replace itself.",
            home.display()
        ))
    })?;
    let mut scratch = Scratch {
        dir: stage.clone(),
        mounted: None,
    };

    // 1. The download, and that it arrived whole.
    let dmg = stage.join(&asset.name);
    let mut child = curl()
        .args(["--retry", "2", "--output"])
        .arg(&dmg)
        .arg(&asset.url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't start the download ({e})."))?;
    let status = loop {
        if cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Cancelled.".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                got(std::fs::metadata(&dmg).map_or(0, |m| m.len()));
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            Err(e) => return Err(format!("The download stopped ({e}).")),
        }
    };
    if !status.success() {
        let mut why = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut e, &mut why);
        }
        return Err(format!(
            "The download didn't finish ({}).",
            why.trim().trim_start_matches("curl: ")
        ));
    }
    let listed = run(curl().args(["--max-time", "30"]).arg(sums))
        .ok()
        .and_then(|text| sum_for(&text, &asset.name))
        .ok_or("The release's checksums couldn't be read, so the download can't be checked.")?;
    let sum = run(Command::new("shasum").args(["-a", "256"]).arg(&dmg))
        .ok()
        .and_then(|out| out.split_whitespace().next().map(str::to_ascii_lowercase))
        .ok_or("The download couldn't be checked.")?;
    if sum != listed {
        return Err("The download doesn't match the release's checksum. Nothing was changed; try again later.".into());
    }
    if cancelled() {
        return Err("Cancelled.".into());
    }

    // 2. Open the disk image, and check the app in it is ZigZag's: signed,
    // intact, by the team this copy is signed by, and the version said.
    let mount = stage.join("image");
    std::fs::create_dir(&mount).map_err(|e| format!("Couldn't open the download ({e})."))?;
    run(Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-quiet"])
        .arg("-mountpoint")
        .arg(&mount)
        .arg(&dmg))
    .map_err(|e| format!("Couldn't open the download ({e})."))?;
    scratch.mounted = Some(mount.clone());
    let fresh = mount.join("ZigZag.app");
    let genuine = |app: &Path| -> Result<(), String> {
        run(Command::new("codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(app))
        .map_err(|e| {
            format!("The update's signature isn't valid, so it wasn't installed ({e}).")
        })?;
        match (&team, team_of(app)) {
            (Some(ours), Some(theirs)) if *ours == theirs => Ok(()),
            (None, _) if unsigned => Ok(()),
            _ => Err(
                "The update isn't signed by ZigZag's developer, so it wasn't installed."
                    .to_string(),
            ),
        }
    };
    genuine(&fresh)?;
    if plist(&fresh, "CFBundleShortVersionString").as_deref() != Some(name.as_str())
        || plist(&fresh, "CFBundleIdentifier") != plist(app, "CFBundleIdentifier")
    {
        return Err(format!(
            "The download isn't ZigZag {name}, so it wasn't installed."
        ));
    }

    // 3. A copy of it beside the app that's running, checked again (a
    // copy can go wrong too).
    let new = stage.join("ZigZag.app");
    run(Command::new("ditto").arg(&fresh).arg(&new))
        .map_err(|e| format!("Couldn't copy the update into place ({e})."))?;
    genuine(&new)?;
    if cancelled() {
        return Err("Cancelled.".into());
    }

    // 4. The two change places. From here the old app is kept (in the
    // staging folder) until the new one is in: if that fails, it goes
    // back.
    let old = stage.join("ZigZag-old.app");
    std::fs::rename(app, &old).map_err(|e| {
        by_hand(&format!(
            "ZigZag couldn't move itself aside ({e}), so it can't replace itself."
        ))
    })?;
    if let Err(e) = std::fs::rename(&new, app) {
        let back = std::fs::rename(&old, app);
        return Err(match back {
            Ok(()) => format!("Couldn't put the update in place ({e}). Nothing was changed."),
            Err(_) => format!(
                "Couldn't put the update in place ({e}), and the old ZigZag is now at {}. Move it back to {}.",
                old.display(),
                app.display()
            ),
        });
    }
    // (The image is let go of here; the folder, with the old app in it,
    // stays until this copy has quit.)
    if let Some(m) = scratch.mounted.take() {
        let _ = Command::new("hdiutil")
            .args(["detach", "-force", "-quiet"])
            .arg(&m)
            .stdin(Stdio::null())
            .output();
    }
    let _ = std::fs::remove_file(&dmg);
    std::mem::forget(scratch);
    Ok(stage)
}

/// Once the process `pid` (this copy) has quit: clear away what's left
/// of it, and open the app at `app` (with `opener`: `open`).
fn relaunch_after_quit(pid: u32, app: &Path, leftovers: &Path, opener: &str) -> Result<(), String> {
    // (Paths go in as arguments, not into the script: no quoting to get
    // wrong.)
    let script = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; rm -rf "$3"; "$4" "$2""#;
    Command::new("/bin/sh")
        .args(["-c", script, "zigzag-update"])
        .arg(pid.to_string())
        .arg(app)
        .arg(leftovers)
        .arg(opener)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| {
            format!("The update is installed, but ZigZag couldn't reopen itself ({e}). Quit and open it again.")
        })
}

/// The updater: what it knows, and the thread doing its work.
pub struct Updater {
    status: Arc<Mutex<Status>>,
    cancel: Arc<AtomicBool>,
    /// The update window is showing.
    pub open: bool,
    /// When it last looked.
    pub checked: Option<std::time::Instant>,
}

impl Default for Updater {
    fn default() -> Self {
        Updater {
            status: Arc::new(Mutex::new(Status::Idle)),
            cancel: Arc::new(AtomicBool::new(false)),
            open: false,
            checked: None,
        }
    }
}

/// Where to ask: GitHub, unless `ZIGZAG_UPDATE_FEED` names somewhere else
/// (for trying the updater against a release that isn't out).
fn feed() -> String {
    std::env::var("ZIGZAG_UPDATE_FEED").unwrap_or_else(|_| LATEST.to_string())
}

impl Updater {
    pub fn status(&self) -> Status {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or(Status::Idle)
    }

    fn set(status: &Arc<Mutex<Status>>, to: Status, ctx: &eframe::egui::Context) {
        if let Ok(mut s) = status.lock() {
            *s = to;
        }
        ctx.request_repaint();
    }

    /// The newer release, when one's known of.
    pub fn available(&self) -> Option<Release> {
        match self.status() {
            Status::Available(r)
            | Status::Downloading { release: r, .. }
            | Status::Installing(r) => Some(r),
            Status::Failed {
                release: Some(r), ..
            } => Some(r),
            _ => None,
        }
    }

    /// Look for a newer release, unless one's being looked for or
    /// installed. `asked`: the person asked (the answer is shown either
    /// way); otherwise only a newer release is.
    pub fn check(&mut self, ctx: &eframe::egui::Context, asked: bool) {
        if matches!(
            self.status(),
            Status::Checking
                | Status::Downloading { .. }
                | Status::Installing(_)
                | Status::Relaunch
        ) {
            self.open |= asked;
            return;
        }
        self.open |= asked;
        self.checked = Some(std::time::Instant::now());
        let (status, ctx) = (self.status.clone(), ctx.clone());
        Self::set(&status, Status::Checking, &ctx);
        std::thread::spawn(move || {
            let to = match fetch(&feed()) {
                Ok(r) if r.version > this_version() => Status::Available(r),
                Ok(_) => Status::UpToDate,
                Err(why) => Status::Failed { release: None, why },
            };
            Self::set(&status, to, &ctx);
        });
    }

    /// Install the release that's available, and say when to quit.
    pub fn install(&mut self, ctx: &eframe::egui::Context) {
        let Some(release) = self.available() else {
            return;
        };
        if matches!(
            self.status(),
            Status::Downloading { .. } | Status::Installing(_) | Status::Relaunch
        ) {
            return;
        }
        self.cancel.store(false, Ordering::Relaxed);
        let (status, cancel, ctx) = (self.status.clone(), self.cancel.clone(), ctx.clone());
        Self::set(
            &status,
            Status::Downloading {
                release: release.clone(),
                got: 0,
            },
            &ctx,
        );
        std::thread::spawn(move || {
            let failed = |why: String| Status::Failed {
                release: Some(release.clone()),
                why,
            };
            let to = match running_app() {
                None => failed(by_hand(
                    "ZigZag isn't running as an app here, so it can't replace itself.",
                )),
                Some(app) => {
                    let total = release.asset.as_ref().map_or(0, |a| a.size);
                    let progress = |got: u64| {
                        // (All of it here: what's left is checking and
                        // putting it in place.)
                        let to = if total > 0 && got >= total {
                            Status::Installing(release.clone())
                        } else {
                            Status::Downloading {
                                release: release.clone(),
                                got,
                            }
                        };
                        Self::set(&status, to, &ctx);
                    };
                    let unsigned = std::env::var_os("ZIGZAG_UPDATE_UNSIGNED").is_some();
                    install_mac(&release, &app, unsigned, &progress, &|| {
                        cancel.load(Ordering::Relaxed)
                    })
                    .and_then(|leftovers| {
                        relaunch_after_quit(std::process::id(), &app, &leftovers, "open")
                    })
                    .map_or_else(failed, |()| Status::Relaunch)
                }
            };
            // (Cancelled: back to the offer, nothing to say.)
            let to = match to {
                Status::Failed {
                    release: Some(r),
                    why,
                } if why == "Cancelled." => Status::Available(r),
                to => to,
            };
            Self::set(&status, to, &ctx);
        });
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Put a status in by hand (the scripted screenshots).
    pub fn show(&mut self, status: Status) {
        if let Ok(mut s) = self.status.lock() {
            *s = status;
        }
    }
}

/// Release notes, read enough to show: headings, bullets (with the lines
/// that continue them) and paragraphs. The release's own title is left
/// out (the window says which release it is).
#[derive(Debug, PartialEq)]
pub enum Note {
    Heading(String),
    Bullet(String),
    Text(String),
}

pub fn read_notes(markdown: &str) -> Vec<Note> {
    let plain = |s: &str| s.replace("**", "").replace('`', "");
    let mut out: Vec<Note> = Vec::new();
    let mut open = false;
    for line in markdown.lines() {
        let t = line.trim();
        if t.is_empty() {
            open = false;
        } else if let Some(h) = t.strip_prefix('#') {
            // (`# ZigZag 0.1.4`: the title.)
            if h.starts_with('#') {
                out.push(Note::Heading(plain(h.trim_start_matches('#').trim())));
            }
            open = false;
        } else if let Some(b) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            out.push(Note::Bullet(plain(b)));
            open = true;
        } else {
            match out.last_mut() {
                Some(Note::Bullet(s) | Note::Text(s)) if open => {
                    s.push(' ');
                    s.push_str(&plain(t));
                }
                _ => {
                    out.push(Note::Text(plain(t)));
                    open = true;
                }
            }
        }
    }
    out
}

/// How much of a download has arrived: `12.3 of 25.0 MB`.
fn megabytes(got: u64, of: u64) -> String {
    let mb = |b: u64| b as f64 / 1_000_000.0;
    if of > 0 {
        format!("{:.1} of {:.1} MB", mb(got.min(of)), mb(of))
    } else {
        format!("{:.1} MB", mb(got))
    }
}

/// How often it looks, with ZigZag left open.
const EVERY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

impl crate::app::Studio {
    /// Look for an update because it's time to (at launch, and once a
    /// day after), if the settings say to.
    pub(crate) fn update_tick(&mut self, ctx: &eframe::egui::Context) {
        if !self.updates_on || !self.prefs.auto_update {
            return;
        }
        if self.updater.checked.is_none_or(|t| t.elapsed() >= EVERY) {
            self.updater.check(ctx, false);
        }
    }

    /// The toolbar's Update button: there when there's a newer release.
    pub(crate) fn update_button(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui;
        let Some(release) = self.updater.available() else {
            return;
        };
        let p = crate::theme::pal_ui(ui);
        let label = match self.updater.status() {
            Status::Downloading { .. } | Status::Installing(_) => "Updating…",
            _ => "Update",
        };
        let b = ui
            .add(
                egui::Button::new(
                    egui::RichText::new(label)
                        .size(12.0)
                        .color(egui::Color32::WHITE),
                )
                .fill(p.accent)
                .corner_radius(11)
                .min_size(egui::Vec2::new(0.0, 22.0)),
            )
            .on_hover_text(format!(
                "ZigZag {} is available (you have {})",
                version_name(release.version),
                env!("CARGO_PKG_VERSION")
            ));
        b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Update ZigZag"));
        if b.clicked() {
            self.updater.open = true;
        }
        ui.add_space(crate::theme::space::XS);
    }

    /// The update window: what there is, how it's going, what went wrong.
    pub(crate) fn update_window(&mut self, ctx: &eframe::egui::Context) {
        use crate::dialog;
        use eframe::egui;
        let status = self.updater.status();
        // Installed: quit, and the new one opens (every tab comes back,
        // as after any quit).
        if status == Status::Relaunch {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // A check nobody asked for says nothing unless it finds one, and
        // then only in the toolbar.
        if !self.updater.open {
            return;
        }
        let here = env!("CARGO_PKG_VERSION");
        let can_install = cfg!(target_os = "macos");
        let (mut close, mut install, mut download, mut cancel, mut again) =
            (false, false, false, false, false);
        let modal = dialog::show(
            ctx,
            egui::Id::unique("update"),
            dialog::Place::Center,
            |ui| {
                ui.set_width(NOTES_W);
                match &status {
                    Status::Idle | Status::Checking => {
                        dialog::heading(ui, "Looking for updates…");
                        ui.horizontal(|ui| {
                            ui.spinner();
                            dialog::body(ui, "Asking GitHub for ZigZag's latest release.");
                        });
                        dialog::buttons(ui, |ui| {
                            close = dialog::secondary(ui, "Cancel").clicked();
                        });
                    }
                    Status::UpToDate => {
                        dialog::heading(ui, "ZigZag is up to date");
                        dialog::body(ui, &format!("{here} is the newest version."));
                        dialog::buttons(ui, |ui| {
                            close = dialog::primary(ui, "OK", true).clicked();
                        });
                    }
                    Status::Available(r) => {
                        dialog::heading(
                            ui,
                            &format!("ZigZag {} is available", version_name(r.version)),
                        );
                        dialog::body(ui, &format!("You have {here}."));
                        notes(ui, &r.notes);
                        dialog::buttons(ui, |ui| {
                            if can_install && r.asset.is_some() {
                                install = dialog::primary(ui, "Install and Relaunch", true)
                                    .on_hover_text("Downloads it, checks it's signed by ZigZag's developer, puts it in place of this one and opens it. Your open documents come back.")
                                    .clicked();
                            } else {
                                download = dialog::primary(ui, "Download", true)
                                    .on_hover_text("Opens the release on GitHub in your browser")
                                    .clicked();
                            }
                            close = dialog::secondary(ui, "Later").clicked();
                        });
                    }
                    Status::Downloading { release, got } => {
                        dialog::heading(
                            ui,
                            &format!("Downloading ZigZag {}…", version_name(release.version)),
                        );
                        let of = release.asset.as_ref().map_or(0, |a| a.size);
                        ui.add(
                            egui::ProgressBar::new(if of > 0 {
                                (*got as f32 / of as f32).clamp(0.0, 1.0)
                            } else {
                                0.0
                            })
                            .desired_height(6.0)
                            .desired_width(NOTES_W),
                        );
                        dialog::body(ui, &megabytes(*got, of));
                        dialog::buttons(ui, |ui| {
                            cancel = dialog::secondary(ui, "Cancel").clicked();
                        });
                    }
                    Status::Installing(r) => {
                        dialog::heading(
                            ui,
                            &format!("Installing ZigZag {}…", version_name(r.version)),
                        );
                        ui.horizontal(|ui| {
                            ui.spinner();
                            dialog::body(
                                ui,
                                "Checking it and putting it in place. ZigZag will reopen.",
                            );
                        });
                    }
                    Status::Relaunch => {
                        dialog::heading(ui, "Reopening ZigZag…");
                    }
                    Status::Failed {
                        release: Some(r),
                        why,
                    } => {
                        dialog::heading(
                            ui,
                            &format!("ZigZag {} wasn't installed", version_name(r.version)),
                        );
                        dialog::body(ui, why);
                        dialog::buttons(ui, |ui| {
                            download = dialog::primary(ui, "Download", true)
                                .on_hover_text("Opens the release on GitHub in your browser")
                                .clicked();
                            close = dialog::secondary(ui, "Close").clicked();
                            if can_install && r.asset.is_some() {
                                again = dialog::apart(ui, |ui| {
                                    dialog::secondary(ui, "Try Again").clicked()
                                });
                            }
                        });
                    }
                    Status::Failed { release: None, why } => {
                        dialog::heading(ui, "Couldn't look for updates");
                        dialog::body(ui, why);
                        dialog::buttons(ui, |ui| {
                            close = dialog::primary(ui, "OK", true).clicked();
                            again = dialog::secondary(ui, "Try Again").clicked();
                        });
                    }
                }
            },
        );
        if install || (again && self.updater.available().is_some()) {
            self.updater.install(ctx);
        } else if again {
            self.updater.check(ctx, true);
        }
        if download {
            let page = self
                .updater
                .available()
                .map_or(DOWNLOAD_PAGE.to_string(), |r| r.page);
            crate::sew::open_url(&page);
            self.updater.open = false;
        }
        if cancel {
            self.updater.cancel();
        }
        // (Closing the window while it downloads leaves it downloading:
        // the toolbar says so, and opens the window again.)
        if close || modal.should_close() {
            self.updater.open = false;
        }
    }
}

/// How wide the update window's text runs.
const NOTES_W: f32 = 440.0;

/// What's new, in a box that scrolls.
fn notes(ui: &mut eframe::egui::Ui, markdown: &str) {
    use eframe::egui;
    let read = read_notes(markdown);
    if read.is_empty() {
        return;
    }
    let p = crate::theme::pal_ui(ui);
    ui.add_space(crate::theme::space::S);
    egui::Frame::new()
        .fill(if p.dark { p.dock } else { p.header })
        .corner_radius(8)
        .inner_margin(egui::Margin::same(crate::theme::space::M as i8))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("update-notes")
                .max_height(260.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = crate::theme::space::XS + 2.0;
                    for (k, n) in read.iter().enumerate() {
                        match n {
                            Note::Heading(h) => {
                                if k > 0 {
                                    ui.add_space(crate::theme::space::S);
                                }
                                ui.label(
                                    egui::RichText::new(h)
                                        .font(crate::theme::semibold(13.0))
                                        .color(p.text),
                                );
                            }
                            Note::Bullet(b) => {
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = crate::theme::space::S;
                                    ui.label(egui::RichText::new("•").color(p.secondary));
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(b).size(12.5).color(p.text),
                                        )
                                        .wrap(),
                                    );
                                });
                            }
                            Note::Text(t) => {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(t).size(12.5).color(p.secondary),
                                    )
                                    .wrap(),
                                );
                            }
                        }
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cleanup_keeps_the_old_app_when_rollback_cannot_restore_it() {
        let dir = std::env::temp_dir().join(format!(
            "zigzag-update-recovery-test-{}",
            std::process::id()
        ));
        let old = dir.join("ZigZag-old.app");
        std::fs::create_dir_all(old.join("Contents")).unwrap();
        let original = old.join("Contents/Info.plist");
        std::fs::write(&original, b"original app").unwrap();

        drop(Scratch {
            dir: dir.clone(),
            mounted: None,
        });
        assert_eq!(std::fs::read(&original).unwrap(), b"original app");

        // Once the app has been restored, ordinary scratch cleanup works.
        std::fs::remove_dir_all(&old).unwrap();
        drop(Scratch {
            dir: dir.clone(),
            mounted: None,
        });
        assert!(!dir.exists());
    }

    #[test]
    fn versions_are_three_numbers_and_compare_as_numbers() {
        assert_eq!(parse_version("0.1.3"), Some([0, 1, 3]));
        assert_eq!(parse_version("v0.1.3"), Some([0, 1, 3]));
        assert_eq!(parse_version(" v12.0.40\n"), Some([12, 0, 40]));
        for odd in [
            "",
            "v",
            "0.1",
            "0.1.3.4",
            "0.1.x",
            "0.1.3-beta",
            "0..3",
            "1.-2.3",
        ] {
            assert_eq!(parse_version(odd), None, "{odd}");
        }
        // (Not as text: 0.10.0 is after 0.9.0.)
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        assert!(parse_version("1.0.0") > parse_version("0.99.99"));
        assert_eq!(version_name([0, 1, 3]), "0.1.3");
        assert!(this_version() > [0, 0, 0]);
    }

    /// GitHub's answer for a release as `publish-release.sh` and the
    /// Windows workflow leave it.
    fn github(tag: &str) -> Value {
        let v = tag.trim_start_matches('v');
        let at = |name: String| {
            json!({
                "name": name,
                "size": 25_000_000,
                "browser_download_url": format!("https://github.com/simpsoka/zigzag/releases/download/{tag}/{name}"),
            })
        };
        json!({
            "tag_name": tag,
            "name": format!("ZigZag {v}"),
            "draft": false,
            "prerelease": false,
            "html_url": format!("https://github.com/simpsoka/zigzag/releases/tag/{tag}"),
            "body": "# ZigZag\n\nWhat's new.\n",
            "assets": [
                at("SHA256SUMS-windows.txt".into()),
                at("SHA256SUMS.txt".into()),
                at(format!("ZigZag-{v}-x86_64-windows.zip")),
                at(format!("ZigZag-{v}.dmg")),
                at("ZigZag.dmg".into()),
            ],
        })
    }

    #[test]
    fn a_release_is_read_as_it_is_published() {
        let r = release_from_json(&github("v0.1.4"), "macos").unwrap();
        assert_eq!(r.version, [0, 1, 4]);
        assert_eq!(r.notes, "# ZigZag\n\nWhat's new.");
        assert_eq!(
            r.page,
            "https://github.com/simpsoka/zigzag/releases/tag/v0.1.4"
        );
        let a = r.asset.unwrap();
        assert_eq!((a.name.as_str(), a.size), ("ZigZag-0.1.4.dmg", 25_000_000));
        assert_eq!(
            a.url,
            "https://github.com/simpsoka/zigzag/releases/download/v0.1.4/ZigZag-0.1.4.dmg"
        );
        assert!(r.sums.unwrap().ends_with("/SHA256SUMS.txt"));
        // Windows has its own download and checksums; Linux has neither,
        // and is still told there's a release.
        let w = release_from_json(&github("v0.1.4"), "windows").unwrap();
        assert_eq!(w.asset.unwrap().name, "ZigZag-0.1.4-x86_64-windows.zip");
        assert!(w.sums.unwrap().ends_with("/SHA256SUMS-windows.txt"));
        let l = release_from_json(&github("v0.1.4"), "linux").unwrap();
        assert_eq!((l.asset, l.sums), (None, None));
        // The Mac release is out before the Windows build is attached: a
        // release, with nothing to install yet.
        let mut early = github("v0.1.4");
        early["assets"]
            .as_array_mut()
            .unwrap()
            .retain(|a| !a["name"].as_str().unwrap().contains("windows"));
        let w = release_from_json(&early, "windows").unwrap();
        assert_eq!((w.asset, w.sums), (None, None));
        // Not ones to update to: a draft, a prerelease, a tag that isn't
        // a version.
        for (key, tag) in [
            ("draft", "v0.1.4"),
            ("prerelease", "v0.1.4"),
            ("", "nightly"),
        ] {
            let mut v = github(tag);
            if !key.is_empty() {
                v[key] = json!(true);
            }
            assert_eq!(release_from_json(&v, "macos"), None, "{key} {tag}");
        }
    }

    #[test]
    fn a_checksum_is_found_by_the_files_name() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  ZigZag-0.1.4.dmg\n{b}  ZigZag.dmg\n");
        assert_eq!(sum_for(&sums, "ZigZag-0.1.4.dmg"), Some(a.clone()));
        assert_eq!(sum_for(&sums, "ZigZag.dmg"), Some(b.to_lowercase()));
        // (PowerShell's list for Windows, and the binary-mode star.)
        assert_eq!(
            sum_for(
                &format!("{a}  ZigZag-0.1.4-x86_64-windows.zip\r\n"),
                "ZigZag-0.1.4-x86_64-windows.zip"
            ),
            Some(a.clone())
        );
        assert_eq!(
            sum_for(&format!("{a} *ZigZag.dmg"), "ZigZag.dmg"),
            Some(a.clone())
        );
        // Not there, or not a checksum.
        assert_eq!(sum_for(&sums, "ZigZag-0.1.5.dmg"), None);
        assert_eq!(sum_for("abc  ZigZag.dmg", "ZigZag.dmg"), None);
        assert_eq!(sum_for("", "ZigZag.dmg"), None);
    }

    #[test]
    fn release_notes_are_read_as_headings_bullets_and_paragraphs() {
        let md = "# ZigZag 0.1.4\n\nFaster, and **two** fixes.\nMore on that.\n\n## Machines\n\n- The `Document` panel has a Machine menu. Choose yours\n  and its hoops lead the Hoop menu.\n- Another.\n\nA closing line.\n";
        assert_eq!(
            read_notes(md),
            [
                Note::Text("Faster, and two fixes. More on that.".into()),
                Note::Heading("Machines".into()),
                Note::Bullet(
                    "The Document panel has a Machine menu. Choose yours and its hoops lead the Hoop menu."
                        .into()
                ),
                Note::Bullet("Another.".into()),
                Note::Text("A closing line.".into()),
            ]
        );
        assert_eq!(read_notes(""), []);
        assert_eq!(megabytes(12_300_000, 25_000_000), "12.3 of 25.0 MB");
        assert_eq!(megabytes(1_500_000, 0), "1.5 MB");
    }

    #[cfg(unix)]
    #[test]
    fn the_old_copy_waits_for_itself_to_quit_then_clears_up_and_opens_the_new() {
        let dir = std::env::temp_dir().join(format!("zigzag-relaunch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let leftovers = dir.join("old");
        std::fs::create_dir_all(leftovers.join("ZigZag-old.app")).unwrap();
        // "Opening" the new app here is touching a file where it would be.
        let app = dir.join("it's opened.txt");
        // Something standing in for this copy, still running.
        let mut running = Command::new("sleep").arg("30").spawn().unwrap();
        relaunch_after_quit(running.id(), &app, &leftovers, "touch").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(700));
        assert!(leftovers.exists() && !app.exists(), "it waits");
        running.kill().unwrap();
        running.wait().unwrap();
        let done = (0..50).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            app.exists()
        });
        assert!(done, "then opens the new one");
        assert!(!leftovers.exists(), "with the old one cleared away");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// GitHub's real answer, read by the real code (asks the network).
    #[test]
    #[ignore = "asks GitHub"]
    fn reads_the_latest_release_on_github() {
        let r = fetch(LATEST).unwrap();
        assert!(r.version >= [0, 1, 2]);
        assert!(
            r.page
                .starts_with("https://github.com/simpsoka/zigzag/releases/")
        );
        assert!(!r.notes.is_empty());
        if cfg!(target_os = "macos") {
            let a = r.asset.unwrap();
            assert_eq!(a.name, format!("ZigZag-{}.dmg", version_name(r.version)));
            assert!(a.size > 1_000_000 && a.url.starts_with("https://github.com/"));
            // (And its checksums are where they're looked for.)
            let sums = run(curl().args(["--max-time", "30"]).arg(r.sums.unwrap())).unwrap();
            assert!(sum_for(&sums, &a.name).is_some(), "{sums}");
        }
    }

    /// The whole of an install, against a release made on this Mac (see
    /// `packaging/macos/test-update.sh`, which makes one and runs this):
    /// `ZIGZAG_TEST_APP` is the app to update, `ZIGZAG_TEST_FEED` the
    /// file describing the release.
    #[test]
    #[ignore = "needs a release made by packaging/macos/test-update.sh"]
    fn installs_a_release_made_here() {
        let app = PathBuf::from(std::env::var("ZIGZAG_TEST_APP").unwrap());
        let feed = std::env::var("ZIGZAG_TEST_FEED").unwrap();
        let release = fetch(&feed).unwrap();
        assert!(release.version > this_version());
        let name = version_name(release.version);
        let version = |app: &Path| plist(app, "CFBundleShortVersionString").unwrap();
        let before = version(&app);
        assert_ne!(before, name);

        // Not signed by a developer, and not told to take it anyway: left
        // alone.
        let why = install_mac(&release, &app, false, &|_| {}, &|| false).unwrap_err();
        assert!(why.contains("wasn't signed by its developer"), "{why}");
        assert_eq!(version(&app), before);

        // Cancelled as the download begins: left alone, nothing beside it.
        let why = install_mac(&release, &app, true, &|_| {}, &|| true).unwrap_err();
        assert_eq!(why, "Cancelled.");
        assert_eq!(version(&app), before);
        let beside = |app: &Path| {
            std::fs::read_dir(app.parent().unwrap())
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(".ZigZag-update"))
                .count()
        };
        assert_eq!(beside(&app), 0);

        // A download that doesn't match the checksum: left alone.
        if let Ok(bad) = std::env::var("ZIGZAG_TEST_BAD_FEED") {
            let tampered = fetch(&bad).unwrap();
            let why = install_mac(&tampered, &app, true, &|_| {}, &|| false).unwrap_err();
            assert!(
                why.contains("doesn't match the release's checksum"),
                "{why}"
            );
            assert_eq!(version(&app), before);
            assert_eq!(beside(&app), 0);
        }

        // The real thing: downloaded (hearing how much), checked, and in
        // place, with the old one kept aside for now.
        let most = std::cell::Cell::new(0u64);
        let leftovers = install_mac(
            &release,
            &app,
            true,
            &|n| most.set(most.get().max(n)),
            &|| false,
        )
        .unwrap();
        assert_eq!(version(&app), name);
        assert_eq!(version(&leftovers.join("ZigZag-old.app")), before);
        run(Command::new("codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(&app))
        .unwrap();
        assert!(app.join("Contents/MacOS/zigzag-studio").exists());
        // (Nothing still mounted from it.)
        assert!(!leftovers.join("image/ZigZag.app").exists());
        std::fs::remove_dir_all(&leftovers).unwrap();
        assert_eq!(beside(&app), 0);
    }

    #[test]
    fn an_update_that_cant_be_installed_here_says_to_download_it() {
        // No download in the release for this computer.
        let mut r = release_from_json(&github("v9.9.9"), "linux").unwrap();
        let no = |r: &Release, app: &Path, unsigned: bool| {
            install_mac(r, app, unsigned, &|_| {}, &|| false).unwrap_err()
        };
        let why = no(&r, Path::new("/Applications/ZigZag.app"), false);
        assert!(why.contains("Download it from GitHub"), "{why}");
        // Running from the disk image, or from where Gatekeeper put it.
        r = release_from_json(&github("v9.9.9"), "macos").unwrap();
        for at in [
            "/Volumes/ZigZag 0.1.3/ZigZag.app",
            "/private/var/folders/x/AppTranslocation/ABC/d/ZigZag.app",
        ] {
            let why = no(&r, Path::new(at), false);
            assert!(why.contains("Move it to Applications"), "{why}");
        }
        // A copy that isn't signed by its developer (as built from
        // source: this test's own folder stands in for one).
        let dir = std::env::temp_dir().join(format!("zigzag-update-test-{}", std::process::id()));
        let app = dir.join("ZigZag.app");
        std::fs::create_dir_all(&app).unwrap();
        let why = no(&r, &app, false);
        assert!(why.contains("wasn't signed by its developer"), "{why}");
        assert!(app.exists(), "and it's left alone");
        assert!(
            std::fs::read_dir(&dir).unwrap().count() == 1,
            "with nothing left beside it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
