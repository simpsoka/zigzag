#!/bin/bash
# Turn a capture-mode recording (ZIGZAG_CAPTURE=…; ⌃⌥⌘R to start and stop) into
# an MP4 and a GIF. Needs ffmpeg (brew install ffmpeg).
#
#   packaging/media/encode.sh <take folder> <output name> [speed] [gif width]
#
# speed: 1 plays as recorded, 2 twice as fast… (default 1). gif width in
# pixels (default 960). Writes <output name>.mp4 and <output name>.gif.
set -eo pipefail
take=$1 out=$2 speed=${3:-1} gifw=${4:-960}
if [[ -z "$take" || -z "$out" || ! -f "$take/frames.txt" ]]; then
    sed -n '2,9p' "$0" >&2
    exit 1
fi
command -v ffmpeg >/dev/null || { echo "ffmpeg isn't installed (brew install ffmpeg)" >&2; exit 1; }
speedup="setpts=PTS/$speed"
# MP4: H.264 at 30 fps, the frames' own timing (sped up), sizes kept even.
ffmpeg -y -loglevel error -f concat -safe 0 -i "$take/frames.txt" \
    -vf "$speedup,fps=30,scale=trunc(iw/2)*2:trunc(ih/2)*2" \
    -c:v libx264 -preset slow -crf 18 -pix_fmt yuv420p -movflags +faststart "$out.mp4"
# GIF: 15 fps, a palette made for this clip, gentle dithering for UI colors.
ffmpeg -y -loglevel error -f concat -safe 0 -i "$take/frames.txt" \
    -vf "$speedup,fps=15,scale=$gifw:-1:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=sierra2_4a:diff_mode=rectangle" \
    "$out.gif"
echo "Wrote $out.mp4 ($(du -h "$out.mp4" | cut -f1)) and $out.gif ($(du -h "$out.gif" | cut -f1))"
