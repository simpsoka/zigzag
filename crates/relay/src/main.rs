//! zigzag-mcp-relay: lets MCP clients work on a ZZZ tab.
//!
//! The desktop studio serves MCP itself, on 127.0.0.1. A page in a browser
//! can't listen for connections, so this small program does it for ZZZ:
//!
//! ```text
//! MCP client ── HTTP 127.0.0.1:7878 ──▶ relay ── WebSocket 127.0.0.1:7879 ──▶ the ZZZ tab
//! ```
//!
//! It answers MCP where the desktop studio does (same port, same `/mcp`),
//! so a client set up for one works with the other, `zigzag-mcp` (the
//! stdio bridge) included. Each JSON-RPC message goes to the tab, which
//! runs the studio's own tools and answers.
//!
//! Files: the tools take paths on this computer, which a page can't reach.
//! The relay reads the files a call names and sends them along, and writes
//! the ones a call saved, under one folder (`--dir`, by default where it
//! was started) and nowhere else.
//!
//! Who may connect: MCP clients on this machine, as for the desktop studio
//! (see its `mcp.rs`); and one tab, from an allowed site, that knows the
//! code this prints (it's in the link to open ZZZ with).

use base64::Engine;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const USAGE: &str = "usage: zigzag-mcp-relay [--port 7878] [--tab-port 7879] [--dir FOLDER] [--site URL] [--allow-origin ORIGIN]…

  --port          where MCP clients connect (http://127.0.0.1:PORT/mcp)
  --tab-port      where the ZZZ tab connects
  --dir           the folder agents may read and write files in (default: here)
  --site          where ZZZ is, for the link to open (default: the public one)
  --allow-origin  another site a ZZZ tab may come from (pages on this computer always may)";

const DEFAULT_SITE: &str = "https://quilombodigital.github.io/zigzag/";
/// How long a call waits for the tab.
const CALL_TIMEOUT: Duration = Duration::from_secs(180);
/// Tools whose `path` is a file to read: it's sent to the tab with the call.
const READS: &[&str] = &["open_file", "trace_image"];
/// Tools whose `path` is a file to write: the tab sends it back.
const WRITES: &[&str] = &["save", "export"];

struct Config {
    port: u16,
    tab_port: u16,
    dir: PathBuf,
    site: String,
    origins: Vec<String>,
    code: String,
}

/// The tab that's connected, and the calls waiting on it.
#[derive(Default)]
struct Tab {
    /// Messages for the tab (None: no tab).
    out: Option<Sender<String>>,
    /// Which tab this is: a new one takes the place of the last.
    serial: u64,
    next_id: u64,
    /// The calls in flight: who's waiting for the answer, and the files the
    /// call may write (as the tab knows them). Nothing else is written.
    waiting: HashMap<u64, (Sender<Value>, Vec<String>)>,
}

type Shared = Arc<Mutex<Tab>>;

fn main() {
    let cfg = match config() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let tab: Shared = Arc::default();
    let http = match tiny_http::Server::http(("127.0.0.1", cfg.port)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "Couldn't listen on port {}: {e}\n(The ZigZag desktop app uses the same port: quit it, or pass --port.)",
                cfg.port
            );
            std::process::exit(1);
        }
    };
    let tabs = match TcpListener::bind(("127.0.0.1", cfg.tab_port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Couldn't listen on port {}: {e}", cfg.tab_port);
            std::process::exit(1);
        }
    };
    let port = if cfg.tab_port == 7879 {
        String::new()
    } else {
        format!("@{}", cfg.tab_port)
    };
    println!("ZZZ relay");
    println!();
    println!("  1. Open ZZZ with this link (Chrome or Edge), so the tab pairs with this relay:");
    println!("       {}#relay={}{port}", cfg.site, cfg.code);
    println!("  2. Point your MCP client at:");
    println!("       http://127.0.0.1:{}/mcp", cfg.port);
    println!();
    println!("  Agents can read and write files in: {}", shown(&cfg.dir));
    println!("  Ctrl+C stops the relay.");

    {
        let (cfg, tab) = (cfg.clone(), tab.clone());
        std::thread::spawn(move || {
            for stream in tabs.incoming().flatten() {
                let (cfg, tab) = (cfg.clone(), tab.clone());
                std::thread::spawn(move || serve_tab(stream, &cfg, &tab));
            }
        });
    }
    for req in http.incoming_requests() {
        let (cfg, tab) = (cfg.clone(), tab.clone());
        // One thread per request: a slow tool call mustn't block pings.
        std::thread::spawn(move || serve_client(req, &cfg, &tab));
    }
}

