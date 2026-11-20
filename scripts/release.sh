#!/usr/bin/env bash
# Select a version, update release metadata, commit, check, package and upload.
# Published tags are never rewritten. Use --dry-run to inspect without changes.
set -euo pipefail

cd "$(dirname "$0")/.."

usage() {
    cat <<'HELP'
Usage: scripts/release.sh [options]
  --bump patch|minor|major|none  Choose the version increment (default: recommend patch)
  --version X.Y.Z               Choose an exact version (exclusive with --bump)
  --dry-run                     Print diagnosis and proposed version; change nothing
  --replace-assets              Allow asset replacement for an existing published tag
  --no-upload                   Build packages locally without uploading
  --yes, -y                     Accept recommendations and confirmation prompts
  --skip-checks                 Skip fmt, clippy and tests
  --notes FILE                  Release notes (default: RELEASE_NOTES.md or GitHub notes)
  --help, -h                    Show this help

An untagged version is released as is. Changes since an existing tag recommend
patch; an unchanged tag keeps its version and asks before replacing assets.
Version changes update Cargo.toml, Cargo.lock, PKGBUILD and AppStream metadata,
then create a Release X.Y.Z commit. Including pending code requires confirmation.
HELP
}
fail() { echo "$*" >&2; exit 1; }
confirm() {
    local answer
    if [[ $ASSUME_YES -eq 1 ]]; then
        printf '%s [--yes]\n' "$1"
        return 0
    fi
    read -r -p "$1 [y/N] " answer || return 1
    [[ "$answer" =~ ^[yY]$ ]]
}
valid_version() { [[ "$1" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; }
step() { printf '\n\033[1m=== %s ===\033[0m\n' "$*"; }

APP_ID=io.github.wfpaisa.Magnifico
UPLOAD=1 ASSUME_YES=0 CHECKS=1 NOTES="" BUMP="" EXACT_VERSION="" DRY_RUN=0 REPLACE_ASSETS=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-upload) UPLOAD=0 ;;
        --yes|-y) ASSUME_YES=1 ;;
        --skip-checks) CHECKS=0 ;;
        --notes|--bump|--version)
            [[ $# -ge 2 && -n "$2" ]] || fail "$1 needs a value"
            case "$1" in
                --notes) NOTES="$2" ;;
                --bump) BUMP="$2" ;;
                --version) EXACT_VERSION="$2" ;;
            esac
            shift ;;
        --dry-run) DRY_RUN=1 ;;
        --replace-assets) REPLACE_ASSETS=1 ;;
        -h|--help) usage; exit 0 ;;
        *) fail "Unknown option: $1" ;;
    esac
    shift
done
[[ -z "$BUMP" || -z "$EXACT_VERSION" ]] || fail "Use either --bump or --version"
case "$BUMP" in ''|patch|minor|major|none) ;; *) fail "Invalid bump: $BUMP" ;; esac
[[ -z "$EXACT_VERSION" ]] || valid_version "$EXACT_VERSION" || fail "Expected version X.Y.Z"

CURRENT_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
valid_version "$CURRENT_VERSION" || fail "Expected Cargo version X.Y.Z"
BASE_TAG="v${CURRENT_VERSION}"
TAG_EXISTS=0 COMMITS=0 DIRTY=0 REMOTE_KNOWN=1
if git show-ref --verify --quiet "refs/tags/$BASE_TAG"; then
    TAG_EXISTS=1
    COMMITS="$(git rev-list "$BASE_TAG..HEAD" --count)"
    # A divergent checkout must not replace code already associated with a tag.
    if [[ "$(git rev-parse "$BASE_TAG^{commit}")" != "$(git rev-parse HEAD)" && $COMMITS -eq 0 ]]; then
        fail "HEAD is behind or diverges from $BASE_TAG; check out the release branch first"
    fi
fi
[[ -z "$(git status --porcelain)" ]] || DIRTY=1
if ! REMOTE_TAGS="$(git ls-remote --tags origin)"; then
    REMOTE_KNOWN=0
    [[ $DRY_RUN -eq 1 ]] || fail "Cannot inspect origin tags; release aborted"
fi
remote_has_tag() {
    awk -v ref="refs/tags/$1" '$2 == ref { found=1 } END { exit !found }' <<< "$REMOTE_TAGS"
}
HEAD_COMMIT="$(git rev-parse HEAD)"
REMOTE_BASE=unknown
REMOTE_COMMIT=""
if [[ $REMOTE_KNOWN -eq 1 ]]; then
    REMOTE_BASE=no
    if remote_has_tag "$BASE_TAG"; then
        REMOTE_BASE=yes
        REMOTE_COMMIT="$(awk -v ref="refs/tags/$BASE_TAG" '
            $2 == ref { direct=$1 }
            $2 == ref "^{}" { peeled=$1 }
            END { print (peeled != "" ? peeled : direct) }
        ' <<< "$REMOTE_TAGS")"
    fi
