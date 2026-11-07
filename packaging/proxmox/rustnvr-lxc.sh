#!/usr/bin/env bash
# RustNVR in a Proxmox VE container (LXC), from the shell of the Proxmox host:
#
#   bash -c "$(curl -fsSL https://raw.githubusercontent.com/Guiyomee/RustNVR/main/packaging/proxmox/rustnvr-lxc.sh)"
#
# Creates an unprivileged Debian LXC, installs FFmpeg and the latest release
# of RustNVR (packaging/install.sh: systemd service, account rustnvr,
# /etc/rustnvr, /var/lib/rustnvr), then prints the address and the password.
# Updates, later: `pct exec <ctid> -- /usr/local/bin/rustnvr-update`.
#
# Options (all optional; `--help`):
#   --ctid N  --hostname NAME  --cores N  --memory MB  --disk GB
#   --storage ID (disk of the container)  --bridge vmbr0
#   --ip dhcp | 192.168.1.50/24 --gw 192.168.1.1
#   --debian 12|13  --port 8080  --version vX.Y.Z  --password PASSWORD
#   --gpu              give the Intel/AMD card (/dev/dri) for the H.264 conversions
#   --gpu-device PATH  another card than /dev/dri/renderD128 (implies --gpu)
#   --recordings DIR   a folder of the host for the recordings (mount point mp0)
#   --archive FILE     a release archive of the host (its .sha256 next to it)
#                      instead of the download: no GitHub needed
#   --source URL       the releases of a Gitea / Forgejo server instead of
#                      GitHub (also RUSTNVR_SOURCE), e.g. from such a server:
#   RUSTNVR_SOURCE=https://gitea.example.org bash -c "$(curl -fsSL https://gitea.example.org/Guiyomee/RustNVR/raw/branch/main/packaging/proxmox/rustnvr-lxc.sh)"
set -euo pipefail
# pct exec passes the language of this shell to the container, where a new
# Debian only has C.UTF-8 (else: « perl: warning: Setting locale failed »).
export LANG=C.UTF-8 LC_ALL=C.UTF-8
unset LANGUAGE

REPO=${RUSTNVR_REPO:-Guiyomee/RustNVR}
SOURCE=${RUSTNVR_SOURCE:-https://github.com}
CTID="" HOSTNAME_CT=rustnvr CORES=2 MEMORY=2048 SWAP=512 DISK=8
STORAGE="" TEMPLATE_STORAGE=local BRIDGE=vmbr0 IP=dhcp GW="" DEBIAN=""
PORT=8080 VERSION="" PASSWORD=${RUSTNVR_WEB_PASSWORD:-} GPU=0 RECORDINGS=""
ARCHIVE=""
GPU_DEVICE=/dev/dri/renderD128

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mAttention :\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mErreur :\033[0m %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --ctid) CTID=${2:?}; shift 2 ;;
        --hostname) HOSTNAME_CT=${2:?}; shift 2 ;;
        --cores) CORES=${2:?}; shift 2 ;;
        --memory) MEMORY=${2:?}; shift 2 ;;
        --disk) DISK=${2:?}; shift 2 ;;
        --storage) STORAGE=${2:?}; shift 2 ;;
        --template-storage) TEMPLATE_STORAGE=${2:?}; shift 2 ;;
        --bridge) BRIDGE=${2:?}; shift 2 ;;
        --ip) IP=${2:?}; shift 2 ;;
        --gw) GW=${2:?}; shift 2 ;;
        --debian) DEBIAN=${2:?}; shift 2 ;;
        --port) PORT=${2:?}; shift 2 ;;
        --version) VERSION=${2:?}; shift 2 ;;
        --password) PASSWORD=${2:?}; shift 2 ;;
        --gpu) GPU=1; shift ;;
        --gpu-device) GPU=1; GPU_DEVICE=${2:?}; shift 2 ;;
        --recordings) RECORDINGS=${2:?}; shift 2 ;;
        --archive) ARCHIVE=${2:?}; shift 2 ;;
        --source) SOURCE=${2:?}; shift 2 ;;
        -h|--help) sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "option inconnue : $1 (--help)" ;;
    esac
done