fn config() -> Result<Config, String> {
    let mut cfg = Config {
        port: 7878,
        tab_port: 7879,
        dir: std::env::current_dir().map_err(|e| format!("no current folder: {e}"))?,
        site: DEFAULT_SITE.into(),
        origins: Vec::new(),
        code: code(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{what} needs a value"));
        match a.as_str() {
            "--port" => cfg.port = value("--port")?.parse().map_err(|_| "--port: not a port")?,
            "--tab-port" => {
                cfg.tab_port = value("--tab-port")?
                    .parse()
                    .map_err(|_| "--tab-port: not a port")?
            }
            "--dir" => cfg.dir = PathBuf::from(value("--dir")?),
            "--site" => cfg.site = value("--site")?,
            "--allow-origin" => cfg.origins.push(value("--allow-origin")?),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    cfg.dir = cfg
        .dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", cfg.dir.display()))?;
    if !cfg.site.ends_with('/') && !cfg.site.contains('#') {
        cfg.site.push('/');
    }
    // The site's own origin is allowed.
    if let Some(origin) = origin_of(&cfg.site) {
        cfg.origins.push(origin);
    }
    Ok(cfg)
}

/// `https://host[:port]` of a URL.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '#', '?']).next()?;
    Some(format!("{scheme}://{host}"))
}

/// A code a page couldn't guess (128 random bits), new each run. (Each
/// `RandomState` is seeded by the system's random source.)
fn code() -> String {
    use std::hash::{BuildHasher, Hasher};
    let half = |salt: u64| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u32(std::process::id());
        h.finish()
    };
    format!("{:016x}{:016x}", half(1), half(2))
}

/// A page on this computer (any port).
fn local(origin: &str) -> bool {
    [
        "http://localhost",
        "http://127.0.0.1",
        "https://localhost",
        "https://127.0.0.1",
    ]
    .iter()
    .any(|o| origin == *o || origin.starts_with(&format!("{o}:")))
}

// ── The tab's side ──────────────────────────────────────────────────────

fn serve_tab(stream: TcpStream, cfg: &Config, tab: &Shared) {
    use tungstenite::handshake::server::{ErrorResponse, Request, Response};
    // Only a page from an allowed site: a browser says where a page is from.
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let origin = req
            .headers()
            .get("Origin")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if local(origin) || cfg.origins.iter().any(|o| o == origin) {
            Ok(resp)
        } else {
            let mut no = ErrorResponse::new(Some("origin not allowed".into()));
            *no.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
            Err(no)
        }
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    // (Files go whole in one message, and designs can be big.)
    const BIG: usize = 512 << 20;
    let config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(BIG))
        .max_frame_size(Some(BIG));
    let Ok(mut ws) = tungstenite::accept_hdr_with_config(stream, check, Some(config)) else {
        return;
    };
    // And only one that knows the code: its first words.
    let hello = ws
        .read()
        .ok()
        .and_then(|m| m.into_text().ok())
        .and_then(|t| serde_json::from_str::<Value>(t.as_str()).ok());
    if hello.as_ref().and_then(|h| h["hello"].as_str()) != Some(cfg.code.as_str()) {
        let _ = ws.close(None);
        return;
    }
    let (tx, rx) = mpsc::channel::<String>();
    let serial = {
        let mut t = tab.lock().unwrap();
        t.serial += 1;
        t.out = Some(tx);
        // (Calls sent to the tab before this one won't be answered.)
        t.waiting.clear();
        t.serial
    };
    println!("  A ZZZ tab is connected.");
    // Short waits on the socket, so what's to send goes out between reads.
    let _ = ws
        .get_mut()
        .set_read_timeout(Some(Duration::from_millis(30)));
    'talk: loop {
        for text in rx.try_iter() {
            if ws.send(tungstenite::Message::text(text)).is_err() {
                break 'talk;
            }
        }
        if tab.lock().unwrap().serial != serial {
            // Another tab paired: this one is told, so it doesn't come back
            // and take the place again (two tabs would take turns forever).
            let _ = ws.send(tungstenite::Message::text(r#"{"replaced":true}"#));
            let _ = ws.close(None);
            break;
        }
        match ws.read() {
            Ok(m) if m.is_text() => {
                let Some(v) = m
                    .into_text()
                    .ok()
                    .and_then(|t| serde_json::from_str::<Value>(t.as_str()).ok())
                else {
                    continue;
                };
                answer(v, cfg, tab);
            }
            Ok(m) if m.is_close() => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    let mut t = tab.lock().unwrap();
    if t.serial == serial {
        t.out = None;
        t.waiting.clear();
        println!("  The ZZZ tab went away.");
    }
}

/// The tab answered a call: write the files it saved, and hand the answer
/// to whoever is waiting for it.
fn answer(mut v: Value, cfg: &Config, tab: &Shared) {
    let Some(id) = v["id"].as_u64() else { return };
    let Some((to, may)) = tab.lock().unwrap().waiting.remove(&id) else {
        return;
    };
    let mut reply = v["reply"].take();
    if let Some(files) = v["files"].as_object() {
        for (path, data) in files {
            // Only what the call was for (a save or an export, and the
            // object map an export may write beside it).
            let allowed = may
                .iter()
                .any(|m| path == m || *path == format!("{m}.objects.json"));
            if !allowed {
                continue;
            }
            let written = within(&cfg.dir, from_tab(path)).and_then(|p| {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data.as_str().unwrap_or(""))
                    .map_err(|e| e.to_string())?;
                std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))
            });
            if let Err(e) = written {
                reply = failed(&reply, &format!("The relay couldn't write {path}: {e}"));
            }
        }
    }
    let _ = to.send(paths_as_here(reply, &cfg.dir));
}

