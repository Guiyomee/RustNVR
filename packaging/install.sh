#!/usr/bin/env bash
# Installs or updates RustNVR on Linux x86-64 with systemd (Debian, Ubuntu,
# a Proxmox LXC…) from the archive of a release: nothing to compile.
#
#   install.sh                     latest release, installed or updated
#   install.sh --version v2026.11.03
#   install.sh --archive rustnvr-2026.11.03-linux-x86_64.tar.gz
#
# Copied to /usr/local/bin/rustnvr-update: later updates are just
# `rustnvr-update`. The layout is the one of docs/INSTALLATION.md:
#   /usr/local/bin/rustnvr, /usr/local/share/rustnvr/web, /etc/rustnvr
#   (config.toml, rustnvr.env), /var/lib/rustnvr (recordings, data).
# An update replaces the program and the interface only: the configuration,
# the settings made in the interface and the recordings are kept.
set -euo pipefail

# Where the releases are (GitHub, or a Gitea / Forgejo server) and the
# repository: given, else as at the installation (update.conf), else GitHub.
REPO=${RUSTNVR_REPO:-}
SOURCE=${RUSTNVR_SOURCE:-}
VERSION=""
ARCHIVE=""
START=1
ROOT=/
PORT=8080

usage() {
    sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
    cat <<'EOF'

Options:
  --version vX.Y.Z   a given release (default: the latest)
  --archive FILE     a downloaded archive (its .sha256 next to it is checked)
  --repo OWNER/NAME  repository of the releases (default Guiyomee/RustNVR)
  --source URL       server of the repository: https://github.com (default), or
                     a Gitea / Forgejo server (https://gitea.example.org)
  --port N           port of the interface at the first installation (8080)
  --no-start         do not start (or restart) the service
  --root DIR         install under DIR instead of / (tests; no account, no service)
EOF
}

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mErreur :\033[0m %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION=${2:?}; shift 2 ;;
        --archive) ARCHIVE=${2:?}; shift 2 ;;
        --repo) REPO=${2:?}; shift 2 ;;
        --source) SOURCE=${2:?}; shift 2 ;;
        --port) PORT=${2:?}; shift 2 ;;
        --no-start) START=0; shift ;;
        --root) ROOT=${2:?}; START=0; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) die "option inconnue : $1 (--help)" ;;
    esac
done

[ "$(uname -m)" = x86_64 ] || die "seul Linux x86-64 est publié (ici : $(uname -m))"
if [ "$ROOT" = / ] && [ "$(id -u)" -ne 0 ]; then
    die "à lancer en root (sudo $0)"
fi
for cmd in tar sha256sum install; do
    command -v "$cmd" >/dev/null || die "commande manquante : $cmd"
done
case "$PORT" in ''|*[!0-9]*) die "--port : un nombre attendu" ;; esac

BIN=$ROOT/usr/local/bin
SHARE=$ROOT/usr/local/share/rustnvr
ETC=$ROOT/etc/rustnvr
VAR=$ROOT/var/lib/rustnvr
UNIT=$ROOT/etc/systemd/system/rustnvr.service
if { [ -z "$SOURCE" ] || [ -z "$REPO" ]; } && [ -f "$ETC/update.conf" ]; then
    SOURCE=${SOURCE:-$(sed -n 's/^RUSTNVR_SOURCE=//p' "$ETC/update.conf")}
    REPO=${REPO:-$(sed -n 's/^RUSTNVR_REPO=//p' "$ETC/update.conf")}