fi
VERSION="$CURRENT_VERSION"
RECOMMENDED=0
if [[ -z "$BUMP" && -z "$EXACT_VERSION" && $TAG_EXISTS -eq 1 && ( $COMMITS -gt 0 || $DIRTY -eq 1 ) ]]; then
    BUMP=patch
    RECOMMENDED=1
fi
if [[ -n "$EXACT_VERSION" ]]; then
    VERSION="$EXACT_VERSION"
elif [[ -n "$BUMP" && "$BUMP" != none ]]; then
    IFS=. read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"
    case "$BUMP" in
        patch) PATCH=$((PATCH + 1)) ;;
        minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
        major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
    esac
    VERSION="$MAJOR.$MINOR.$PATCH"
fi
TAG="v${VERSION}"
printf 'Current version: %s\nLocal tag: %s\nCommits since tag: %s\nDirty tree: %s\nTag on origin: %s\nProposed version: %s\n' \
    "$CURRENT_VERSION" "$TAG_EXISTS" "$COMMITS" "$DIRTY" "$REMOTE_BASE" "$VERSION"
if [[ $TAG_EXISTS -eq 1 && $COMMITS -eq 0 && $DIRTY -eq 0 ]]; then
    echo "No changes since $BASE_TAG; keeping the version unless explicitly overridden."
fi
if [[ $VERSION != "$CURRENT_VERSION" ]] && { git show-ref --verify --quiet "refs/tags/$TAG" || remote_has_tag "$TAG"; }; then
    fail "$TAG already exists; choose a new version"
fi
if [[ $VERSION == "$CURRENT_VERSION" && $REPLACE_ASSETS -eq 0 ]]; then
    if [[ "$REMOTE_BASE" == yes && ( "$REMOTE_COMMIT" != "$HEAD_COMMIT" || $DIRTY -eq 1 ) ]]; then
        fail "$TAG is published with different code; choose a new version or explicitly use --replace-assets"
    fi
    if [[ $TAG_EXISTS -eq 1 && ( $COMMITS -gt 0 || $DIRTY -eq 1 ) ]]; then
        fail "$TAG already identifies different code; choose a new version or explicitly use --replace-assets"
    fi
fi
[[ $DRY_RUN -eq 0 ]] || exit 0
if [[ $RECOMMENDED -eq 1 ]]; then
    confirm "Changes since $BASE_TAG: bump patch to $VERSION?" || { echo "Aborted."; exit 0; }
fi
if [[ -n "$NOTES" ]]; then [[ -f "$NOTES" ]] || fail "Notes file not found: $NOTES"; fi
if [[ $UPLOAD -eq 1 ]]; then
    command -v gh >/dev/null || fail "gh (GitHub CLI) is required to upload."
    gh auth status >/dev/null 2>&1 || fail "Run 'gh auth login' first."
fi
if [[ $VERSION == "$CURRENT_VERSION" && ( $TAG_EXISTS -eq 1 || "$REMOTE_BASE" == yes ) ]]; then
    confirm "Keep $TAG and replace only its assets?" || exit 0
fi

# Resolve pending changes before editing metadata, so a refusal leaves files intact.
INCLUDE_ALL=0
if [[ $DIRTY -eq 1 ]]; then
    if confirm "Include ALL pending changes with git add -A in Release $VERSION?"; then
        INCLUDE_ALL=1
    else
        fail "Commit or stash pending changes first; only version files can be committed with a clean tree"
    fi
fi
if [[ $VERSION != "$CURRENT_VERSION" ]]; then
    METAINFO="data/${APP_ID}.metainfo.xml"
    TMP_DIR="$(mktemp -d)"
    trap 'rm -rf "$TMP_DIR"' EXIT
    if [[ $TAG_EXISTS -eq 1 ]]; then
        git log --reverse --format=%s "$BASE_TAG..HEAD" > "$TMP_DIR/commits"
    else
        git log --reverse --format=%s > "$TMP_DIR/commits"
    fi
    {
        printf '    <release version="%s" date="%s">\n      <description>\n' "$VERSION" "$(date +%F)"
        if [[ -s "$TMP_DIR/commits" ]]; then
            echo '        <ul>'
            sed -e 's/\&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' \
                -e 's/^/          <li>/' -e 's|$|</li>|' "$TMP_DIR/commits"
            echo '        </ul>'
        else
            echo '        <p>Maintenance release.</p>'
        fi
        printf '      </description>\n    </release>\n'
    } > "$TMP_DIR/release"
    awk -v entry="$TMP_DIR/release" '{ print; if ($0 ~ /<releases>/) { while ((getline line < entry) > 0) print line; close(entry) } }' \
        "$METAINFO" > "$TMP_DIR/metainfo"
    sed -i "0,/^version = /s/^version = .*/version = \"$VERSION\"/" Cargo.toml
    cargo update -p magnifico --offline
    sed -i "s/^pkgver=.*/pkgver=$VERSION/" packaging/arch/PKGBUILD
    cat "$TMP_DIR/metainfo" > "$METAINFO"
