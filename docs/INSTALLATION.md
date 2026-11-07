# Installation sur un serveur Linux

Ce guide installe RustNVR comme service systemd sur une machine Linux x86_64
(mini-PC, serveur domestique). Chaque commande ci-dessous a été exécutée
telle quelle sur Ubuntu 26.04 (systemd 259) : service démarré sous un compte
dédié, connexion à l'interface, enregistrement, arrêt propre, désinstallation.
Non testé à ce jour : Debian, Fedora, Raspberry Pi (ARM), Intel N100 réel.

Docker : voir [DOCKER.md](DOCKER.md). Proxmox VE (conteneur LXC) : voir
[plus bas](#proxmox-ve).

## Installation rapide (archive précompilée)

Depuis la version 2026.11.03, chaque publication contient une archive prête à
l'emploi pour Linux x86-64 (`rustnvr-<version>-linux-x86_64.tar.gz` et son
empreinte `.sha256`) : programme (reconnaissance ONNX comprise), interface
web, service systemd. Compilée sur Debian 12 : fonctionne sur Debian 12 et 13
(et les distributions aussi récentes). Rien à compiler, ni Rust ni Node.js :

```sh
sudo apt install curl ffmpeg
curl -fsSLO https://raw.githubusercontent.com/Guiyomee/RustNVR/main/packaging/install.sh
sudo bash install.sh
```

[`install.sh`](../packaging/install.sh) télécharge la dernière version,
vérifie son empreinte, puis fait tout ce que décrivent les § 3 à 5 ci-dessous :
compte `rustnvr`, dossiers `/etc/rustnvr` et `/var/lib/rustnvr`, configuration
de départ (HTTPS, compte `admin`, mot de passe tiré au hasard et affiché une
fois, gardé dans `/etc/rustnvr/rustnvr.env`), service démarré. Options :
`--version v2026.11.03`, `--port 8081`, `--no-start`.

**Mettre à jour** : `sudo rustnvr-update` (la même commande, installée au
passage). Seuls le programme et l'interface sont remplacés : la configuration,
les réglages de l'interface et les enregistrements sont gardés.

La suite du guide décrit l'installation à la main, depuis le code.

## 1. Prérequis

