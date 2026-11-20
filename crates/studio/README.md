# zigzag-studio

The ZigZag editor (egui on OpenGL via glow) with its MCP server, as a
library (`zigzag_studio`) and as the desktop app (the `zigzag-studio`
binary). The browser version, ZZZ, is built on the same library
(`crates/web`); what only the browser needs is in `src/platform/`.

```sh
cargo run --release -p zigzag-studio --features ble
```

See [docs/architecture.md](../../docs/architecture.md) and
[docs/development.md](../../docs/development.md).
