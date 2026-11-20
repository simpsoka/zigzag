# Changelog

## Unreleased

- File › Place… (⇧⌘P, was Place SVG…) also takes PES, PEC, DST, EXP
  and JEF files, putting them in the open design as Embrilliance's Merge
  does: at their real size, as one group named after the file in the
  Layers list, sewn after what's there. Agents can do it with
  `place_file`, and the CLI with `zigzag convert … --place FILE[@X,Y]`.

## 0.1.6 — 2026-11-19

- Satin lettering is read stroke by stroke, with Corners (Cut, Auto or
  Mitred) in the Stitch panel and an even zigzag; more letters sew in one
  piece of thread.
- Object › Break into Columns makes a satin shape the two-rail columns
  it's sewn as, to reorder and reshape.
- Settings › General can hide the advanced settings in the Stitch and
  Document panels.
- Annotate pins comments on the canvas for the agent; each prompt keeps
  the design before it, to flip back to, set side by side or revert to;
  and the agent can make versions to choose from.
- Agents can reshape a column's rails and rungs, turn text into shapes,
  and measure whether a change made the stitching cleaner.
- Issues checks the stitches themselves: too short, too long, pieces a
  trimmer can miss, piled needle points and thread.
- Scaled text says its new height, new text starts in the last font, and
  new objects in a reopened design no longer repeat an id.

[Full release notes](packaging/releases/0.1.6.md)
· [Downloads](https://github.com/simpsoka/zigzag/releases/tag/v0.1.6)

## Previous releases

- [0.1.5](packaging/releases/0.1.5.md): Rotate, Curvature, Blob Brush,
  Lasso, Shape Builder, Knife and Eyedropper tools, Layers with the top at
  the top, a movable task bar, interface size.
- [0.1.4](packaging/releases/0.1.4.md): live corner handles, Add Outline,
  Simplify Design, draggable layers, unsaved-changes prompt on quit.

- [0.1.3](packaging/releases/0.1.3.md): machine profiles, in-app updates,
  satin lettering from filled letters, locking and hiding, more hoops.
- [0.1.2](packaging/releases/0.1.2.md): travel routed through fills and
  under later stitching, a Connect slider, and Skitch sewing fixes.
- [0.1.1](packaging/releases/0.1.1.md): faster editing and stitch planning,
  copy/paste and export fixes, and Windows builds through GitHub Actions.
- [0.1.0](packaging/releases/0.1.0.md): initial release.