/// The tab names this computer's files its own way (`/C:/…`, see
/// [`for_tab`]): in what it says back, name them as they are here.
fn paths_as_here(reply: Value, dir: &Path) -> Value {
    let (told, here) = (for_tab(dir), shown(dir));
    if told == here {
        return reply;
    }
    let text = reply.to_string();
    if !text.contains(&told) {
        return reply;
    }
    serde_json::from_str(&text.replace(&told, &here)).unwrap_or(reply)
}

/// A tool result saying the call failed, in place of `reply`.
fn failed(reply: &Value, why: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": reply["id"],
        "result": { "content": [{ "type": "text", "text": why }], "isError": true },
    })
}

/// `path`, if it's inside the folder agents may use: as a full path there.
/// (Its folder has to exist; the file needn't.)
fn within(dir: &Path, path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    let p = if p.is_absolute() {
        p.to_path_buf()
    } else {
        dir.join(p)
    };
    let name = p.file_name().ok_or("not a file path")?.to_os_string();
    let parent = p
        .parent()
        .ok_or("not a file path")?
        .canonicalize()
        .map_err(|e| format!("its folder: {e}"))?;
    let outside = || {
        format!(
            "it's outside {} (the folder this relay was started for)",
            shown(dir)
        )
    };
    if !parent.starts_with(dir) {
        return Err(outside());
    }
    let full = parent.join(name);
    // A file that's there may be a link to somewhere else: where it leads
    // has to be inside too.
    if std::fs::symlink_metadata(&full).is_ok() {
        let real = full.canonicalize().map_err(|e| format!("{e}"))?;
        if !real.starts_with(dir) {
            return Err(outside());
        }
    }
    Ok(full)
}

/// A path as people read it: forward slashes, and without the long-path
/// prefix Windows' `canonicalize` gives.
fn shown(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").map(String::from).unwrap_or(s)
}

/// A path as the tab is told it. The page reads paths the Unix way: their
/// parts are split at forward slashes, and a full path starts with one
/// (the studio's tools ask for a full path). So a Windows path with a
/// drive goes as `/C:/a/b.svg`; [`from_tab`] is the way back.
fn for_tab(p: &Path) -> String {
    let s = shown(p);
    if s.starts_with('/') { s } else { format!("/{s}") }
}

/// The path on this computer that the tab knows as `path` (see [`for_tab`]).
fn from_tab(path: &str) -> &str {
    let b = path.as_bytes();
    let drive = b.len() > 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':';
    if drive { &path[1..] } else { path }
}

// ── The MCP client's side ───────────────────────────────────────────────

