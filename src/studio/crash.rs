//! Crash logs: when something inside ZigZag fails badly enough to stop it
//! (a Rust panic), what failed and where is written to a text file in the
//! settings folder (`crashes/`). The next time ZigZag opens it says so and
//! offers to report it: a GitHub issue started with the gist of it, and the
//! file shown so it can be attached. Nothing is sent anywhere by itself.
//!
//! Failures ZigZag carries on from (a shape the planner couldn't stitch)
//! are logged the same way, once each, without the prompt.
//!
//! A crash outside Rust (the system ending the app) isn't seen here; on a
//! Mac those are in Console's own crash reports.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How many logs are kept (the oldest go first).
const KEEP: usize = 10;
/// Names the newest log of a crash nobody has been told about yet.
const MARKER: &str = "unreported";
/// How much of a log goes into the address of a new issue: addresses
/// can't be long, and the whole file can be attached.
const IN_ISSUE: usize = 3_000;

/// Where the logs are (`None`: there's no settings folder).
pub fn dir() -> Option<PathBuf> {
    crate::dock::config_file("crashes")
}

/// People's names out of paths (`/Users/quentin/…` becomes `~/…`): a log may
/// be posted in public.
fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'scan: while !rest.is_empty() {
        for home in ["/Users/", "/home/", "\\Users\\"] {
            if let Some(after) = rest.strip_prefix(home) {
                let end = after
                    .find(['/', '\\', ':', '"', ' ', '\n'])
                    .unwrap_or(after.len());
                out.push('~');
                rest = &after[end..];
                continue 'scan;
            }
        }
        let mut chars = rest.chars();
        out.extend(chars.next());
        rest = chars.as_str();
    }
    out
}

/// A log's text.
fn text(
    when: SystemTime,
    quit: bool,
    thread: &str,
    message: &str,
    location: &str,
    backtrace: &str,
) -> String {
    scrub(&format!(
        "ZigZag {} on {} ({})\n{}\n{}\n\nWhat failed: {message}\nWhere: {location}\nThread: {thread}\n\nBacktrace:\n{backtrace}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        crate::agent_log::iso_utc(when),
        if quit {
            "ZigZag stopped."
        } else {
            "ZigZag carried on without this piece."
        },
    ))
}

/// Write a log into `dir` (and, for a crash, note that it hasn't been
/// reported), keeping only the newest few. Returns the file.
fn write(dir: &Path, when: SystemTime, quit: bool, log: &str) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let stamp = crate::agent_log::iso_utc(when).replace(':', "-");
    let kind = if quit { "crash" } else { "error" };
    let mut file = dir.join(format!("{kind}-{stamp}.txt"));
    let mut n = 2;
    while file.exists() {
        file = dir.join(format!("{kind}-{stamp}-{n}.txt"));
        n += 1;
    }
    std::fs::write(&file, log).ok()?;
    if quit {
        let _ = std::fs::write(
            dir.join(MARKER),
            file.file_name()?.to_string_lossy().as_bytes(),
        );
    }
    // The oldest out (names sort by time).
    let mut logs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    logs.sort_by(|a, b| {
        let key = |p: &PathBuf| {
            p.file_name().map(|n| {
                n.to_string_lossy()
                    .split_once('-')
                    .map_or(String::new(), |x| x.1.to_string())
            })
        };
        key(a).cmp(&key(b))
    });
    for old in logs.iter().rev().skip(KEEP) {
        let _ = std::fs::remove_file(old);
    }
    Some(file)
}

/// The log of a crash nobody has been told about, if there is one.
pub fn unreported(dir: &Path) -> Option<PathBuf> {
    let name = std::fs::read_to_string(dir.join(MARKER)).ok()?;
    let file = dir.join(name.trim());
    file.is_file().then_some(file)
}

/// That crash has been dealt with (reported, or not): don't ask again.
pub fn settle(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(MARKER));
}

/// From now on, a panic leaves a log. Call once, at startup.
pub fn install() {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // (What it always did: the message on stderr.)
        before(info);
        let Some(dir) = dir() else { return };
        let quit = !zigzag::design::catching();
        let location = info
            .location()
            .map_or("unknown".to_string(), |l| l.to_string());
        if !quit {
            // One log for each place that fails, not one every time it's
            // tried again (every frame of a drag, say).
            use std::sync::Mutex;
            static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
            let Ok(mut seen) = SEEN.lock() else { return };
            if seen.contains(&location) {
                return;
            }
            seen.push(location.clone());
        }
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".to_string());
        let thread = std::thread::current();
        let now = SystemTime::now();
        let log = text(
            now,
            quit,
            thread.name().unwrap_or("unnamed"),
            &message,
            &location,
            &std::backtrace::Backtrace::force_capture().to_string(),
        );
        if let Some(file) = write(&dir, now, quit, &log) {
            eprintln!("ZigZag: this was written to {}", file.display());
        }
    }));
}

/// A new issue on GitHub about a crash, started with the gist of its log.
pub fn issue_url(log: &str) -> String {
    let mut gist: String = log.chars().take(IN_ISSUE).collect();
    if gist.len() < log.len() {
        gist.push_str("\n… (the rest is in the attached log)");
    }
    let body = format!(
        "**What you were doing when ZigZag stopped**\n\n\n**The crash log** (please also drag the log file in here)\n\n```\n{gist}\n```\n"
    );
    let mut url = String::from(
        "https://github.com/simpsoka/zigzag/issues/new?title=ZigZag%20stopped%20unexpectedly&body=",
    );
    for b in body.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                url.push(b as char)
            }
            _ => url.push_str(&format!("%{b:02X}")),
        }
    }
    url
}

