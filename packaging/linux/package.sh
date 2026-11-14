#!/usr/bin/env bash
# Package already-built Linux binaries and test the extracted download.
# Usage: bash packaging/linux/package.sh <source checkout> <version>
set -euo pipefail
[[ $# == 2 && "$2" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
    echo "Usage: $0 <source checkout> <version x.y.z>" >&2
    exit 1
}
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || {
    echo 'Package on x86-64 Linux.' >&2
    exit 1
}
source_dir=$(cd "$1" && pwd)
packaging_dir=$(cd "$(dirname "$0")" && pwd)
version=$2
name="ZigZag-$version-x86_64-linux"
bin="$source_dir/target/x86_64-unknown-linux-gnu/release"
dist="$source_dir/dist"
mkdir -p "$dist"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/$name"
for binary in zigzag-studio zigzag zigzag-mcp; do
    install -m 755 "$bin/$binary" "$work/$name/$binary"
done
cp "$source_dir/LICENSE" "$source_dir/NOTICE" "$source_dir/THIRD-PARTY-NOTICES.md" "$work/$name/"
cp "$packaging_dir/README.txt" "$work/$name/README.txt"
tar -C "$work" -czf "$dist/$name.tar.gz" "$name"
(
    cd "$dist"
    sha256sum "$name.tar.gz" > SHA256SUMS-linux.txt
    sha256sum --check SHA256SUMS-linux.txt
)

# Test what users extract, including executable permissions and shared libraries.
mkdir "$work/extracted"
tar -C "$work/extracted" -xzf "$dist/$name.tar.gz"
app="$work/extracted/$name"
evidence="$dist/linux-smoke"
rm -rf "$evidence"
mkdir -p "$evidence"
for binary in zigzag-studio zigzag zigzag-mcp; do
    test -x "$app/$binary"
    ldd "$app/$binary" > "$evidence/$binary-libraries.txt"
    if grep -q 'not found' "$evidence/$binary-libraries.txt"; then
        cat "$evidence/$binary-libraries.txt" >&2
        exit 1
    fi
done
[[ $("$app/zigzag" --version) == "zigzag $version" ]]
"$app/zigzag-studio" --help > "$evidence/studio-help.txt"
grep -q 'usage: zigzag-studio' "$evidence/studio-help.txt"
"$app/zigzag" convert "$source_dir/examples/badge.svg" -o "$evidence/badge.pes" -o "$evidence/badge.png"
"$app/zigzag" info "$evidence/badge.pes" > "$evidence/pes-info.txt"
test -s "$evidence/badge.png"

# Exercise the shipped bridge and its adjacent studio, without a display.
python3 - "$app/zigzag-mcp" <<'PY'
import json
import subprocess
import sys

request = {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
    "protocolVersion": "2024-11-05", "capabilities": {},
    "clientInfo": {"name": "linux-release-check", "version": "1"},
}}
result = subprocess.run([sys.argv[1], "--headless"], input=json.dumps(request) + "\n",
                        text=True, capture_output=True, check=True, timeout=30)
replies = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
assert any(reply.get("id") == 1 and "result" in reply for reply in replies), result
PY

# Start the real OpenGL editor under Xvfb and require a rendered screenshot.
mkdir -m 700 "$work/runtime"
timeout 60s xvfb-run -a -s '-screen 0 1440x1000x24' dbus-run-session -- \
    env -u WAYLAND_DISPLAY XDG_RUNTIME_DIR="$work/runtime" LIBGL_ALWAYS_SOFTWARE=1 \
    ZIGZAG_CAPTURE="$evidence" ZIGZAG_CAPTURE_STILL=2 ZIGZAG_CAPTURE_SHOW=settings \
    "$app/zigzag-studio" "$source_dir/examples/badge.svg" --no-mcp \
    > "$evidence/studio-launch.log" 2>&1
test -s "$evidence/still-001.png"
echo "Verified $name.tar.gz: CLI, imports/exports, MCP, and graphical launch."
