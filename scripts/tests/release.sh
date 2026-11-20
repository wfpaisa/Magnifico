#!/usr/bin/env bash
# Exercise release decisions with fake Git/Cargo; never contact a remote or commit.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TEST_DIR="$(mktemp -d)"
trap 'rm -rf "$TEST_DIR"' EXIT
mkdir -p "$TEST_DIR/scripts" "$TEST_DIR/bin" "$TEST_DIR/data" "$TEST_DIR/packaging/arch"
cp "$ROOT/scripts/release.sh" "$TEST_DIR/scripts/"
cat > "$TEST_DIR/bin/git" <<'GIT'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
    show-ref) [[ $LOCAL_TAG == 1 && "${*: -1}" == refs/tags/v1.0.1 ]] ;;
    rev-list) echo "$FAKE_COMMITS" ;;
    rev-parse)
        if [[ "${*: -1}" == *'^{commit}' ]]; then echo base; else echo "$HEAD_SHA"; fi ;;
    status) if [[ $FAKE_DIRTY == 1 && ! -f .fake-committed ]]; then echo ' M pending.rs'; fi ;;
    ls-remote)
        [[ $REMOTE != error ]] || exit 1
        if [[ $REMOTE == 1 ]]; then
            printf '%s\trefs/tags/v1.0.1\n' "$REMOTE_SHA"
        elif [[ $REMOTE == annotated ]]; then
            printf 'tag-object\trefs/tags/v1.0.1\n%s\trefs/tags/v1.0.1^{}\n' "$REMOTE_SHA"
        elif [[ $REMOTE == collision ]]; then
            printf 'other\trefs/tags/v1.0.2\n'
        fi ;;
    log) echo 'Fix <Trash> & release' ;;
    diff) [[ ! -f .fake-staged ]] ;;
    add) printf '%s\n' "$*" >> actions; touch .fake-staged ;;
    commit) printf '%s\n' "$*" >> actions; touch .fake-committed ;;
    *) echo "Unexpected Git mutation: $*" >&2; exit 90 ;;
esac
GIT
cat > "$TEST_DIR/bin/cargo" <<'CARGO'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$*" == 'update -p magnifico --offline' ]]; then
    sed -n 's/^version = /version = /p' Cargo.toml > Cargo.lock
else
    echo 'Stopped before installing tools or building packages.' >&2
    exit 89
fi
CARGO
for tool in cargo-deb cargo-generate-rpm linuxdeploy appimagetool; do
    printf '#!/usr/bin/env bash\nexit 0\n' > "$TEST_DIR/bin/$tool"
done
printf '#!/usr/bin/env bash\nexit 89\n' > "$TEST_DIR/scripts/package-rpm.sh"
chmod +x "$TEST_DIR/bin/"* "$TEST_DIR/scripts/package-rpm.sh"
export PATH="$TEST_DIR/bin:$PATH"
export LOCAL_TAG=1 FAKE_COMMITS=0 FAKE_DIRTY=0 REMOTE=1 HEAD_SHA=base REMOTE_SHA=base
reset_fixture() {
    printf '[package]\nname = "magnifico"\nversion = "1.0.1"\n' > "$TEST_DIR/Cargo.toml"
    printf 'version = "1.0.1"\n' > "$TEST_DIR/Cargo.lock"
    printf 'pkgver=1.0.1\n' > "$TEST_DIR/packaging/arch/PKGBUILD"
    printf '<component>\n  <releases>\n  </releases>\n</component>\n' > "$TEST_DIR/data/io.github.wfpaisa.Magnifico.metainfo.xml"
    rm -f "$TEST_DIR/actions" "$TEST_DIR/.fake-staged" "$TEST_DIR/.fake-committed"
}
run() {
    local expected=$1 status=0
    shift
    bash "$TEST_DIR/scripts/release.sh" "$@" > "$TEST_DIR/output" 2>&1 || status=$?
    if [[ $status != "$expected" ]]; then
        cat "$TEST_DIR/output" >&2
        echo "Expected status $expected, got $status: $*" >&2
        exit 1
    fi
}
proposed() { grep -q "^Proposed version: $1$" "$TEST_DIR/output"; }
reset_fixture
LOCAL_TAG=0 REMOTE=0
run 0 --dry-run; proposed 1.0.1
LOCAL_TAG=1 REMOTE=1
run 0 --dry-run; proposed 1.0.1
FAKE_COMMITS=2 HEAD_SHA=new
run 0 --dry-run; proposed 1.0.2
run 0 --dry-run --bump minor; proposed 1.1.0
run 0 --dry-run --bump major; proposed 2.0.0
run 0 --dry-run --version 3.2.1; proposed 3.2.1
run 1 --dry-run --bump none
run 0 --dry-run --bump none --replace-assets; proposed 1.0.1
FAKE_COMMITS=0 HEAD_SHA=base FAKE_DIRTY=1
run 0 --dry-run; proposed 1.0.2
FAKE_DIRTY=0 REMOTE=annotated
run 0 --dry-run --bump none; proposed 1.0.1
REMOTE_SHA=other
run 1 --dry-run --bump none
REMOTE=collision FAKE_COMMITS=1 HEAD_SHA=new
run 1 --dry-run
REMOTE=error
run 0 --dry-run; proposed 1.0.2
run 1 --no-upload --yes
REMOTE=0
run 1 --dry-run --version 1.2
run 1 --dry-run --bump invalid
run 1 --dry-run --version 1.2.3 --bump patch
run 1 --version
[[ ! -f "$TEST_DIR/actions" ]]
grep -q 'version = "1.0.1"' "$TEST_DIR/Cargo.toml"
# Refusing to include pending code must leave metadata untouched.
FAKE_DIRTY=1
run 1 --no-upload <<< $'y\nn'
[[ ! -f "$TEST_DIR/actions" ]]
grep -q 'version = "1.0.1"' "$TEST_DIR/Cargo.toml"
# Stop at packaging, after checking all metadata and the simulated commit.
run 89 --no-upload --yes --skip-checks
for file in Cargo.toml Cargo.lock; do
    grep -q 'version = "1.0.2"' "$TEST_DIR/$file"
done
grep -q '^pkgver=1.0.2$' "$TEST_DIR/packaging/arch/PKGBUILD"
grep -q "<release version=\"1.0.2\" date=\"$(date +%F)\">" "$TEST_DIR/data/io.github.wfpaisa.Magnifico.metainfo.xml"
grep -q 'Fix &lt;Trash&gt; &amp; release' "$TEST_DIR/data/io.github.wfpaisa.Magnifico.metainfo.xml"
grep -q '^add -A$' "$TEST_DIR/actions"
grep -q '^commit -m Release 1.0.2$' "$TEST_DIR/actions"
# A clean checkout stages only version files.
reset_fixture
FAKE_DIRTY=0
run 89 --no-upload --yes --skip-checks --bump minor
grep -q '^add -- Cargo.toml Cargo.lock packaging/arch/PKGBUILD data/io.github.wfpaisa.Magnifico.metainfo.xml$' "$TEST_DIR/actions"
echo 'Release decision and metadata tests passed.'
