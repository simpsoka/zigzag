ZigZag for Windows
==================

ZigZag.exe       The studio editor. Double-click to open it.
zigzag-cli.exe   Command-line tool.
zigzag-mcp.exe   MCP bridge for stdio-launching clients.
LICENSE          Apache-2.0.
NOTICE           Credits for code ZigZag ports.
THIRD-PARTY-NOTICES.md
                 Notices for the open source libraries ZigZag is built from.

Installer or zip
----------------
ZigZag-<version>-x86_64-windows-setup.exe installs these files for your
Windows account (no administrator rights needed), adds ZigZag to the Start
menu and to Open with for SVG and stitch files, and can put zigzag-cli on
the PATH. Uninstall it from Settings > Apps. The zip holds the same files
to keep wherever you like.

First run
---------
Until ZigZag's downloads are signed and Windows has seen enough of them,
SmartScreen may warn the first time one runs. Click "More info" and then
"Run anyway" to let ZigZag through.

Bluetooth to the Brother Skitch (PP1) uses Windows Runtime Bluetooth LE and
needs Windows 10 version 1709 or later.

Open from zigzag.diy
--------------------
The Open button on a zigzag.diy share page opens the design in ZigZag. The
installer sets up the zigzag:// link it uses. From the zip, ZigZag.exe
registers the link for your Windows account each time it starts
(HKCU\Software\Classes\zigzag), pointing at itself, so start it once from
where you keep it (again after moving the folder). Designs it opens from
links are saved in Downloads\ZigZag.

Put it on the PATH
------------------
The installer offers this as a choice. From the zip, copy or move this
folder somewhere permanent and add that folder to your PATH if you want the
zigzag-cli command in any terminal.

MCP for Claude Desktop
----------------------
Point claude_desktop_config.json at the full path to zigzag-mcp.exe. The
installer puts it in %LOCALAPPDATA%\Programs\ZigZag:

  {
    "mcpServers": {
      "zigzag": {
        "command": "C:\\Users\\<you>\\AppData\\Local\\Programs\\ZigZag\\zigzag-mcp.exe"
      }
    }
  }

Verifying the download
----------------------
SHA256SUMS-windows.txt on the release page lists the checksums of the
installer and the zip. Run:
  certutil -hashfile ZigZag-<version>-x86_64-windows-setup.exe SHA256
