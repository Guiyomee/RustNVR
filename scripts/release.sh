#!/usr/bin/env bash
# Builds, tests and gathers a release, from the shell of a machine with Git
# and Docker (Linux, or Git Bash with Docker Desktop):
#
#   scripts/release.sh 2026.11.05 [--notes notes.md] [--out DIR]
#                      [--previous OLD.tar.gz] [--github ../GitHub/RustNVR]
#   scripts/release.sh --rehearse    # the same for the current version, nothing published
#   scripts/release.sh --github-only --github ../GitHub/RustNVR
#                                    # step 6 alone, once built and released on Gitea
#
# 1. checks: clean tree pushed to origin/main, version of Cargo.toml, tag free;
# 2. exports HEAD with LF line endings (git archive) and refuses any CR in it
#    (a CRLF export once shipped migrations that refused every database);
# 3. builds the interface (node:24) and the program on Debian 12 (rust:1-bookworm,
#    so that it runs on Debian 12 and 13), after fmt and clippy -D warnings with
#    that latest stable Rust (the GitHub image of 2026.11.07 was missed for a
#    warning a new Rust brought), runs the tests with the FFmpeg of Debian 12
#    (5.1), Ubuntu 24.04 (6.1, as GitHub) and Debian 13 (7.1), packages the
#    archive and its .sha256; then the same program cross-compiled for ARM
#    64-bit (aarch64) and its archive;
# 4. tests a first installation on Debian 12 and 13 (also on ARM when this
#    machine emulates it with QEMU), and the update from the latest
#    published release (or --previous);
# 5. copies the archives, their .sha256 and the notes to --out (default
#    dist-release/VERSION);
# 6. with --github: the same code in the GitHub clone, without the internal
#    files, committed there but never pushed.
# Publishing (tag, release, push) stays a deliberate step.
set -euo pipefail

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mErreur :\033[0m %s\n' "$*" >&2; exit 1; }

VERSION="" NOTES="" OUT="" PREVIOUS="" GITHUB="" REHEARSE=0 GITHUB_ONLY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --notes) NOTES=${2:?}; shift 2 ;;
        --out) OUT=${2:?}; shift 2 ;;
        --previous) PREVIOUS=${2:?}; shift 2 ;;
        --github) GITHUB=${2:?}; shift 2 ;;
        --github-only) GITHUB_ONLY=1; shift ;;
        --rehearse) REHEARSE=1; shift ;;
        -h|--help) sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) die "option inconnue : $1 (--help)" ;;
        *) VERSION=$1; shift ;;
    esac
done

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
CARGO_VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
# 2026.11.5 in Cargo (SemVer, no leading zero) is released as 2026.11.05.
IFS=. read -r Y M N <<<"$CARGO_VERSION"
CURRENT=$(printf '%s.%s.%02d' "$Y" "$M" "$N")

# Kept out of the public copy: notes for the maintainers and the homelab setup.
INTERNAL=(CLAUDE.md ROADMAP.md compose.homelab.yaml
    docs/CAHIER_DES_CHARGES.md docs/MESURES.md docs/PROCEDURE.md docs/VERIFICATION.md
    scripts/push-image.sh .claude .gitea)

