#!/bin/bash
# The SHA-256 of Cargo.lock (macOS has shasum; Linux sha256sum).
cd "$(dirname "$0")/../.."
if command -v shasum >/dev/null; then shasum -a 256 Cargo.lock; else sha256sum Cargo.lock; fi | cut -d' ' -f1
