# zigzag-mcp-relay

Lets MCP clients (Claude Code, Claude Desktop, Codex…) work on a ZZZ tab,
the ZigZag studio in the browser. It answers MCP where the desktop studio
does, `http://127.0.0.1:7878/mcp`, and passes each call to the paired tab.

```sh
cargo run --release -p zigzag-mcp-relay -- --dir path/to/designs
```

It prints a link: open ZZZ with it to pair the tab. Agents read and write
files only inside `--dir`. `--help` lists the options (`--site` for a local
`trunk serve`, other ports, other sites).

See [docs/zzz.md](../../docs/zzz.md#agents-mcp).