# Step 6: HEAD replaces the files of the GitHub clone $GITHUB, without the
# internal ones, with GitHub's own files; committed there, never pushed.
export_github() {
    say "Export vers le clone GitHub $GITHUB"
    (
        cd "$GITHUB"
        git fetch -q origin
        [ -z "$(git status --porcelain)" ] || die "le clone GitHub a des modifications"
        [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "le clone GitHub n'est pas sur origin/main"
        git rm -r -q --cached .
        find . -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
        git -C "$ROOT" -c core.autocrlf=false archive HEAD | tar -x
        # GitHub's own files, kept here in packaging/github: its workflows,
        # Dependabot and its compose.yaml (published image).
        mkdir .github
        mv packaging/github/workflows packaging/github/dependabot.yml .github/
        mv packaging/github/compose.yaml compose.yaml
        rm -rf packaging/github "${INTERNAL[@]}"
        git add -A
        # Scripts stay executable (a Windows clone may have lost the bit).
        git ls-files 'scripts/*.sh' 'scripts/*.py' 'packaging/*.sh' 'packaging/**/*.sh' \
            | xargs -r git update-index --chmod=+x
        # Secrets of any project, plus the maintainer's own words (domains,
        # addresses): one regular expression per line in .release-leaks at the
        # root of this repository, never published, so never in this script.
        PATTERN='BEGIN [A-Z ]*PRIVATE KEY|ghp_[A-Za-z0-9]{20}|github_pat_[A-Za-z0-9_]{22}|discord(app)?\.com/api/webhooks/[0-9]{17,}/'
        if [ -f "$ROOT/.release-leaks" ]; then
            while IFS= read -r p; do
                p=${p%$'\r'}
                [ -n "$p" ] && [ "${p#\#}" = "$p" ] && PATTERN="$PATTERN|$p"
            done < "$ROOT/.release-leaks"
        fi
        LEAKS=$(git diff --cached -U0 | grep '^+' | grep -n -i -E "$PATTERN" | cut -c1-200 || true)
        if [ -n "$LEAKS" ]; then
            # Back to the clone as it was (untracked files stay).
            git reset -q && git checkout -q -- .
            die "données sensibles dans l'export, rien n'est commité : $LEAKS"
        fi
        git commit -q -m "RustNVR $VERSION"
        say "Commité dans le clone GitHub : $(git log --oneline -1) (rien n'est poussé)"
    )
}

# Only step 6, on the code already built and released on Gitea (the full run
# refuses once the tag exists): no Docker needed.
if [ "$GITHUB_ONLY" = 1 ]; then
    [ -n "$GITHUB" ] || die "--github-only : donnez aussi --github <clone GitHub>"
    VERSION=${VERSION:-$CURRENT}
    [ "$VERSION" = "$CURRENT" ] || die "Cargo.toml dit $CURRENT, pas $VERSION"
    [ -z "$(git status --porcelain)" ] || die "des modifications ne sont pas commitées"
    git fetch -q origin
    [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "HEAD n'est pas origin/main : poussez d'abord (git push)"
    export_github
    exit 0
fi

command -v docker >/dev/null || die "docker introuvable"
command -v python3 >/dev/null || die "python3 introuvable (ONNX Runtime : scripts/fetch-onnxruntime.sh)"
docker info >/dev/null 2>&1 || die "Docker ne répond pas (Docker Desktop lancé ?)"

# Paths given to `docker -v`: Windows form under Git Bash.
host() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else printf '%s' "$1"; fi; }
# Always the platform asked for: one docker run --platform linux/arm64 replaces
# the local debian:12 by its ARM image, which would then fail elsewhere.
run() { MSYS_NO_PATHCONV=1 docker run --rm --platform "${PLATFORM:-linux/amd64}" "$@"; }

# --- 1. Checks -------------------------------------------------------------------
if [ "$REHEARSE" = 1 ]; then
    VERSION=${VERSION:-$CURRENT}
    say "Répétition pour $VERSION : rien n'est publié"
else
    [ -n "$VERSION" ] || die "version attendue, par exemple 2026.11.05 (--help)"
    [ "$VERSION" = "$CURRENT" ] || die "Cargo.toml dit $CARGO_VERSION ($CURRENT), pas $VERSION : changez [workspace.package] version et web/package.json"
    [ -z "$(git status --porcelain)" ] || die "des modifications ne sont pas commitées"
    git fetch -q origin
    [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "HEAD n'est pas origin/main : poussez d'abord (git push)"
    ! git ls-remote --exit-code --tags origin "refs/tags/v$VERSION" >/dev/null 2>&1 || die "le tag v$VERSION existe déjà sur origin"
fi
grep -q "\"version\": \"$CARGO_VERSION\"" web/package.json || die "web/package.json n'a pas la version $CARGO_VERSION"
OUT=${OUT:-$ROOT/dist-release/$VERSION}
[ -z "$NOTES" ] || [ -f "$NOTES" ] || die "--notes : fichier introuvable : $NOTES"
[ -z "$PREVIOUS" ] || [ -f "$PREVIOUS" ] || die "--previous : fichier introuvable : $PREVIOUS"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/src" "$WORK/dist" "$WORK/old"

# --- 2. LF export ------------------------------------------------------------------
say "Export de $(git rev-parse --short HEAD) en fins de ligne LF"
git -c core.autocrlf=false archive HEAD | tar -x -C "$WORK/src"
# Checked by a Linux grep: the one of Git Bash reports a CR in every file.
CR=$(run -v "$(host "$WORK/src"):/src:ro" debian:12 \
    bash -c "cd /src && grep -rlI --exclude-dir=fixtures -e \$'\\r' . || true")
[ -z "$CR" ] || die "fins de ligne CRLF dans l'export : $CR"

# --- 3. Build ----------------------------------------------------------------------
# The latest images, not those of an earlier release kept by Docker (the
# controls must run with the latest stable Rust).
say "Images à jour (node:24, rust:1-bookworm)"
docker pull -q node:24-bookworm >/dev/null
docker pull -q rust:1-bookworm >/dev/null
say "Interface (node:24)"
run -v "$(host "$WORK/src"):/src" -w /src/web node:24-bookworm \
    sh -c 'npm ci --silent --no-audit --no-fund >/dev/null && npm run build >/dev/null && ls dist/index.html >/dev/null'
say "Contrôles (fmt, clippy) et programme (Debian 12 : quelques minutes)"
# CARGO_BUILD_JOBS=2 scripts/release.sh … on a machine short of memory.
run -e CARGO_BUILD_JOBS -v rnvr-cargo-registry:/usr/local/cargo/registry -v rnvr-bookworm-target:/target \
    -v "$(host "$WORK/src"):/src" -v "$(host "$WORK/dist"):/out" rust:1-bookworm bash -c '
        set -euo pipefail
        apt-get update -qq && apt-get install -y -qq --no-install-recommends cmake clang >/dev/null
        cd /src && export CARGO_TARGET_DIR=/target
        rustup component add clippy rustfmt >/dev/null 2>&1 || true
        rustc --version
        cargo fmt --all --check
        cargo clippy --locked --workspace --all-targets -- -D warnings 2>&1 | tail -n 1
        cargo build --release --locked -p rustnvr-app 2>&1 | tail -n 1
        cargo test --release --locked -p rustnvr-storage migrations_are 2>&1 | grep "test result"
        cp /target/release/rustnvr /out/rustnvr
        # Test programs of the workspace, run below with three FFmpeg: the
        # folder of their crate (cargo runs them from there) and their path.
        cargo test --locked --workspace --no-run --message-format=json 2>/dev/null \
            | grep "\"test\":true},\"features\"" | grep "\"executable\":\"/" \
            | sed -n "s|.*\"manifest_path\":\"\([^\"]*\)/Cargo.toml\".*\"executable\":\"\([^\"]*\)\".*|\1 \2|p" \
            > /target/tests.txt
        echo "$(wc -l < /target/tests.txt) programmes de test"'
# The time-lapse of 2026.11.10 failed with the FFmpeg 6.1 of GitHub only: the
# tests that run FFmpeg (motion, segments read back, live view, conversions,
# time-lapse, webcams) go through the three versions users have.
for image in debian:12 ubuntu:24.04 debian:13; do
    say "Tests avec le FFmpeg de $image"
    run -v rnvr-bookworm-target:/target -v "$(host "$WORK/src"):/src" "$image" bash -c '
        set -uo pipefail
        apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
            --no-install-recommends ffmpeg fonts-dejavu-core ca-certificates >/dev/null
        ffmpeg -version | head -n 1
        failed=0
        while read -r dir exe; do
            # Without stdin: a test reading it would eat the list.
            if ! (cd "$dir" && "$exe" -q < /dev/null) > /tmp/out 2>&1; then
                failed=1
                echo "--- échec : $exe"
                tail -n 60 /tmp/out
            fi
            grep "^test result" /tmp/out >> /tmp/results || true
        done < /target/tests.txt
        awk "{ok += \$4; ko += \$6; skip += \$8} END {printf \"%d réussis, %d échoués, %d ignorés\n\", ok, ko, skip}" /tmp/results
        exit $failed' || die "des tests échouent avec le FFmpeg de $image"
done
say "ONNX Runtime (bibliothèque officielle, empreinte vérifiée)"
bash "$WORK/src/scripts/fetch-onnxruntime.sh" x86_64 "$WORK/dist/ort" >/dev/null
say "Archive"
run -v "$(host "$WORK/src"):/src" -v "$(host "$WORK/dist"):/out" debian:12 bash -c "
    cd /src && RUSTNVR_BIN=/out/rustnvr RUSTNVR_ORT=/out/ort bash scripts/package-release.sh $VERSION /out/archive >/dev/null
    /out/rustnvr --version"
ls "$WORK/dist/archive/"*-linux-x86_64.tar.gz >/dev/null || die "archive non produite"

# ARM 64-bit (Raspberry Pi 4 and 5…): cross-compiled from the same Debian 12,
# the code being the one tested above.
say "Programme ARM 64 bits (aarch64, compilé depuis Debian 12)"
run -e CARGO_BUILD_JOBS -v rnvr-cargo-registry:/usr/local/cargo/registry -v rnvr-arm-target:/target \
    -v "$(host "$WORK/src"):/src" -v "$(host "$WORK/dist"):/out" rust:1-bookworm bash -c '
        set -euo pipefail
        apt-get update -qq && apt-get install -y -qq --no-install-recommends cmake clang \
            gcc-aarch64-linux-gnu g++-aarch64-linux-gnu libc6-dev-arm64-cross >/dev/null
        rustup target add aarch64-unknown-linux-gnu >/dev/null 2>&1
        cd /src && export CARGO_TARGET_DIR=/target
        export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
            CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
            CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
            AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
        cargo build --release --locked -p rustnvr-app --target aarch64-unknown-linux-gnu 2>&1 | tail -n 1
        cp /target/aarch64-unknown-linux-gnu/release/rustnvr /out/rustnvr-aarch64
        # ELF machine 0xb7: AArch64.
        [ "$(od -An -tx1 -j18 -N1 /out/rustnvr-aarch64 | tr -d " ")" = b7 ] || { echo "pas un programme aarch64" >&2; exit 1; }'
bash "$WORK/src/scripts/fetch-onnxruntime.sh" aarch64 "$WORK/dist/ort-aarch64" >/dev/null
run -v "$(host "$WORK/src"):/src" -v "$(host "$WORK/dist"):/out" debian:12 bash -c "
    cd /src && RUSTNVR_ARCH=aarch64 RUSTNVR_BIN=/out/rustnvr-aarch64 RUSTNVR_ORT=/out/ort-aarch64 \
        bash scripts/package-release.sh $VERSION /out/archive >/dev/null"
ls "$WORK/dist/archive/"*-linux-aarch64.tar.gz >/dev/null || die "archive ARM non produite"
# Run on ARM only when this machine emulates it (QEMU binfmt), e.g. after
# docker run --privileged --rm tonistiigi/binfmt --install arm64
ARM_EMULATED=0
if PLATFORM=linux/arm64 run debian:12 true >/dev/null 2>&1; then ARM_EMULATED=1; fi

# --- 4. Tests ----------------------------------------------------------------------
for debian in 12 13; do
    say "Installation neuve sur Debian $debian"
    run -v "$(host "$WORK/dist/archive"):/dist:ro" -v "$(host "$WORK/src/scripts/release"):/t:ro" \
        "debian:$debian" bash /t/test-install.sh 2>&1 | grep -v "^debconf:" | tail -n 3
done
if [ "$ARM_EMULATED" = 1 ]; then
    for debian in 12 13; do
        say "Installation neuve sur Debian $debian ARM 64 (émulé, lent)"
        PLATFORM=linux/arm64 run -v "$(host "$WORK/dist/archive"):/dist:ro" -v "$(host "$WORK/src/scripts/release"):/t:ro" \
            "debian:$debian" bash /t/test-install.sh 2>&1 | grep -v "^debconf:" | tail -n 3
    done
else
    say "Archive ARM 64 non essayée : cette machine n'émule pas ARM (QEMU)"
fi
if [ -n "$PREVIOUS" ]; then
    # With its .sha256: rustnvr-update checks it, as for a download.
    [ -f "$PREVIOUS.sha256" ] || die "--previous : empreinte introuvable : $PREVIOUS.sha256"
    cp "$PREVIOUS" "$PREVIOUS.sha256" "$WORK/old/"
    (cd "$WORK/old" && sha256sum -c --quiet ./*.sha256)
else
    say "Dernière release publiée (GitHub)"
    URL=$(curl -fsSL https://api.github.com/repos/Guiyomee/RustNVR/releases/latest \
        | sed -n 's/.*"browser_download_url": *"\([^"]*-linux-x86_64\.tar\.gz\)".*/\1/p' | head -n 1)
    [ -n "$URL" ] || die "aucune archive dans la dernière release GitHub (--previous)"
    (cd "$WORK/old" && curl -fsSLO "$URL" && curl -fsSLO "$URL.sha256" && sha256sum -c --quiet ./*.sha256)
fi
say "Mise à jour depuis $(basename "$(ls "$WORK/old/"*.tar.gz)")"
run -v "$(host "$WORK/old"):/old:ro" -v "$(host "$WORK/dist/archive"):/dist:ro" \
    -v "$(host "$WORK/src/scripts/release"):/t:ro" debian:12 bash /t/test-upgrade.sh 2>&1 | grep -v "^debconf:" | tail -n 3

# --- 5. Output ---------------------------------------------------------------------
mkdir -p "$OUT"
cp "$WORK/dist/archive/"* "$OUT/"
# The notes may already be there (written in --out).
[ -z "$NOTES" ] || [ "$NOTES" -ef "$OUT/notes-v$VERSION.md" ] || cp "$NOTES" "$OUT/notes-v$VERSION.md"
(cd "$OUT" && sha256sum -c --quiet ./*.sha256)
say "Prêt dans $OUT"

# --- 6. GitHub export --------------------------------------------------------------
if [ -n "$GITHUB" ] && [ "$REHEARSE" = 0 ]; then
    export_github
fi
[ "$REHEARSE" = 0 ] || exit 0
cat <<EOF

Reste à faire, à la main :
  - Gitea : tag v$VERSION sur main, release avec les notes, l'archive et son .sha256 ($OUT)
  - GitHub : git push dans le clone, puis la release v$VERSION (l'archive y est jointe par
    le workflow release.yml, l'image par docker.yml)
EOF
