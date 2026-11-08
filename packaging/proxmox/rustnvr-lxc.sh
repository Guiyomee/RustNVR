#!/usr/bin/env bash
# RustNVR in a Proxmox VE container (LXC), from the shell of the Proxmox host:
#
#   bash -c "$(curl -fsSL https://raw.githubusercontent.com/Guiyomee/RustNVR/main/packaging/proxmox/rustnvr-lxc.sh)"
#
# Creates a Debian LXC, installs FFmpeg and the latest release of RustNVR
# (packaging/install.sh: systemd service, account rustnvr, /etc/rustnvr,
# /var/lib/rustnvr), then prints the address and the password.
#
# In a terminal and without options, a menu offers the default settings, the
# advanced settings (every setting of the Proxmox VE Helper-Scripts, one step
# at a time with a way back, then RustNVR's own), or saved settings. The
# settings use the var_* names of the Helper-Scripts: they can be preset in
# the environment and are read from their files
# (/usr/local/community-scripts/default.vars, defaults/rustnvr.vars).
# Updates, later: `pct exec <ctid> -- /usr/local/bin/rustnvr-update`.
# The options are listed by usage() below (`--help`).
set -euo pipefail
# pct exec passes the language of this shell to the container, where a new
# Debian only has C.UTF-8 (else: « perl: warning: Setting locale failed »).
export LANG=C.UTF-8 LC_ALL=C.UTF-8
unset LANGUAGE

# The help is kept here rather than read from "$0": with
# `bash -c "$(curl …)"`, $0 is "bash", not this file.
usage() {
    cat <<'EOF'
RustNVR dans un conteneur LXC Proxmox VE (à lancer en root sur l'hôte).

Sans option, dans un terminal : menu « Paramètres par défaut / avancés /
enregistrés », puis résumé des choix avant la création. Avec des options :
rien n'est demandé. Avec la commande d'une ligne, les options se placent
après « -- » :
  bash -c "$(curl -fsSL …/rustnvr-lxc.sh)" -- --ctid 120 --gpu

Conteneur :
  --ctid N               numéro (défaut : le prochain libre)
  --hostname NOM         nom (défaut : rustnvr)
  --cores N  --memory Mo  --swap Mo  --disk Go     (défaut : 2, 2048, 512, 8)
  --storage ID           stockage du disque (défaut : le premier actif)
  --template-storage ID  stockage du modèle Debian (défaut : local)
  --debian 12|13         version de Debian (défaut : 13, sinon 12)
  --privileged           conteneur privilégié (déconseillé)
  --root-password MDP    mot de passe root (défaut : aucun, connexion
                         automatique à la console Proxmox)
  --ssh-key FICHIER      clé(s) SSH publique(s) autorisée(s) pour root
  --ssh                  accès SSH root (installe et active le serveur SSH)
  --tags LISTE           étiquettes séparées par « ; » (défaut : rustnvr)
  --timezone ZONE        fuseau horaire, ex. Europe/Paris (défaut : celui de l'hôte)
  --protection           protège le conteneur contre la suppression
  --console tty|console|shell   mode de la console Proxmox
  --no-nesting  --keyctl  --fuse  --tun  --mknod   fonctions du conteneur
  --mount-fs nfs,cifs    systèmes de fichiers montables dans le conteneur
  --post-install FICHIER script de l'hôte lancé à la fin (root, sur l'hôte)
  --verbose              affiche le détail de l'installation

Réseau :
  --bridge vmbr0         pont ou vnet SDN (défaut : vmbr0)
  --ip dhcp|IP/MASQUE    adresse IPv4 (défaut : dhcp), ex. 192.168.1.50/24
  --gw IP                passerelle IPv4
  --ipv6 none|auto|dhcp|IP/MASQUE   IPv6 (défaut : none)
  --gw6 IP               passerelle IPv6
  --mtu N  --mac AA:BB:CC:DD:EE:FF  --vlan N
  --dns "IP IP"          serveurs DNS (défaut : ceux de l'hôte)
  --searchdomain NOM     domaine de recherche DNS
  --apt-cacher IP|URL    cache apt (apt-cacher-ng, port 3142 par défaut)
  --proxy URL            proxy HTTP(S) pour l'installation
  --no-proxy LISTE       exceptions du proxy (défaut : localhost,127.0.0.1,.local)
  --host-ca              copie les autorités de /usr/local/share/ca-certificates

RustNVR :
  --port N               port de l'interface web (défaut : 8080)
  --password MDP         mot de passe du compte admin (défaut : tiré au hasard)
  --version vX.Y.Z       une version donnée (défaut : la dernière)
  --gpu                  donne la carte Intel/AMD (/dev/dri) pour les conversions H.264
  --gpu-device CHEMIN    une autre carte que /dev/dri/renderD128 (implique --gpu)
  --recordings DOSSIER   un dossier de l'hôte pour les enregistrements (point
                         de montage mp0, hors des sauvegardes vzdump)
  --smb '\\IP\partage\dossier'   un partage SMB pour les enregistrements :
                         monté sur l'hôte, donné au conteneur (/nas) et
                         déclaré destination par défaut de RustNVR
  --smb-user NOM  --smb-domain DOMAINE  (mot de passe : RUSTNVR_SMB_PASSWORD
                         ou --smb-password)
  --archive FICHIER      une archive de release de l'hôte (son .sha256 à côté)
  --source URL           les releases d'un serveur Gitea / Forgejo (aussi RUSTNVR_SOURCE)
  -y, --yes              pas de menu, même dans un terminal
  -h, --help             cette aide

Les réglages peuvent aussi venir de l'environnement, avec les noms des
Proxmox VE Helper-Scripts : var_cpu=4 var_ram=4096 var_disk=16 bash -c "…"
Mise à jour : pct exec <ctid> -- /usr/local/bin/rustnvr-update
EOF
}

REPO=${RUSTNVR_REPO:-Guiyomee/RustNVR}
SOURCE=${RUSTNVR_SOURCE:-https://github.com}
CS_DIR=/usr/local/community-scripts
USER_DEFAULTS=$CS_DIR/default.vars
APP_DEFAULTS=$CS_DIR/defaults/rustnvr.vars

# --- Settings -----------------------------------------------------------------
# Names of the Proxmox VE Helper-Scripts (var_*), RustNVR's own after them;
# a value already in the environment wins over the default.
: "${var_ctid:=}" "${var_hostname:=rustnvr}"
: "${var_cpu:=2}" "${var_ram:=2048}" "${var_swap:=512}" "${var_disk:=8}"
: "${var_unprivileged:=1}" "${var_pw:=}"
: "${var_container_storage:=}" "${var_template_storage:=local}" "${var_version:=}"
: "${var_brg:=vmbr0}" "${var_net:=dhcp}" "${var_gateway:=}"
: "${var_ipv6_method:=none}" "${var_ipv6_static:=}" "${var_ipv6_gateway:=}"
: "${var_mtu:=}" "${var_searchdomain:=}" "${var_ns:=}" "${var_mac:=}" "${var_vlan:=}"
: "${var_tags:=rustnvr}" "${var_ssh:=no}" "${var_ssh_authorized_key:=}"
: "${var_nesting:=1}" "${var_keyctl:=0}" "${var_fuse:=no}" "${var_tun:=no}" "${var_mknod:=0}"
: "${var_mount_fs:=}" "${var_gpu:=no}" "${var_gpu_device:=/dev/dri/renderD128}"
: "${var_protection:=no}" "${var_cmode:=}" "${var_timezone:=}"
: "${var_apt_cacher:=no}" "${var_apt_cacher_ip:=}"
: "${var_http_proxy:=}" "${var_http_no_proxy:=}" "${var_inherit_host_ca:=no}"
: "${var_post_install:=}" "${var_verbose:=no}"
: "${var_rustnvr_port:=8080}" "${var_rustnvr_version:=}"
: "${var_recordings:=}" "${var_smb:=}" "${var_smb_user:=}" "${var_smb_domain:=}"
SMB_PASSWORD=${RUSTNVR_SMB_PASSWORD:-}
PASSWORD=${RUSTNVR_WEB_PASSWORD:-}
ARCHIVE="" SSH_KEY_FILES=()

# Settings read from the files of saved settings (no passwords written).
KNOWN_VARS=(var_hostname var_cpu var_ram var_swap var_disk var_unprivileged var_pw
    var_container_storage var_template_storage var_version var_brg var_net var_gateway
    var_ipv6_method var_ipv6_gateway var_mtu var_searchdomain var_ns var_vlan var_tags
    var_ssh var_ssh_authorized_key var_nesting var_keyctl var_fuse var_tun var_mknod
    var_mount_fs var_gpu var_gpu_device var_protection var_cmode var_timezone var_apt_cacher
    var_apt_cacher_ip var_http_proxy var_http_no_proxy var_inherit_host_ca var_post_install
    var_verbose var_rustnvr_port var_rustnvr_version var_recordings var_smb var_smb_user
    var_smb_domain)

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mAttention :\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mErreur :\033[0m %s\n' "$*" >&2; exit 1; }
is_yes() { case "${1,,}" in yes|y|1|true|on|oui) return 0 ;; *) return 1 ;; esac; }

OPTIONS=$#
while [ $# -gt 0 ]; do
    case "$1" in
        --ctid) var_ctid=${2:?}; shift 2 ;;
        --hostname) var_hostname=${2:?}; shift 2 ;;
        --cores) var_cpu=${2:?}; shift 2 ;;
        --memory) var_ram=${2:?}; shift 2 ;;
        --swap) var_swap=${2:?}; shift 2 ;;
        --disk) var_disk=${2:?}; shift 2 ;;
        --storage) var_container_storage=${2:?}; shift 2 ;;
        --template-storage) var_template_storage=${2:?}; shift 2 ;;
        --debian) var_version=${2:?}; shift 2 ;;
        --privileged) var_unprivileged=0; shift ;;
        --root-password) var_pw=${2:?}; shift 2 ;;
        --ssh-key) SSH_KEY_FILES+=("${2:?}"); shift 2 ;;
        --ssh) var_ssh=yes; shift ;;
        --tags) var_tags=${2:?}; shift 2 ;;
        --timezone) var_timezone=${2:?}; shift 2 ;;
        --protection) var_protection=yes; shift ;;
        --console) var_cmode=${2:?}; shift 2 ;;
        --no-nesting) var_nesting=0; shift ;;
        --keyctl) var_keyctl=1; shift ;;
        --fuse) var_fuse=yes; shift ;;
        --tun) var_tun=yes; shift ;;
        --mknod) var_mknod=1; shift ;;
        --mount-fs) var_mount_fs=${2:?}; shift 2 ;;
        --post-install) var_post_install=${2:?}; shift 2 ;;
        --verbose) var_verbose=yes; shift ;;
        --bridge) var_brg=${2:?}; shift 2 ;;
        --ip) var_net=${2:?}; shift 2 ;;
        --gw) var_gateway=${2:?}; shift 2 ;;
        --ipv6)
            case "${2:?}" in
                none|auto|dhcp) var_ipv6_method=$2 ;;
                *) var_ipv6_method=static; var_ipv6_static=$2 ;;
            esac
            shift 2 ;;
        --gw6) var_ipv6_gateway=${2:?}; shift 2 ;;
        --mtu) var_mtu=${2:?}; shift 2 ;;
        --mac) var_mac=${2:?}; shift 2 ;;
        --vlan) var_vlan=${2:?}; shift 2 ;;
        --dns) var_ns=${2:?}; shift 2 ;;
        --searchdomain) var_searchdomain=${2:?}; shift 2 ;;
        --apt-cacher) var_apt_cacher=yes; var_apt_cacher_ip=${2:?}; shift 2 ;;
        --proxy) var_http_proxy=${2:?}; shift 2 ;;
        --no-proxy) var_http_no_proxy=${2:?}; shift 2 ;;
        --host-ca) var_inherit_host_ca=yes; shift ;;
        --port) var_rustnvr_port=${2:?}; shift 2 ;;
        --version) var_rustnvr_version=${2:?}; shift 2 ;;
        --password) PASSWORD=${2:?}; shift 2 ;;
        --gpu) var_gpu=yes; shift ;;
        --gpu-device) var_gpu=yes; var_gpu_device=${2:?}; shift 2 ;;
        --recordings) var_recordings=${2:?}; shift 2 ;;
        --smb) var_smb=${2:?}; shift 2 ;;
        --smb-user) var_smb_user=${2:?}; shift 2 ;;
        --smb-domain) var_smb_domain=${2:?}; shift 2 ;;
        --smb-password) SMB_PASSWORD=${2:?}; shift 2 ;;
        --archive) ARCHIVE=${2:?}; shift 2 ;;
        --source) SOURCE=${2:?}; shift 2 ;;
        -y|--yes) shift ;;  # any option skips the menu
        -h|--help) usage; exit 0 ;;
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

