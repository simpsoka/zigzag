# Changelog

## 0.1.12 — 2026-11-25

- A machine file placed with File › Place comes in as one group named
  after the file, with its whole name (words that are also stitch types
  too, like "Red Cross Logo"); renamed and copied groups keep theirs.
- Agents can place files with `place_file`, and the command line with
  `zigzag convert … --place FILE[@X,Y]` (PES, PEC, DST, EXP, JEF or VP3).
- The Pathfinder has all of Illustrator's operations: Divide, Trim,
  Merge, Crop, Outline and Minus Back join the Shape Modes, in the
  Transform panel and Object › Pathfinder, with Repeat Pathfinder (⌘4).
  Trim and Merge take out stitching hidden under what's sewn later;
  Outline makes running stitch along every edge, sewn once.
- A group selected among other objects is one shape to the Shape Modes,
  results stay in the group their objects were in, and corners stay
  sharp.
- Bundled lettering fonts work offline, including six single-line fonts
  for running stitch; more fonts can be downloaded from the font menu.
- Use artwork as a motif for patterned fills or repeated running stitch.
- Print Worksheet includes a picture of each color stop, thread-chart
  matches, adjustable production times and an optional client proof page.
- Open Recent and Contact Support are available from the menus.
- Trim Before can be set per object; manual routes survive saving and
  reopening, and travel previews stay clear while transforming objects.
- Windows downloads include an installer and Azure-signed executables.

[Full release notes](packaging/releases/0.1.12.md)
· [Downloads](https://github.com/simpsoka/zigzag/releases/tag/v0.1.12)

## 0.1.11 — 2026-11-23

- Layers shows a picture of each layer beside its stitch type, in rows
  laid out as Illustrator's.
- Layers are dragged into groups and out of them, and one group into
  another; Object › Group and Ungroup (⌘G, ⇧⌘G).
- A group with other layers sewn between its own is a row for each run,
  each selected, hidden, locked and opened on its own.
- Added artwork no longer arrives as one group of everything in it.
- A docked panel's bottom edge sets its height, and the Stitch panel
  keeps one height whichever layer is selected.
- VP3 files open, and File › Place takes machine files.
- Agents group what they draw, and can group and ungroup objects.
- On Windows, ZigZag opens without a console window.

[Full release notes](packaging/releases/0.1.11.md)
· [Downloads](https://github.com/simpsoka/zigzag/releases/tag/v0.1.11)

## Previous releases

- [0.1.10](packaging/releases/0.1.10.md): a Setup page in the Agent panel
  to install an assistant and sign in, Antigravity as a third agent, Jump
  Stitches and Smart Guides remembered, the file checker's engine.
- [0.1.9](packaging/releases/0.1.9.md): the agent reviews a design as an
  embroiderer would, chosen trim settings are kept, pull-compensated
  fills, Auto-Hide for the Agent panel, tabs that show their stitches at
  once.
- [0.1.8](packaging/releases/0.1.8.md): sharing to zigzag.diy, opening
  shared designs, an account that syncs machines and palettes, variable
  fonts, queued messages to the agent.

- [0.1.7](packaging/releases/0.1.7.md): chats kept with each design, images
  and slash commands in the prompt, design review, Blend, files opened in
  the running window.
- [0.1.6](packaging/releases/0.1.6.md): satin lettering read stroke by
  stroke with Corners, Break into Columns, Annotate and versions for the
  agent, stitch checks in Issues.
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
