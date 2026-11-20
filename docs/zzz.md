# ZZZ — ZigZag in the browser

ZZZ is the [ZigZag](https://github.com/simpsoka/zigzag) embroidery studio
running in the browser: the same studio, compiled to WebAssembly. Open a
design, edit it, export PES, DST, EXP, JEF or VP3, and send it to a Brother
Skitch (PP1) over Bluetooth, with nothing to install.

**Use it:** <https://quilombodigital.github.io/zigzag/> (Chrome or Edge on a
computer).

## What works

- Open SVG artwork, machine files (PES, PEC, DST, EXP, JEF) and images to
  trace (PNG, JPEG, WebP, GIF); drop them on the window or use File › Open.
- Every editing tool of the desktop studio: shapes, pen, lettering, stitch
  types and settings, undo, the digitizing checks.
- Save goes back to the file you opened (File System Access API); Save As
  and Export ask where.
- Your work is kept in the browser between visits: open documents, unsaved
  changes, settings and panel layout come back when you return.
- Lettering in 27 fonts made for embroidery (see
  [crates/web/assets/fonts/library](../crates/web/assets/fonts/library/README.md)), plus any you add
  with Type › Add Fonts….
- The Skitch window over Web Bluetooth, or a simulated machine to try the
  whole job without one (Skitch window › ⋯ › Connect to a Simulated
  Machine).
- AI agents (MCP) through a small relay program on your computer, below.

What the browser version leaves out: the in-app agent panel, the updater,
crash logs, and the desktop's own MCP server.

## Agents (MCP)

A web page can't be reached by an MCP client by itself.
`zigzag-mcp-relay` (in [crates/relay](../crates/relay/)) stands in for it: it answers MCP
where the desktop studio does, `http://127.0.0.1:7878/mcp`, and passes each
call to the ZZZ tab.

```sh
cargo run --release -p zigzag-mcp-relay -- --dir path/to/designs
```

It prints a link; open ZZZ with it to pair the tab. Agents then have the
same tools as in the desktop app, and read and write files only inside
`--dir`.

## Building and how it's put together

How to build, run and test it: [development.md](development.md). How the
pieces fit (one studio library for the desktop and the browser, the file
layer, the browser's platform module): [architecture.md](architecture.md).
The plan and log of the port, phase by phase (in Portuguese):
[plans/zzz-port.md](plans/zzz-port.md).

## Publishing

`.github/workflows/web.yml` tests, builds and publishes the site on GitHub
Pages on every push to the `web` branch. Once, in the repository's
settings: Pages › Source: GitHub Actions, and Environments › github-pages ›
Deployment branches: add `web`.

## License

Apache-2.0, as ZigZag. The fonts are under their own licenses (SIL Open
Font License or Apache 2.0), each beside its files.
