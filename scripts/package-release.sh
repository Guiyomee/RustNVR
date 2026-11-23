#!/usr/bin/env bash
# Builds the archive of a release from a compiled tree:
#
#   cargo build --release && (cd web && npm ci && npm run build)
#   scripts/package-release.sh 2026.11.03 [output folder, default dist]
#
# → dist/rustnvr-2026.11.03-linux-x86_64.tar.gz and its .sha256 (linux-aarch64
# with RUSTNVR_ARCH=aarch64, for a program built for ARM 64-bit), with:
#   bin/rustnvr, share/web/, rustnvr.service, rustnvr-update.path and
#   rustnvr-update.service, config.example.toml,
#   install.sh (also the `rustnvr-update` command), the licenses, and
#   lib/libonnxruntime.so with its licenses when RUSTNVR_ORT is the folder
#   made by scripts/fetch-onnxruntime.sh for the same architecture.
# RUSTNVR_BIN: the program, when not in target/release (CARGO_TARGET_DIR).
set -euo pipefail

VERSION=${1:?version attendue, par exemple 2026.11.03}
OUT=${2:-dist}
BIN=${RUSTNVR_BIN:-target/release/rustnvr}
ARCH=${RUSTNVR_ARCH:-x86_64}
case "$ARCH" in x86_64 | aarch64) ;; *) echo "RUSTNVR_ARCH : x86_64 ou aarch64" >&2; exit 1 ;; esac
NAME="rustnvr-$VERSION-linux-$ARCH"

cd "$(dirname "$0")/.."
[ -x "$BIN" ] || { echo "programme introuvable : $BIN (cargo build --release)" >&2; exit 1; }
[ -f web/dist/index.html ] || { echo "interface introuvable : web/dist (npm run build)" >&2; exit 1; }

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
DIR=$STAGE/$NAME
mkdir -p "$DIR/bin" "$DIR/share"
install -m 0755 "$BIN" "$DIR/bin/rustnvr"
if [ -n "${RUSTNVR_ORT:-}" ]; then
    [ -f "$RUSTNVR_ORT/libonnxruntime.so" ] || { echo "ONNX Runtime introuvable : $RUSTNVR_ORT/libonnxruntime.so" >&2; exit 1; }
    mkdir -p "$DIR/lib"
    install -m 0644 "$RUSTNVR_ORT/libonnxruntime.so" "$DIR/lib/libonnxruntime.so"
    install -m 0644 "$RUSTNVR_ORT/LICENSE-onnxruntime" "$RUSTNVR_ORT/ThirdPartyNotices-onnxruntime.txt" "$DIR/"
fi
cp -r web/dist "$DIR/share/web"
install -m 0644 packaging/systemd/rustnvr.service packaging/systemd/rustnvr-update.path packaging/systemd/rustnvr-update.service config.example.toml LICENSE licenses/LICENSE-MIT licenses/LICENSE-APACHE LICENSE-libopus "$DIR/"
install -m 0755 packaging/install.sh "$DIR/install.sh"

mkdir -p "$OUT"
tar -C "$STAGE" --owner=0 --group=0 --numeric-owner -czf "$OUT/$NAME.tar.gz" "$NAME"
(cd "$OUT" && sha256sum "$NAME.tar.gz" > "$NAME.tar.gz.sha256")
echo "$OUT/$NAME.tar.gz"