- Linux x86_64 avec systemd ;
- pour compiler : Rust stable (testé avec 1.99.1), `gcc`, Node.js 24 et npm
  (seulement pour construire l'interface) ;
- **FFmpeg, optionnel** (`apt install ffmpeg`) : détection de mouvement,
  captures, vignettes d'événements, conversion H.264 du direct. Sans FFmpeg,
  l'enregistrement, le direct et la relecture fonctionnent ; ces fonctions
  sont signalées « non disponibles » dans l'interface ;
- des caméras RTSP en H.264 ou H.265, idéalement avec un sous-flux de faible
  définition (direct et détection plus légers).

## 2. Compiler

Sur la machine cible ou une machine de même architecture :

```sh
cargo build --release              # binaire : target/release/rustnvr
cd web && npm ci && npm run build  # interface : web/dist
```

## 3. Installer

```sh
# Compte système dédié, sans connexion possible
sudo useradd --system --home-dir /var/lib/rustnvr --shell /usr/sbin/nologin rustnvr

# Dossiers : configuration (écrite aussi par l'interface) et données
sudo install -d -o rustnvr -g rustnvr -m 0750 /etc/rustnvr /var/lib/rustnvr /var/lib/rustnvr/recordings

# Binaire et interface
sudo install -m 0755 target/release/rustnvr /usr/local/bin/rustnvr
sudo install -d /usr/local/share/rustnvr
sudo cp -r web/dist /usr/local/share/rustnvr/web
```

## 4. Configurer

Partez de [config.example.toml](../config.example.toml), copié en
`/etc/rustnvr/config.toml` (propriétaire `rustnvr`, mode `0640`). Pour ce
type d'installation :

```toml
[storage]
recordings_dir = "/var/lib/rustnvr/recordings"

[web]
listen = "0.0.0.0:8080"                # accès depuis le réseau local
static_dir = "/usr/local/share/rustnvr/web"
username = "admin"                     # premier compte administrateur
password_env = "RUSTNVR_WEB_PASSWORD"
tls_self_signed = true                 # HTTPS (voir § 7)

[[cameras]]
id = "entree"
name = "Porte d'entrée"
main = { url = "rtsp://192.168.1.20:554/Streaming/Channels/101", username = "admin", password_env = "RUSTNVR_ENTREE_PASSWORD" }
sub = { url = "rtsp://192.168.1.20:554/Streaming/Channels/102", username = "admin", password_env = "RUSTNVR_ENTREE_PASSWORD" }
```

Les caméras peuvent aussi être ajoutées plus tard depuis l'interface. Les
mots de passe restent hors de `config.toml`, dans un fichier lisible par
root seulement, lu par systemd au démarrage :

```sh
sudo install -m 0600 /dev/null /etc/rustnvr/rustnvr.env
sudoedit /etc/rustnvr/rustnvr.env
```

```sh
RUSTNVR_WEB_PASSWORD=un-mot-de-passe-long
RUSTNVR_ENTREE_PASSWORD=mot-de-passe-de-la-camera
```

Vérifiez la configuration (les variables du fichier sont chargées pour le
test) :

```sh
sudo sh -c 'set -a; . /etc/rustnvr/rustnvr.env; /usr/local/bin/rustnvr check-config -c /etc/rustnvr/config.toml'
```

Les réglages modifiés dans l'interface sont écrits à côté de `config.toml`
(`cameras.ui.toml`, `storage.ui.toml`, `mqtt.ui.toml`, `settings.ui.toml`) ;
ceux qui contiennent des mots de passe sont en mode `0600`.

## 5. Démarrer le service

```sh
sudo install -m 0644 packaging/systemd/rustnvr.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now rustnvr
systemctl status rustnvr
journalctl -u rustnvr -f          # journaux (aucune URL ni mot de passe de caméra n'y figure)
```

Le service ([packaging/systemd/rustnvr.service](../packaging/systemd/rustnvr.service))
tourne sous le compte `rustnvr`, ne peut écrire que dans `/etc/rustnvr` et
`/var/lib/rustnvr`, et redémarre en cas d'erreur. À l'arrêt (`systemctl
stop`), les segments en cours sont terminés et renommés : aucun fichier
partiel ne reste (vérifié).

## 6. Premier accès

Ouvrez `https://<adresse-du-serveur>:8080` (ou `http://` sans HTTPS) et
connectez-vous avec `admin` et `RUSTNVR_WEB_PASSWORD`. Ce compte est créé au
premier démarrage depuis `web.username` ; les comptes se gèrent ensuite dans
**Paramètres → Utilisateurs** (administrateur ou lecture seule). Une fois un
compte créé, `password_env` ne sert plus : changez le mot de passe depuis le
menu du profil.

## 7. HTTPS

- `web.tls_self_signed = true` : RustNVR génère un certificat dans
  `/var/lib/rustnvr/recordings/.rustnvr/tls` et journalise son empreinte
  SHA-256. Le navigateur affiche un avertissement la première fois : comparez
  l'empreinte qu'il montre avec celle du journal (`journalctl -u rustnvr | grep
  fingerprint`) avant d'accepter.
- Ou votre certificat : `web.tls_cert` et `web.tls_key` (fichiers PEM lisibles
  par le compte `rustnvr`).
- Ou un proxy inverse (Caddy, nginx) devant RustNVR resté sur `127.0.0.1` ;
  il doit transmettre les WebSocket (`/api/v1/cameras/{id}/live`,
  `/api/v1/ws`).

## 8. Pare-feu

| Flux | Port | Nécessaire pour |
|---|---|---|
| TCP entrant | `web.listen` (8080) | interface, API, direct standard |
| UDP entrant | 32768–60999 (ports éphémères Linux) | direct « Faible latence » (WebRTC), un port par spectateur |
| TCP sortant | 554 (RTSP) des caméras | réception des flux |
| TCP sortant | 1883 / 8883 | MQTT, si utilisé |

Avec ufw, en limitant au réseau local :

```sh
sudo ufw allow from 192.168.1.0/24 to any port 8080 proto tcp
sudo ufw allow from 192.168.1.0/24 to any port 32768:60999 proto udp   # WebRTC seulement
```

Sans l'ouverture UDP, le mode « Faible latence » échoue avec un message
explicite ; le direct standard fonctionne toujours.

## 9. NAS, second disque, S3, SFTP

- **NAS ou second disque** : montez le partage (`/etc/fstab`, options `soft`
  pour NFS afin qu'une panne du NAS renvoie des erreurs au lieu de bloquer),
  ajoutez le point de montage à `ReadWritePaths=` du service
  (`sudo systemctl edit rustnvr`), puis déclarez-le dans **Stockage** ou dans
  `[[storage.destinations]]` avec `require_mount = true` : RustNVR n'écrit
  jamais dans le dossier vide d'un partage non monté et se rabat sur le disque
  local pendant la panne.
- **S3, SFTP** : depuis **Paramètres → Stockage**. Une clé privée SFTP placée
  dans `/home` demande `ProtectHome=read-only` dans le service ; préférez
  `/etc/rustnvr` (propriétaire `rustnvr`, mode `0600`).

## 10. Mettre à jour

Installé avec l'archive (voir plus haut) : `sudo rustnvr-update`. Depuis le
code :

```sh
cargo build --release && (cd web && npm ci && npm run build)
sudo systemctl stop rustnvr
sudo install -m 0755 target/release/rustnvr /usr/local/bin/rustnvr
sudo rm -rf /usr/local/share/rustnvr/web && sudo cp -r web/dist /usr/local/share/rustnvr/web
sudo systemctl start rustnvr
```

La base de données (`.rustnvr/rustnvr.db`) est migrée automatiquement au
démarrage ; l'index des enregistrements se reconstruit depuis les fichiers si
besoin.

Pour être prévenu des nouvelles versions : À propos → Mises à jour, adresse du
dépôt (voir [DOCKER.md](DOCKER.md#être-prévenu-des-nouvelles-versions), même
fonctionnement hors Docker ; la procédure affichée est celle ci-dessus).

## 11. Sauvegarder

À sauvegarder : `/etc/rustnvr` (configuration et réglages de l'interface,
**dont des mots de passe**) et le dossier de données
`/var/lib/rustnvr/recordings/.rustnvr` (comptes, événements, vignettes,
réglages de détection, certificat). Copiez la base service arrêté, ou avec
`sqlite3 rustnvr.db ".backup /chemin/sauvegarde.db"`. Les vidéos elles-mêmes
sont des fichiers MP4 ordinaires, lisibles sans RustNVR.

## 12. Désinstaller

```sh
sudo systemctl disable --now rustnvr
sudo rm /etc/systemd/system/rustnvr.service && sudo systemctl daemon-reload
sudo rm -rf /usr/local/bin/rustnvr /usr/local/share/rustnvr
sudo rm -rf /etc/rustnvr /var/lib/rustnvr     # supprime aussi les enregistrements
sudo userdel rustnvr
```

## Proxmox VE

> État : **installé sur un vrai Proxmox VE** (LXC Debian 13 non privilégié,
> release prise sur un serveur Gitea : conteneur créé, FFmpeg, archive
> vérifiée, service démarré). Points de montage (`--recordings`) et carte
> graphique (`--gpu`) vérifiés seulement contre des commandes Proxmox
> simulées : retours bienvenus.

### Conteneur LXC (le plus léger)

Depuis le shell de l'**hôte** Proxmox (en root) :

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/Guiyomee/RustNVR/main/packaging/proxmox/rustnvr-lxc.sh)"
```

[`rustnvr-lxc.sh`](../packaging/proxmox/rustnvr-lxc.sh) crée un conteneur
Debian **non privilégié** (13, sinon 12), avec démarrage automatique, y
installe FFmpeg et la dernière version de RustNVR (avec `install.sh`, comme
ci-dessus), puis affiche l'adresse, le compte et le mot de passe. Réglages :

| Option | Défaut | |
|---|---|---|
| `--ctid N` | le prochain libre | numéro du conteneur |
| `--hostname`, `--cores`, `--memory`, `--disk` | `rustnvr`, 2, 2048 Mo, 8 Go | taille ; 2 Go laissent de la place à la reconnaissance d'objets |
| `--storage ID` | le premier stockage de conteneurs actif | disque du conteneur |
| `--bridge`, `--ip`, `--gw` | `vmbr0`, `dhcp` | réseau (`--ip 192.168.1.50/24 --gw 192.168.1.1`) |
| `--debian 12\|13`, `--version vX`, `--port N`, `--password P` | 13, la dernière, 8080, tiré au hasard | |
| `--recordings DIR` | — | dossier de l'hôte pour les enregistrements |
| `--gpu`, `--gpu-device PATH` | — | carte graphique pour les conversions H.264 |
| `--archive FICHIER` | — | une archive de l'hôte (son `.sha256` à côté) au lieu du téléchargement |

Mise à jour, depuis l'hôte : `pct exec <ctid> -- /usr/local/bin/rustnvr-update`.

**Depuis un serveur Gitea ou Forgejo** (un dépôt RustNVR hébergé chez soi) :
`RUSTNVR_SOURCE` (ou `--source`) fait prendre le script, `install.sh` et les
archives sur ce serveur plutôt que sur GitHub. Les releases de ce serveur
doivent contenir l'archive et son `.sha256` (jointes à la main, ou par un
runner Gitea Actions) :

```sh
RUSTNVR_SOURCE=https://gitea.example.org bash -c "$(curl -fsSL https://gitea.example.org/Guiyomee/RustNVR/raw/branch/main/packaging/proxmox/rustnvr-lxc.sh)"
```

`rustnvr-update` reprend ensuite ce serveur tout seul (`/etc/rustnvr/update.conf`).

**Sans accès à GitHub** (machine hors ligne, version de développement) :
`--archive` installe une archive déjà présente sur l'hôte. Pour en faire une
depuis le code : `cargo build --release`, `cd web && npm ci && npm run build`,
puis `scripts/package-release.sh <version>` (le programme doit être compilé
sur une Debian aussi ancienne que celle du conteneur, à cause de la glibc).
Avec une archive, les mises à jour se font de même :
`pct push <ctid> <archive> /root/…` (et son `.sha256`), puis
`pct exec <ctid> -- /usr/local/bin/rustnvr-update --archive /root/…`.

**Enregistrements.** Sans `--recordings`, ils vont sur le disque du conteneur
(`--disk`). Pour un disque ou un volume dédié de l'hôte, `--recordings
/chemin/sur/l-hôte` l'ajoute comme point de montage `mp0` sur
`/var/lib/rustnvr/recordings`. Dans un conteneur non privilégié, le compte N
du conteneur est le compte 100000 + N de l'hôte : le script donne le dossier
au compte `rustnvr` du conteneur (par exemple `100990:100990`). La base et
les vignettes restent sur le disque du conteneur (`/var/lib/rustnvr/data`).

**NAS.** Un conteneur non privilégié ne peut pas monter un partage SMB/CIFS
lui-même : montez-le sur l'hôte (fstab), puis donnez-le avec `--recordings`
ou `pct set <ctid> -mp1 /mnt/nas/rustnvr,mp=/nas` ; déclarez ensuite `/nas`
dans Paramètres → Stockage (« Partage réseau monté obligatoire »). Le compte du
partage côté hôte doit correspondre (option `uid=100990,gid=100990` du
montage CIFS, par exemple).

**Carte graphique (Intel Quick Sync, AMD).** `--gpu` installe les pilotes
VAAPI et transmet `/dev/dri/renderD128` au conteneur (option `dev0` de
Proxmox VE 8.2 et plus, avec le groupe `render` du conteneur). À la main, ou
sur une version plus ancienne, dans `/etc/pve/lxc/<ctid>.conf` :

```
dev0: /dev/dri/renderD128,gid=<gid du groupe render dans le conteneur>
```

(avant 8.2 : `lxc.cgroup2.devices.allow: c 226:128 rwm` et
`lxc.mount.entry: /dev/dri/renderD128 dev/dri/renderD128 none bind,optional,create=file`).
Puis Paramètres → Réseau → « Convertir sur la carte graphique » : l'état de
la carte s'affiche à côté. L'installation ouvre `/dev` au service quand une
carte graphique ou une webcam est présente
(`/etc/systemd/system/rustnvr.service.d/devices.conf`, `PrivateDevices=no`) ;
une carte ajoutée après coup est prise en compte au prochain
`rustnvr-update`.

**Webcam USB.** Transmettez-la au conteneur comme la carte graphique, avec le
groupe `video` du conteneur (44 sous Debian), puis relancez l'installation
pour ouvrir `/dev` au service :

```
pct set <ctid> -dev1 /dev/video0,gid=44
pct reboot <ctid>
pct exec <ctid> -- /usr/local/bin/rustnvr-update
```

Puis Caméras → Ajouter → « Webcam USB ».

### Machine virtuelle avec Docker

Pour ceux qui préfèrent une VM : une VM Debian avec Docker, puis l'image
publiée (voir [DOCKER.md](DOCKER.md)). La carte graphique se transmet alors à
la VM (passage PCI de l'iGPU), puis au conteneur (`/dev/dri`, groupe
`render`). C'est l'installation de l'auteur (VM Debian, Portainer, iGPU
Intel HD 530).

## Home Assistant

Voir [HOME_ASSISTANT.md](HOME_ASSISTANT.md).
