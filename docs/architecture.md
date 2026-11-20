# Architecture

ZigZag is one Cargo workspace. The engine sits at the root; everything
built on it is in `crates/`.

```
Cargo.toml          the workspace, and the engine's package (zigzag)
src/                the engine: SVG import, stitch planning, file formats,
                    the Skitch protocol; plus the zigzag CLI and the
                    zigzag-mcp bridge
crates/studio/      zigzag-studio: the editor, as a library, and the desktop
                    app (the zigzag-studio binary)
crates/web/         zigzag-web: ZZZ, the studio in the browser (WebAssembly)
crates/relay/       zigzag-mcp-relay: lets MCP clients reach a ZZZ tab
vendor/             Image Trace (VectorCraft), vendored; not a member
packaging/          macOS, Windows and Linux release packaging
docs/               this
```

```
                 ┌────────────── zigzag (engine) ──────────────┐
                 │  svg · document · stitch · design · formats │
                 │  machine (Skitch) · vfs (files)             │
                 └──────────────────────┬──────────────────────┘
                                        │
                          zigzag-studio (editor library)
                     app · canvas · tools · panels · mcp · …
                     platform (browser only)
                        │                          │
           zigzag-studio binary              zigzag-web (cdylib)
           (desktop: winit + OpenGL)         (browser: WebGL 2, Trunk)
                                                    ▲
                                    zigzag-mcp-relay│ (WebSocket)
                                    MCP clients ────┘ (HTTP)
```

## The engine (`zigzag`)

Pure Rust with no system dependencies, except for the optional `ble`
feature (desktop Bluetooth with btleplug). It builds for the desktop and for
`wasm32-unknown-unknown` alike. Two pieces make that work:

- **`vfs`** (`src/vfs.rs`): every file read and write in the engine and the
  studio goes through it. On the desktop it is `std::fs`. In the browser it
  is a map of files in memory, by path: a file picked or dropped is put
  there, and what's written there is taken out (to IndexedDB, and to the
  person's file on disk) by the studio's `platform` module. So code that
  opens, saves and keeps settings by path runs unchanged in both.
- **The machine conversation** (`src/machine/session.rs`) is written once
  with `maybe_async`: blocking on the desktop (a worker thread holds it, as
  it always did) and async in the browser, where Bluetooth answers with
  promises. `Transport` is the link: btleplug on the desktop
  (`machine/skitch.rs`), Web Bluetooth in the browser, or the simulated
  machine (`machine/sim.rs`) in both.

## The studio (`zigzag-studio`)

The editor: egui on OpenGL (glow), with an MCP server so agents can work on
the open document. It is a library, `zigzag_studio`, used by two starts:

- **The desktop app**, the `zigzag-studio` binary: `src/main.rs` calls
  `zigzag_studio::run()` (`src/desktop.rs`: the command line, then the
  window, or the agent tools with no window for `--mcp-stdio`).
- **The browser**, through `zigzag-web`, which creates a `Studio` in the
  page.

What differs in the browser is kept in two places:

- **`src/platform/`** (browser only): files and storage
  (`files.js`: IndexedDB, the File System Access API), Bluetooth
  (`ble.js`), the relay for agents (`relay.js`), and the Rust side of each.
- **`cfg(target_arch = "wasm32")`** in a few spots of the studio: no
  threads (plans are made in the frame, an image is traced in the frame),
  the browser's file panels, the menus without the desktop-only commands
  (`commands::desktop_only`), the settings without the updater and the
  in-app agent, and wording for the browser where the desktop's would be
  wrong.

Everything else (every tool, panel and check) is the same code in both.

## ZZZ (`zigzag-web`)

A small crate: the page (`index.html`: loading screen, browser notice,
failure screen), its start (`src/lib.rs`: fonts, the panic hook, the
leave-page warning, then the studio), the fonts it comes with
(`assets/fonts`), and the tests in `tests/` that run the engine natively
and in WebAssembly to compare them. Built with Trunk (`Trunk.toml`) and
published on GitHub Pages (`.github/workflows/web.yml`). On a native target
the crate is empty.

## The relay (`zigzag-mcp-relay`)

A web page can't be reached by an MCP client, so this small desktop program
stands in for the studio's MCP server: it answers MCP where the desktop app
does (`http://127.0.0.1:7878/mcp`) and passes each message to the paired
ZZZ tab over a WebSocket. It reads the files a call names and writes the
ones a call saved, inside one folder only. Pairing is by a link it prints
(a 128-bit code); only pages on this computer and the ZZZ site may connect.

## Features

| Package | Feature | What it does |
|---|---|---|
| `zigzag` | `ble` | Desktop Bluetooth (btleplug) for the CLI and the studio |
| `zigzag-studio` | `ble` | The Skitch window talks to a real machine (desktop: `zigzag/ble`; browser: Web Bluetooth) |
| `zigzag-studio` | `menu-check` | Compile-checks the macOS menu code on Linux |
| `zigzag-web` | `ble` (default) | `zigzag-studio/ble` |
