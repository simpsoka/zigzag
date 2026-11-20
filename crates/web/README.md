# zigzag-web — ZZZ

ZigZag's studio in the browser: the page (`index.html`), its start
(`src/lib.rs`), the fonts it comes with (`assets/fonts`), and tests that run
the engine natively and in WebAssembly (`tests/`).

```sh
trunk serve            # http://localhost:8087
trunk build --release  # into dist/
```

- What it is and what works: [docs/zzz.md](../../docs/zzz.md)
- Building and testing: [docs/development.md](../../docs/development.md)
- How it fits with the engine and the studio: [docs/architecture.md](../../docs/architecture.md)
