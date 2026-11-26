# Fonts that come with ZigZag

Lettering fonts bundled with the app, so they work offline from the first launch. They're files beside the program, not built into it:
- On a Mac, in `ZigZag.app/Contents/Resources/fonts`.
- On Windows and Linux, in `fonts` beside the programs.

`src/svg.rs` (`bundled_font_dir`) finds the folder by its list, `fonts.json`. The rest of zigzag.diy's catalogue (`GET /v1/fonts`) downloads when it's first picked, or when a design names one. Downloads are kept in ZigZag's settings folder under `fonts/` (see `src/studio/fonts.rs`).

`fonts.json` lists each font's family, file, license, copyright and source. `embroidery: true` marks a font made for embroidery: the font menu shows a stitch mark beside it, and picking it sets the lettering to running stitch. The licenses are in `licenses/`. `packaging/licenses/generate.sh` adds them to THIRD-PARTY-NOTICES.md.

## Outline fonts

These are eleven Google Fonts families, the files exactly as zigzag.diy's catalogue serves them (their SHA-256 is in `fonts.json`). They were chosen on 2026-11-25 from the catalogue's files under about 170 KB, by the letter census: every letter and digit as a satin at 21 mm. Popular system fonts score 83.7% of letters clean on every measure.

| Font | Clean | | Font | Clean |
|---|---|---|---|---|
| Sacramento | 100% | | Yellowtail | 81% |
| Dancing Script | 98% | | Graduate | 79% |
| Cookie | 95% | | Staatliches | 74% |
| Bebas Neue | 92% | | Righteous | 74% |
| Satisfy | 89% | | | |
| Fredoka | 87% | | | |
| Oswald | 82% | | | |

These were left out, and stay download-only:
- Ultra (16%: heavy slabs, 25 letters filled instead of satin).
- Alfa Slab One (32%), Permanent Marker (47%), Abril Fatface (48%) and Bungee (58%).
- Cinzel, Archivo Black, Anton and Quicksand (64–69%).

To census a font that isn't installed, put it in a folder and run:

    ZIGZAG_FONT_DIR=folder ZIGZAG_FACES="Family" target/trial/deps/zigzag-<hash> --ignored letters_in_faces_of_your_own --nocapture

## Single-line fonts

Hershey Sans, Hershey Script, EMS Allure, EMS Readability, EMS Tech and Relief SingleLine are drawn as single strokes. They're for running stitch and bean stitch (more repeats).

Their sources are SVG fonts:
- Hershey and EMS: Inkscape's `svg_fonts/`.
- Relief SingleLine: `fonts/open_svg` of github.com/isdat-type/Relief-SingleLine.

A TrueType outline must be closed, so `tools/fonts/single_line.py` writes each stroke out and back along itself. ZigZag reads such a contour as the line it traces (`retraced` in `src/svg.rs`), and sews each letter's strokes as one piece of thread, going back along a stroke where they branch (`stitch::stroke::joined`). At 8 and 15 mm, 97–100% of letters sew without a trim. The exceptions are the dots of i and j, and strokes the font leaves apart: Relief's K and k, and the crossbars of EMS Allure's F and T.

    python3 tools/fonts/single_line.py HersheySans1.svg fonts/hershey-sans.ttf "Hershey Sans"

Hershey Serif was left out: its strokes are doubled, so it sews heavy and gains nothing.
