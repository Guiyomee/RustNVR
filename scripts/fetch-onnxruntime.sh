#!/usr/bin/env bash
# Fetches ONNX Runtime for the release archives: the official build of
# Microsoft, taken from its PyPI package (a manylinux_2_28 build: glibc 2.28,
# Debian 10 and later), checked against a fixed SHA-256, then reduced to:
#   libonnxruntime.so, LICENSE-onnxruntime (MIT), ThirdPartyNotices-onnxruntime.txt
#
#   scripts/fetch-onnxruntime.sh x86_64|aarch64 OUTPUT_FOLDER
#
# RustNVR loads it at start (crates/rustnvr-ai/src/ort_engine.rs); without
# it, object recognition runs on tract. Needs curl, sha256sum and python3.
set -euo pipefail

ARCH=${1:?architecture attendue : x86_64 ou aarch64}
OUT=${2:?dossier de sortie attendu}
VERSION=1.32.0
case "$ARCH" in
    x86_64)
        URL=https://files.pythonhosted.org/packages/a3/81/55845186c20cba59f614991f4065dea148e48eeb9749893768c85ce64327/onnxruntime-1.32.0-cp312-cp312-manylinux_2_28_x86_64.whl
        SHA=99e1074c4b2197accd5073052a465361be17b89c093b8ffb6e6bd8f986c52df8 ;;
    aarch64)
        URL=https://files.pythonhosted.org/packages/4a/8f/1e43fb994513c7c7a9c6965272ef89d963c1bd4080993a6267bf913701d1/onnxruntime-1.32.0-cp312-cp312-manylinux_2_28_aarch64.whl
        SHA=e36c187006e7a7c8b8d6d57f1a209dd63f52430dca6c282b177718ad6b841054 ;;
    *) echo "architecture inconnue : $ARCH" >&2; exit 1 ;;
esac

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
curl -fsSL -o "$WORK/ort.whl" "$URL"
echo "$SHA  $WORK/ort.whl" | sha256sum -c --quiet \
    || { echo "empreinte incorrecte pour ONNX Runtime $VERSION ($ARCH)" >&2; exit 1; }
mkdir -p "$OUT"
python3 - "$WORK/ort.whl" "$OUT" "$VERSION" <<'PY'
import sys, zipfile
wheel, out, version = sys.argv[1:]
z = zipfile.ZipFile(wheel)
files = {
    f"onnxruntime/capi/libonnxruntime.so.{version}": "libonnxruntime.so",
    "onnxruntime/LICENSE": "LICENSE-onnxruntime",
    "onnxruntime/ThirdPartyNotices.txt": "ThirdPartyNotices-onnxruntime.txt",
}
for name, target in files.items():
    with open(f"{out}/{target}", "wb") as f:
        f.write(z.read(name))
PY
echo "$OUT/libonnxruntime.so (ONNX Runtime $VERSION, $ARCH)"
