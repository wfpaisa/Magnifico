#!/usr/bin/env bash
# Download the tools needed by scripts/package-appimage.sh into ~/.local/bin:
# linuxdeploy, its GTK plugin and appimagetool (official upstream releases).
#   scripts/setup-appimage-tools.sh
set -euo pipefail

BIN_DIR="${BIN_DIR:-${HOME}/.local/bin}"
mkdir -p "$BIN_DIR"

fetch() {
    local url="$1" dest="$2"
    echo "Downloading ${url}"
    curl -fsSL -o "${dest}.part" "$url"
    chmod +x "${dest}.part"
    mv "${dest}.part" "$dest"
}

fetch "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage" \
    "${BIN_DIR}/linuxdeploy"
fetch "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh" \
    "${BIN_DIR}/linuxdeploy-plugin-gtk"
fetch "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage" \
    "${BIN_DIR}/appimagetool"

echo
echo "Installed in ${BIN_DIR}: linuxdeploy, linuxdeploy-plugin-gtk, appimagetool"
case ":${PATH}:" in
    *":${BIN_DIR}:"*) ;;
    *) echo "Add ${BIN_DIR} to your PATH: export PATH=\"${BIN_DIR}:\$PATH\"" ;;
esac
echo
echo "Also install the system packages the plugin relies on:"
echo "  Arch:   sudo pacman -S --needed gtk4 libadwaita librsvg patchelf squashfs-tools desktop-file-utils"
echo "  Fedora: sudo dnf install gtk4-devel libadwaita-devel librsvg2 patchelf squashfs-tools desktop-file-utils"
echo "  Debian: sudo apt install libgtk-4-dev libadwaita-1-dev librsvg2-common patchelf squashfs-tools desktop-file-utils"
echo
echo "Then build the AppImage with: scripts/package-appimage.sh"
