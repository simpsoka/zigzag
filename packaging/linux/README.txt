ZigZag for Linux (x86-64)
========================

zigzag-studio   The visual editor. Run ./zigzag-studio to open it.
zigzag          Command-line tool. Run ./zigzag --help for commands.
zigzag-mcp      MCP bridge; keep it beside zigzag-studio for --headless mode.
LICENSE, NOTICE, THIRD-PARTY-NOTICES.md
               License and notices for the included open source code.

Requirements
------------
An x86-64 Linux desktop with glibc 2.35 or newer (Ubuntu 22.04 or newer,
Debian 12 or newer), X11 or Wayland, and OpenGL support. No Rust installation
is needed. Alpine/musl and ARM Linux are not supported by this download.

File dialogs use XDG Desktop Portal and your desktop's portal backend
(GNOME, KDE, or GTK), with Zenity for message dialogs and as a fallback.
Bluetooth requires a working Bluetooth adapter, BlueZ, and system D-Bus.
Update checks use curl; opening the download page uses xdg-open.

On Ubuntu, install the runtime dependencies if they are missing:

  sudo apt install libgl1 libegl1 libx11-6 libx11-xcb1 libxcursor1 libxrandr2 libxi6 libxcb1 \
    libxcb-render0 libxcb-shape0 libxcb-xfixes0 libxkbcommon0 \
    libxkbcommon-x11-0 libwayland-client0 libwayland-cursor0 libwayland-egl1 \
    libdbus-1-3 curl xdg-utils xdg-desktop-portal zenity bluez

Also install the portal backend appropriate for your desktop. Most desktop
installations already include it. Use your normal desktop account to run
ZigZag; Bluetooth access follows your distribution's BlueZ permissions.

Install and run
---------------
Extract the archive, open a terminal in the extracted folder, and run:

  ./zigzag-studio

Keep the folder somewhere permanent and add it to PATH to use its tools
from any terminal. For an MCP client, set its command to the absolute path
of zigzag-mcp. Add --headless to use it without an open editor window.

Updates and checksums
---------------------
Help > Check for Updates opens the latest download page. Extract the new
version into a new folder and replace the previous installation yourself.

Download SHA256SUMS-linux.txt beside the archive and verify it before
extracting:

  sha256sum --check SHA256SUMS-linux.txt
