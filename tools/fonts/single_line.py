#!/usr/bin/env python3
"""A single-line SVG font (Hershey, the EMS fonts, Relief SingleLine) as a
TrueType font ZigZag can letter with.

    tools/fonts/single_line.py IN.svg OUT.ttf ["Family Name"]

A font's outlines have to be closed, and a single-line font's strokes are
open lines. So each stroke is written out and back along itself: `a b c`
becomes the contour `a b c b`. ZigZag reads a contour that retraces itself
as the line it traces (`retraced` in src/svg.rs) and sews it as a run;
other apps draw such a font as hairlines, or nothing where they fill it.

Needs fontTools (pip install fonttools).
"""
import re
import sys
import xml.etree.ElementTree as ET

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.recordingPen import RecordingPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.svgLib.path import parse_path

SVG = "{http://www.w3.org/2000/svg}"


def strokes(d):
    """The path's strokes: lists of (op, points) segments from a start."""
    rec = RecordingPen()
    parse_path(d, rec)
    out, cur, start = [], None, None
    for op, pts in rec.value:
        if op == "moveTo":
            if cur and cur["segs"]:
                out.append(cur)
            start = pts[0]
            cur = {"start": start, "segs": []}
        elif op in ("lineTo", "qCurveTo", "curveTo"):
            cur["segs"].append((op, list(pts)))
        elif op in ("closePath", "endPath"):
            if op == "closePath" and cur and cur["segs"]:
                # A closed stroke (an o, say) goes round to where it began.
                last = cur["segs"][-1][1][-1]
                if last != start:
                    cur["segs"].append(("lineTo", [start]))
            if cur and cur["segs"]:
                out.append(cur)
            cur = None
    if cur and cur["segs"]:
        out.append(cur)
    return out


def draw_retraced(stroke, pen):
    """The stroke out, and back the way it came, as one closed contour."""
    pen.moveTo(stroke["start"])
    for op, pts in stroke["segs"]:
        getattr(pen, op)(*pts)
    # Back: each segment reversed, from the last.
    ends = [stroke["start"]] + [pts[-1] for _, pts in stroke["segs"]]
    for k in range(len(stroke["segs"]) - 1, -1, -1):
        op, pts = stroke["segs"][k]
        back = list(reversed(pts[:-1])) + [ends[k]]
        if k == 0 and op == "lineTo":
            # (The contour closes there by itself.)
            break
        getattr(pen, op)(*back)
    pen.closePath()


def main():
    src, dst = sys.argv[1], sys.argv[2]
    root = ET.parse(src).getroot()
    font = root.find(f".//{SVG}font")
    face = font.find(f"{SVG}font-face")
    family = sys.argv[3] if len(sys.argv) > 3 else face.get("font-family")
    upm = int(float(face.get("units-per-em", "1000")))
    ascent = int(float(face.get("ascent", upm * 0.8)))
    # (Below the baseline: negative, though some fonts give it as positive.)
    descent = -abs(int(float(face.get("descent", -upm * 0.2))))
    default_adv = int(float(font.get("horiz-adv-x", upm // 2)))
    meta = root.find(f"{SVG}metadata")
    notice = re.sub(r"\s+", " ", (meta.text or "").strip()) if meta is not None else ""

    order, glyphs, advances, cmap = [".notdef"], {}, {".notdef": (default_adv, 0)}, {}
    glyphs[".notdef"] = TTGlyphPen(None).glyph()
    for g in font.findall(f"{SVG}glyph"):
        uni = g.get("unicode")
        if not uni or len(uni) != 1:
            continue  # ligatures and unnamed glyphs: not needed for lettering
        code = ord(uni)
        name = f"uni{code:04X}"
        if name in glyphs:
            continue
        tt = TTGlyphPen(None)
        pen = Cu2QuPen(tt, max_err=1.0, reverse_direction=False)
        for s in strokes(g.get("d") or ""):
            draw_retraced(s, pen)
        glyphs[name] = tt.glyph()
        adv = int(float(g.get("horiz-adv-x", default_adv)))
        advances[name] = (adv, 0)
        cmap[code] = name
        order.append(name)

    fb = FontBuilder(upm, isTTF=True)
    fb.setupGlyphOrder(order)
    fb.setupCharacterMap(cmap)
    fb.setupGlyf(glyphs)
    metrics = {}
    for n in order:
        g = glyphs[n]
        g.recalcBounds(fb.font["glyf"])
        metrics[n] = (advances[n][0], getattr(g, "xMin", 0))
    fb.setupHorizontalMetrics(metrics)
    fb.setupHorizontalHeader(ascent=ascent, descent=descent)
    fb.setupNameTable({
        "familyName": family,
        "styleName": "Regular",
        # (fontdb, which ZigZag finds fonts with, skips a font without one.)
        "psName": re.sub(r"[^A-Za-z0-9-]", "", family) + "-Regular",
        "copyright": notice[:900],
        "description": "Single-line font: each stroke is drawn out and back along itself.",
    })
    fb.setupOS2(sTypoAscender=ascent, sTypoDescender=descent, usWinAscent=ascent,
                usWinDescent=-descent, fsType=0)
    fb.setupPost()
    fb.save(dst)


if __name__ == "__main__":
    main()
