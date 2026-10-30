#!/bin/bash
# Double-click in Finder to build ZigZag.app and install it in /Applications.
cd "$(dirname "$0")/../.." || exit 1
echo "Building ZigZag from $(pwd)…"
echo
if packaging/macos/make-app.sh --install; then
    echo
    echo "Done. Quit ZigZag if it's open, then open it again from Applications."
else
    echo
    echo "The build failed — review the error messages above."
fi
