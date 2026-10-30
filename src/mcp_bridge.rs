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
    use serde_json::{Value, json};
    let error = |id: Value, code: i64, message: &str| {
        Some(
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
                .to_string(),
        )
    };
    let request: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => return error(Value::Null, -32700, "Parse error"),
    };
    // MCP 2025-06-18 permits one message, string/integer IDs, object params.
    // Check the message header before classifying request versus notification.
    let valid_id = request.get("id").is_none_or(|id| {
        id.is_string()
            || id.is_i64()
            || id.is_u64()
            || id
                .as_f64()
                .is_some_and(|n| n.is_finite() && n.fract() == 0.0)
    });
    if !request.is_object()
        || request.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || request.get("method").and_then(Value::as_str).is_none()
        || !valid_id
    {
        return error(Value::Null, -32600, "Invalid request for MCP 2025-06-18");
    }
    // Notifications never get replies, including parameter-validation errors.
    // Establish direction before generating parameter-validation replies.
    let id = request.get("id")?;
    if request
        .get("params")
        .is_some_and(|params| !params.is_object())
    {
        return error(
            id.clone(),
            -32602,
            "Invalid params: MCP parameters must be an object",
        );
    }
    error(id.clone(), -32000, message)
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
                &format!("ZigZag returned HTTP {code}: {}", body.trim()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, ensure};
    use serde_json::{Value, json};

    fn reply(input: &str, message: &str) -> anyhow::Result<Value> {
        let text = error_reply(input, message).context("missing error reply")?;
        Ok(serde_json::from_str(&text)?)
    }

    #[test]
    fn bridge_errors_preserve_ids_and_escape_messages() -> anyhow::Result<()> {
        let message = "ZigZag isn't running: \"quoted\" \\path\nnext line";
        for id in [
            json!(0),
            json!(-17),
            json!(u64::MAX),
            json!(17.0),
            json!("job,part-2}"),
            json!("quote\"slash\\brace}comma,"),
        ] {
            let request = json!({"jsonrpc": "2.0", "params": {"id": "misleading"}, "id": id, "method": "tools/list"});
            let result = reply(&request.to_string(), message)?;
            ensure!(result["id"] == id, "ID did not round-trip: {result}");
            ensure!(
                result["error"]["message"] == message,
                "message did not round-trip"
            );
            ensure!(result["error"]["code"] == -32000, "wrong connection error");
        }
        ensure!(
            error_reply(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{"id":42}}"#,
                message
            )
            .is_none(),
            "notification replied"
        );
        Ok(())
    }

    #[test]
    fn bridge_errors_reject_envelopes_invalid_in_advertised_mcp_era() -> anyhow::Result<()> {
        let message = "unavailable";
        for invalid in [
            json!([]),
            json!([{"jsonrpc":"2.0","id":1,"method":"ping"}]),
            json!(null),
            json!(3),
            json!({}),
            json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
            json!({"jsonrpc":"2.0","id":1}),
            json!({"jsonrpc":"2.0","id":1,"method":3}),
            json!({"jsonrpc":"2.0","id":null,"method":"ping"}),
            json!({"jsonrpc":"2.0","id":1.5,"method":"ping"}),
            json!({"jsonrpc":"2.0","id":{},"method":"ping"}),
        ] {
            let result = reply(&invalid.to_string(), message)?;
            ensure!(
                result["error"]["code"] == -32600 && result["id"].is_null(),
                "invalid request accepted: {invalid}"
            );
        }
        for invalid in ["{", "not json", ""] {
            let result = reply(invalid, message)?;
            ensure!(
                result["error"]["code"] == -32700 && result["id"].is_null(),
                "missing parse error"
            );
        }
        Ok(())
    }

    #[test]
    fn bridge_parameter_errors_preserve_ids_and_suppress_notifications() -> anyhow::Result<()> {
        for params in [json!([]), json!(null), json!(3), json!("invalid")] {
            let notification =
                json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": params});
            ensure!(
                error_reply(&notification.to_string(), "unavailable").is_none(),
                "invalid-parameter notification received a reply"
            );
            for id in [json!(1), json!("job-7")] {
                let request =
                    json!({"jsonrpc": "2.0", "id": id, "method": "ping", "params": params});
                let response = reply(&request.to_string(), "unavailable")?;
                ensure!(
                    response["error"]["code"] == -32602 && response["id"] == id,
                    "parameter error lost its request ID: {response}"
                );
            }
        }
        Ok(())
    }
}
