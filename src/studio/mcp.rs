//! MCP server (Streamable HTTP transport, JSON responses) hosted by the studio.
//!
//! Listens on 127.0.0.1 only. Each `tools/call` is handed to the UI thread
//! through a channel and answered when the studio has applied it, so agents
//! edit the same live document (with undo) that the person sees.
//!
//! `zigzag-studio --mcp-stdio` serves the same tools over stdio with no
//! window (headless), for agents working while the app is closed.

use serde_json::{Value, json};
use std::io::Read;
use std::net::SocketAddr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const DEFAULT_PORT: u16 = 7878;

#[derive(Clone, Debug)]
pub enum Content {
    Text(String),
    Image(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct ToolResult {
    pub content: Vec<Content>,
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(s: impl Into<String>) -> Self {
        ToolResult {
            content: vec![Content::Text(s.into())],
            is_error: false,
        }
    }
    pub fn json(v: &Value) -> Self {
        Self::text(serde_json::to_string_pretty(v).unwrap_or_default())
    }
    pub fn error(s: impl Into<String>) -> Self {
        ToolResult {
            content: vec![Content::Text(s.into())],
            is_error: true,
        }
    }
    fn to_json(&self) -> Value {
        use base64::Engine;
        let content: Vec<Value> = self
            .content
            .iter()
            .map(|c| match c {
                Content::Text(t) => json!({ "type": "text", "text": t }),
                Content::Image(png) => json!({
                    "type": "image",
                    "mimeType": "image/png",
                    "data": base64::engine::general_purpose::STANDARD.encode(png),
                }),
            })
            .collect();
        json!({ "content": content, "isError": self.is_error })
    }
}

#[cfg(test)]
mod execution_tests {
    use super::*;
    use anyhow::{Context, ensure};

    fn message(result: &ToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn queued_expiry_and_deadline_both_prevent_start() -> anyhow::Result<()> {
        let now = Instant::now();
        let future = now
            .checked_add(Duration::from_secs(60))
            .context("deadline")?;
        let queued = Execution::new(future);
        ensure!(
            message(&queued.no_reply()).contains("before starting"),
            "no definite queued expiry"
        );
        ensure!(!queued.try_start_at(|| now), "expired call started");
        let overdue = Execution::new(now);
        ensure!(
            !overdue.try_start_at(|| now),
            "deadline did not prevent start"
        );
        ensure!(
            message(&overdue.no_reply()).contains("before starting"),
            "deadline result ambiguous"
        );
        Ok(())
    }

    #[test]
    fn running_timeout_is_uncertain_and_does_not_cancel_completion() -> anyhow::Result<()> {
        let now = Instant::now();
        let future = now
            .checked_add(Duration::from_secs(60))
            .context("deadline")?;
        let execution = Arc::new(Execution::new(future));
        ensure!(execution.try_start_at(|| now), "live call not admitted");
        let timeout = execution.no_reply();
        ensure!(
            timeout.is_error && message(&timeout).contains("outcome unknown"),
            "running operation reported cancelled"
        );
        ensure!(!execution.try_start_at(|| now), "call admitted twice");
        let (reply, receive) = mpsc::channel();
        let call = Call {
            name: "edit".into(),
            args: json!({}),
            reply,
            execution,
        };
        call.complete(ToolResult::text("completed"));
        let result = receive.recv_timeout(Duration::from_secs(1))?;
        ensure!(
            !result.is_error && message(&result) == "completed",
            "timeout cancelled running operation"
        );
        Ok(())
    }
}

/// A tool call waiting for the UI thread.
pub struct Call {
    pub name: String,
    pub args: Value,
    reply: Sender<ToolResult>,
    execution: Arc<Execution>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Queued,
    Running,
    Completed,
    Expired,
}

/// Start and expiry must arbitrate on the same lock. A failed response wait is
/// never itself cancellation of an operation that has already started.
struct Execution {
    deadline: Instant,
    phase: Mutex<Phase>,
}

impl Execution {
    fn new(deadline: Instant) -> Self {
        Self {
            deadline,
            phase: Mutex::new(Phase::Queued),
        }
    }

    fn try_start_at(&self, now: impl FnOnce() -> Instant) -> bool {
        let Ok(mut phase) = self.phase.lock() else {
            return false;
        };
        if *phase != Phase::Queued {
            return false;
        }
        if now() >= self.deadline {
            *phase = Phase::Expired;
            return false;
        }
        *phase = Phase::Running;
        true
    }

    fn no_reply(&self) -> ToolResult {
        let Ok(mut phase) = self.phase.lock() else {
            return ToolResult::error(
                "execution state unavailable; outcome unknown; inspect the design before retrying",
            );
        };
        match *phase {
            Phase::Queued | Phase::Expired => {
                *phase = Phase::Expired;
                ToolResult::error(
                    "request expired before starting; no operation was executed (is a dialog open?)",
                )
            }
            Phase::Running | Phase::Completed => ToolResult::error(
                "the operation started but its reply was not received in time; outcome unknown; inspect the design or external destination before retrying",
            ),
        }
    }
}

impl Call {
    pub fn try_start(&self) -> bool {
        self.execution.try_start_at(Instant::now)
    }

    pub fn complete(self, result: ToolResult) {
        if let Ok(mut phase) = self.execution.phase.lock() {
            *phase = Phase::Completed;
        }
        let _ = self.reply.send(result);
    }
}

#[derive(Clone)]
struct CallQueue {
    sender: Sender<Call>,
    timeout: Duration,
}

pub struct Server {
    pub rx: Receiver<Call>,
    pub addr: SocketAddr,
    /// Who has connected and when the server last heard from anyone
    /// (written by the HTTP threads, read by the status bar's MCP chip).
    pub activity: Arc<Mutex<Activity>>,
}

/// What the HTTP side has seen.
#[derive(Default, Clone, Debug)]
pub struct Activity {
    /// Clients that said hello (`initialize`), newest last; one entry per
    /// name and version.
    pub clients: Vec<Client>,
    /// JSON-RPC requests answered (pings and lists too).
    pub requests: u64,
    pub last_request: Option<Instant>,
}

#[derive(Clone, Debug)]
pub struct Client {
    pub name: String,
    pub version: String,
    pub protocol: String,
    /// When it last initialized.
    pub at: Instant,
}

impl Activity {
    #[cfg(test)]
    pub fn note_for_test(&mut self, msg: &Value) {
        self.note(msg)
    }

    /// Note one incoming JSON-RPC message.
    fn note(&mut self, msg: &Value) {
        self.requests += 1;
        self.last_request = Some(Instant::now());
        if msg.get("method").and_then(Value::as_str) != Some("initialize") {
            return;
        }
        let params = msg.get("params");
        let info = params.and_then(|p| p.get("clientInfo"));
        let text = |v: Option<&Value>, k: &str| {
            v.and_then(|v| v.get(k))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let mut name = text(info, "title");
        if name.is_empty() {
            name = text(info, "name");
        }
        if name.is_empty() {
            name = "Unnamed client".into();
        }
        let version = text(info, "version");
        let protocol = text(params, "protocolVersion");
        self.clients
            .retain(|c| !(c.name == name && c.version == version));
        self.clients.push(Client {
            name,
            version,
            protocol,
            at: Instant::now(),
        });
        // Keep the list short: the most recent few.
        if self.clients.len() > 12 {
            self.clients.remove(0);
        }
    }
}

/// Start the HTTP server. `tools` is the `tools/list` result; `wake` nudges
/// the UI thread so it picks up queued calls even when idle.
pub fn start(
    port: u16,
    tools: Value,
    instructions: String,
    wake: impl Fn() + Send + Sync + 'static,
) -> std::io::Result<Server> {
    start_with_timeout(port, tools, instructions, wake, Duration::from_secs(120))
}

pub(crate) fn start_with_timeout(
    port: u16,
    tools: Value,
    instructions: String,
    wake: impl Fn() + Send + Sync + 'static,
    timeout: Duration,
) -> std::io::Result<Server> {
    let server = tiny_http::Server::http(("127.0.0.1", port)).map_err(std::io::Error::other)?;
    let addr = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| std::io::Error::other("no IP address"))?;
    let (tx, rx) = mpsc::channel::<Call>();
    let tx = CallQueue { sender: tx, timeout };
    let wake = std::sync::Arc::new(wake);
    let activity = Arc::new(Mutex::new(Activity::default()));
    let seen = activity.clone();
    std::thread::Builder::new()
        .name("mcp-http".into())
        .spawn(move || {
            for req in server.incoming_requests() {
                let tx = tx.clone();
                let tools = tools.clone();
                let wake = wake.clone();
                let instructions = instructions.clone();
                let seen = seen.clone();
                // One thread per request: a slow tool call mustn't block pings.
                std::thread::spawn(move || {
                    handle_http(req, &tools, &instructions, &tx, &*wake, &seen)
                });
            }
        })?;
    Ok(Server { rx, addr, activity })
}

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

fn handle_http(
    mut req: tiny_http::Request,
    tools: &Value,
    instructions: &str,
    queue: &CallQueue,
    wake: &(dyn Fn() + Send + Sync),
    seen: &Mutex<Activity>,
) {
    // Web pages mustn't drive the studio (its tools write files). Browsers
    // say where a request comes from: only pages on this machine pass.
    // ("null" is a sandboxed iframe or a local file, which any website can
    // make: refused too. MCP clients send no Origin at all.)
    if let Some(origin) = header(&req, "Origin") {
        let local = [
            "http://localhost",
            "http://127.0.0.1",
            "https://localhost",
            "https://127.0.0.1",
        ]
        .iter()
        .any(|o| origin == *o || origin.starts_with(&format!("{o}:")));
        if !local {
            return respond(req, 403, Some(json!({ "error": "origin not allowed" })));
        }
    }
    // JSON only, as MCP clients send it: a browser can't send that to
    // another site without asking first (a CORS preflight, which this server
    // never allows), so a page can't slip a call through as a plain form or
    // text post either.
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
        tiny_http::Method::Get => return respond(req, 405, None), // no server-initiated stream
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
    let msg: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return respond(
                req,
                400,
                Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
            );
        }
    };
    if let Ok(mut a) = seen.lock() {
        match &msg {
            Value::Array(batch) => batch.iter().for_each(|m| a.note(m)),
            m => a.note(m),
        }
    }
    // Wake the UI so the MCP chip shows the activity.
    wake();
    // Tool calls go to the UI thread, which answers when it has applied them.
    let mut call = |name: String, args: Value| -> ToolResult {
        let (rtx, rrx) = mpsc::channel();
        let Some(deadline) = Instant::now().checked_add(queue.timeout) else {
            return ToolResult::error("request timeout exceeds the supported clock range");
        };
        let execution = Arc::new(Execution::new(deadline));
        if queue.sender
            .send(Call {
                name,
                args,
                reply: rtx,
                execution: execution.clone(),
            })
            .is_err()
        {
            return ToolResult::error("the studio is shutting down");
        }
        wake();
        rrx.recv_timeout(queue.timeout)
            .unwrap_or_else(|_| execution.no_reply())
    };
    let out = match &msg {
        Value::Array(batch) => {
            let rs: Vec<Value> = batch
                .iter()
                .filter_map(|m| handle_rpc(m, tools, instructions, &mut call))
                .collect();
            (!rs.is_empty()).then_some(Value::Array(rs))
        }
        m => handle_rpc(m, tools, instructions, &mut call),
    };
    match out {
        Some(v) => respond(req, 200, Some(v)),
        None => respond(req, 202, None),
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Handle one JSON-RPC message, running tool calls with `call`. `None` for
/// notifications.
pub fn handle_rpc(
    msg: &Value,
    tools: &Value,
    instructions: &str,
    call: &mut dyn FnMut(String, Value) -> ToolResult,
) -> Option<Value> {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let Some(id) = msg.get("id").cloned() else {
        return None; // notification (e.g. notifications/initialized)
    };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            let version = if ["2025-06-18", "2025-03-26", "2024-11-05"].contains(&requested) {
                requested
            } else {
                PROTOCOL_VERSION
            };
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "zigzag", "version": env!("CARGO_PKG_VERSION") },
                "instructions": instructions,
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools }),
        "resources/list" => json!({ "resources": [] }),
        "prompts/list" => json!({ "prompts": [] }),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            call(name, args).to_json()
        }
        _ => {
            return Some(rpc_error(
                id,
                -32601,
                &format!("method not found: {method}"),
            ));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// Serve MCP over stdio (one JSON-RPC message per line), running tools with
/// `call` on this thread: the headless mode.
pub fn serve_stdio(
    tools: &Value,
    instructions: &str,
    mut call: impl FnMut(&str, &Value) -> ToolResult,
) {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut run = |name: String, args: Value| call(&name, &args);
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let rs: Vec<Value> = batch
                    .iter()
                    .filter_map(|m| handle_rpc(m, tools, instructions, &mut run))
                    .collect();
                (!rs.is_empty()).then_some(Value::Array(rs))
            }
            Ok(m) => handle_rpc(&m, tools, instructions, &mut run),
            Err(e) => Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = reply {
            let _ = writeln!(out, "{r}");
            let _ = out.flush();
        }
    }
}
