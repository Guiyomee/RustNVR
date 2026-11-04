# Installation sur un serveur Linux

Ce guide installe RustNVR comme service systemd sur une machine Linux x86_64
(mini-PC, serveur domestique). Chaque commande ci-dessous a été exécutée
telle quelle sur Ubuntu 26.04 (systemd 259) : service démarré sous un compte
dédié, connexion à l'interface, enregistrement, arrêt propre, désinstallation.
Non testé à ce jour : Debian, Fedora, Raspberry Pi (ARM), Intel N100 réel.

Docker : voir [DOCKER.md](DOCKER.md) (image construite depuis les sources).

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

## Home Assistant

Voir [HOME_ASSISTANT.md](HOME_ASSISTANT.md).
