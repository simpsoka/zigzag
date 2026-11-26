#!/usr/bin/env bash
# Build the Windows executables on a Mac, for trying the UI in a Windows VM.
#
#   packaging/windows/build-from-mac.sh [--x64] [--debug] [--to DIR]
#
# By default it builds for arm64 Windows, which runs natively in a
# Windows 11 ARM VM (Parallels or UTM) on Apple Silicon. --x64 builds the
# same target as the release instead, which Windows on ARM runs emulated.
# The result is staged like the release zip in
# dist/ZigZag-dev-<arch>-windows/, and with --to (or ZIGZAG_WINDOWS_DIR)
# also copied to a folder the VM can see, such as a Parallels or UTM
# shared folder.
#
# These builds are for looking at the app, not for shipping: releases come
# from .github/workflows/windows-release.yml. The icon and version
# resources are compiled in only by a build on Windows (see build.rs), so
# these .exe files show the default icon in Explorer and the taskbar.
#
# Needs: brew install llvm lld; cargo install --locked cargo-xwin.
# The first build downloads Microsoft's CRT and Windows SDK (about 1 GB)
# into cargo-xwin's cache, which means accepting Microsoft's license for
# them; the script asks before that download unless --accept-license is
# given.
set -euo pipefail

cd "$(dirname "$0")/../.."

arch=aarch64
profile=release
dest="${ZIGZAG_WINDOWS_DIR:-}"
accept_license=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --x64) arch=x86_64 ;;
    --arm64) arch=aarch64 ;;
    --debug) profile=debug ;;
    --to) dest="${2:?--to needs a folder}"; shift ;;
    --accept-license) accept_license=1 ;;
    -h|--help) sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "Unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done
target="$arch-pc-windows-msvc"

missing=()
command -v cargo-xwin >/dev/null || missing+=("cargo install --locked cargo-xwin")
llvm_bin=""
if command -v brew >/dev/null; then
  llvm_bin="$(brew --prefix llvm 2>/dev/null)/bin"
  [[ -x "$llvm_bin/clang-cl" ]] || missing+=("brew install llvm")
  command -v lld-link >/dev/null || [[ -x "$(brew --prefix lld 2>/dev/null)/bin/lld-link" ]] \
    || missing+=("brew install lld")
  export PATH="$llvm_bin:$(brew --prefix lld 2>/dev/null)/bin:$PATH"
else
  command -v clang-cl >/dev/null || missing+=("install LLVM (clang-cl, llvm-lib)")
  command -v lld-link >/dev/null || missing+=("install LLD (lld-link)")
fi
if [[ ${#missing[@]} -gt 0 ]]; then
  echo "Missing tools. Install them with:" >&2
  printf '  %s\n' "${missing[@]}" >&2
  exit 1
fi
rustup target list --installed | grep -qx "$target" || rustup target add "$target"

cache="${XWIN_CACHE_DIR:-$HOME/Library/Caches/cargo-xwin}"
if [[ ! -d "$cache/xwin" && $accept_license -eq 0 ]]; then
  echo "The first build downloads Microsoft's CRT and Windows SDK (about 1 GB)"
  echo "into $cache, under Microsoft's license for the Visual Studio Build Tools."
  read -r -p "Accept it and download? [y/N] " reply
  [[ "$reply" == [yY]* ]] || exit 1
fi

profile_flag=()
[[ $profile == release ]] && profile_flag=(--release)
# Static CRT, as in the release: the .exe files need no VC++ redistributable.
# /ignore:4099 hides warnings about debug files Microsoft leaves out of the CRT.
RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static -C link-arg=/ignore:4099" \
  cargo xwin build --locked ${profile_flag[@]+"${profile_flag[@]}"} \
  --features gui,ble --target "$target" \
  --bin zigzag-studio --bin zigzag --bin zigzag-mcp

bin="target/$target/$profile"
stage="dist/ZigZag-dev-$arch-windows"
rm -rf "$stage"
mkdir -p "$stage"
# Same names as the release zip: Windows filenames ignore case, so the CLI
# needs a name distinct from the studio's.
cp "$bin/zigzag-studio.exe" "$stage/ZigZag.exe"
cp "$bin/zigzag.exe" "$stage/zigzag-cli.exe"
cp "$bin/zigzag-mcp.exe" "$stage/"
cp packaging/windows/README.txt "$stage/"
# The lettering fonts that come with ZigZag, beside the programs.
mkdir -p "$stage/fonts"
cp fonts/fonts.json fonts/*.ttf "$stage/fonts/"
echo "Built $stage"

if [[ -n "$dest" ]]; then
  mkdir -p "$dest"
  rm -rf "${dest:?}/$(basename "$stage")"
  cp -R "$stage" "$dest/"
  echo "Copied to $dest/$(basename "$stage")"
fi
