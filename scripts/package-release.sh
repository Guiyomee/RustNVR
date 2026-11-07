#!/usr/bin/env bash
# Builds the archive of a release from a compiled tree:
#
#   cargo build --release && (cd web && npm ci && npm run build)
#   scripts/package-release.sh 2026.11.03 [output folder, default dist]
#
# → dist/rustnvr-2026.11.03-linux-x86_64.tar.gz and its .sha256, with:
#   bin/rustnvr, share/web/, rustnvr.service, config.example.toml,
#   install.sh (also the `rustnvr-update` command), the licenses.
# RUSTNVR_BIN: the program, when not in target/release (CARGO_TARGET_DIR).
set -euo pipefail

VERSION=${1:?version attendue, par exemple 2026.11.03}
OUT=${2:-dist}
BIN=${RUSTNVR_BIN:-target/release/rustnvr}
NAME="rustnvr-$VERSION-linux-x86_64"

cd "$(dirname "$0")/.."
[ -x "$BIN" ] || { echo "programme introuvable : $BIN (cargo build --release)" >&2; exit 1; }
[ -f web/dist/index.html ] || { echo "interface introuvable : web/dist (npm run build)" >&2; exit 1; }

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
DIR=$STAGE/$NAME
mkdir -p "$DIR/bin" "$DIR/share"
install -m 0755 "$BIN" "$DIR/bin/rustnvr"
cp -r web/dist "$DIR/share/web"
install -m 0644 packaging/systemd/rustnvr.service config.example.toml LICENSE-MIT LICENSE-APACHE "$DIR/"
install -m 0755 packaging/install.sh "$DIR/install.sh"

mkdir -p "$OUT"
tar -C "$STAGE" --owner=0 --group=0 --numeric-owner -czf "$OUT/$NAME.tar.gz" "$NAME"
(cd "$OUT" && sha256sum "$NAME.tar.gz" > "$NAME.tar.gz.sha256")
echo "$OUT/$NAME.tar.gz"
