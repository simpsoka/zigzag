"""THIRD-PARTY-NOTICES.md from cargo-about's JSON (on stdin): each license
text once per distinct text, with the libraries it covers. ZigZag itself is
left out (its license is LICENSE).

Where cargo-about could only offer its generic template (a crate whose license
file it couldn't match: "<copyright holders>" left in), the crate's own
license files are quoted instead, so its real copyright notice is there; a
crate that ships none gets the template with its listed authors."""
import json
import os
import sys

ME = "zigzag"
LICENSE_FILES = ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT")


def generic(text):
    return "<copyright holders>" in text or "<year>" in text


def own_license_files(crate):
    root = os.path.dirname(crate["manifest_path"])
    found = []
    for f in sorted(os.listdir(root)):
        if f.upper().startswith(LICENSE_FILES) and os.path.isfile(os.path.join(root, f)):
            with open(os.path.join(root, f), encoding="utf-8", errors="replace") as fh:
                found.append((f, fh.read().rstrip()))
    return found


def entry(c):
    repo = f" ({c['repository']})" if c.get("repository") else ""
    return f"- {c['name']} {c['version']}{repo}"


data = json.load(sys.stdin)
sections = []  # (title, [crates], [(heading or None, text)])
for lic in data["licenses"]:
    users = sorted(
        (u["crate"] for u in lic["used_by"] if u["crate"]["name"] != ME),
        key=lambda c: (c["name"], c["version"]),
    )
    if not users:
        continue
    if not generic(lic["text"]):
        sections.append((lic["name"], users, [(None, lic["text"].rstrip())]))
        continue
    for c in users:
        files = own_license_files(c)
        if files:
            sections.append((f"{lic['name']}: {c['name']}", [c], files))
        else:
            holders = ", ".join(c.get("authors") or []) or f"the {c['name']} authors"
            text = lic["text"].replace("<copyright holders>", holders)
            text = text.replace("<year> ", "").replace("<year>", "").rstrip()
            sections.append((lic["name"], [c], [(None, text)]))

names = []
for lic in data["licenses"]:
    if lic["name"] not in names and any(u["crate"]["name"] != ME for u in lic["used_by"]):
        names.append(lic["name"])
out = [
    "# Third-party notices",
    "",
    "ZigZag is built from the open-source libraries below. Their licenses ask for",
    "these notices to go with every copy. (The code ZigZag ports from other",
    "projects is credited in NOTICE.)",
    "",
    "Licenses used:",
] + [f"- {n}" for n in names] + [""]
libraries = set()
for title, users, texts in sections:
    out += ["---", "", f"## {title}", "", "Used by:"] + [entry(c) for c in users] + [""]
    libraries.update((c["name"], c["version"]) for c in users)
    for heading, text in texts:
        if heading:
            out.append(f"{heading}:")
            out.append("")
        out += ["```", text, "```", ""]
sys.stdout.write("\n".join(out))
print(f"{len(libraries)} libraries", file=sys.stderr)
