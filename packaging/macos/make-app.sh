#!/bin/bash
# Build ZigZag.app (and optionally a .dmg) on a Mac.
#
#   packaging/macos/make-app.sh             # dist/ZigZag.app
#   packaging/macos/make-app.sh --install   # …and copy it to /Applications
#   packaging/macos/make-app.sh --dmg       # …and make dist/ZigZag-<version>.dmg to share
#   packaging/macos/make-app.sh --universal # Apple Silicon + Intel in one app
#   packaging/macos/make-app.sh --release   # all of it, signed with your Developer ID
#                                           # and notarized: the DMG to publish
#
# --release is --universal --dmg --sign --notarize. One-time setup for it:
#   1. A "Developer ID Application" certificate in your keychain
#      (Xcode › Settings › Accounts › Manage Certificates › + › Developer ID Application).
#   2. Notarization credentials saved in the keychain under the name zigzag-notary:
#        xcrun notarytool store-credentials zigzag-notary \
#            --apple-id you@example.com --team-id YOURTEAMID
#      (it asks for an app-specific password: make one at account.apple.com).
# SIGN_IDENTITY and NOTARY_PROFILE override the certificate and the profile name.
#
# Needs Rust (rustup.rs) and the Xcode command line tools (xcode-select --install).
# Written for the bash 3.2 that ships with macOS (no empty arrays, no `set -u`).
set -eo pipefail

cd "$(dirname "$0")/../.."
if [[ "$(uname)" != "Darwin" ]]; then
    echo "This builds a macOS app, so it has to run on a Mac." >&2
    exit 1
fi

install=0 dmg=0 universal=0 sign=0 notarize=0
for arg in "$@"; do
    case "$arg" in
        --install) install=1 ;;
        --dmg) dmg=1 ;;
        --universal) universal=1 ;;
        --sign) sign=1 ;;
        --notarize) sign=1 notarize=1 ;;
        --release) universal=1 dmg=1 sign=1 notarize=1 ;;
        -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
        *) echo "unknown option: $arg" >&2; exit 1 ;;
    esac
done

# Use rustup's toolchain even when another Rust (e.g. conda's) is earlier on PATH.
if [[ -x "$HOME/.cargo/bin/cargo" ]]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi
command -v cargo >/dev/null || { echo "Rust isn't installed: see https://rustup.rs" >&2; exit 1; }

# The editor needs Rust 1.95 or newer; update through rustup if it's older.
need_minor=95
have=$(rustc --version | awk '{print $2}')
have_minor=$(echo "$have" | cut -d. -f2)
if [[ $(echo "$have" | cut -d. -f1) == 1 && $have_minor -lt $need_minor ]]; then
    if command -v rustup >/dev/null; then
        echo "Rust $have is too old (need 1.$need_minor); running: rustup update stable"
        rustup update stable
        have=$(rustc --version | awk '{print $2}')
        if [[ $(echo "$have" | cut -d. -f2) -lt $need_minor ]]; then
            echo "Still on Rust $have ($(command -v rustc)). Run 'rustup default stable' and try again." >&2
            exit 1
        fi
    else
        echo "Rust $have is too old (need 1.$need_minor). Install Rust from https://rustup.rs" >&2
        exit 1
    fi
fi

# Signing and notarizing: check the certificate and the credentials before
# the (long) build, so a missing one is found in seconds, not minutes.
identity=""
if [[ $sign == 1 ]]; then
    identity="${SIGN_IDENTITY:-}"
    if [[ -z "$identity" ]]; then
        found=$(security find-identity -v -p codesigning | grep '"Developer ID Application' || true)
        count=$(printf '%s' "$found" | grep -c . || true)
        if [[ $count == 0 ]]; then
            echo "No \"Developer ID Application\" certificate in your keychain." >&2
            echo "Add one in Xcode › Settings › Accounts › Manage Certificates › + › Developer ID Application," >&2
            echo "then run this again." >&2
            exit 1
        elif [[ $count != 1 ]]; then
            echo "More than one Developer ID Application certificate; choose one with SIGN_IDENTITY:" >&2
            printf '%s\n' "$found" >&2
            exit 1
        fi
        identity=$(printf '%s' "$found" | sed -E 's/.*"(Developer ID Application[^"]*)".*/\1/')
    fi
    echo "Signing with: $identity"
fi
profile="${NOTARY_PROFILE:-zigzag-notary}"
if [[ $notarize == 1 ]]; then
    if ! xcrun notarytool history --keychain-profile "$profile" >/dev/null 2>&1; then
        echo "Can't use the notarization credentials \"$profile\" (not saved yet, or not accepted)." >&2
        echo "Save them once with:" >&2
        echo "  xcrun notarytool store-credentials $profile --apple-id you@example.com --team-id YOURTEAMID" >&2
        echo "(it asks for an app-specific password: make one at account.apple.com)." >&2
        exit 1
    fi
    echo "Notarizing with the keychain profile: $profile"
fi

# The libraries' notices go in the app: they must be for this Cargo.lock.
lock=$(bash packaging/licenses/lock-hash.sh)
if ! grep -q "<!-- Cargo.lock: $lock -->" THIRD-PARTY-NOTICES.md 2>/dev/null; then
    if command -v cargo-about >/dev/null; then
        bash packaging/licenses/generate.sh
    else
        echo "THIRD-PARTY-NOTICES.md isn't for this Cargo.lock (the dependencies changed)." >&2
        echo "Regenerate it: cargo install cargo-about --locked --features cli," >&2
        echo "then packaging/licenses/generate.sh" >&2
        exit 1
    fi