fi
SOURCE=${SOURCE:-https://github.com}
SOURCE=${SOURCE%/}
REPO=${REPO:-Guiyomee/RustNVR}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# Address of the latest release, in the API of the server.
latest_url() {
    if [ "$SOURCE" = https://github.com ]; then
        echo "https://api.github.com/repos/$REPO/releases/latest"
    else
        echo "$SOURCE/api/v1/repos/$REPO/releases/latest"
    fi
}

# --- The archive ------------------------------------------------------------
if [ -z "$ARCHIVE" ]; then
    command -v curl >/dev/null || die "curl est nécessaire (apt install curl)"
    if [ -z "$VERSION" ]; then
        say "Recherche de la dernière version sur $SOURCE/$REPO"
        VERSION=$(curl -fsSL "$(latest_url)" \
            | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)
        [ -n "$VERSION" ] || die "aucune version publiée trouvée sur $SOURCE/$REPO"
    fi
    NAME="rustnvr-${VERSION#v}-linux-x86_64.tar.gz"
    URL="$SOURCE/$REPO/releases/download/$VERSION/$NAME"
    say "Téléchargement de $NAME"
    curl -fsSL -o "$WORK/$NAME" "$URL" || die "téléchargement impossible : $URL"
    curl -fsSL -o "$WORK/$NAME.sha256" "$URL.sha256" || die "empreinte introuvable : $URL.sha256"
    ARCHIVE=$WORK/$NAME
fi
[ -f "$ARCHIVE" ] || die "archive introuvable : $ARCHIVE"
[ -f "$ARCHIVE.sha256" ] || die "empreinte introuvable : $ARCHIVE.sha256"
say "Vérification de l'empreinte SHA-256"
(cd "$(dirname "$ARCHIVE")" && sha256sum -c --quiet "$(basename "$ARCHIVE").sha256") \
    || die "empreinte incorrecte : archive abîmée ou modifiée"
tar -xzf "$ARCHIVE" -C "$WORK"
SRC=$(find "$WORK" -mindepth 1 -maxdepth 1 -type d -name 'rustnvr-*' | head -n 1)
[ -n "$SRC" ] && [ -x "$SRC/bin/rustnvr" ] || die "archive inattendue (pas de bin/rustnvr)"
NEW_VERSION=$("$SRC/bin/rustnvr" --version 2>/dev/null | awk '{print $2}') \
    || die "le programme de l'archive ne démarre pas sur ce système"

FIRST=1
[ -x "$BIN/rustnvr" ] && FIRST=0
OLD_VERSION=""
[ "$FIRST" = 0 ] && OLD_VERSION=$("$BIN/rustnvr" --version 2>/dev/null | awk '{print $2}' || true)

systemd_running() {
    [ "$ROOT" = / ] && command -v systemctl >/dev/null && [ -d /run/systemd/system ]
}

# --- Account and folders (first installation) --------------------------------
if [ "$ROOT" = / ] && ! id rustnvr >/dev/null 2>&1; then
    say "Création du compte système rustnvr"
    useradd --system --home-dir /var/lib/rustnvr --shell /usr/sbin/nologin rustnvr
fi
install -d -m 0755 "$BIN" "$SHARE"
install -d -m 0750 "$ETC" "$VAR" "$VAR/recordings" "$VAR/data"
if [ "$ROOT" = / ]; then
    chown rustnvr:rustnvr "$ETC" "$VAR" "$VAR/recordings" "$VAR/data"
    # The graphics card (H.264 conversions on VAAPI), when the machine has one.
    for group in render video; do
        getent group "$group" >/dev/null && usermod -a -G "$group" rustnvr
    done
fi

# --- Program and interface ----------------------------------------------------
if [ "$FIRST" = 0 ] && systemd_running && systemctl is-active --quiet rustnvr; then
    say "Arrêt du service (les segments en cours sont terminés)"
    systemctl stop rustnvr
fi
say "Installation de RustNVR $NEW_VERSION"
install -m 0755 "$SRC/bin/rustnvr" "$BIN/rustnvr.new"
mv -f "$BIN/rustnvr.new" "$BIN/rustnvr"
rm -rf "$SHARE/web.new"
cp -r "$SRC/share/web" "$SHARE/web.new"
rm -rf "$SHARE/web.old"
[ -d "$SHARE/web" ] && mv "$SHARE/web" "$SHARE/web.old"
mv "$SHARE/web.new" "$SHARE/web"
rm -rf "$SHARE/web.old"
install -m 0644 "$SRC/config.example.toml" "$SHARE/config.example.toml"
# This script, for the next updates.
install -m 0755 "$SRC/install.sh" "$BIN/rustnvr-update"
# Where it came from, for the next `rustnvr-update`.
printf 'RUSTNVR_SOURCE=%s\nRUSTNVR_REPO=%s\n' "$SOURCE" "$REPO" > "$ETC/update.conf"
install -d -m 0755 "$(dirname "$UNIT")"
install -m 0644 "$SRC/rustnvr.service" "$UNIT"
# A graphics card (H.264 conversions) or a webcam (USB cameras): the service
# must see /dev (PrivateDevices=yes hides it); the rest of the hardening
# stays. Checked again at each update: a device added later is picked up.
DROPIN=$UNIT.d/devices.conf
if compgen -G "$ROOT/dev/dri/renderD*" >/dev/null || compgen -G "$ROOT/dev/video*" >/dev/null; then
    install -d -m 0755 "$UNIT.d"
    printf '%s\n' "# Written by install.sh: a graphics card or a webcam is present." \
        "[Service]" "PrivateDevices=no" > "$DROPIN"
    say "Carte graphique ou webcam présente : accès à /dev donné au service"
else
    rm -f "$DROPIN"
fi

# --- Configuration (first installation only) ----------------------------------
PASSWORD=""
if [ ! -f "$ETC/config.toml" ]; then
    say "Configuration de départ dans /etc/rustnvr"
    cat > "$ETC/config.toml" <<EOF
# RustNVR configuration (all options: /usr/local/share/rustnvr/config.example.toml).
# Cameras, storage destinations, MQTT and AI are set in the web interface:
# they are saved next to this file (*.ui.toml).

[storage]
recordings_dir = "/var/lib/rustnvr/recordings"
# Database, thumbnails, H.264 copies, exports.
data_dir = "/var/lib/rustnvr/data"

[web]
listen = "0.0.0.0:$PORT"
static_dir = "/usr/local/share/rustnvr/web"
# First administrator account; its password is in rustnvr.env.
username = "admin"
password_env = "RUSTNVR_WEB_PASSWORD"
# HTTPS with a certificate made for this machine (the browser warns once).
tls_self_signed = true
EOF
    chmod 0640 "$ETC/config.toml"
fi
if [ ! -f "$ETC/rustnvr.env" ]; then
    PASSWORD=${RUSTNVR_WEB_PASSWORD:-$(tr -dc 'A-Za-z0-9' </dev/urandom 2>/dev/null | head -c 20 || true)}
    install -m 0600 /dev/null "$ETC/rustnvr.env"
    printf 'RUSTNVR_WEB_PASSWORD=%s\n' "$PASSWORD" > "$ETC/rustnvr.env"
fi
if [ "$ROOT" = / ]; then
    chown rustnvr:rustnvr "$ETC/config.toml"
    say "Vérification de la configuration"
    # shellcheck disable=SC1091  # written above, read as systemd does
    (set -a; . "$ETC/rustnvr.env"; "$BIN/rustnvr" check-config -c "$ETC/config.toml") >/dev/null \
        || die "configuration refusée : /usr/local/bin/rustnvr check-config -c /etc/rustnvr/config.toml"
fi

# --- Service -------------------------------------------------------------------
if [ "$START" = 1 ] && systemd_running; then
    systemctl daemon-reload
    systemctl enable --quiet rustnvr
    systemctl restart rustnvr
    say "Service démarré (journaux : journalctl -u rustnvr -f)"
elif [ "$START" = 1 ]; then
    say "systemd ne tourne pas ici : démarrez RustNVR vous-même"
fi
command -v ffmpeg >/dev/null || [ "$ROOT" != / ] \
    || say "FFmpeg absent : détection de mouvement et captures indisponibles (apt install ffmpeg)"

# --- Summary -------------------------------------------------------------------
LISTEN=$(sed -n 's/^listen *= *"\(.*\)"/\1/p' "$ETC/config.toml" | head -n 1)
PORT_NOW=${LISTEN##*:}
IP=$(hostname -I 2>/dev/null | awk '{print $1}')
echo
if [ "$FIRST" = 1 ]; then
    echo "RustNVR $NEW_VERSION est installé."
else
    echo "RustNVR mis à jour : ${OLD_VERSION:-?} → $NEW_VERSION (configuration et enregistrements gardés)."
fi
echo "  Interface : https://${IP:-<adresse>}:${PORT_NOW:-$PORT}/"
if [ -n "$PASSWORD" ]; then
    echo "  Compte    : admin"
    echo "  Mot de passe : $PASSWORD"
    echo "  (gardé dans /etc/rustnvr/rustnvr.env ; changez-le ensuite dans l'interface)"
fi
echo "  Mise à jour : rustnvr-update"
