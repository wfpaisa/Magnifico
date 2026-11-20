#!/bin/bash
# Build Magnífico and install it for the current user only (no root).
#   scripts/install-local.sh              build and install
#   scripts/install-local.sh --uninstall  remove what this script installed
set -euo pipefail

cd "$(dirname "$0")/.."

APP_ID=io.github.wfpaisa.Magnifico
BIN_DIR="${HOME}/.local/bin"
DATA_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}"

FILES=(
    "${BIN_DIR}/magnifico"
    "${DATA_DIR}/applications/${APP_ID}.desktop"
    "${DATA_DIR}/metainfo/${APP_ID}.metainfo.xml"
    "${DATA_DIR}/icons/hicolor/scalable/apps/${APP_ID}.svg"
    "${DATA_DIR}/icons/hicolor/symbolic/apps/${APP_ID}-symbolic.svg"
)

remove_files() {
    local file
    for file in "$@"; do
        if [[ -e "$file" || -L "$file" ]]; then
            rm -f -- "$file"
            echo "Removed $file"
        fi
    done
}

refresh_caches() {
    if command -v update-desktop-database >/dev/null; then
        update-desktop-database -q "${DATA_DIR}/applications" || true
    fi
    if command -v gtk4-update-icon-cache >/dev/null; then
        gtk4-update-icon-cache -q -t -f "${DATA_DIR}/icons/hicolor" || true
    elif command -v gtk-update-icon-cache >/dev/null; then
        gtk-update-icon-cache -q -t -f "${DATA_DIR}/icons/hicolor" || true
    fi
}

if [[ "${1:-}" == "--uninstall" ]]; then
    remove_files "${FILES[@]}"
    refresh_caches
    echo "Magnífico uninstalled. Your settings and logs were kept."
    exit 0
fi

echo "=== Building release binary ==="
cargo build --release --locked

echo "=== Installing for ${USER} ==="
# Translations are compiled into the binary, so there is nothing else to build.
install -Dm755 target/release/magnifico "${BIN_DIR}/magnifico"
install -Dm644 "data/${APP_ID}.metainfo.xml" "${DATA_DIR}/metainfo/${APP_ID}.metainfo.xml"
install -Dm644 "data/icons/hicolor/scalable/apps/${APP_ID}.svg" \
    "${DATA_DIR}/icons/hicolor/scalable/apps/${APP_ID}.svg"
install -Dm644 "data/icons/hicolor/symbolic/apps/${APP_ID}-symbolic.svg" \
    "${DATA_DIR}/icons/hicolor/symbolic/apps/${APP_ID}-symbolic.svg"

# Point the launcher at the full path: the desktop session's PATH often
# lacks ~/.local/bin.
mkdir -p "${DATA_DIR}/applications"
sed "s|^Exec=magnifico|Exec=${BIN_DIR}/magnifico|" "data/${APP_ID}.desktop" \
    > "${DATA_DIR}/applications/${APP_ID}.desktop"
chmod 644 "${DATA_DIR}/applications/${APP_ID}.desktop"

refresh_caches

echo ""
echo "✅ Installed: ${BIN_DIR}/magnifico"
case ":${PATH}:" in
    *":${BIN_DIR}:"*) ;;
    *) echo "Note: ${BIN_DIR} is not in your PATH; the app menu entry still works." ;;
esac
