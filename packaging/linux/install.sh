#!/bin/sh
# Adds Modular Terrain Planner to your applications menu and opens .mtp
# files with it, for the current user. Run from the unpacked folder.
set -e
here=$(cd "$(dirname "$0")" && pwd)
bin="$HOME/.local/bin"
apps="$HOME/.local/share/applications"
icons="$HOME/.local/share/icons/hicolor/256x256/apps"
mime="$HOME/.local/share/mime/packages"
mkdir -p "$bin" "$apps" "$icons" "$mime"
ln -sf "$here/modular-terrain-planner" "$bin/modular-terrain-planner"
# The program's path goes in quotes, with the characters desktop entries
# reserve inside quotes escaped, so folders with spaces work
exe=$(printf '%s' "$here/modular-terrain-planner" | sed 's/[\\"`$]/\\&/g')
{
    grep -v '^Exec=' "$here/modular-terrain-planner.desktop"
    printf 'Exec="%s" %%f\n' "$exe"
} > "$apps/modular-terrain-planner.desktop"
cp "$here/modular-terrain-planner.png" "$icons/modular-terrain-planner.png"
cp "$here/modular-terrain-planner.xml" "$mime/modular-terrain-planner.xml"
update-mime-database "$HOME/.local/share/mime" 2>/dev/null || true
update-desktop-database "$apps" 2>/dev/null || true
xdg-mime default modular-terrain-planner.desktop application/x-modular-terrain-planner 2>/dev/null || true
echo "Installed. Start it from your applications menu, or run modular-terrain-planner."
