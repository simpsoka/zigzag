//! The agents' log: every tool call an agent makes, kept in a file so
//! what it did (and what it was told when something failed) can be read
//! afterwards. One line of JSON a call, in ZigZag's settings folder:
//!
//! ```text
//! {"time":"2026-11-13T21:23:59Z","client":"codex-mcp-client","document":"gorilla",
//!  "tool":"set_object","arguments":{"object":"Face_mask","kind":"fill"},
//!  "ms":0.3,"ok":false,"error":"no object named 'Face_mask'. Objects: …"}
//! ```
//!
//! The status bar's MCP chip and Settings › Agent open it; Settings turns
//! it off. Nothing leaves the computer.

use crate::mcp::{Content, ToolResult};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The longest string kept whole, in arguments and in a result. (An SVG
/// an agent wrote is most of what it did, so most are kept whole; a
/// traced logo's worth isn't.)
const LONGEST: usize = 6000;
/// The file starts again once it's this big; the one before is kept
/// beside it (`agent-log.1.jsonl`).
const BIGGEST: u64 = 8 * 1024 * 1024;

/// Where calls are logged; nowhere, by default (tests, and when it's been
/// turned off).
#[derive(Clone, Debug, Default)]
pub struct AgentLog {
    path: Option<PathBuf>,
}

impl AgentLog {
    /// The log in ZigZag's settings folder.
    pub fn open() -> AgentLog {
        AgentLog {
            path: crate::dock::config_file("agent-log.jsonl"),
        }
    }

    #[cfg(test)]
    pub fn at(path: PathBuf) -> AgentLog {
        AgentLog { path: Some(path) }
    }

    /// The file, once something has been logged to it.
    pub fn file(&self) -> Option<&Path> {
        self.path.as_deref().filter(|p| p.exists())
    }

    /// Add one call. (A log that can't be written is no reason to fail
    /// the call: it's left out.)
    pub fn write(&self, entry: &Value) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::metadata(path).is_ok_and(|m| m.len() > BIGGEST) {
            let _ = std::fs::rename(path, path.with_file_name("agent-log.1.jsonl"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{entry}");
        }
    }
}

/// One call as the log has it. `client` is the agent that connected last
/// (the server can't tell its clients' calls apart); `ms` how long the
/// call took.
pub fn entry(
    time: SystemTime,
    client: Option<&str>,
    document: &str,
    tool: &str,
    args: &Value,
    ms: f64,
    result: &ToolResult,
) -> Value {
    let said: Vec<&str> = result
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.as_str()),
            Content::Image(_) => None,
        })
        .collect();
    let said = clip_str(&said.join("\n"));
    let images = result
        .content
        .iter()
        .filter(|c| matches!(c, Content::Image(_)))
        .count();
    let mut e = json!({
        "time": iso_utc(time),
        "client": client,
        "document": document,
        "tool": tool,
        "arguments": clip(args),
        "ms": (ms * 10.0).round() / 10.0,
        "ok": !result.is_error,
    });
    e[if result.is_error { "error" } else { "result" }] = json!(said);
    if images > 0 {
        e["images"] = json!(images);
    }
    e
}

/// `v` with its long strings cut short (and said to be).
fn clip(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(clip_str(s)),
        Value::Array(a) => Value::Array(a.iter().map(clip).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), clip(x))).collect()),
        other => other.clone(),
    }
}

fn clip_str(s: &str) -> String {
    if s.len() <= LONGEST {
        return s.to_string();
    }
    let mut end = LONGEST;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… (+{} more characters)", &s[..end], s.len() - end)
}

/// `t` as `2026-11-13T21:23:59Z` (UTC).
pub fn iso_utc(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // (Days since 1970 to a date: Howard Hinnant's civil_from_days.)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Open a text file in the system's text editor.
pub fn open_text(path: &Path) {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-t");
        c
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("notepad")
    } else {
        std::process::Command::new("xdg-open")
    };
    let _ = cmd.arg(path).spawn();
}

impl crate::app::Studio {
    /// Note a tool call an agent made, in the log.
    pub(crate) fn log_call(
        &mut self,
        tool: &str,
        args: &Value,
        at: std::time::Instant,
        result: &ToolResult,
    ) {
        let client = self.mcp.as_ref().and_then(|server| {
            let a = server.activity.lock().ok()?;
            a.clients.last().map(|c| c.name.clone())
        });
        let document = if self.doc.name.is_empty() {
            "untitled"
        } else {
            &self.doc.name
        };
        self.agent_log.write(&entry(
            SystemTime::now(),
            client.as_deref(),
            document,
            tool,
            args,
            at.elapsed().as_secs_f64() * 1000.0,
            result,
        ));
    }