fn header(req: &tiny_http::Request, name: &'static str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

fn respond(req: tiny_http::Request, status: u16, body: Option<Value>) {
    let mut resp = match body {
        Some(v) => tiny_http::Response::from_string(v.to_string())
            .with_status_code(status)
            .boxed(),
        None => tiny_http::Response::empty(status).boxed(),
    };
    if let Ok(h) = tiny_http::Header::from_bytes("Content-Type", "application/json") {
        resp.add_header(h);
    }
    let _ = req.respond(resp);
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn serve_client(mut req: tiny_http::Request, cfg: &Config, tab: &Shared) {
    // As the desktop studio's server: web pages mustn't drive the tools
    // (they write files). Only pages on this machine pass; MCP clients
    // send no Origin at all.
    if let Some(origin) = header(&req, "Origin")
        && !local(&origin)
    {
        return respond(req, 403, Some(json!({ "error": "origin not allowed" })));
    }
    let json_body = header(&req, "Content-Type").is_some_and(|t| {
        t.split(';')
            .next()
            .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
    });
    if *req.method() == tiny_http::Method::Post && !json_body {
        return respond(
            req,
            415,
            Some(json!({ "error": "Content-Type must be application/json" })),
        );
    }
    let path = req.url().split('?').next().unwrap_or("").to_string();
    if path != "/mcp" && path != "/" {
        return respond(req, 404, None);
    }
    match req.method() {
        tiny_http::Method::Post => {}
        tiny_http::Method::Delete => return respond(req, 200, None),
        _ => return respond(req, 405, None),
    }
    let mut body = String::new();
    if req
        .as_reader()
        .take(32 << 20)
        .read_to_string(&mut body)
        .is_err()
    {
        return respond(
            req,
            400,
            Some(rpc_error(Value::Null, -32700, "unreadable body")),
        );
    }
    let mut msg: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return respond(
                req,
                400,
                Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
            );
        }
    };
    // The files its calls name, to go with it; a call that names one out
    // of bounds is answered here.
    let mut files = serde_json::Map::new();
    let mut writes = Vec::new();
    let refused = match &mut msg {
        Value::Array(batch) => batch
            .iter_mut()
            .find_map(|m| prepare(m, cfg, &mut files, &mut writes)),
        m => prepare(m, cfg, &mut files, &mut writes),
    };
    if let Some(no) = refused {
        return respond(req, 200, Some(no));
    }
    let first_id = match &msg {
        Value::Array(batch) => batch
            .iter()
            .find_map(|m| m.get("id").cloned())
            .unwrap_or(Value::Null),
        m => m.get("id").cloned().unwrap_or(Value::Null),
    };
    let (tx, rx) = mpsc::channel();
    let sent = {
        let mut t = tab.lock().unwrap();
        let id = t.next_id;
        t.next_id += 1;
        let text = json!({ "id": id, "rpc": msg, "files": files }).to_string();
        match &t.out {
            Some(out) if out.send(text).is_ok() => {
                t.waiting.insert(id, (tx, writes));
                true
            }
            _ => false,
        }
    };
    if !sent {
        let why = format!(
            "ZZZ isn't open, or isn't paired with this relay. Open {}#relay={} in Chrome or Edge.",
            cfg.site, cfg.code
        );
        return respond(req, 200, Some(rpc_error(first_id, -32000, &why)));
    }
    match rx.recv_timeout(CALL_TIMEOUT) {
        Ok(Value::Null) => respond(req, 202, None),
        Ok(reply) => respond(req, 200, Some(reply)),
        Err(_) => respond(
            req,
            200,
            Some(rpc_error(
                first_id,
                -32000,
                "ZZZ didn't answer in time (is a dialog open in the tab, or the tab in the background?)",
            )),
        ),
    }
}

