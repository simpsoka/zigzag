#!/bin/bash
# Try the updater's install, start to finish, without a real release:
# makes a pretend "installed" ZigZag and a pretend release 99.0.0 (a disk
# image and checksums, as publish-release.sh makes them) in a scratch
# folder, and has the updater install the one over the other.
#
#   packaging/macos/test-update.sh
#
# Nothing outside the scratch folder is touched: not /Applications, not
# your settings. Needs dist/ZigZag.app (it builds one if there isn't).
set -euo pipefail
cd "$(dirname "$0")/../.."
[[ $(uname) == Darwin ]] || { echo "Run this on a Mac." >&2; exit 1; }

[[ -d dist/ZigZag.app ]] || bash packaging/macos/make-app.sh
work=$(mktemp -d "${TMPDIR:-/tmp}/zigzag-update-test.XXXXXX")
trap 'rm -rf "$work"' EXIT
new=99.0.0

# The copy that's "installed".
mkdir -p "$work/Applications"
cp -R dist/ZigZag.app "$work/Applications/"

# The release: the same app with a later version, in a disk image.
mkdir -p "$work/image" "$work/release"
cp -R dist/ZigZag.app "$work/image/"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $new" "$work/image/ZigZag.app/Contents/Info.plist"
codesign --force --deep --sign - "$work/image/ZigZag.app" 2>/dev/null
ln -s /Applications "$work/image/Applications"
dmg="$work/release/ZigZag-$new.dmg"
hdiutil create -volname "ZigZag $new" -srcfolder "$work/image" -ov -format UDZO "$dmg" >/dev/null
(cd "$work/release" && shasum -a 256 "ZigZag-$new.dmg" > SHA256SUMS.txt)
# (And one whose checksum is for something else.)
mkdir -p "$work/bad"
echo "$(printf 'x' | shasum -a 256 | cut -d' ' -f1)  ZigZag-$new.dmg" > "$work/bad/SHA256SUMS.txt"

feed() { # feed <checksums file> <out>
    cat > "$2" <<JSON
{ "tag_name": "v$new", "draft": false, "prerelease": false,
  "html_url": "https://github.com/simpsoka/zigzag/releases/tag/v$new",
  "body": "## Pretend\n\n- A release made by test-update.sh.",
  "assets": [
    { "name": "ZigZag-$new.dmg", "size": $(stat -f %z "$dmg"), "browser_download_url": "file://$dmg" },
    { "name": "SHA256SUMS.txt", "size": 1, "browser_download_url": "file://$1" }
  ] }
JSON
}
feed "$work/release/SHA256SUMS.txt" "$work/latest.json"
feed "$work/bad/SHA256SUMS.txt" "$work/latest-bad.json"

ZIGZAG_TEST_APP="$work/Applications/ZigZag.app" \
ZIGZAG_TEST_FEED="file://$work/latest.json" \
ZIGZAG_TEST_BAD_FEED="file://$work/latest-bad.json" \
    cargo test --features gui --bin zigzag-studio -- --ignored --exact update::tests::installs_a_release_made_here
echo "The updater installed $new over $(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1), and left nothing behind."
