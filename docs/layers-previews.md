# Layers artwork previews

Purpose: show recognizable source geometry beside every Layers object and a
combined preview for each actual SVG group. Observed in ZigZag code: the shared
`src/studio/shape_thumbnail.rs` painter projects `Shape.geometry`, using thread
colors and one aspect-preserving transform over member union bounds. Group
previews omit hidden members; individual hidden objects retain their preview.
Fill rings are outlined rather than filled, preserving visible holes without
introducing a tessellator. Open strokes remain open; closed paths remain closed.
Empty paths paint nothing, points paint dots, and degenerate bounds center safely.
Coordinates are normalized before subtraction to avoid overflow.

The 22 px painter has no separate interaction area. Existing row selection,
collapse, eye/lock controls, counts, warnings, swatches, labels and drag/reorder
remain in place. `show_rows` still virtualizes Layers, offscreen previews are
skipped, and each preview emits at most 2,048 sampled points (128 per path).
Bounds use all finite source points. Extremely dense composites can omit paths
once that drawing budget is exhausted; outline sampling can simplify fine detail.

Out of scope: Threads previews, file/schema changes, planning/export changes,
external assets and new dependencies. No backward-compatibility migration.
Threads preview work must wait for reviewer validation of this change.

## Acceptance and validation

- Pure renderer tests: uniform scaling, group member positions and colors,
  same-color duplicates, open/closed paths, extreme coordinates, points, empty
  geometry, hidden members and the drawing budget.
- GUI fixture at 260 px in dark/light: both individual paths and both members of
  the composite are painted; object selection, group collapse and group drag
  still work. Existing object/group drag/reorder tests run in the full suite.
- `CARGO_BUILD_JOBS=2 cargo test --offline --lib` on host:
  **330 passed, 0 failed, 7 ignored**.
- `CARGO_BUILD_JOBS=2 cargo test --offline --features gui --bin zigzag-studio`
  on host: **477 passed, 0 failed, 16 ignored**.
- `CARGO_BUILD_JOBS=2 cargo build --offline --features gui --bin zigzag-studio --bin zigzag`:
  passed.
- Targeted Rust 2024 formatting applied; unrelated baseline formatting preserved.
  `cargo fmt --all -- --check` reports existing formatting differences (including
  `src/formats/charts.rs` and existing Studio code). `git diff --check`: passed.

Observed in application: real Studio captures of an author-created multielement
SVG show coral/blue petals together in their actual group, separate member
previews, an open green stem and a gold ring with a visible hole. Dark/light
screenshots are `/tmp/zigzag-thumbnail-captures/still-001.png` and `still-002.png`;
fixture is `/tmp/zigzag-thumbnail-fixture.svg`. Capture command (repeat with
`ZIGZAG_THEME=light`):

```sh
XDG_CONFIG_HOME=/tmp/zigzag-thumbnail-config ZIGZAG_THEME=dark \
ZIGZAG_CAPTURE=/tmp/zigzag-thumbnail-captures ZIGZAG_CAPTURE_STILL=2 \
target/debug/zigzag-studio /tmp/zigzag-thumbnail-fixture.svg
```

Real captures use the default wider dock; 260 px layout and interactions are
covered by the GUI harness. Initial sandbox GUI runs could not run local-server
and process tests; the complete host run above passed.
