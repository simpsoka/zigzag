#!/bin/bash
# Regenerate THIRD-PARTY-NOTICES.md (the notices of every library in the macOS
# app). Needs cargo-about: cargo install cargo-about --locked --features cli
set -eo pipefail
cd "$(dirname "$0")/../.."
# --offline: the licenses as the crates themselves state them; --fail: stop
# rather than leave out a library whose license can't be read.
# The studio's dependencies (the engine's among them), as the app is built:
# not the whole workspace, whose browser and relay crates aren't in it.
cargo about generate --manifest-path crates/studio/Cargo.toml --features ble \
    --offline --fail --format json \
    --config packaging/licenses/about.toml \
    | python3 packaging/licenses/render.py > THIRD-PARTY-NOTICES.md
# Which dependencies these notices are for (make-app.sh checks it).
printf '\n<!-- Cargo.lock: %s -->\n' "$(bash packaging/licenses/lock-hash.sh)" >> THIRD-PARTY-NOTICES.md
echo "Wrote THIRD-PARTY-NOTICES.md"
