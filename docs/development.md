# Development

Rust 1.95 or newer. The repository is a Cargo workspace (see
[architecture.md](architecture.md)); run the commands below from its root.

## The desktop

```sh
cargo build --release                                   # zigzag CLI + zigzag-mcp bridge
cargo run --release -p zigzag-studio                    # the studio
cargo run --release -p zigzag-studio --features ble     # with Skitch Bluetooth
```

Linux needs the X11/Wayland and OpenGL development libraries for the studio;
libdbus is vendored for `ble`.

The release binaries, as the release workflows and `packaging/` build them:

```sh
cargo build --release -p zigzag -p zigzag-studio \
    --features zigzag/ble,zigzag-studio/ble \
    --bin zigzag-studio --bin zigzag --bin zigzag-mcp
```

## The browser (ZZZ)

Once:

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked trunk
cargo install --locked wasm-bindgen-cli --version <the one in Cargo.lock>   # for the WASM tests
```

Then, in `crates/web`:

```sh
trunk serve            # http://localhost:8087, rebuilt on every change (engine and studio too)
trunk build --release  # into crates/web/dist
```

`trunk serve` needs Chrome or Edge to try everything (Web Bluetooth, the
File System Access API); the page tells other browsers so.

Agents on a ZZZ tab:

```sh
cargo run --release -p zigzag-mcp-relay -- --site http://localhost:8087/ --dir path/to/designs
```

and open the link it prints.

## Tests

```sh
cargo test --workspace                                          # everything, natively
cargo test --release -p zigzag-web --target wasm32-unknown-unknown --test engine   # the engine in WebAssembly
```

What's where:

| Package | Tests |
|---|---|
| `zigzag` | The engine: import, stitches, planning, formats, checks |
| `zigzag-studio` | The editor, driven through egui_kittest (needs a desktop) |
| `zigzag-web` | `tests/engine.rs`: open, plan, export and reopen the examples, trace an image, set text in every font of the library; natively and in WebAssembly, to compare |
| `zigzag-mcp-relay` | Folder bounds, which files a call may read and write, origins |

`zigzag-studio`'s `agent::tests::runs_an_agent_program_and_streams_its_output`
uses a `#!/bin/sh` script and fails on Windows.

With a PES of your own at `crates/web/tests/fixtures/local/sample1.pes`
(ignored by git), `RUSTFLAGS="--cfg zzz_local"` adds it to the engine tests.

## Releases

- **Desktop:** see [RELEASE.md](../RELEASE.md). The version is
  `[workspace.package]`'s in the root `Cargo.toml`, one for every package
  (the engine, the studio, and so the app's About, crash logs and updater).
  The release workflows can still rebuild tags from before the workspace.
- **Third-party notices:** `packaging/licenses/generate.sh` (cargo-about)
  lists what ships in the app: the studio's dependencies, the engine's among
  them, with Bluetooth. Regenerate it whenever `Cargo.lock` changes
  (`make-app.sh` checks the two match).
- **ZZZ:** every push to the `web` branch publishes it on GitHub Pages
  (`.github/workflows/web.yml`). Once, in the repository's settings: Pages ›
  Source: GitHub Actions, and Environments › github-pages › Deployment
  branches: add `web`.
