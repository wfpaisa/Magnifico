#!/usr/bin/env bash
# Install the latest Magnífico release from GitHub (x86_64 Linux).
#   curl -fsSL https://raw.githubusercontent.com/wfpaisa/Magnifico/main/scripts/install.sh | bash
# Picks .deb (apt), .rpm (dnf/zypper) or the generic tarball (anything else,
# e.g. Arch). Uses sudo when not run as root. Set DRY_RUN=1 to only print.
set -euo pipefail

REPO="wfpaisa/Magnifico"
APP_ID="io.github.wfpaisa.Magnifico"

[[ "$(uname -m)" == "x86_64" ]] || { echo "Only x86_64 builds are published." >&2; exit 1; }
command -v curl >/dev/null || { echo "curl is required." >&2; exit 1; }

SUDO=""
[[ $EUID -eq 0 ]] || SUDO="sudo"
run() { echo "+ $*"; [[ -n "${DRY_RUN:-}" ]] || "$@"; }

release_json="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest")"
version="$(sed -nE 's/.*"tag_name": *"v?([^"]+)".*/\1/p' <<<"$release_json" | head -n1)"
[[ -n "$version" ]] || { echo "Could not find the latest release." >&2; exit 1; }
base="https://github.com/${REPO}/releases/download/v${version}"
echo "Installing Magnífico ${version}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

if command -v apt-get >/dev/null; then
    file="magnifico_${version}-1_amd64.deb"
    curl -fsSL -o "${tmp}/${file}" "${base}/${file}"
    run $SUDO apt-get install -y "${tmp}/${file}"
elif command -v dnf >/dev/null; then
    file="magnifico-${version}-1.x86_64.rpm"
    curl -fsSL -o "${tmp}/${file}" "${base}/${file}"
    run $SUDO dnf install -y "${tmp}/${file}"
elif command -v zypper >/dev/null; then
    file="magnifico-${version}-1.x86_64.rpm"
    curl -fsSL -o "${tmp}/${file}" "${base}/${file}"
    run $SUDO zypper --non-interactive --no-gpg-checks install "${tmp}/${file}"
else
    name="magnifico-${version}-linux-x86_64"
    curl -fsSL -o "${tmp}/${name}.tar.gz" "${base}/${name}.tar.gz"
    curl -fsSL -o "${tmp}/SHA256SUMS" "${base}/SHA256SUMS"
    (cd "$tmp" && sha256sum -c --ignore-missing SHA256SUMS)
    tar -C "$tmp" -xzf "${tmp}/${name}.tar.gz"
    src="${tmp}/${name}"
    run $SUDO install -Dm755 "${src}/magnifico" /usr/local/bin/magnifico
    run $SUDO install -Dm644 "${src}/${APP_ID}.desktop" "/usr/local/share/applications/${APP_ID}.desktop"
    run $SUDO install -Dm644 "${src}/${APP_ID}.metainfo.xml" "/usr/local/share/metainfo/${APP_ID}.metainfo.xml"
    run $SUDO install -Dm644 "${src}/${APP_ID}.svg" "/usr/local/share/icons/hicolor/scalable/apps/${APP_ID}.svg"
    run $SUDO install -Dm644 "${src}/${APP_ID}-symbolic.svg" "/usr/local/share/icons/hicolor/symbolic/apps/${APP_ID}-symbolic.svg"
    echo "Make sure GTK 4.8+ and libadwaita 1.5+ are installed (Arch: pacman -S gtk4 libadwaita)."
fi

echo "Done. Launch it from the app menu or run: magnifico"
