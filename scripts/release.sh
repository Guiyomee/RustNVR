#!/usr/bin/env bash
# Builds, tests and gathers a release, from the shell of a machine with Git
# and Docker (Linux, or Git Bash with Docker Desktop):
#
#   scripts/release.sh 2026.11.05 [--notes notes.md] [--out DIR]
#                      [--previous OLD.tar.gz] [--github ../GitHub/RustNVR]
#   scripts/release.sh --rehearse    # the same for the current version, nothing published
#
# 1. checks: clean tree pushed to origin/main, version of Cargo.toml, tag free;
# 2. exports HEAD with LF line endings (git archive) and refuses any CR in it
#    (a CRLF export once shipped migrations that refused every database);
# 3. builds the interface (node:24) and the program on Debian 12 (rust:1-bookworm,
#    so that it runs on Debian 12 and 13), after fmt and clippy -D warnings with
#    that latest stable Rust (the GitHub image of 2026.11.07 was missed for a
#    warning a new Rust brought), packages the archive and its .sha256;
# 4. tests a first installation on Debian 12 and 13, and the update from the
#    latest published release (or --previous);
# 5. copies the archive, its .sha256 and the notes to --out (default
#    dist-release/VERSION);
# 6. with --github: the same code in the GitHub clone, without the internal
#    files, committed there but never pushed.
# Publishing (tag, release, push) stays a deliberate step.
set -euo pipefail

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mErreur :\033[0m %s\n' "$*" >&2; exit 1; }

VERSION="" NOTES="" OUT="" PREVIOUS="" GITHUB="" REHEARSE=0
while [ $# -gt 0 ]; do
    case "$1" in
        --notes) NOTES=${2:?}; shift 2 ;;
        --out) OUT=${2:?}; shift 2 ;;
        --previous) PREVIOUS=${2:?}; shift 2 ;;
        --github) GITHUB=${2:?}; shift 2 ;;
        --rehearse) REHEARSE=1; shift ;;
        -h|--help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) die "option inconnue : $1 (--help)" ;;
        *) VERSION=$1; shift ;;
    esac
done

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
command -v docker >/dev/null || die "docker introuvable"
docker info >/dev/null 2>&1 || die "Docker ne répond pas (Docker Desktop lancé ?)"

# Paths given to `docker -v`: Windows form under Git Bash.
host() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else printf '%s' "$1"; fi; }
run() { MSYS_NO_PATHCONV=1 docker run --rm "$@"; }

# --- 1. Checks -------------------------------------------------------------------
CARGO_VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
# 2026.11.5 in Cargo (SemVer, no leading zero) is released as 2026.11.05.
IFS=. read -r Y M N <<<"$CARGO_VERSION"
CURRENT=$(printf '%s.%s.%02d' "$Y" "$M" "$N")
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
say "Interface (node:24)"
run -v "$(host "$WORK/src"):/src" -w /src/web node:24-bookworm \
    sh -c 'npm ci --silent --no-audit --no-fund >/dev/null && npm run build >/dev/null && ls dist/index.html >/dev/null'
say "Contrôles (fmt, clippy) et programme (Debian 12 : quelques minutes)"
run -v rnvr-cargo-registry:/usr/local/cargo/registry -v rnvr-bookworm-target:/target \
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
        cp /target/release/rustnvr /out/rustnvr'
say "Archive"
run -v "$(host "$WORK/src"):/src" -v "$(host "$WORK/dist"):/out" debian:12 bash -c "
    cd /src && RUSTNVR_BIN=/out/rustnvr bash scripts/package-release.sh $VERSION /out/archive >/dev/null
    /out/rustnvr --version"
ls "$WORK/dist/archive/"*.tar.gz >/dev/null || die "archive non produite"

# --- 4. Tests ----------------------------------------------------------------------
for debian in 12 13; do
    say "Installation neuve sur Debian $debian"
    run -v "$(host "$WORK/dist/archive"):/dist:ro" -v "$(host "$WORK/src/scripts/release"):/t:ro" \
        "debian:$debian" bash /t/test-install.sh 2>&1 | grep -v "^debconf:" | tail -n 3
done
if [ -n "$PREVIOUS" ]; then
    cp "$PREVIOUS" "$WORK/old/"
else
    say "Dernière release publiée (GitHub)"
    URL=$(curl -fsSL https://api.github.com/repos/Guiyomee/RustNVR/releases/latest \
        | sed -n 's/.*"browser_download_url": *"\([^"]*\.tar\.gz\)".*/\1/p' | head -n 1)
    [ -n "$URL" ] || die "aucune archive dans la dernière release GitHub (--previous)"
    (cd "$WORK/old" && curl -fsSLO "$URL" && curl -fsSLO "$URL.sha256" && sha256sum -c --quiet ./*.sha256)
fi
say "Mise à jour depuis $(basename "$(ls "$WORK/old/"*.tar.gz)")"
run -v "$(host "$WORK/old"):/old:ro" -v "$(host "$WORK/dist/archive"):/dist:ro" \
    -v "$(host "$WORK/src/scripts/release"):/t:ro" debian:12 bash /t/test-upgrade.sh 2>&1 | grep -v "^debconf:" | tail -n 3

# --- 5. Output ---------------------------------------------------------------------
mkdir -p "$OUT"
cp "$WORK/dist/archive/"* "$OUT/"
[ -z "$NOTES" ] || cp "$NOTES" "$OUT/notes-v$VERSION.md"
(cd "$OUT" && sha256sum -c --quiet ./*.sha256)
say "Prêt dans $OUT"

# --- 6. GitHub export --------------------------------------------------------------
# Kept out of the public copy: notes for the maintainers and the homelab setup.
INTERNAL=(CLAUDE.md ROADMAP.md compose.homelab.yaml
    docs/CAHIER_DES_CHARGES.md docs/MESURES.md docs/VERIFICATION.md scripts/push-image.sh)
if [ -n "$GITHUB" ] && [ "$REHEARSE" = 0 ]; then
    say "Export vers le clone GitHub $GITHUB"
    (
        cd "$GITHUB"
        git fetch -q origin
        # Untracked files are welcome: a new workflow in .github goes with the export.
        [ -z "$(git status --porcelain --untracked-files=no)" ] || die "le clone GitHub a des modifications"
        [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "le clone GitHub n'est pas sur origin/main"
        KEEP=$(mktemp -d)
        # GitHub's own files: its compose.yaml (published image) and workflows
        # (with any new one not committed yet).
        cp compose.yaml "$KEEP/" && cp -r .github "$KEEP/"
        git rm -r -q --cached .
        find . -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
        git -C "$ROOT" -c core.autocrlf=false archive HEAD | tar -x
        cp "$KEEP/compose.yaml" . && cp -r "$KEEP/.github" . && rm -rf "$KEEP"
        rm -f "${INTERNAL[@]}"
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
fi
[ "$REHEARSE" = 0 ] || exit 0
cat <<EOF

Reste à faire, à la main :
  - Gitea : tag v$VERSION sur main, release avec les notes, l'archive et son .sha256 ($OUT)
  - GitHub : git push dans le clone, puis la release v$VERSION (l'archive y est jointe par
    le workflow release.yml, l'image par docker.yml)
EOF