/// One JSON-RPC message on its way to the tab: a tool call's `path` is
/// checked (and made a full path with forward slashes), and a file to be
/// read is read into `files`. A refusal, as the answer to give, if the
/// path is out of bounds or the file can't be read.
fn prepare(
    msg: &mut Value,
    cfg: &Config,
    files: &mut serde_json::Map<String, Value>,
    writes: &mut Vec<String>,
) -> Option<Value> {
    if msg.get("method").and_then(Value::as_str) != Some("tools/call") {
        return None;
    }
    let name = msg["params"]["name"].as_str().unwrap_or("").to_string();
    let reads = READS.contains(&name.as_str());
    if !reads && !WRITES.contains(&name.as_str()) {
        return None;
    }
    let path = msg["params"]["arguments"]["path"].as_str()?.to_string();
    let full = match within(&cfg.dir, &path) {
        Ok(p) => p,
        Err(e) => return Some(failed(msg, &format!("{path}: {e}"))),
    };
    let told = for_tab(&full);
    if reads {
        match std::fs::read(&full) {
            Ok(bytes) => {
                files.insert(
                    told.clone(),
                    base64::engine::general_purpose::STANDARD.encode(bytes).into(),
                );
            }
            Err(e) => return Some(failed(msg, &format!("couldn't read {path}: {e}"))),
        }
    }
    if !reads {
        writes.push(told.clone());
    }
    msg["params"]["arguments"]["path"] = told.into();
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_stay_inside_the_folder() {
        let dir = std::env::temp_dir()
            .join(format!("zzz-relay-{}", std::process::id()))
            .canonicalize()
            .unwrap_or_else(|_| {
                let d = std::env::temp_dir().join(format!("zzz-relay-{}", std::process::id()));
                std::fs::create_dir_all(&d).unwrap();
                d.canonicalize().unwrap()
            });
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        assert_eq!(within(&dir, "a.svg").unwrap(), dir.join("a.svg"));
        assert_eq!(within(&dir, "sub/b.pes").unwrap(), dir.join("sub").join("b.pes"));
        let inside = dir.join("c.dst");
        assert_eq!(within(&dir, inside.to_str().unwrap()).unwrap(), inside);
        assert!(within(&dir, "../out.svg").is_err(), "not above it");
        assert!(within(&dir, "sub/../../out.svg").is_err());
        let outside = std::env::temp_dir().join("elsewhere.svg");
        assert!(within(&dir, outside.to_str().unwrap()).is_err());
        assert!(within(&dir, "nowhere/x.svg").is_err(), "the folder must be there");
        // A link inside that leads outside is outside.
        let target = std::env::temp_dir().join(format!("zzz-relay-out-{}.txt", std::process::id()));
        std::fs::write(&target, b"secret").unwrap();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&target, dir.join("link.svg")).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&target, dir.join("link.svg")).is_ok();
        if linked {
            assert!(within(&dir, "link.svg").is_err(), "a link out is refused");
        }
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_read_goes_along_and_a_path_out_of_bounds_is_refused() {
        let dir = std::env::temp_dir().join(format!("zzz-relay-prep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        std::fs::write(dir.join("in.svg"), b"<svg/>").unwrap();
        let cfg = Config {
            port: 0,
            tab_port: 0,
            dir: dir.clone(),
            site: DEFAULT_SITE.into(),
            origins: Vec::new(),
            code: "0".into(),
        };
        let call = |name: &str, path: &str| {
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": { "name": name, "arguments": { "path": path } } })
        };
        let mut files = serde_json::Map::new();
        let mut writes = Vec::new();
        let mut m = call("open_file", "in.svg");
        assert!(prepare(&mut m, &cfg, &mut files, &mut writes).is_none());
        let told = m["params"]["arguments"]["path"].as_str().unwrap().to_string();
        assert!(
            told.starts_with('/') && !told.contains('\\') && told.ends_with("/in.svg"),
            "{told}"
        );
        // And back to where it is on this computer.
        assert_eq!(within(&dir, from_tab(&told)).unwrap(), dir.join("in.svg"));
        assert_eq!(from_tab("/C:/a/b.svg"), "C:/a/b.svg");
        assert_eq!(from_tab("/home/a/b.svg"), "/home/a/b.svg");
        // What the tab says about a file names it as it is here.
        let said = json!({ "result": { "content": [{ "text": format!("Wrote {told}.") }] } });
        let heard = paths_as_here(said, &dir);
        assert_eq!(
            heard["result"]["content"][0]["text"],
            format!("Wrote {}/in.svg.", shown(&dir))
        );
        assert_eq!(files[&told], "PHN2Zy8+");
        // A file to write isn't read; one out of bounds is refused.
        let mut m = call("export", "out.pes");
        assert!(prepare(&mut m, &cfg, &mut files, &mut writes).is_none());
        assert_eq!(files.len(), 1);
        assert_eq!(writes.len(), 1, "the export may write its file");
        assert!(writes[0].ends_with("/out.pes"));
        let mut m = call("export", "../out.pes");
        let no = prepare(&mut m, &cfg, &mut files, &mut writes).expect("refused");
        assert_eq!(no["result"]["isError"], true);
        // Other tools pass as they are.
        let mut m = json!({ "method": "tools/call", "params": { "name": "get_design" } });
        assert!(prepare(&mut m, &cfg, &mut files, &mut writes).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_local_pages_and_the_site_are_let_in() {
        assert!(local("http://localhost:8087") && local("http://127.0.0.1"));
        assert!(!local("http://localhost.evil.example") && !local("https://example.com"));
        assert_eq!(
            origin_of(DEFAULT_SITE).as_deref(),
            Some("https://quilombodigital.github.io")
        );
    }
}
