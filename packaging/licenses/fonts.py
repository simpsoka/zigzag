"""The fonts that come with ZigZag (fonts/fonts.json), for the end of
THIRD-PARTY-NOTICES.md: each license text once, with the fonts it covers
and their copyright notices. (An OFL file's copyright lines are its own;
the license after them is the same for every font, so it's given once.)"""
import json
import os
import re

root = os.path.join(os.path.dirname(__file__), "..", "..", "fonts")
fonts = json.load(open(os.path.join(root, "fonts.json"), encoding="utf-8"))["fonts"]


def body(path):
    text = open(os.path.join(root, path), encoding="utf-8").read().rstrip()
    m = re.search(r"^This Font Software is licensed", text, re.M)
    # (The OFL from its first line; anything else whole.)
    return text[m.start():] if m else text


def same(text):
    """What a license says, not how it's spaced or linked: the OFL copies
    differ in line breaks and in http or https."""
    return re.sub(r"\s+", " ", text).replace("http://", "https://").strip()


groups = []  # (license text, [fonts])
for f in fonts:
    text = body(f["licenseFile"])
    for t, users in groups:
        if same(t) == same(text):
            users.append(f)
            break
    else:
        groups.append((text, [f]))
out = [
    "",
    "---",
    "",
    "# Fonts",
    "",
    "The lettering fonts that come with ZigZag (in its `fonts` folder), under",
    "the licenses below. Fonts downloaded later from zigzag.diy aren't part of",
    "the app; each comes under its own license, listed in the catalogue.",
    "",
]
for text, users in groups:
    names = sorted({f["license"] for f in users})
    out += ["---", "", f"## {', '.join(names)}: {', '.join(f['family'] for f in users)}", ""]
    for f in users:
        out.append(f"- {f['family']} ({f['file']}): {f['copyright'].rstrip('.')}. From {f['source']}.")
    out += ["", "```", text, "```", ""]
print("\n".join(out))