SOURCE=${SOURCE%/}
# A file of the repository, on its server.
raw_url() {
    if [ "$SOURCE" = https://github.com ]; then
        echo "https://raw.githubusercontent.com/$REPO/main/$1"
    else
        echo "$SOURCE/$REPO/raw/branch/main/$1"
    fi
}

for n in CORES MEMORY DISK PORT; do
    case "${!n}" in ''|*[!0-9]*) die "--${n,,} : un nombre attendu" ;; esac
done
[ "$(id -u)" -eq 0 ] || die "à lancer en root sur l'hôte Proxmox"
for cmd in pct pveam pvesh pvesm pveversion; do
    command -v "$cmd" >/dev/null || die "$cmd introuvable : ce script se lance sur l'hôte Proxmox VE"
done
if [ -n "$RECORDINGS" ]; then
    [ -d "$RECORDINGS" ] || die "--recordings : dossier introuvable sur l'hôte : $RECORDINGS"
fi
if [ -n "$ARCHIVE" ]; then
    [ -f "$ARCHIVE" ] && [ -f "$ARCHIVE.sha256" ] || die "--archive : $ARCHIVE et $ARCHIVE.sha256 attendus"
    (cd "$(dirname "$ARCHIVE")" && sha256sum -c --quiet "$(basename "$ARCHIVE").sha256") || die "--archive : empreinte incorrecte"
fi

# --- Choices ------------------------------------------------------------------
[ -n "$CTID" ] || CTID=$(pvesh get /cluster/nextid)
pct status "$CTID" >/dev/null 2>&1 && die "le conteneur $CTID existe déjà (--ctid)"
if [ -z "$STORAGE" ]; then
    STORAGE=$(pvesm status -content rootdir | awk 'NR > 1 && $3 == "active" {print $1; exit}')
    [ -n "$STORAGE" ] || die "aucun stockage pour les conteneurs (--storage)"
fi

say "Recherche du modèle Debian"
pveam update >/dev/null
# RustNVR is published for x86-64 only; the list also offers arm64 templates.
[ "$(uname -m)" = x86_64 ] || die "seul x86-64 est publié (cet hôte : $(uname -m))"
find_template() {
    pveam available --section system \
        | awk -v v="debian-$1-standard_" '$2 ~ "^"v && $2 ~ /_amd64\.tar/ {print $2}' \
        | sort -V | tail -n 1
}
if [ -n "$DEBIAN" ]; then
    TEMPLATE=$(find_template "$DEBIAN")
else
    TEMPLATE=$(find_template 13)
    [ -n "$TEMPLATE" ] || TEMPLATE=$(find_template 12)
fi
[ -n "$TEMPLATE" ] || die "aucun modèle Debian ${DEBIAN:-12/13} disponible (pveam available)"
if ! pveam list "$TEMPLATE_STORAGE" | grep -qF "$TEMPLATE"; then
    say "Téléchargement du modèle $TEMPLATE"
    pveam download "$TEMPLATE_STORAGE" "$TEMPLATE" >/dev/null
fi

NET="name=eth0,bridge=$BRIDGE,ip=$IP"
[ -n "$GW" ] && NET="$NET,gw=$GW"

# --- The container ------------------------------------------------------------
say "Création du conteneur $CTID ($HOSTNAME_CT : $CORES cœurs, $MEMORY Mo, $DISK Go sur $STORAGE)"
pct create "$CTID" "$TEMPLATE_STORAGE:vztmpl/$TEMPLATE" \
    --hostname "$HOSTNAME_CT" --ostype debian \
    --cores "$CORES" --memory "$MEMORY" --swap "$SWAP" \
    --rootfs "$STORAGE:$DISK" --net0 "$NET" \
    --unprivileged 1 --features nesting=1 --onboot 1 \
    --description "RustNVR — $SOURCE/$REPO" >/dev/null

if [ -n "$RECORDINGS" ]; then
    say "Enregistrements dans $RECORDINGS (point de montage mp0)"
    pct set "$CTID" -mp0 "$RECORDINGS,mp=/var/lib/rustnvr/recordings"
fi

say "Démarrage du conteneur"
pct start "$CTID" || die "le conteneur $CTID ne démarre pas (raison : pct start $CTID --debug). Pour recommencer : pct destroy $CTID"
for _ in $(seq 1 60); do
    pct exec "$CTID" -- getent hosts github.com >/dev/null 2>&1 && break
    sleep 2
done
pct exec "$CTID" -- getent hosts github.com >/dev/null 2>&1 \
    || die "le conteneur n'a pas accès à Internet (réseau $NET)"

