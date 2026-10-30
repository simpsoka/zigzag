# Vendored crates

## vectorcraft-trace, vectorcraft-geom

From [VectorCraft](https://github.com/storytold/vectorcraft) (the ArtCraft
team), commit `f05c33cdfc0de6bd3f7d71e2a1628622af9a50f8`, dual-licensed
MIT OR Apache-2.0 (license files in each folder).

- `vectorcraft-trace`: Image Trace, used by `src/trace.rs` to turn PNG/JPEG
  logos into stitchable shapes.
- `vectorcraft-geom`: its geometry types.

Only the manifests were changed (concrete versions instead of workspace
inheritance), and the trace image-quality harness (`tests/quality`) and the
geom property tests were left out. To update, copy `crates/trace/src` and
`crates/geom/src` from a newer commit and update the hash above.

Ported pieces (not vendored) are credited where they live: CIEDE2000 in
`src/formats/mod.rs`, curve fitting and corner cutting in `src/bezier.rs`,
snap targets in `src/studio/snap.rs`.