fi
if [[ $INCLUDE_ALL -eq 1 ]]; then
    git add -A
elif [[ $VERSION != "$CURRENT_VERSION" ]]; then
    git add -- Cargo.toml Cargo.lock packaging/arch/PKGBUILD "data/${APP_ID}.metainfo.xml"
fi
if ! git diff --cached --quiet; then
    git commit -m "Release $VERSION"
fi
[[ -z "$(git status --porcelain)" ]] || fail "Working tree must be clean before packaging"

ARCH="$(uname -m)"
OUT="dist/release"
[[ -z "$NOTES" && -f RELEASE_NOTES.md ]] && NOTES="RELEASE_NOTES.md"
echo "Magnífico ${VERSION} (${TAG})"

step "Tools"
command -v cargo-deb >/dev/null || cargo install --locked cargo-deb
command -v cargo-generate-rpm >/dev/null || cargo install --locked cargo-generate-rpm
have_tool() { command -v "$1" >/dev/null || [[ -x "${HOME}/.local/bin/$1" ]]; }
if ! have_tool linuxdeploy || ! have_tool appimagetool; then
    scripts/setup-appimage-tools.sh
fi

# ---- checks -----------------------------------------------------------------
if [[ $CHECKS -eq 1 ]]; then
    step "Checks"
    cargo fmt --check
    cargo clippy --all-targets --locked -- -D warnings
    cargo test --locked
fi

# ---- packages ---------------------------------------------------------------
step "Cleaning old ${VERSION} artifacts"
rm -rf "$OUT"
rm -f dist/rpm/*"${VERSION}"*.rpm dist/deb/*"${VERSION}"*.deb dist/appimage/*"${VERSION}"*.AppImage
mkdir -p "$OUT"

step "RPM"
scripts/package-rpm.sh
step "DEB"
scripts/package-deb.sh
step "AppImage"
scripts/package-appimage.sh

step "Tarball"
NAME="magnifico-${VERSION}-linux-${ARCH}"
STAGE="build/tarball/${NAME}"
rm -rf build/tarball
mkdir -p "$STAGE"
install -m755 target/release/magnifico "$STAGE/magnifico"
install -m644 "data/${APP_ID}.desktop" "$STAGE/"
install -m644 "data/${APP_ID}.metainfo.xml" "$STAGE/"
install -m644 "data/icons/hicolor/scalable/apps/${APP_ID}.svg" "$STAGE/"
install -m644 "data/icons/hicolor/symbolic/apps/${APP_ID}-symbolic.svg" "$STAGE/"
install -m644 LICENSE "$STAGE/"
tar -C build/tarball -czf "${OUT}/${NAME}.tar.gz" "$NAME"

step "Collecting assets"
cp dist/rpm/*"${VERSION}"*.rpm dist/deb/*"${VERSION}"*.deb dist/appimage/*"${VERSION}"*.AppImage "$OUT/"
(cd "$OUT" && sha256sum -- * | grep -v ' SHA256SUMS$' > SHA256SUMS)
ls -lh "$OUT"

if [[ $UPLOAD -eq 0 ]]; then
    echo; echo "✅ Built ${VERSION} in ${OUT} (upload skipped)."
    exit 0
fi

# ---- upload -----------------------------------------------------------------
step "Upload"
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
if gh release view "$TAG" >/dev/null 2>&1; then
    confirm "Replace the assets of ${TAG} on GitHub?" || exit 0
else
    confirm "Push ${BRANCH} and create ${TAG} on GitHub?" || exit 0
fi

git push origin "$BRANCH"
if gh release view "$TAG" >/dev/null 2>&1; then
    gh release upload "$TAG" "$OUT"/* --clobber
else
    if [[ -n "$NOTES" ]]; then NOTES_ARGS=(--notes-file "$NOTES"); else NOTES_ARGS=(--generate-notes); fi
    gh release create "$TAG" "$OUT"/* --target "$(git rev-parse HEAD)" --title "Magnífico ${VERSION}" "${NOTES_ARGS[@]}"
fi
echo; echo "✅ Released ${TAG}: $(gh release view "$TAG" --json url -q .url)"
