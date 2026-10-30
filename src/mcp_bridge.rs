//! zigzag-mcp: stdio ↔ HTTP bridge for MCP clients that launch servers as
//! a command (e.g. Claude Desktop). Forwards each JSON-RPC line from stdin to
//! the running studio's MCP endpoint and prints the reply.
//!
//!   zigzag-mcp [--port 7878]
//!   zigzag-mcp --headless
//!
//! The studio must be open; requests sent while it isn't get a JSON-RPC error
//! explaining that, and the bridge keeps running so it works once it's opened.
//! With `--headless` it runs the studio's tools without a window instead
//! (`zigzag-studio --mcp-stdio`, found next to this program or on PATH),
//! so an agent can work while the app is closed.

use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn post(port: u16, body: &str) -> std::io::Result<(u16, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(180)))?;
    write!(
        s,
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let (head, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let chunked = head.lines().any(|l| {
        l.to_ascii_lowercase().starts_with("transfer-encoding:")
            && l.to_ascii_lowercase().contains("chunked")
    });
    let body = if chunked {
        dechunk(rest)
    } else {
        rest.to_string()
    };
    Ok((status, body))
}

fn dechunk(mut s: &str) -> String {
    let mut out = String::new();
    while let Some((len, rest)) = s.split_once("\r\n") {
        let n = usize::from_str_radix(len.trim(), 16).unwrap_or(0);
        if n == 0 || rest.len() < n {
            break;
        }
        out.push_str(&rest[..n]);
        s = rest[n..].trim_start_matches("\r\n");
    }
    out
}

fn error_reply(line: &str, message: &str) -> Option<String> {
    // Only requests (with an id) get a reply.
    let id_start = line.find("\"id\"")?;
    let after = line[id_start + 4..]
        .trim_start()
        .strip_prefix(':')?
        .trim_start();
    let end = after.find([',', '}']).unwrap_or(after.len());
    let id = after[..end].trim();
    Some(format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32000,\"message\":\"{message}\"}}}}"
    ))
}

fn main() {
    let mut port: u16 = std::env::var("ZIGZAG_MCP_PORT")
        .or_else(|_| std::env::var("THREADLINE_MCP_PORT"))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(7878);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--port" {
            port = args.next().and_then(|v| v.parse().ok()).unwrap_or(port);
        } else if a == "--headless" {
            let name = format!("zigzag-studio{}", std::env::consts::EXE_SUFFIX);
            let beside = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join(&name)))
                .filter(|p| p.exists());
            let program = beside.unwrap_or_else(|| name.clone().into());
            let status = std::process::Command::new(&program)
                .arg("--mcp-stdio")
                .status();
            match status {
                Ok(s) => std::process::exit(s.code().unwrap_or(0)),
                Err(e) => {
                    eprintln!("zigzag-mcp: couldn't run {}: {e}", program.display());
                    std::process::exit(1);
                }
            }
        }
    }
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match post(port, &line) {
            Ok((200, body)) if !body.trim().is_empty() => Some(body.trim().to_string()),
            Ok((202, _)) | Ok((200, _)) => None,
            Ok((code, body)) => error_reply(
                &line,
                &format!(
                    "ZigZag returned HTTP {code}: {}",
                    body.trim().replace('"', "'")
                ),
            ),
            Err(_) => error_reply(
                &line,
                &format!(
                    "ZigZag isn't running (nothing on 127.0.0.1:{port}). Open ZigZag, then retry, or configure zigzag-mcp --headless to work without the app."
                ),
            ),
        };
        if let Some(r) = reply {
            let _ = writeln!(stdout, "{r}");
            let _ = stdout.flush();
        }
    }
}