fi

FEATURES="${FEATURES:-gui,ble}"
BINS="zigzag-studio zigzag zigzag-mcp"
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
# The oldest macOS the app runs on (Info.plist's LSMinimumSystemVersion):
# the binaries are built for it too.
export MACOSX_DEPLOYMENT_TARGET=11.0

build() { # build [target triple]
    if [[ -n "$1" ]]; then
        cargo build --release --features "$FEATURES" --target "$1" \
            --bin zigzag-studio --bin zigzag --bin zigzag-mcp
    else
        cargo build --release --features "$FEATURES" \
            --bin zigzag-studio --bin zigzag --bin zigzag-mcp
    fi
}

# Send a file to Apple's notary service and wait (a few minutes). Stops the
# script, with Apple's log of what was wrong, unless it's accepted.
notarize_file() { # notarize_file <file>
    echo "Notarizing $1 (this takes a few minutes)…"
    local out id
    out=$(xcrun notarytool submit "$1" --keychain-profile "$profile" --wait 2>&1) || true
    echo "$out"
    if ! printf '%s' "$out" | grep -q "status: Accepted"; then
        id=$(printf '%s' "$out" | awk '/^ *id: /{print $2; exit}')
        if [[ -n "$id" ]]; then
            echo "Apple's log for $id:" >&2
            xcrun notarytool log "$id" --keychain-profile "$profile" >&2 || true
        fi
        echo "Notarization of $1 failed." >&2
        exit 1
    fi
}

APP="dist/ZigZag.app"
DMG="dist/ZigZag-$version.dmg"
bash packaging/macos/make-icons.sh
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

if [[ $universal == 1 ]]; then
    rustup target add aarch64-apple-darwin x86_64-apple-darwin
    build aarch64-apple-darwin
    build x86_64-apple-darwin
    for b in $BINS; do
        lipo -create -output "$APP/Contents/MacOS/$b" \
            "target/aarch64-apple-darwin/release/$b" "target/x86_64-apple-darwin/release/$b"
    done
else
    build ""
    for b in $BINS; do cp "target/release/$b" "$APP/Contents/MacOS/"; done
fi

sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
cp packaging/macos/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
cp packaging/macos/AppIcon-dark.icns "$APP/Contents/Resources/AppIcon-dark.icns"
# The license, the credits for the code ZigZag ports, and the notices of the
# libraries it's built from go with every copy.
cp LICENSE NOTICE THIRD-PARTY-NOTICES.md "$APP/Contents/Resources/"
printf 'APPL????' > "$APP/Contents/PkgInfo"

if [[ $sign == 1 ]]; then
    # Developer ID, hardened runtime, secure timestamp: what notarization
    # requires. The helper tools first, then the app around them.
    for b in zigzag zigzag-mcp zigzag-studio; do
        codesign --force --options runtime --timestamp --sign "$identity" "$APP/Contents/MacOS/$b"
    done
    codesign --force --options runtime --timestamp --sign "$identity" "$APP"
    codesign --verify --strict --deep --verbose=2 "$APP"
else
    # Ad-hoc signature: required on Apple Silicon, and it lets macOS remember
    # the Bluetooth permission for the app. (Not notarized: on other Macs,
    # Open Anyway in System Settings › Privacy & Security the first time.)
    codesign --force --deep --sign - "$APP"
    codesign --verify --deep "$APP"
fi
echo "Built $APP ($version)"

if [[ $notarize == 1 ]]; then
    # The app itself, so it opens without a network check even out of the DMG.
    zip="dist/ZigZag-notarize.zip"
    rm -f "$zip"
    ditto -c -k --keepParent "$APP" "$zip"
    notarize_file "$zip"
    rm -f "$zip"
    xcrun stapler staple "$APP"
    spctl --assess --type execute --verbose=2 "$APP"
fi

if [[ $dmg == 1 ]]; then
    rm -f "$DMG"
    staging=$(mktemp -d)
    cp -R "$APP" "$staging/"
    cp LICENSE NOTICE THIRD-PARTY-NOTICES.md "$staging/"
    ln -s /Applications "$staging/Applications"
    hdiutil create -volname "ZigZag $version" -srcfolder "$staging" -ov -format UDZO "$DMG" >/dev/null
    rm -rf "$staging"
    if [[ $sign == 1 ]]; then
        codesign --force --timestamp --sign "$identity" "$DMG"
    fi
    if [[ $notarize == 1 ]]; then
        notarize_file "$DMG"
        xcrun stapler staple "$DMG"
        spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG"
    fi
    echo "Built $DMG"
fi

if [[ $install == 1 ]]; then
    rm -rf /Applications/ZigZag.app
    # The app was called Threadline before: don't leave the old one behind.
    rm -rf /Applications/Threadline.app
    cp -R "$APP" /Applications/
    echo "Installed /Applications/ZigZag.app"
fi

if [[ $notarize == 1 && $dmg == 1 ]]; then
    echo
    echo "Ready to publish: $DMG (Apple Silicon and Intel, signed and notarized)."
    shasum -a 256 "$DMG"
fi