    /// Keep the log, or stop keeping it, as the settings say.
    pub(crate) fn apply_agent_log(&mut self) {
        self.agent_log = if self.prefs.agent_log && self.prefs.kept() {
            AgentLog::open()
        } else {
            AgentLog::default()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn times_are_written_as_utc_dates() {
        let at = |s: u64| iso_utc(UNIX_EPOCH + Duration::from_secs(s));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_794_605_039), "2026-11-13T21:23:59Z");
        assert_eq!(at(4_102_444_799), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn the_studio_logs_the_calls_agents_make() {
        let file = std::env::temp_dir().join(format!(
            "zigzag-agent-log-studio-{}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&file);
        let mut s = crate::app::Studio::with_file(None, None);
        // (Nothing is kept until the app says where: not from tests.)
        s.apply_agent_log();
        assert!(s.agent_log.path.is_none());
        s.agent_log = AgentLog::at(file.clone());
        for (tool, args) in [
            ("get_design", json!({})),
            (
                "set_object",
                json!({ "object": "Face_mask", "kind": "fill" }),
            ),
        ] {
            let at = std::time::Instant::now();
            let r = s.run_mcp_call(tool, &args);
            s.log_call(tool, &args, at, &r);
        }
        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["tool"], "get_design");
        assert_eq!(lines[0]["ok"], true);
        assert_eq!(lines[0]["document"], "untitled");
        assert!(lines[0]["result"].as_str().unwrap().contains("hoop"));
        assert_eq!(lines[1]["arguments"]["object"], "Face_mask");
        assert_eq!(lines[1]["ok"], false);
        assert!(
            lines[1]["error"]
                .as_str()
                .unwrap()
                .starts_with("no object named 'Face_mask'")
        );
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn every_call_is_a_line_saying_what_was_asked_and_answered() {
        let file =
            std::env::temp_dir().join(format!("zigzag-agent-log-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let log = AgentLog::at(file.clone());
        assert!(log.file().is_none(), "nothing logged yet");
        let when = UNIX_EPOCH + Duration::from_secs(1_794_605_039);
        let bad = ToolResult::error("no object named 'Face_mask'. Objects: Face mask, Mouth");
        log.write(&entry(
            when,
            Some("codex-mcp-client"),
            "gorilla",
            "set_object",
            &json!({ "object": "Face_mask", "kind": "fill", "angle": 135 }),
            0.34,
            &bad,
        ));
        // A long SVG is cut short; a picture is counted, not kept.
        let long = "x".repeat(LONGEST + 500);
        let mut good = ToolResult::text("Rendered.");
        good.content.push(Content::Image(vec![1, 2, 3]));
        log.write(&entry(
            when,
            None,
            "gorilla",
            "add_svg",
            &json!({ "svg": long, "steps": [{ "deep": long }] }),
            12.0,
            &good,
        ));
        let text = std::fs::read_to_string(log.file().unwrap()).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            json!({
                "time": "2026-11-13T21:23:59Z",
                "client": "codex-mcp-client",
                "document": "gorilla",
                "tool": "set_object",
                "arguments": { "object": "Face_mask", "kind": "fill", "angle": 135 },
                "ms": 0.3,
                "ok": false,
                "error": "no object named 'Face_mask'. Objects: Face mask, Mouth",
            })
        );
        assert_eq!(lines[1]["ok"], true);
        assert_eq!(lines[1]["result"], "Rendered.");
        assert_eq!(lines[1]["images"], 1);
        assert!(lines[1]["client"].is_null());
        for s in [
            &lines[1]["arguments"]["svg"],
            &lines[1]["arguments"]["steps"][0]["deep"],
        ] {
            let s = s.as_str().unwrap();
            assert!(s.len() < LONGEST + 60 && s.ends_with("(+500 more characters)"));
        }
        // Too big: it starts again, and the one before is kept beside it.
        let f = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
        f.set_len(BIGGEST + 1).unwrap();
        log.write(&json!({ "tool": "get_design" }));
        let old = file.with_file_name("agent-log.1.jsonl");
        assert_eq!(std::fs::metadata(&old).unwrap().len(), BIGGEST + 1);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "{\"tool\":\"get_design\"}\n"
        );
        // Turned off, nothing is written.
        AgentLog::default().write(&json!({ "tool": "get_design" }));
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(&old);
    }
}