/// Show a file where it is: selected in Finder or Explorer, or its folder.
pub fn reveal(path: &Path) {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    let _ = cmd.spawn();
}

impl crate::app::Studio {
    /// Help › Show Crash Logs: the folder they're in.
    pub fn show_crash_logs(&mut self) {
        match dir() {
            Some(d) => {
                let _ = std::fs::create_dir_all(&d);
                crate::sew::open_url(&d.to_string_lossy());
                self.status = format!("Crash logs are kept in {}.", d.display());
            }
            None => self.status = "There's no settings folder to keep crash logs in.".into(),
        }
    }

    /// "ZigZag stopped last time": report it, look at the log, or neither.
    pub(crate) fn crash_window(&mut self, ctx: &eframe::egui::Context) {
        use crate::dialog;
        use eframe::egui;
        let Some(file) = self.crash.clone() else {
            return;
        };
        let (mut report, mut later, mut show) = (false, false, false);
        dialog::show(
            ctx,
            egui::Id::unique("crash"),
            dialog::Place::Center,
            |ui| {
                dialog::heading(ui, "ZigZag stopped unexpectedly last time");
                dialog::body(
                    ui,
                    "It kept a short log of what went wrong. Reporting it helps get the problem fixed: a GitHub issue opens with the gist of the log already in it, and the log file is shown so you can attach it. Nothing is sent unless you post it.",
                );
                dialog::buttons(ui, |ui| {
                    report = dialog::primary(ui, "Report on GitHub…", true).clicked();
                    later = dialog::secondary(ui, "Not Now").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape));
                    show = dialog::apart(ui, |ui| dialog::secondary(ui, "Show Log").clicked());
                });
            },
        );
        if show {
            crate::agent_log::open_text(&file);
        }
        if report {
            let log = std::fs::read_to_string(&file).unwrap_or_default();
            crate::sew::open_url(&issue_url(&log));
            reveal(&file);
        }
        if report || later {
            if let Some(d) = file.parent() {
                settle(d);
            }
            self.crash = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zigzag-crash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_log_says_what_failed_without_saying_whose_computer() {
        let log = text(
            UNIX_EPOCH + Duration::from_secs(1_794_600_000),
            true,
            "main",
            "index out of bounds: the len is 3 but the index is 7",
            "/Users/quentin/.cargo/registry/src/x/kurbo-0.11/src/bezpath.rs:10:5",
            "0: zigzag::design::plan\n   at /home/ximena/zigzag/src/design.rs:1\n1: C:\\Users\\Zoltan\\zigzag\\src\\main.rs",
        );
        assert!(log.starts_with(&format!("ZigZag {} on ", env!("CARGO_PKG_VERSION"))));
        assert!(log.contains("2026-11-1"), "{log}");
        assert!(log.contains("ZigZag stopped."));
        assert!(log.contains("What failed: index out of bounds: the len is 3 but the index is 7"));
        assert!(log.contains("Where: ~/.cargo/registry/src/x/kurbo-0.11/src/bezpath.rs:10:5"));
        assert!(log.contains("at ~/zigzag/src/design.rs:1"));
        assert!(log.contains("1: C:~\\zigzag\\src\\main.rs"));
        for name in ["quentin", "ximena", "Zoltan"] {
            assert!(!log.contains(name), "{name} in {log}");
        }
        // One that was handled says so.
        let log = text(UNIX_EPOCH, false, "planner", "x", "src/design.rs:1:1", "");
        assert!(log.contains("ZigZag carried on without this piece."));
    }

    #[test]
    fn a_crash_is_remembered_until_its_dealt_with_and_old_logs_go() {
        let d = scratch("logs");
        assert_eq!(unreported(&d), None);
        let at = |k: u64| UNIX_EPOCH + Duration::from_secs(1_794_600_000 + k);
        // Something handled: a log, and nothing to report.
        let e = write(&d, at(0), false, "handled").unwrap();
        assert!(
            e.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("error-2026-")
        );
        assert_eq!(unreported(&d), None);
        // A crash: its log is the one to report, until that's settled.
        let c = write(&d, at(1), true, "crashed").unwrap();
        assert_eq!(std::fs::read_to_string(&c).unwrap(), "crashed");
        assert_eq!(unreported(&d), Some(c.clone()));
        // (Two in the same second don't write over each other.)
        let c2 = write(&d, at(1), true, "again").unwrap();
        assert_ne!(c, c2);
        assert_eq!(unreported(&d), Some(c2));
        settle(&d);
        assert_eq!(unreported(&d), None);
        settle(&d);
        // Only the newest few are kept.
        for k in 2..20 {
            write(&d, at(k * 60), k % 2 == 0, "more").unwrap();
        }
        let logs = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
            .count();
        assert_eq!(logs, KEEP);
        assert!(!e.exists() && !c.exists(), "the oldest went");
        assert!(unreported(&d).is_some_and(|f| f.exists()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_issue_starts_with_the_gist_of_the_log() {
        let url = issue_url("What failed: a & b\nWhere: src/x.rs:1:2\n");
        assert!(url.starts_with("https://github.com/simpsoka/zigzag/issues/new?title="));
        assert!(url.contains("What%20failed%3A%20a%20%26%20b%0AWhere%3A%20src%2Fx.rs%3A1%3A2"));
        assert!(!url.contains("the%20rest%20is%20in"));
        // A long log is cut to fit an address; the file has the rest.
        let long = "frame\n".repeat(5_000);
        let url = issue_url(&long);
        assert!(url.len() < 16_000, "{}", url.len());
        assert!(url.contains("the%20rest%20is%20in%20the%20attached%20log"));
    }
}
