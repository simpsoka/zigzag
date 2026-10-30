#!/bin/bash
set -eo pipefail

cd "$(dirname "$0")"
icon_work=$(mktemp -d)
trap 'rm -rf "$icon_work"' EXIT

for appearance in AppIcon AppIcon-dark; do
    iconset="$icon_work/$appearance.iconset"
    mkdir -p "$iconset"
    for size in 16 32 128 256 512; do
        sips -z "$size" "$size" "$appearance.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
        retina_size=$((size * 2))
        sips -z "$retina_size" "$retina_size" "$appearance.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
    done
    iconutil -c icns "$iconset" -o "$appearance.icns"
done