[ "$(id -u)" -eq 0 ] || die "à lancer en root sur l'hôte Proxmox"
for cmd in pct pveam pvesh pvesm pveversion; do
    command -v "$cmd" >/dev/null || die "$cmd introuvable : ce script se lance sur l'hôte Proxmox VE"
done
# RustNVR is published for x86-64 only; the list also offers arm64 templates.
[ "$(uname -m)" = x86_64 ] || die "seul x86-64 est publié (cet hôte : $(uname -m))"

SSH_KEYS=$(mktemp)
SMB_TEST_DIR=""
cleanup() {
    rm -f "$SSH_KEYS"
    if [ -n "$SMB_TEST_DIR" ]; then
        umount "$SMB_TEST_DIR" 2>/dev/null || true
        rmdir "$SMB_TEST_DIR" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# --- Host facts -------------------------------------------------------------------
# Storages able to hold container disks (rootdir) or templates (vztmpl).
storages() { pvesm status -content "$1" 2>/dev/null | awk 'NR > 1 && $3 == "active" {print $1}'; }
free_gb() { pvesm status -storage "$1" 2>/dev/null | awk 'NR == 2 {printf "%.0f Go libres", $6 / 1048576}'; }
# Linux bridges, then SDN vnets.
bridges() {
    ip -o link show type bridge 2>/dev/null | awk -F': ' '{print $2}' | cut -d@ -f1
    [ -f /etc/pve/sdn/vnets.cfg ] && awk '/^vnet:/ {print $2}' /etc/pve/sdn/vnets.cfg
    true
}
host_timezone() {
    local tz
    tz=$(timedatectl show --value --property=Timezone 2>/dev/null || cat /etc/timezone 2>/dev/null || true)
    # pct takes no Etc/* zone; "host" copies the host's.
    case "$tz" in ''|Etc/*) echo host ;; *) echo "$tz" ;; esac
}
id_taken() {
    pct status "$1" >/dev/null 2>&1 && return 0
    compgen -G "/etc/pve/nodes/*/lxc/$1.conf" >/dev/null || compgen -G "/etc/pve/nodes/*/qemu-server/$1.conf" >/dev/null
}
host_cas() { compgen -G "/usr/local/share/ca-certificates/*.crt" || true; }

[ -n "$var_ctid" ] || var_ctid=$(pvesh get /cluster/nextid)
[ -n "$var_container_storage" ] || var_container_storage=$(storages rootdir | head -n 1)
[ -n "$var_timezone" ] || var_timezone=$(host_timezone)

# --- Checks of the values -----------------------------------------------------
valid_ipv4() {
    local IFS=. o
    [[ "$1" =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}$ ]] || return 1
    for o in $1; do [ "$o" -le 255 ] || return 1; done
}
valid_ipv4_cidr() { [[ "$1" =~ ^([0-9.]+)/([0-9]{1,2})$ ]] && [ "${BASH_REMATCH[2]}" -le 32 ] && valid_ipv4 "${1%/*}"; }
valid_ipv6() { [[ "$1" =~ ^[0-9A-Fa-f:.]+$ && "$1" == *:* ]]; }
valid_ipv6_cidr() { [[ "$1" =~ ^([0-9A-Fa-f:.]+)/([0-9]{1,3})$ ]] && [ "${BASH_REMATCH[2]}" -le 128 ] && valid_ipv6 "${1%/*}"; }
valid_number() { [[ "$1" =~ ^[1-9][0-9]*$ ]] && [ "$1" -ge "$2" ] && [ "$1" -le "$3" ]; }
valid_hostname() { [[ "$1" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?$ ]]; }
valid_mac() { [[ "$1" =~ ^([0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}$ ]]; }
valid_tags() { [[ "$1" =~ ^[A-Za-z0-9_][A-Za-z0-9_.+-]*(;[A-Za-z0-9_][A-Za-z0-9_.+-]*)*$ ]]; }
valid_domain() { [[ "$1" =~ ^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$ ]]; }
valid_dns() {
    local s
    for s in ${1//,/ }; do valid_ipv4 "$s" || valid_ipv6 "$s" || return 1; done
}
valid_timezone() { [ "$1" = host ] || [ "$1" = UTC ] || { [[ "$1" != *..* ]] && [ -f "/usr/share/zoneinfo/$1" ]; }; }
valid_mount_fs() { [[ "$1" =~ ^[a-z0-9]+([,;][a-z0-9]+)*$ ]]; }
valid_url() { [[ "$1" =~ ^https?://[^[:space:]\"\'\;\`\$]+$ ]]; }
plain_path() { [[ "$1" == /* && "$1" != *[\;\`\$\&\|\<\>]* ]]; }
# \\server\share\dir or //server/share/dir -> //server/share/dir
smb_share() { local s=${1//\\//}; [[ "$s" =~ ^//[A-Za-z0-9._-]+/[^/]+(/.*)?$ ]] && echo "${s%/}"; }

# Every problem, at once, before anything is created.
check_settings() {
    local p=()
    valid_number "$var_ctid" 100 999999999 || p+=("--ctid : un nombre à partir de 100, sans zéro au début")
    id_taken "$var_ctid" && p+=("--ctid : le numéro $var_ctid est déjà pris")
    valid_hostname "$var_hostname" || p+=("--hostname : lettres, chiffres et « - »")
    valid_number "$var_cpu" 1 512 || p+=("--cores : de 1 à 512")
    valid_number "$var_ram" 256 4194304 || p+=("--memory : 256 Mo au moins")
    [[ "$var_swap" =~ ^[0-9]+$ ]] || p+=("--swap : un nombre de Mo")
    valid_number "$var_disk" 2 65536 || p+=("--disk : 2 Go au moins")
    [ -n "$var_container_storage" ] || p+=("--storage : aucun stockage pour les conteneurs")
    case "$var_version" in ''|12|13) ;; *) p+=("--debian : 12 ou 13") ;; esac
    case "$var_unprivileged" in 0|1) ;; *) p+=("var_unprivileged : 0 ou 1") ;; esac
    [ "$var_net" = dhcp ] || valid_ipv4_cidr "$var_net" || p+=("--ip : « dhcp » ou une adresse avec son masque (192.168.1.50/24)")
    [ -z "$var_gateway" ] || valid_ipv4 "$var_gateway" || p+=("--gw : une adresse IPv4")
    case "$var_ipv6_method" in
        none|auto|dhcp) ;;
        static) valid_ipv6_cidr "$var_ipv6_static" || p+=("--ipv6 : une adresse avec son masque (2001:db8::10/64)") ;;
        *) p+=("--ipv6 : none, auto, dhcp ou une adresse") ;;
    esac
    [ -z "$var_ipv6_gateway" ] || valid_ipv6 "$var_ipv6_gateway" || p+=("--gw6 : une adresse IPv6")
    [ -z "$var_mtu" ] || valid_number "$var_mtu" 576 65535 || p+=("--mtu : de 576 à 65535")
    [ -z "$var_mac" ] || valid_mac "$var_mac" || p+=("--mac : six octets, ex. BC:24:11:12:34:56")
    [ -z "$var_vlan" ] || valid_number "$var_vlan" 1 4094 || p+=("--vlan : de 1 à 4094")
    [ -z "$var_ns" ] || valid_dns "$var_ns" || p+=("--dns : des adresses IP séparées par des espaces")
    [ -z "$var_searchdomain" ] || valid_domain "$var_searchdomain" || p+=("--searchdomain : un nom de domaine (maison.lan)")
    [ -z "$var_tags" ] || valid_tags "$var_tags" || p+=("--tags : lettres, chiffres, « - » et « _ », séparés par « ; »")
    valid_timezone "$var_timezone" || p+=("--timezone : zone inconnue : $var_timezone (ex. Europe/Paris)")
    case "$var_cmode" in ''|tty|console|shell) ;; *) p+=("--console : tty, console ou shell") ;; esac
    [ -z "$var_mount_fs" ] || valid_mount_fs "$var_mount_fs" || p+=("--mount-fs : des types de systèmes de fichiers (nfs,cifs), pas des chemins")
    [ -z "$var_http_proxy" ] || valid_url "$var_http_proxy" || p+=("--proxy : une adresse http://… ou https://…")
    if is_yes "$var_apt_cacher"; then
        [[ -n "$var_apt_cacher_ip" && "$var_apt_cacher_ip" != *[[:space:]\;\`\$]* ]] || p+=("--apt-cacher : l'adresse du cache apt")
    fi
    if [ -n "$var_post_install" ]; then
        plain_path "$var_post_install" && [ -f "$var_post_install" ] || p+=("--post-install : un fichier de l'hôte (chemin absolu) : $var_post_install")
    fi
    local f
    for f in "${SSH_KEY_FILES[@]}"; do [ -f "$f" ] || p+=("--ssh-key : fichier introuvable : $f"); done
    valid_number "$var_rustnvr_port" 1 65535 || p+=("--port : de 1 à 65535")
    if is_yes "$var_gpu" && [ ! -e "$var_gpu_device" ]; then
        p+=("--gpu : pas de $var_gpu_device sur l'hôte")
    fi
    if [ -n "$var_recordings" ]; then
        [ -d "$var_recordings" ] || p+=("--recordings : dossier introuvable sur l'hôte : $var_recordings")
    fi
    if [ -n "$var_smb" ]; then
        smb_share "$var_smb" >/dev/null || p+=("--smb : un partage \\\\serveur\\partage\\dossier ou //serveur/partage/dossier")
        [ -n "$var_smb_user" ] || p+=("--smb-user : le compte du partage")
        [ -n "$SMB_PASSWORD" ] || p+=("--smb : le mot de passe du partage (RUSTNVR_SMB_PASSWORD ou --smb-password)")
    fi
    if [ -n "$ARCHIVE" ]; then
        [ -f "$ARCHIVE" ] && [ -f "$ARCHIVE.sha256" ] || p+=("--archive : $ARCHIVE et $ARCHIVE.sha256 attendus")
    fi
    if [ ${#p[@]} -gt 0 ]; then
        printf '\033[1;31mErreur :\033[0m %s\n' "${p[@]}" >&2
        exit 1
    fi
}

# --- Saved settings -------------------------------------------------------------
# KEY=value lines (quotes allowed), as written by the Helper-Scripts. Read,
# never sourced: only known keys, no shell syntax.
load_vars() {
    local line key value
    while IFS= read -r line || [ -n "$line" ]; do
        [[ "$line" =~ ^[[:space:]]*(var_[a-z0-9_]+)[[:space:]]*=[[:space:]]*(.*)$ ]] || continue
        key=${BASH_REMATCH[1]} value=${BASH_REMATCH[2]}
        value=${value%"${value##*[![:space:]]}"}
        if [[ "$value" =~ ^\"(.*)\"$ || "$value" =~ ^\'(.*)\'$ ]]; then value=${BASH_REMATCH[1]}; fi
        [[ " ${KNOWN_VARS[*]} " == *" $key "* ]] || continue
        case "$value" in *'$('*|*'`'*|*';'*|*'&'*|*'<('*) [ "$key" = var_tags ] || continue ;; esac
        printf -v "$key" '%s' "$value"
    done <"$1"
}

# Writes the settings worth reusing (not the number, the addresses or the
# passwords) to the RustNVR file of the Helper-Scripts.
save_vars() {
    local k v
    mkdir -p "$(dirname "$APP_DEFAULTS")"
    (
        umask 077
        {
            echo "# RustNVR (rustnvr-lxc.sh), $(date '+%Y-%m-%d %H:%M')"
            for k in "${KNOWN_VARS[@]}"; do
                case "$k" in var_pw) continue ;; esac
                v=${!k}
                if [ "$k" = var_net ] && [ "$v" != dhcp ]; then continue; fi
                if [ "$k" = var_gateway ] && [ "$var_net" != dhcp ]; then continue; fi
                # A fixed IPv6 address is not reused either.
                if [[ "$k" == var_ipv6_* ]] && [ "$var_ipv6_method" = static ]; then continue; fi
                if [ -n "$v" ]; then printf '%s=%s\n' "$k" "$v"; fi
            done
        } >"$APP_DEFAULTS"
    )
}

# --- Wizard ---------------------------------------------------------------------
TITLE="RustNVR — conteneur LXC"
STEP_INFO=""
DIR=forward
cls() { clear 2>/dev/null || true; }
cancelled() { cls >&2; die "annulé : rien n'a été créé"; }
# whiptail draws on the terminal and gives the answer on stderr: swapped so
# that $(…) gets it. In the wizard, Cancel (« Retour ») goes one step back.
wt() { whiptail --backtitle "$TITLE${STEP_INFO:+ — $STEP_INFO}" --ok-button "Suivant" --cancel-button "Retour" "$@" 3>&1 1>&2 2>&3; }
note() { whiptail --backtitle "$TITLE" --title "${2:-Valeur refusée}" --msgbox "$1" 12 70; }
# A step that does not apply: passed over in the direction of travel.
skip() { [ "$DIR" = forward ]; }
# no / yes question whose Cancel goes back.
yes_no() { # title text current(yes|no)
    local cur=no
    is_yes "$3" && cur=yes
    wt --title "$1" --default-item "$cur" --menu "\n$2" 16 72 2 no "Non" yes "Oui"
}

st_type() {
    local v
    v=$(wt --title "TYPE DE CONTENEUR" --default-item "$var_unprivileged" --menu \
        "\nRustNVR n'a pas besoin d'un conteneur privilégié, même pour la carte graphique." 14 76 2 \
        1 "Non privilégié (recommandé)" \
        0 "Privilégié : root du conteneur = root de l'hôte") || return 1
    var_unprivileged=$v
}
st_root_password() {
    local v w
    while :; do
        v=$(wt --title "MOT DE PASSE ROOT" --passwordbox \
            "\nMot de passe root du conteneur.\n\nVide : pas de mot de passe, connexion automatique à la console Proxmox (pct enter / console web)." 14 72) || return 1
        if [ -z "$v" ]; then var_pw=""; return 0; fi
        if [ ${#v} -lt 5 ]; then note "5 caractères au moins."; continue; fi
        w=$(wt --title "MOT DE PASSE ROOT" --passwordbox "\nLe même mot de passe, encore :" 10 72) || continue
        if [ "$v" = "$w" ]; then var_pw=$v; return 0; fi
        note "Les deux mots de passe diffèrent."
    done
}
st_ctid() {
    local v
    while :; do
        v=$(wt --title "NUMÉRO DU CONTENEUR" --inputbox "\nNuméro (CTID) :" 10 60 "$var_ctid") || return 1
        if ! valid_number "$v" 100 999999999; then note "Un nombre à partir de 100, sans zéro au début."; continue; fi
        if id_taken "$v"; then note "Le numéro $v est déjà pris (conteneur ou VM)."; continue; fi
        var_ctid=$v; return 0
    done
}
st_hostname() {
    local v
    while :; do
        v=$(wt --title "NOM" --inputbox "\nNom du conteneur (hostname) :" 10 60 "$var_hostname") || return 1
        valid_hostname "$v" && { var_hostname=$v; return 0; }
        note "Lettres, chiffres et « - », 63 caractères au plus."
    done
}
st_number() { # variable title text min max
    local v
    while :; do
        v=$(wt --title "$2" --inputbox "\n$3" 12 70 "${!1}") || return 1
        valid_number "$v" "$4" "$5" && { printf -v "$1" '%s' "$v"; return 0; }
        note "Un nombre de $4 à $5."
    done
}
st_disk() { st_number var_disk "DISQUE" "Taille du disque en Go (sans dossier ni partage d'enregistrements, les vidéos y vont aussi) :" 2 65536; }
st_cpu() { st_number var_cpu "PROCESSEUR" "Nombre de cœurs :" 1 512; }
st_ram() { st_number var_ram "MÉMOIRE" "Mémoire en Mo (2048 laissent de la place à la reconnaissance d'objets) :" 256 4194304; }
st_swap() {
    local v
    while :; do
        v=$(wt --title "SWAP" --inputbox "\nSwap en Mo (0 : aucun) :" 10 60 "$var_swap") || return 1
        [[ "$v" =~ ^[0-9]+$ ]] && { var_swap=$v; return 0; }
        note "Un nombre de Mo."
    done
}
st_storage_menu() { # variable content title text
    local s list=()
    for s in $(storages "$2"); do list+=("$s" "$(free_gb "$s")"); done
    if [ ${#list[@]} -le 2 ]; then
        [ ${#list[@]} -eq 2 ] && printf -v "$1" '%s' "${list[0]}"
        skip; return
    fi
    s=$(wt --title "$3" --default-item "${!1}" --menu "\n$4" 18 72 8 "${list[@]}") || return 1
    printf -v "$1" '%s' "$s"
}
st_storage() { st_storage_menu var_container_storage rootdir "STOCKAGE DU CONTENEUR" "Stockage du disque du conteneur :"; }
st_template_storage() { st_storage_menu var_template_storage vztmpl "STOCKAGE DES MODÈLES" "Stockage du modèle Debian (téléchargé s'il manque) :"; }
st_debian() {
    local v
    v=$(wt --title "DEBIAN" --default-item "${var_version:-13}" --menu "\nVersion de Debian :" 13 60 2 \
        13 "Debian 13 « trixie » (recommandé)" 12 "Debian 12 « bookworm »") || return 1
    var_version=$v
}
st_bridge() {
    local b list=()
    for b in $(bridges); do list+=("$b" ""); done
    if [ ${#list[@]} -le 2 ]; then
        [ ${#list[@]} -eq 2 ] && var_brg=${list[0]}
        skip; return
    fi
    b=$(wt --title "RÉSEAU" --default-item "$var_brg" --menu "\nPont réseau ou vnet SDN :" 18 60 8 "${list[@]}") || return 1
    var_brg=$b
}
st_ipv4() {
    local mode v
    while :; do
        mode=dhcp
        [ "$var_net" = dhcp ] || mode=static
        mode=$(wt --title "IPv4" --default-item "$mode" --menu "\nAdresse IPv4 :" 13 70 2 \
            dhcp "Automatique (DHCP)" static "Adresse fixe") || return 1
        if [ "$mode" = dhcp ]; then var_net=dhcp; var_gateway=""; return 0; fi
        while :; do
            v=$(wt --title "IPv4" --inputbox "\nAdresse avec son masque (192.168.1.50/24) :" 10 70 "$([ "$var_net" = dhcp ] || echo "$var_net")") || continue 2
            valid_ipv4_cidr "$v" && break
            note "Une adresse IPv4 avec son masque : 192.168.1.50/24"
        done
        var_net=$v
        while :; do
            v=$(wt --title "IPv4" --inputbox "\nPasserelle (192.168.1.1) :" 10 70 "$var_gateway") || continue 2
            [ -z "$v" ] || valid_ipv4 "$v" && break
            note "Une adresse IPv4, ou vide."
        done
        var_gateway=$v
        return 0
    done
}
st_ipv6() {
    local v
    while :; do
        v=$(wt --title "IPv6" --default-item "$var_ipv6_method" --menu "\nAdresse IPv6 :" 15 72 4 \
            none "Aucune (le plus courant)" auto "SLAAC (annonces du routeur)" \
            dhcp "DHCPv6" static "Adresse fixe") || return 1
        var_ipv6_method=$v
        [ "$v" = static ] || { var_ipv6_static=""; var_ipv6_gateway=""; return 0; }
        while :; do
            v=$(wt --title "IPv6" --inputbox "\nAdresse avec son masque (2001:db8::10/64) :" 10 70 "$var_ipv6_static") || continue 2
            valid_ipv6_cidr "$v" && break
            note "Une adresse IPv6 avec son masque : 2001:db8::10/64"
        done
        var_ipv6_static=$v
        while :; do
            v=$(wt --title "IPv6" --inputbox "\nPasserelle IPv6 (vide : aucune) :" 10 70 "$var_ipv6_gateway") || continue 2
            [ -z "$v" ] || valid_ipv6 "$v" && break
            note "Une adresse IPv6, ou vide."
        done
        var_ipv6_gateway=$v
        return 0
    done
}
st_text() { # variable title text check message
    local v
    while :; do
        v=$(wt --title "$2" --inputbox "\n$3" 12 72 "${!1}") || return 1
        [ -z "$v" ] || "$4" "$v" && { printf -v "$1" '%s' "$v"; return 0; }
        note "$5"
    done
}
mtu_ok() { valid_number "$1" 576 65535; }
vlan_ok() { valid_number "$1" 1 4094; }
st_mtu() { st_text var_mtu "MTU" "Taille MTU (vide : 1500 ; 9000 pour les trames jumbo) :" mtu_ok "De 576 à 65535, ou vide."; }
st_searchdomain() { st_text var_searchdomain "DNS" "Domaine de recherche DNS (vide : celui de l'hôte) :" valid_domain "Un nom de domaine (maison.lan), ou vide."; }
st_dns() { st_text var_ns "DNS" "Serveurs DNS séparés par des espaces (vide : ceux de l'hôte) :" valid_dns "Des adresses IP séparées par des espaces, ou vide."; }
st_mac() { st_text var_mac "ADRESSE MAC" "Adresse MAC (vide : tirée par Proxmox ; utile pour une réservation DHCP) :" valid_mac "Six octets : BC:24:11:12:34:56, ou vide."; }
st_vlan() { st_text var_vlan "VLAN" "Étiquette VLAN (vide : aucune) :" vlan_ok "De 1 à 4094, ou vide."; }
st_tags() { st_text var_tags "ÉTIQUETTES" "Étiquettes du conteneur, séparées par « ; » :" valid_tags "Lettres, chiffres, « - » et « _ », séparés par « ; »."; }
st_ssh() {
    local mode v f line n=0 list=() picked
    local -A key_of=()
    for f in /root/.ssh/authorized_keys /root/.ssh/*.pub; do
        [ -f "$f" ] || continue
        while IFS= read -r line; do
            [[ "$line" =~ ^(ssh-|ecdsa-|sk-) ]] || continue
            n=$((n + 1))
            key_of[k$n]=$line
            list+=("k$n" "$(awk '{print $1, ($3 == "" ? "(sans nom)" : $3)}' <<<"$line")" OFF)
        done <"$f"
    done
    while :; do
        if [ $n -gt 0 ]; then
            mode=$(wt --title "SSH" --menu "\nClés SSH publiques autorisées pour root :" 15 72 4 \
                found "Choisir parmi les clés de l'hôte ($n)" manual "Coller une clé" \
                file "Un fichier de clés" none "Aucune clé") || return 1
        else
            mode=$(wt --title "SSH" --menu "\nClés SSH publiques autorisées pour root :" 13 72 3 \
                manual "Coller une clé" file "Un fichier de clés" none "Aucune clé") || return 1
        fi
        : >"$SSH_KEYS"
        case "$mode" in
            found)
                picked=$(wt --title "SSH" --checklist "\nClés à autoriser :" 18 90 8 "${list[@]}") || continue
                for v in $picked; do v=${v//\"/}; printf '%s\n' "${key_of[$v]}" >>"$SSH_KEYS"; done ;;
            manual)
                v=$(wt --title "SSH" --inputbox "\nUne ligne de clé publique (ssh-ed25519 AAAA… nom) :" 10 90) || continue
                [ -n "$v" ] && printf '%s\n' "$v" >>"$SSH_KEYS" ;;
            file)
                v=$(wt --title "SSH" --inputbox "\nFichier de clés sur l'hôte :" 10 72 "/root/.ssh/authorized_keys") || continue
                if [ ! -f "$v" ]; then note "Fichier introuvable : $v"; continue; fi
                cat "$v" >>"$SSH_KEYS" ;;
        esac
        v=$(yes_no "SSH" "Activer l'accès SSH root (serveur SSH installé et démarré) ?" "$( [ -s "$SSH_KEYS" ] && echo yes || echo "$var_ssh")") || continue
        var_ssh=$v
        SSH_KEY_FILES=()
        return 0
    done
}
st_features() {
    local v
    on() { is_yes "$1" && echo ON || echo OFF; }
    v=$(wt --title "FONCTIONS DU CONTENEUR" --checklist \
        "\nRustNVR n'utilise que nesting (systemd récent). Espace pour cocher :" 18 78 5 \
        nesting "Nesting (systemd, recommandé)" "$(on "$var_nesting")" \
        keyctl "Keyctl (Docker dans le conteneur)" "$(on "$var_keyctl")" \
        fuse "FUSE (montages en espace utilisateur)" "$(on "$var_fuse")" \
        tun "TUN/TAP (VPN dans le conteneur)" "$(on "$var_tun")" \
        mknod "Mknod (création de périphériques)" "$(on "$var_mknod")") || return 1
    var_nesting=0 var_keyctl=0 var_fuse=no var_tun=no var_mknod=0
    [[ "$v" == *'"nesting"'* ]] && var_nesting=1
    [[ "$v" == *'"keyctl"'* ]] && var_keyctl=1
    [[ "$v" == *'"fuse"'* ]] && var_fuse=yes
    [[ "$v" == *'"tun"'* ]] && var_tun=yes
    [[ "$v" == *'"mknod"'* ]] && var_mknod=1
    true
}
st_gpu() {
    local d list=(none "Aucune : conversions H.264 sur le processeur")
    for d in /dev/dri/renderD*; do [ -e "$d" ] && list+=("$d" "Intel Quick Sync / AMD (VAAPI)"); done
    [ ${#list[@]} -gt 2 ] || { var_gpu=no; skip; return; }
    d=none
    is_yes "$var_gpu" && d=$var_gpu_device
    d=$(wt --title "CARTE GRAPHIQUE" --default-item "$d" --menu \
        "\nCarte graphique pour les conversions H.264 (direct H.265 dans un navigateur, relecture, exports) :" 16 76 5 "${list[@]}") || return 1
    if [ "$d" = none ]; then var_gpu=no; else var_gpu=yes; var_gpu_device=$d; fi
}
st_mount_fs() { st_text var_mount_fs "MONTAGES DANS LE CONTENEUR" "Systèmes de fichiers que le conteneur peut monter lui-même (nfs,cifs…), vide : aucun.\nEn général, il faut un conteneur privilégié. Pour un partage, l'étape « Enregistrements » le monte plutôt sur l'hôte." valid_mount_fs "Des types (nfs,cifs,ext4), pas des chemins."; }
st_apt_cacher() {
    local v
    v=$(wt --title "CACHE APT" --inputbox "\nAdresse d'un cache apt-cacher-ng (IP ou URL ; vide : aucun) :" 11 72 "$var_apt_cacher_ip") || return 1
    var_apt_cacher_ip=$v
    if [ -n "$v" ]; then var_apt_cacher=yes; else var_apt_cacher=no; fi
}
st_proxy() {
    local v
    while :; do
        v=$(wt --title "PROXY HTTP" --inputbox "\nProxy HTTP(S) pour l'installation (http://proxy:3128 ; vide : aucun) :" 11 76 "$var_http_proxy") || return 1
        if [ -z "$v" ]; then var_http_proxy=""; var_http_no_proxy=""; return 0; fi
        if ! valid_url "$v"; then note "Une adresse http://… ou https://…"; continue; fi
        var_http_proxy=$v
        v=$(wt --title "PROXY HTTP" --inputbox "\nExceptions (séparées par des virgules) :" 10 76 "${var_http_no_proxy:-localhost,127.0.0.1,.local}") || continue
        var_http_no_proxy=$v
        return 0
    done
}
st_host_ca() {
    local n
    n=$(host_cas | wc -l)
    [ "$n" -gt 0 ] || { var_inherit_host_ca=no; skip; return; }
    var_inherit_host_ca=$(yes_no "AUTORITÉS DE L'HÔTE" "Copier dans le conteneur les $n autorité(s) de /usr/local/share/ca-certificates (PKI privée, proxy qui inspecte le HTTPS) ?" "$var_inherit_host_ca") || return 1
}
st_timezone() {
    local v
    while :; do
        v=$(wt --title "FUSEAU HORAIRE" --inputbox "\nFuseau horaire du conteneur (Europe/Paris, UTC, ou « host » : celui de l'hôte).\nRustNVR s'en sert pour les heures de nuit de la reconnaissance d'objets." 13 76 "$var_timezone") || return 1
        valid_timezone "$v" && { var_timezone=$v; return 0; }
        note "Zone inconnue : $v (ex. Europe/Paris)"
    done
}
st_protection() { var_protection=$(yes_no "PROTECTION" "Protéger le conteneur contre la suppression (à retirer avant de le supprimer) ?" "$var_protection") || return 1; }
st_console() {
    local v
    v=$(wt --title "CONSOLE" --default-item "${var_cmode:-tty}" --menu "\nMode de la console web de Proxmox :" 14 72 3 \
        tty "Terminal virtuel /dev/tty1 (défaut de Proxmox)" console "/dev/console, comme pct console" \
        shell "Shell root directement") || return 1
    [ "$v" = tty ] && v=""
    var_cmode=$v
}
st_post_install() {
    local v
    while :; do
        v=$(wt --title "SCRIPT DE FIN (HÔTE)" --inputbox "\nChemin d'un script de l'HÔTE lancé en root à la fin (vide : aucun).\nVariables : APP, CTID, IP, HN, STORAGE, BRG." 13 76 "$var_post_install") || return 1
        [ -z "$v" ] && { var_post_install=""; return 0; }
        plain_path "$v" && [ -f "$v" ] && { var_post_install=$v; return 0; }
        note "Un fichier existant de l'hôte, chemin absolu, sans caractères spéciaux."
    done
}
st_port() { st_number var_rustnvr_port "RUSTNVR" "Port de l'interface web (HTTPS) :" 1 65535; }
st_admin_password() {
    local v w
    while :; do
        v=$(wt --title "RUSTNVR" --passwordbox "\nMot de passe du compte admin de RustNVR (vide : tiré au hasard et affiché à la fin) :" 11 76) || return 1
        if [ -z "$v" ]; then PASSWORD=""; return 0; fi
        if [ ${#v} -lt 8 ]; then note "8 caractères au moins."; continue; fi
        w=$(wt --title "RUSTNVR" --passwordbox "\nLe même mot de passe, encore :" 10 72) || continue
        if [ "$v" = "$w" ]; then PASSWORD=$v; return 0; fi
        note "Les deux mots de passe diffèrent."
    done
}
st_rustnvr_version() {
    [ -z "$ARCHIVE" ] || { skip; return; }
    local v
    v=$(wt --title "RUSTNVR" --inputbox "\nVersion à installer (v2026.11.04 ; vide : la dernière) :" 10 72 "$var_rustnvr_version") || return 1
    var_rustnvr_version=$v
}
st_recordings() {
    local mode=local v
    [ -n "$var_recordings" ] && mode=dir
    [ -n "$var_smb" ] && mode=smb
    while :; do
        mode=$(wt --title "ENREGISTREMENTS" --default-item "$mode" --menu "\nOù enregistrer les vidéos ?" 15 78 3 \
            local "Le disque du conteneur" \
            dir "Un dossier de l'hôte (disque dédié, déjà monté)" \
            smb "Un partage SMB (NAS) : \\\\IP\\partage\\dossier") || return 1
        case "$mode" in
            local) var_recordings=""; var_smb=""; return 0 ;;
            dir)
                v=$(wt --title "ENREGISTREMENTS" --inputbox "\nDossier de l'hôte (monté sur /var/lib/rustnvr/recordings, hors des sauvegardes vzdump) :" 11 78 "$var_recordings") || continue
                if [ ! -d "$v" ]; then note "Dossier introuvable sur l'hôte : $v"; continue; fi
                var_recordings=$v; var_smb=""; return 0 ;;
            smb)
                v=$(wt --title "PARTAGE SMB" --inputbox "\nPartage et dossier (\\\\192.168.1.10\\partage\\cameras) :" 11 78 "$var_smb") || continue
                if ! smb_share "$v" >/dev/null; then note "Un partage \\\\serveur\\partage\\dossier"; continue; fi
                var_smb=$v
                v=$(wt --title "PARTAGE SMB" --inputbox "\nCompte du partage :" 10 70 "$var_smb_user") || continue
                [ -n "$v" ] || { note "Le compte du partage est nécessaire."; continue; }
                var_smb_user=$v
                v=$(wt --title "PARTAGE SMB" --inputbox "\nDomaine ou groupe de travail (vide : aucun) :" 10 70 "$var_smb_domain") || continue
                var_smb_domain=$v
                v=$(wt --title "PARTAGE SMB" --passwordbox "\nMot de passe du compte $var_smb_user :" 10 70) || continue
                [ -n "$v" ] || { note "Le mot de passe du partage est nécessaire."; continue; }
                SMB_PASSWORD=$v
                if ! smb_try; then continue; fi
                var_recordings=""; return 0 ;;
        esac
    done
}
st_verbose() { var_verbose=$(yes_no "DÉTAILS" "Afficher le détail de l'installation (apt, téléchargements) ?" "$var_verbose") || return 1; }
st_summary() {
    whiptail --backtitle "$TITLE${STEP_INFO:+ — $STEP_INFO}" --title "RÉSUMÉ" --scrolltext \
        --yes-button "Créer" --no-button "Retour" --yesno "$(summary)" 30 80
}

STEPS=(st_type st_root_password st_ctid st_hostname st_disk st_cpu st_ram st_swap
    st_storage st_template_storage st_debian st_bridge st_ipv4 st_ipv6 st_mtu
    st_searchdomain st_dns st_mac st_vlan st_tags st_ssh st_features st_gpu
    st_mount_fs st_apt_cacher st_proxy st_host_ca st_timezone st_protection
    st_console st_post_install st_port st_admin_password st_rustnvr_version
    st_recordings st_verbose st_summary)

# Runs the steps; Retour on the first one goes back to the main menu.
advanced() {
    local i=0
    DIR=forward
    while [ $i -lt ${#STEPS[@]} ]; do
        STEP_INFO="Étape $((i + 1))/${#STEPS[@]}"
        if "${STEPS[$i]}"; then
            DIR=forward; i=$((i + 1))
        else
            DIR=back
            [ $i -eq 0 ] && { STEP_INFO=""; return 1; }
            i=$((i - 1))
        fi
    done
    STEP_INFO=""
}

summary() {
    local root="pas de mot de passe (connexion automatique à la console)" gpu=non rec="disque du conteneur"
    local features="nesting=$var_nesting keyctl=$var_keyctl fuse=$var_fuse tun=$var_tun mknod=$var_mknod"
    local dns="DNS de l'hôte"
    [ -n "$var_ns" ] && dns="DNS $var_ns"
    [ -n "$var_pw" ] && root="mot de passe choisi"
    is_yes "$var_gpu" && gpu=$var_gpu_device
    [ -n "$var_recordings" ] && rec="$var_recordings (hôte, mp0)"
    [ -n "$var_smb" ] && rec="partage $(smb_share "$var_smb") ($var_smb_user), monté sur l'hôte, /nas"
    cat <<EOF
Conteneur $var_ctid « $var_hostname », Debian ${var_version:-13}, $([ "$var_unprivileged" = 1 ] && echo "non privilégié" || echo PRIVILÉGIÉ)
  $var_cpu cœurs, $var_ram Mo (swap $var_swap), $var_disk Go sur $var_container_storage (modèle : $var_template_storage)
  root : $root ; SSH : $var_ssh$( [ -s "$SSH_KEYS" ] || [ ${#SSH_KEY_FILES[@]} -gt 0 ] && echo ", clés fournies")
  Étiquettes : $var_tags ; fuseau : $var_timezone ; protection : $var_protection ; console : ${var_cmode:-tty}
  Fonctions : $features${var_mount_fs:+ ; montages : $var_mount_fs}
Réseau : pont $var_brg, IPv4 $var_net${var_gateway:+ (passerelle $var_gateway)}, IPv6 $var_ipv6_method${var_ipv6_static:+ $var_ipv6_static}
  ${var_mac:+MAC $var_mac ; }${var_vlan:+VLAN $var_vlan ; }${var_mtu:+MTU $var_mtu ; }$dns${var_searchdomain:+, domaine $var_searchdomain}
  Cache apt : $(is_yes "$var_apt_cacher" && echo "$var_apt_cacher_ip" || echo aucun) ; proxy : ${var_http_proxy:-aucun} ; autorités de l'hôte : $var_inherit_host_ca
RustNVR : port $var_rustnvr_port, ${var_rustnvr_version:-dernière version}, depuis ${ARCHIVE:-$SOURCE/$REPO}
  Compte admin : $([ -n "$PASSWORD" ] && echo "mot de passe choisi" || echo "mot de passe tiré au hasard")
  Carte graphique : $gpu
  Enregistrements : $rec
Fin : ${var_post_install:-aucun script} ; détails : $var_verbose
EOF
}

# --- SMB share (on the host) ----------------------------------------------------
SMB_SHARE=""
smb_credentials() { # file
    # umask in the subshell, before the file is opened: never readable by others.
    (
        umask 077
        {
            printf 'username=%s\npassword=%s\n' "$var_smb_user" "$SMB_PASSWORD"
            if [ -n "$var_smb_domain" ]; then printf 'domain=%s\n' "$var_smb_domain"; fi
        } >"$1"
    )
}
# Mounts the share once, read-only, to check the address and the account
# before anything is created.
smb_try() {
    local cred dir err
    SMB_SHARE=$(smb_share "$var_smb") || return 1
    if ! command -v mount.cifs >/dev/null; then
        say "Installation de cifs-utils sur l'hôte"
        apt-get install -y -qq cifs-utils >/dev/null || { note "cifs-utils ne s'installe pas sur l'hôte" "Partage SMB"; return 1; }
    fi
    dir=$(mktemp -d) cred=$(mktemp)
    smb_credentials "$cred"
    SMB_TEST_DIR=$dir
    err=$(mount -t cifs "$SMB_SHARE" "$dir" -o "ro,credentials=$cred" 2>&1) || {
        rm -f "$cred"; rmdir "$dir"; SMB_TEST_DIR=""
        if [ -t 1 ] && command -v whiptail >/dev/null && [ -n "$STEP_INFO" ]; then
            note "Montage refusé : ${err:-raison inconnue}\n\nVérifiez l'adresse, le compte et le mot de passe." "Partage SMB"
        else
            warn "partage $SMB_SHARE : montage refusé : ${err:-raison inconnue}"
        fi
        return 1
    }
    umount "$dir"; rm -f "$cred"; rmdir "$dir"; SMB_TEST_DIR=""
}

# Mounts the share on the host for the account rustnvr of the container, at
# every boot (fstab), and gives it to the container on /nas.
smb_setup() { # host_uid host_gid
    local mnt=/mnt/rustnvr-$var_ctid cred=/etc/rustnvr-lxc/smb-$var_ctid.cred opts fstab_share
    install -d -m 0700 /etc/rustnvr-lxc
    smb_credentials "$cred"
    mkdir -p "$mnt"
    # Unmounted, the mount point must refuse writes, else RustNVR would fill
    # the system disk of the host: immutable directory under the mount.
    mountpoint -q "$mnt" || chattr +i "$mnt" 2>/dev/null \
        || warn "chattr +i impossible sur $mnt : partage absent, RustNVR écrirait sur le disque de l'hôte"
    opts="credentials=$cred,uid=$1,gid=$2,file_mode=0660,dir_mode=0770,soft,nofail,_netdev,x-systemd.mount-timeout=30"
    fstab_share=${SMB_SHARE// /\\040}
    sed -i "\| $mnt cifs |d" /etc/fstab
    printf '%s %s cifs %s 0 0\n' "$fstab_share" "$mnt" "$opts" >>/etc/fstab
    systemctl daemon-reload 2>/dev/null || true
    mountpoint -q "$mnt" || mount "$mnt" || die "le partage $SMB_SHARE ne se monte pas sur $mnt (voir /etc/fstab)"
    setpriv --reuid="$1" --regid="$2" --clear-groups touch "$mnt/.rustnvr-write-test" 2>/dev/null \
        && rm -f "$mnt/.rustnvr-write-test" \
        || warn "le compte rustnvr du conteneur ($1) ne peut pas écrire dans $SMB_SHARE : droits du partage à revoir"
    # Proxmox can start before the NAS (a VM of the same host): mounted
    # later, then the container restarted to see it.
    cat >/usr/local/sbin/rustnvr-smb-mount <<'EOF'
#!/bin/sh
# Mounts the share of the RustNVR container $1 when it is missing (NAS
# started after Proxmox), then restarts the container so that it sees it.
mnt=/mnt/rustnvr-$1
pct config "$1" >/dev/null 2>&1 || exit 0
mountpoint -q "$mnt" || mount "$mnt" 2>/dev/null || exit 0
pct status "$1" 2>/dev/null | grep -q running || exit 0
case "$(pct exec "$1" -- stat -f -c %T /nas 2>/dev/null)" in
    cifs|smb2|smb3) ;;
    *) logger -t rustnvr-smb "share of $1 mounted late: restarting the container"; pct reboot "$1" ;;
esac
EOF
    chmod 0755 /usr/local/sbin/rustnvr-smb-mount
    cat >/etc/systemd/system/rustnvr-smb@.service <<'EOF'
[Unit]
Description=SMB share of the RustNVR container %i

[Service]
Type=oneshot
ExecStart=/usr/local/sbin/rustnvr-smb-mount %i
EOF
    cat >/etc/systemd/system/rustnvr-smb@.timer <<'EOF'
[Unit]
Description=Mount the SMB share of the RustNVR container %i when it comes back

[Timer]
OnBootSec=2min
OnUnitActiveSec=2min

[Install]
WantedBy=timers.target
EOF
    systemctl daemon-reload
    systemctl enable --now "rustnvr-smb@$var_ctid.timer" >/dev/null 2>&1 || warn "minuterie rustnvr-smb@$var_ctid.timer non activée"
    pct set "$var_ctid" -mp1 "$mnt,mp=/nas"
    say "Partage $SMB_SHARE monté sur $mnt (hôte), /nas dans le conteneur"
}

# --- Menu -------------------------------------------------------------------------
if [ "$OPTIONS" -eq 0 ] && [ -t 0 ] && [ -t 1 ] && command -v whiptail >/dev/null; then
    while :; do
        items=(default "Paramètres par défaut" advanced "Paramètres avancés (tous les réglages, étape par étape)")
        [ -f "$APP_DEFAULTS" ] && items+=(app "Réglages enregistrés pour RustNVR")
        [ -f "$USER_DEFAULTS" ] && items+=(user "Mes réglages par défaut (Helper-Scripts)")
        items+=(help "Options de la ligne de commande")
        CHOICE=$(whiptail --backtitle "$TITLE" --title "$TITLE" --cancel-button "Quitter" --notags \
            --menu "\nInstaller RustNVR dans un nouveau conteneur LXC :" 16 72 5 "${items[@]}" 3>&1 1>&2 2>&3) || cancelled
        case "$CHOICE" in
            help) cls; usage; exit 0 ;;
            advanced) advanced || continue ;;
            app|user)
                if [ "$CHOICE" = app ]; then load_vars "$APP_DEFAULTS"; else load_vars "$USER_DEFAULTS"; fi
                id_taken "$var_ctid" && var_ctid=$(pvesh get /cluster/nextid)
                if [ -n "$var_smb" ] && [ -z "$SMB_PASSWORD" ]; then
                    SMB_PASSWORD=$(whiptail --backtitle "$TITLE" --title "PARTAGE SMB" --passwordbox \
                        "\nMot de passe de $var_smb_user sur $var_smb :" 10 70 3>&1 1>&2 2>&3) || cancelled
                fi
                ;;
        esac
        if [ "$CHOICE" != advanced ]; then
            whiptail --backtitle "$TITLE" --title "RÉSUMÉ" --scrolltext --yes-button "Créer" --no-button "Retour" \
                --yesno "$(summary)" 30 80 || continue
        fi
        break
    done
    if [ "$CHOICE" = advanced ] && whiptail --backtitle "$TITLE" --title "ENREGISTRER" --defaultno --yesno \
        "Enregistrer ces réglages pour les prochaines installations (sans le numéro, les adresses ni les mots de passe) ?\n\n$APP_DEFAULTS" 12 76; then
        save_vars
    fi
    cls
fi

# --- Checks -----------------------------------------------------------------------
for f in "${SSH_KEY_FILES[@]}"; do [ -f "$f" ] && cat "$f" >>"$SSH_KEYS"; done
[ -n "$var_ssh_authorized_key" ] && printf '%s\n' "$var_ssh_authorized_key" >>"$SSH_KEYS"
check_settings
if [ -n "$ARCHIVE" ]; then
    (cd "$(dirname "$ARCHIVE")" && sha256sum -c --quiet "$(basename "$ARCHIVE").sha256") || die "--archive : empreinte incorrecte"
fi
if [ -n "$var_smb" ] && [ -z "$SMB_SHARE" ]; then
    say "Essai du partage $(smb_share "$var_smb")"
    smb_try || die "partage SMB inaccessible : rien n'a été créé"
fi
CTID=$var_ctid
VERBOSE=0
is_yes "$var_verbose" && VERBOSE=1
quiet() { if [ "$VERBOSE" = 1 ]; then "$@"; else "$@" >/dev/null; fi; }

say "Recherche du modèle Debian"
quiet pveam update
find_template() {
    pveam available --section system \
        | awk -v v="debian-$1-standard_" '$2 ~ "^"v && $2 ~ /_amd64\.tar/ {print $2}' \
        | sort -V | tail -n 1
}
if [ -n "$var_version" ]; then
    TEMPLATE=$(find_template "$var_version")
else
    TEMPLATE=$(find_template 13)
    [ -n "$TEMPLATE" ] || TEMPLATE=$(find_template 12)
fi
[ -n "$TEMPLATE" ] || die "aucun modèle Debian ${var_version:-12/13} disponible (pveam available)"
if ! pveam list "$var_template_storage" | grep -qF "$TEMPLATE"; then
    say "Téléchargement du modèle $TEMPLATE"
    quiet pveam download "$var_template_storage" "$TEMPLATE"
fi

NET="name=eth0,bridge=$var_brg,ip=$var_net"
[ -n "$var_gateway" ] && NET+=",gw=$var_gateway"
case "$var_ipv6_method" in
    auto|dhcp) NET+=",ip6=$var_ipv6_method" ;;
    static) NET+=",ip6=$var_ipv6_static"; [ -n "$var_ipv6_gateway" ] && NET+=",gw6=$var_ipv6_gateway" ;;
esac
[ -n "$var_mac" ] && NET+=",hwaddr=$var_mac"
[ -n "$var_vlan" ] && NET+=",tag=$var_vlan"
[ -n "$var_mtu" ] && NET+=",mtu=$var_mtu"

FEATURES=()
[ "$var_nesting" = 1 ] && FEATURES+=(nesting=1)
[ "$var_keyctl" = 1 ] && FEATURES+=(keyctl=1)
is_yes "$var_fuse" && FEATURES+=(fuse=1)
[ "$var_mknod" = 1 ] && FEATURES+=(mknod=1)
[ -n "$var_mount_fs" ] && FEATURES+=("mount=${var_mount_fs//,/;}")

# Environment of the downloads in the container (proxy).
CT_ENV=()
if [ -n "$var_http_proxy" ]; then
    NO_PROXY_LIST=${var_http_no_proxy:-localhost,127.0.0.1,.local}
    CT_ENV=(http_proxy="$var_http_proxy" https_proxy="$var_http_proxy" HTTP_PROXY="$var_http_proxy"
        HTTPS_PROXY="$var_http_proxy" no_proxy="$NO_PROXY_LIST" NO_PROXY="$NO_PROXY_LIST")
fi
ct() { pct exec "$CTID" -- env ${CT_ENV[@]+"${CT_ENV[@]}"} "$@"; }

# The host the container must reach before downloading: the server of the
# releases, or only Debian's with a local archive.
if [ -n "$ARCHIVE" ]; then
    NET_HOST=deb.debian.org
elif [ "$SOURCE" = https://github.com ]; then
    NET_HOST=github.com
else
    NET_HOST=${SOURCE#*://}; NET_HOST=${NET_HOST%%/*}; NET_HOST=${NET_HOST%%:*}
fi
# Waits for the network of the container (DHCP can take a while, also after
# a reboot).
wait_network() {
    for _ in $(seq 1 60); do
        pct exec "$CTID" -- getent hosts "$NET_HOST" >/dev/null 2>&1 && return 0
        sleep 2
    done
    die "le conteneur n'a pas accès à $NET_HOST (réseau $NET). Pour recommencer : pct destroy $CTID"
}

# --- The container ------------------------------------------------------------
say "Création du conteneur $CTID ($var_hostname : $var_cpu cœurs, $var_ram Mo, $var_disk Go sur $var_container_storage)"
CREATE=(--hostname "$var_hostname" --ostype debian
    --cores "$var_cpu" --memory "$var_ram" --swap "$var_swap"
    --rootfs "$var_container_storage:$var_disk" --net0 "$NET"
    --unprivileged "$var_unprivileged" --onboot 1
    --timezone "$var_timezone"
    --description "RustNVR — $SOURCE/$REPO")
[ ${#FEATURES[@]} -gt 0 ] && CREATE+=(--features "$(IFS=,; echo "${FEATURES[*]}")")
[ -n "$var_tags" ] && CREATE+=(--tags "$var_tags")
[ -n "$var_ns" ] && CREATE+=(--nameserver "${var_ns//,/ }")
[ -n "$var_searchdomain" ] && CREATE+=(--searchdomain "$var_searchdomain")
[ -n "$var_cmode" ] && CREATE+=(--cmode "$var_cmode")
is_yes "$var_protection" && CREATE+=(--protection 1)
[ -s "$SSH_KEYS" ] && CREATE+=(--ssh-public-keys "$SSH_KEYS")
quiet pct create "$CTID" "$var_template_storage:vztmpl/$TEMPLATE" "${CREATE[@]}"

if is_yes "$var_tun"; then
    printf '%s\n' "lxc.cgroup2.devices.allow: c 10:200 rwm" \
        "lxc.mount.entry: /dev/net/tun dev/net/tun none bind,create=file" >>"/etc/pve/lxc/$CTID.conf"
fi
if [ -n "$var_recordings" ]; then
    say "Enregistrements dans $var_recordings (point de montage mp0, hors des sauvegardes vzdump)"
    pct set "$CTID" -mp0 "$var_recordings,mp=/var/lib/rustnvr/recordings"
fi

say "Démarrage du conteneur"
pct start "$CTID" || die "le conteneur $CTID ne démarre pas (raison : pct start $CTID --debug). Pour recommencer : pct destroy $CTID"
wait_network

if [ -n "$var_pw" ]; then
    # On stdin rather than on a command line (visible in the process list).
    printf 'root:%s\n' "$var_pw" | pct exec "$CTID" -- chpasswd \
        || warn "mot de passe root non défini (pct exec $CTID -- passwd)"
else
    # No password: root logged in automatically on the consoles of Proxmox
    # (web console, pct console), like the Helper-Scripts.
    pct exec "$CTID" -- bash -c '
        passwd -d root >/dev/null 2>&1 || true
        opts="--autologin root --noissue --keep-baud"
        if [ -f /usr/lib/systemd/system/container-getty@.service ] || [ -f /lib/systemd/system/container-getty@.service ]; then
            mkdir -p /etc/systemd/system/container-getty@.service.d
            printf "[Service]\nEnvironment=TERM=linux\nExecStart=\nExecStart=-/sbin/agetty %s - 115200,38400,9600 linux\n" "$opts" \
                >/etc/systemd/system/container-getty@.service.d/override.conf
        fi
        if [ -f /usr/lib/systemd/system/console-getty.service ] || [ -f /lib/systemd/system/console-getty.service ]; then
            port=- stdio=""
            if grep -qs "^StandardInput=null" /run/systemd/system/console-getty.service.d/override.conf; then
                port=console stdio="StandardInput=null
StandardOutput=null"
            fi
            mkdir -p /etc/systemd/system/console-getty.service.d
            printf "[Service]\nEnvironment=TERM=linux\nExecStart=\nExecStart=-/sbin/agetty %s 115200,38400,9600 %s linux\n%s\n" "$opts" "$port" "$stdio" \
                >/etc/systemd/system/console-getty.service.d/override.conf
        fi
        systemctl daemon-reload
        systemctl restart "container-getty@*" console-getty 2>/dev/null || true
    ' || warn "connexion automatique à la console non réglée"
fi

# Downloads through the proxy, the apt cache, the authorities of the host.
if [ -n "$var_http_proxy" ]; then
    pct exec "$CTID" -- sh -c 'cat >/etc/apt/apt.conf.d/95http-proxy <<EOF
Acquire::http::Proxy "$1";
Acquire::https::Proxy "$1";
EOF
printf "export http_proxy=\"%s\" https_proxy=\"%s\" HTTP_PROXY=\"%s\" HTTPS_PROXY=\"%s\" no_proxy=\"%s\" NO_PROXY=\"%s\"\n" "$1" "$1" "$1" "$1" "$2" "$2" >/etc/profile.d/90-http-proxy.sh' \
        sh "$var_http_proxy" "$NO_PROXY_LIST"
fi
if is_yes "$var_apt_cacher"; then
    CACHER=$var_apt_cacher_ip
    [[ "$CACHER" == *://* ]] || CACHER="http://$CACHER:3142"
    pct exec "$CTID" -- sh -c 'printf "Acquire::http::Proxy \"%s\";\n" "$1" >/etc/apt/apt.conf.d/00aptproxy' sh "$CACHER"
fi
if is_yes "$var_inherit_host_ca"; then
    for cert in $(host_cas); do
        pct push "$CTID" "$cert" "/usr/local/share/ca-certificates/$(basename "$cert")"
    done
    pct exec "$CTID" -- sh -c 'command -v update-ca-certificates >/dev/null && update-ca-certificates >/dev/null' || true
fi

# The account first: its identifiers are needed for the mount points and the
# graphics card. In an unprivileged container, the user N of the container is
# the user 100000 + N of the host.
pct exec "$CTID" -- useradd --system --home-dir /var/lib/rustnvr --shell /usr/sbin/nologin rustnvr
CT_UID=$(pct exec "$CTID" -- id -u rustnvr)
CT_GID=$(pct exec "$CTID" -- id -g rustnvr)
OFFSET=0
[ "$var_unprivileged" = 1 ] && OFFSET=100000
HOST_UID=$((OFFSET + CT_UID)) HOST_GID=$((OFFSET + CT_GID))
NEED_REBOOT=0
if [ -n "$var_recordings" ]; then
    if chown "$HOST_UID:$HOST_GID" "$var_recordings" 2>/dev/null; then
        say "Dossier $var_recordings donné au compte rustnvr du conteneur ($HOST_UID sur l'hôte)"
    else
        # NFS with root_squash, CIFS mounted with a fixed uid…: the owner is
        # decided by the server or the mount options, not by chown.
        warn "impossible de donner $var_recordings à $HOST_UID:$HOST_GID (partage NFS en root_squash, CIFS…). Réglez le propriétaire côté serveur ou dans les options de montage (uid=$HOST_UID,gid=$HOST_GID), sinon RustNVR ne pourra pas y écrire."
    fi
fi
if [ -n "$var_smb" ]; then
    smb_setup "$HOST_UID" "$HOST_GID"
    NEED_REBOOT=1
fi

PACKAGES="curl ca-certificates ffmpeg"
WHAT="FFmpeg"
if is_yes "$var_gpu"; then
    PACKAGES="$PACKAGES intel-media-va-driver i965-va-driver mesa-va-drivers"
    WHAT="FFmpeg et des pilotes VAAPI"
fi
is_yes "$var_ssh" && PACKAGES="$PACKAGES openssh-server"
say "Installation de $WHAT (quelques minutes)"
APT_QUIET="-qq"
[ "$VERBOSE" = 1 ] && APT_QUIET=""
quiet ct bash -c "apt-get update $APT_QUIET && DEBIAN_FRONTEND=noninteractive apt-get install -y $APT_QUIET --no-install-recommends $PACKAGES"
is_yes "$var_inherit_host_ca" && { pct exec "$CTID" -- update-ca-certificates >/dev/null 2>&1 || true; }

if is_yes "$var_ssh"; then
    # Root by password only when one was chosen; by key otherwise.
    LOGIN=prohibit-password
    [ -n "$var_pw" ] && LOGIN=yes
    pct exec "$CTID" -- sh -c "mkdir -p /etc/ssh/sshd_config.d && printf 'PermitRootLogin %s\n' $LOGIN >/etc/ssh/sshd_config.d/rustnvr-lxc.conf && systemctl enable --now ssh >/dev/null 2>&1 && systemctl restart ssh" \
        || warn "serveur SSH non démarré"
fi

if is_yes "$var_gpu"; then
    PVE=$(pveversion | sed -n 's|^pve-manager/\([0-9]*\)\.\([0-9]*\).*|\1 \2|p')
    read -r PVE_MAJOR PVE_MINOR <<<"${PVE:-0 0}"
    if [ "$PVE_MAJOR" -gt 8 ] || { [ "$PVE_MAJOR" -eq 8 ] && [ "$PVE_MINOR" -ge 2 ]; }; then
        # No render group (unusual image): the card goes to group 0.
        RENDER_GID=$(pct exec "$CTID" -- getent group render | cut -d: -f3) || true
        [ -n "$RENDER_GID" ] || warn "pas de groupe render dans le conteneur : carte donnée au groupe 0"
        say "Carte graphique transmise ($var_gpu_device, groupe render ${RENDER_GID:-0})"
        pct set "$CTID" -dev0 "$var_gpu_device,gid=${RENDER_GID:-0}"
        NEED_REBOOT=1
    else
        warn "Proxmox VE 8.2 ou plus est nécessaire pour transmettre /dev/dri avec ce script : voir docs/INSTALLATION.md"
    fi
fi
if [ "$NEED_REBOOT" = 1 ]; then
    pct reboot "$CTID"
    wait_network
fi

# --- RustNVR ------------------------------------------------------------------
say "Installation de RustNVR"
ARGS=(--port "$var_rustnvr_port")
if [ -n "$ARCHIVE" ]; then
    # A local archive: copied into the container, its own install.sh used.
    NAME=$(basename "$ARCHIVE")
    pct push "$CTID" "$ARCHIVE" "/root/$NAME"
    pct push "$CTID" "$ARCHIVE.sha256" "/root/$NAME.sha256"
    pct exec "$CTID" -- bash -c "tar -xzf '/root/$NAME' -C /root --wildcards '*/install.sh' && mv /root/rustnvr-*/install.sh /root/rustnvr-install.sh && rm -rf /root/rustnvr-*/"
    ARGS+=(--archive "/root/$NAME")
else
    [ -n "$var_rustnvr_version" ] && ARGS+=(--version "$var_rustnvr_version")
    ct bash -c "curl -fsSL --retry 5 --retry-all-errors '$(raw_url packaging/install.sh)' -o /root/rustnvr-install.sh"
fi
ct RUSTNVR_WEB_PASSWORD="$PASSWORD" RUSTNVR_REPO="$REPO" RUSTNVR_SOURCE="$SOURCE" bash /root/rustnvr-install.sh "${ARGS[@]}"
pct exec "$CTID" -- rm -f /root/rustnvr-install.sh
[ -n "$ARCHIVE" ] && pct exec "$CTID" -- rm -f "/root/$(basename "$ARCHIVE")" "/root/$(basename "$ARCHIVE").sha256"

if [ -n "$var_smb" ]; then
    # The share as a destination of RustNVR, used by default; « monté
    # obligatoire »: the local disk meanwhile when it is missing.
    pct exec "$CTID" -- sh -c '
        cd /etc/rustnvr || exit 1
        [ -e storage.ui.toml ] || printf "[[destinations]]\nid = \"nas\"\nname = \"Partage SMB\"\npath = \"/nas\"\nrequire_mount = true\n" >storage.ui.toml
        [ -e settings.ui.toml ] || printf "default_destination = \"nas\"\n" >settings.ui.toml
        chown rustnvr:rustnvr storage.ui.toml settings.ui.toml && chmod 600 storage.ui.toml settings.ui.toml
        (set -a; . ./rustnvr.env; /usr/local/bin/rustnvr check-config -c config.toml) >/dev/null 2>&1 \
            || { rm -f storage.ui.toml settings.ui.toml; exit 1; }
        systemctl restart rustnvr' \
        && say "Partage déclaré dans RustNVR (Paramètres → Stockage : « nas », par défaut)" \
        || warn "partage non déclaré dans RustNVR : ajoutez /nas dans Paramètres → Stockage"
fi

if [ -n "$var_post_install" ]; then
    say "Script de fin : $var_post_install"
    IP=$(pct exec "$CTID" -- hostname -I 2>/dev/null | awk '{print $1}') || IP=""
    APP=RustNVR NSAPP=rustnvr CTID="$CTID" IP="$IP" HN="$var_hostname" STORAGE="$var_container_storage" BRG="$var_brg" \
        bash "$var_post_install" || warn "le script de fin a échoué ($var_post_install)"
fi

echo
echo "Conteneur $CTID prêt. Mise à jour : pct exec $CTID -- /usr/local/bin/rustnvr-update"
[ -n "$ARCHIVE" ] && echo "(ou, avec une archive : pct push $CTID <archive> /root/… puis /usr/local/bin/rustnvr-update --archive /root/…)"
[ -n "$var_recordings" ] && echo "Les vidéos de $var_recordings (mp0) ne sont pas dans les sauvegardes vzdump du conteneur."
[ -n "$var_smb" ] && echo "Partage $SMB_SHARE : /mnt/rustnvr-$CTID sur l'hôte (fstab), /nas dans le conteneur, hors des sauvegardes vzdump."
[ -z "$var_pw" ] && echo "Console : pct enter $CTID, ou la console web de Proxmox (connexion automatique)."
true
