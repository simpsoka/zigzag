#!/bin/bash
# Point the Homebrew cask at a release: writes Casks/zigzag.rb in the tap
# (github.com/simpsoka/homebrew-zigzag) for the version and the checksum of
# its disk image, and pushes it. publish-release.sh runs this after a
# release is out; run it by hand to redo one:
#
#   packaging/homebrew/update-cask.sh 0.1.3 <sha256 of ZigZag-0.1.3.dmg>
#   packaging/homebrew/update-cask.sh 0.1.3 <sha256> --print   # just show the cask
#
# With the tap, people install and update ZigZag with:
#
#   brew install --cask simpsoka/zigzag/zigzag
#
# One-time setup (the tap is an ordinary public repository; brew finds it
# by its name):
#
#   gh repo create simpsoka/homebrew-zigzag --public \
#       --description "Homebrew tap for ZigZag, the machine embroidery editor"
#
# Until it exists this says so and does nothing: a release doesn't wait
# for it.
set -euo pipefail

version=${1:-}
sha=${2:-}
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && "$sha" =~ ^[0-9a-f]{64}$ ]] || {
    echo "Usage: $0 <version x.y.z> <sha256 of ZigZag-x.y.z.dmg> [--print]" >&2
    exit 1
}
tap=simpsoka/homebrew-zigzag
IFS=. read -r major minor patch <<< "$version"

cask() {
    cat <<RUBY
cask "zigzag" do
  version "$version"
  sha256 "$sha"

  url "https://github.com/simpsoka/zigzag/releases/download/v#{version}/ZigZag-#{version}.dmg"
  name "ZigZag"
  desc "Machine embroidery editor: SVG to stitches, by hand or by agent"
  homepage "https://github.com/simpsoka/zigzag"

  livecheck do
    url :url
    strategy :github_latest
  end

RUBY
    # Releases before 0.1.3 have no updater: Homebrew must still offer
    # their upgrades without requiring --greedy.
    if (( 10#$major > 0 || 10#$minor > 1 || (10#$minor == 1 && 10#$patch >= 3) )); then
        cat <<'RUBY'
  # ZigZag updates itself (ZigZag > Check for Updates): brew leaves a
  # newer copy alone unless asked to upgrade it (--greedy).
  auto_updates true
RUBY
    fi
    cat <<'RUBY'
  depends_on :macos

  app "ZigZag.app"

  zap trash: "~/Library/Application Support/ZigZag"
end
RUBY
}

if [[ "${3:-}" == "--print" ]]; then
    cask
    exit 0
fi
command -v gh >/dev/null || { echo "Install the GitHub CLI: https://cli.github.com/" >&2; exit 1; }
if ! gh repo view "$tap" >/dev/null 2>&1; then
    echo "No Homebrew tap yet ($tap): the cask wasn't updated."
    echo "To make one:  gh repo create $tap --public"
    echo "Then:         $0 $version $sha"
    exit 0
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
gh repo clone "$tap" "$work/tap" -- --quiet 2>/dev/null || git clone --quiet "https://github.com/$tap.git" "$work/tap"
mkdir -p "$work/tap/Casks"
cask > "$work/tap/Casks/zigzag.rb"
cd "$work/tap"
if [[ ! -f README.md ]]; then
    cat > README.md <<'MARKDOWN'
# ZigZag for Homebrew

[ZigZag](https://github.com/simpsoka/zigzag) is a machine embroidery editor.
Requires macOS 11 or later, on Apple Silicon or Intel.

Install:

```sh
brew install --cask simpsoka/zigzag/zigzag
```

Update through Homebrew:

```sh
brew update
brew upgrade --cask --greedy simpsoka/zigzag/zigzag
```

ZigZag 0.1.3 and later can also install updates from inside the app:
ZigZag > Check for Updates.

The cask uses the signed, notarized DMG and its SHA-256 checksum from
[ZigZag's releases](https://github.com/simpsoka/zigzag/releases).
MARKDOWN
fi
git add -A
if git diff --cached --quiet; then
    echo "The cask is already at $version."
    exit 0
fi
git commit --quiet -m "zigzag $version"
git push --quiet origin HEAD
echo "Homebrew: brew install --cask simpsoka/zigzag/zigzag  (now $version)"
