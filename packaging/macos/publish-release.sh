#!/bin/bash
# Build and publish a signed Mac release; --existing reuses the current DMG.
set -euo pipefail
cd "$(dirname "$0")/../.."

existing=0
case "${1:-}" in
    --existing) existing=1 ;;
    -h|--help) echo "Usage: bash packaging/macos/publish-release.sh [--existing]"; exit 0 ;;
    "") ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
esac
[[ $# -le 1 ]] || { echo "Too many arguments" >&2; exit 1; }
[[ $(uname) == Darwin ]] || { echo "Run this on a Mac." >&2; exit 1; }
command -v gh >/dev/null || { echo "Install the GitHub CLI: https://cli.github.com/" >&2; exit 1; }
gh auth status >/dev/null

repo=simpsoka/zigzag
remote=$(git remote get-url origin)
case "$remote" in
    git@github.com:simpsoka/zigzag.git|git@github.com:simpsoka/zigzag|https://github.com/simpsoka/zigzag.git|https://github.com/simpsoka/zigzag) ;;
    *) echo "origin must point to $repo." >&2; exit 1 ;;
esac
[[ $(git branch --show-current) == main ]] || { echo "Check out main first." >&2; exit 1; }
[[ -z $(git status --porcelain) ]] || { echo "Commit all changes first." >&2; exit 1; }
head=$(git rev-parse HEAD)
remote_head=$(git ls-remote --exit-code origin refs/heads/main | cut -f1)
[[ "$head" == "$remote_head" ]] || { echo "Push main to origin first." >&2; exit 1; }

version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Expected a stable x.y.z version." >&2; exit 1; }
tag="v$version"
notes="packaging/releases/$version.md"
[[ -f "$notes" ]] || { echo "Write $notes first." >&2; exit 1; }
if git rev-parse --verify --quiet "refs/tags/$tag" >/dev/null; then
    echo "$tag already exists locally; refusing to replace it." >&2
    exit 1
fi
remote_tag=$(git ls-remote --tags origin "refs/tags/$tag")
[[ -z "$remote_tag" ]] || { echo "$tag already exists on GitHub." >&2; exit 1; }

if [[ $existing == 0 ]]; then
    bash packaging/macos/make-app.sh --release
fi
[[ -z $(git status --porcelain) ]] || { echo "The build changed tracked files; commit and rebuild first." >&2; exit 1; }
dmg="dist/ZigZag-$version.dmg"
[[ -f "$dmg" ]] || { echo "Missing $dmg" >&2; exit 1; }
hdiutil verify "$dmg"
codesign --verify --strict "$dmg"
xcrun stapler validate "$dmg"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
cp "$dmg" dist/ZigZag.dmg
(
    cd dist
    shasum -a 256 "ZigZag-$version.dmg" ZigZag.dmg > SHA256SUMS.txt
)

git tag -a "$tag" -m "ZigZag $version" "$head"
git push origin "refs/tags/$tag"
# Keep incomplete uploads out of the public latest-download link.
gh release create "$tag" --repo "$repo" --verify-tag --draft \
    --title "ZigZag $version" --notes-file "$notes" \
    "$dmg" dist/ZigZag.dmg dist/SHA256SUMS.txt
gh release edit "$tag" --repo "$repo" --draft=false --latest
echo "Download: https://github.com/$repo/releases/latest/download/ZigZag.dmg"
