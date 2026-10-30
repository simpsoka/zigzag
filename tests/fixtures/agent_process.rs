//! Standalone process fixture compiled by the agent integration test.
use std::io::{self, Read};

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|s| s == "--strict-mcp-config")
        || !args.windows(2).any(|w| {
            w.first().is_some_and(|s| s == "--tools") && w.get(1).is_some_and(String::is_empty)
        })
    {
        return Err(io::Error::other("generated Claude arguments changed"));
    }
    let file = std::env::var_os("ZIGZAG_AGENT_STDIN")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| io::Error::other("missing fixture output path"))?;
    let mut prompt = Vec::new();
    io::stdin().read_to_end(&mut prompt)?;
    std::fs::write(&file, prompt)?;
    std::fs::write(
        file.with_extension("program"),
        std::env::current_exe()?.to_string_lossy().as_bytes(),
    )?;
    for event in [
        r#"{"type":"system","subtype":"init","session_id":"s1"}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"fixture received stdin"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"duration_ms":10}"#,
    ] {
        println!("{event}");
    }
    Ok(())
}