# The account first: its identifiers are needed for the mount point and the
# graphics card. In an unprivileged container, the user N of the container is
# the user 100000 + N of the host.
pct exec "$CTID" -- useradd --system --home-dir /var/lib/rustnvr --shell /usr/sbin/nologin rustnvr
CT_UID=$(pct exec "$CTID" -- id -u rustnvr)
CT_GID=$(pct exec "$CTID" -- id -g rustnvr)
if [ -n "$RECORDINGS" ]; then
    chown "$((100000 + CT_UID)):$((100000 + CT_GID))" "$RECORDINGS"
    say "Dossier $RECORDINGS donné au compte rustnvr du conteneur ($((100000 + CT_UID)) sur l'hôte)"
fi

PACKAGES="curl ca-certificates ffmpeg"
WHAT="FFmpeg"
if [ "$GPU" = 1 ]; then
    PACKAGES="$PACKAGES intel-media-va-driver i965-va-driver mesa-va-drivers"
    WHAT="FFmpeg et des pilotes VAAPI"
fi
say "Installation de $WHAT (quelques minutes)"
pct exec "$CTID" -- bash -c "apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends $PACKAGES >/dev/null"

if [ "$GPU" = 1 ]; then
    if [ ! -e "$GPU_DEVICE" ]; then
        warn "pas de $GPU_DEVICE sur l'hôte : carte graphique non transmise"
    else
        PVE=$(pveversion | sed -n 's|^pve-manager/\([0-9]*\)\.\([0-9]*\).*|\1 \2|p')
        read -r PVE_MAJOR PVE_MINOR <<<"${PVE:-0 0}"
        if [ "$PVE_MAJOR" -gt 8 ] || { [ "$PVE_MAJOR" -eq 8 ] && [ "$PVE_MINOR" -ge 2 ]; }; then
            RENDER_GID=$(pct exec "$CTID" -- getent group render | cut -d: -f3)
            say "Carte graphique transmise ($GPU_DEVICE, groupe render $RENDER_GID)"
            pct set "$CTID" -dev0 "$GPU_DEVICE,gid=${RENDER_GID:-0}"
            pct reboot "$CTID"
            for _ in $(seq 1 30); do
                pct exec "$CTID" -- true >/dev/null 2>&1 && break
                sleep 2
            done
        else
            warn "Proxmox VE 8.2 ou plus est nécessaire pour transmettre /dev/dri avec ce script : voir docs/INSTALLATION.md"
        fi
    fi
fi

# --- RustNVR ------------------------------------------------------------------
say "Installation de RustNVR"
ARGS=(--port "$PORT")
if [ -n "$ARCHIVE" ]; then
    # A local archive: copied into the container, its own install.sh used.
    NAME=$(basename "$ARCHIVE")
    pct push "$CTID" "$ARCHIVE" "/root/$NAME"
    pct push "$CTID" "$ARCHIVE.sha256" "/root/$NAME.sha256"
    pct exec "$CTID" -- bash -c "tar -xzf '/root/$NAME' -C /root --wildcards '*/install.sh' && mv /root/rustnvr-*/install.sh /root/rustnvr-install.sh && rm -rf /root/rustnvr-*/"
    ARGS+=(--archive "/root/$NAME")
else
    [ -n "$VERSION" ] && ARGS+=(--version "$VERSION")
    pct exec "$CTID" -- bash -c "curl -fsSL '$(raw_url packaging/install.sh)' -o /root/rustnvr-install.sh"
fi
pct exec "$CTID" -- env RUSTNVR_WEB_PASSWORD="$PASSWORD" RUSTNVR_REPO="$REPO" RUSTNVR_SOURCE="$SOURCE" bash /root/rustnvr-install.sh "${ARGS[@]}"
pct exec "$CTID" -- rm -f /root/rustnvr-install.sh
[ -n "$ARCHIVE" ] && pct exec "$CTID" -- rm -f "/root/$(basename "$ARCHIVE")" "/root/$(basename "$ARCHIVE").sha256"

echo
echo "Conteneur $CTID prêt. Mise à jour : pct exec $CTID -- /usr/local/bin/rustnvr-update"
[ -n "$ARCHIVE" ] && echo "(ou, avec une archive : pct push $CTID <archive> /root/… puis /usr/local/bin/rustnvr-update --archive /root/…)"
true
