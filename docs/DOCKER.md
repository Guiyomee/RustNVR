# RustNVR sous Docker

Image multi-étapes (`Dockerfile`) : interface web (Node 24), programme (Rust,
avec la reconnaissance ONNX intégrée), puis une Debian 13 minimale avec
FFmpeg. Le programme tourne sous un compte sans privilèges (uid 1000).

Deux façons de l'obtenir :

- **image publiée** `ghcr.io/guiyomee/rustnvr` (Linux x86-64), construite par
  GitHub à chaque mise à jour du dépôt : rien à compiler ;
- **construite sur votre machine** depuis le code (`build: .`), par exemple
  pour une version modifiée.

## 1. Image publiée (le plus simple)

Il suffit de `compose.yaml` (celui du dépôt GitHub, qui contient
`image: ghcr.io/guiyomee/rustnvr:latest`) dans un dossier :

```sh
mkdir rustnvr && cd rustnvr
curl -fsSLO https://raw.githubusercontent.com/Guiyomee/RustNVR/main/compose.yaml
echo 'RUSTNVR_WEB_PASSWORD=un-mot-de-passe-solide' > .env
chmod 600 .env
docker compose up -d
docker compose logs -f
```

Étiquettes : `latest` (dernière version du dépôt), `2026.11.01` (une version
publiée, qui ne change plus), `sha-…` (un commit précis).

Avec Portainer : Stacks → Add stack → « Web editor », collez `compose.yaml`
et ajoutez la variable `RUSTNVR_WEB_PASSWORD`.

## 2. Construire depuis le code

Récupérez le dépôt (Git ou copie du dossier). **Ne copiez pas** `config.toml`
ni les `*.ui.toml` d'une installation : ils contiennent les mots de passe des
caméras, du MQTT et du stockage (`.gitignore` et `.dockerignore` les
écartent ; `target/`, `web/node_modules/` et `web/dist/` sont inutiles aussi).
Dans `compose.yaml`, remplacez la ligne `image: ghcr.io/…` par `build: .`,
puis :

```sh
echo 'RUSTNVR_WEB_PASSWORD=un-mot-de-passe-solide' > .env
chmod 600 .env
docker compose up -d --build
docker compose logs -f
```

La première construction compile tout le projet (plusieurs minutes, quelques
Go d'espace disque pour Docker) ; les suivantes réutilisent le cache.

Interface : **`https://<machine-docker>:8080/`**, compte `admin` et le mot de
passe du `.env`. Ajoutez les caméras dans la page Caméras (« Rechercher sur le
réseau » les trouve).

**HTTPS.** La configuration Docker active un certificat auto-signé, créé au
premier démarrage pour l'adresse de la machine (`tls_self_signed`). Le
navigateur avertit une fois : comparez l'empreinte qu'il montre avec celle du
journal (« HTTPS enabled … sha256= »), puis acceptez. Une ancienne adresse
`http://…:8080` est redirigée vers `https://`. Autres noms (adresse VPN, nom
DNS) : `tls_names` dans `/config/config.toml`. Un vrai certificat : un reverse
proxy (Nginx Proxy Manager, Caddy…) qui transmet l'en-tête `Host` et les
WebSockets, ou `tls_cert` / `tls_key`.

Installation antérieure (sans HTTPS) : `/config/config.toml` n'est créé qu'au
premier démarrage, ajoutez la ligne une fois :

```sh
docker exec rustnvr sh -c "grep -q '^tls_self_signed' /config/config.toml || sed -i 's/^\[web\]$/[web]\ntls_self_signed = true/' /config/config.toml"
docker restart rustnvr
```

## 3. Réseau

`compose.yaml` utilise le réseau de l'hôte (`network_mode: host`) : RustNVR
joint les caméras comme l'hôte et le direct WebRTC (UDP) fonctionne. En réseau
« bridge » (retirez `network_mode`, décommentez `ports`), le direct standard
marche mais pas le WebRTC (adresses du conteneur non joignables).

**Autre port.** En réseau de l'hôte, `ports` n'a aucun effet : RustNVR écoute
sur `listen` de `/config/config.toml` (8080). Pour un autre port, sans toucher
à ce fichier, ajoutez dans `environment` de la stack :

```yaml
      RUSTNVR_WEB_LISTEN: "0.0.0.0:8082"
```

Pratique pour faire tourner deux RustNVR sur la même machine (un essai à côté
de l'installation habituelle). Le contrôle de santé suit le même port.

Une caméra n'accepte souvent que quelques connexions RTSP : arrêtez l'autre
RustNVR (ou Frigate) qui lit les mêmes caméras pendant l'essai si une caméra
reste « hors ligne » (erreur 500 au SETUP).

**Réglages propres à votre machine** (dossier du NAS, carte graphique) : ne
modifiez pas `compose.yaml`, mettez-les dans `compose.override.yaml` (ignoré
par git), que `docker compose up -d` fusionne tout seul. Modèle :
`compose.homelab.example.yaml`. Avec Portainer (stack depuis un dépôt Git),
gardez `compose.yaml` comme fichier principal et ajoutez votre fichier dans
« Additional paths ».

**Webcam USB.** Donnez le périphérique au conteneur, avec le groupe `video` de
l'hôte (son numéro : `getent group video`, souvent 44), dans
`compose.override.yaml` :

```yaml
services:
  rustnvr:
    devices:
      - /dev/video0:/dev/video0
    group_add:
      - "44"
```

Puis Caméras → Ajouter → « Webcam USB » : la liste montre les webcams que le
conteneur voit, avec leurs formats. Un chemin `/dev/v4l/by-id/…` ne change pas
quand la webcam est branchée ailleurs, mais il faut alors le monter aussi
(`- /dev/v4l:/dev/v4l:ro`).

## 4. Données

| Volume | Contenu |
|---|---|
| `rustnvr-config` → `/config` | `config.toml` (créé au premier démarrage depuis `docker/config.toml`) et les réglages faits dans l'interface (`*.ui.toml`, mots de passe des caméras compris) |
| `rustnvr-data` → `/data` | enregistrements (`/data/recordings`), base, vignettes, copies H.264, exports (`/data/rustnvr`) |

Modifier `config.toml` :

```sh
docker compose exec rustnvr sh -c 'cat /config/config.toml'
docker compose cp ./config.toml rustnvr:/config/config.toml   # puis :
docker compose restart
```

Disque ou NAS monté sur l'hôte : ajoutez-le dans `volumes` (exemple commenté
`/mnt/nas/rustnvr:/nas`), donnez le dossier au compte 1000
(`sudo chown -R 1000:1000 /mnt/nas/rustnvr`), puis déclarez `/nas` comme
destination dans Paramètres → Stockage.

## 5. Reconnaissance d'objets intégrée

Paramètres → Intelligence artificielle → « Intégrée » → **Télécharger**, puis
« Enregistrer » : le modèle YOLOX-nano est récupéré par le conteneur (accès à
Internet nécessaire), vérifié, et gardé dans le volume des données
(`/data/rustnvr/models/yolox_nano.onnx`). Sans accès à Internet, copiez-le :

```sh
docker compose cp yolox_nano.onnx rustnvr:/config/yolox_nano.onnx
```

puis indiquez `/config/yolox_nano.onnx` comme fichier du modèle (voir
[AI.md](AI.md)).

**CodeProject.AI** peut tourner dans son propre conteneur (image
`codeproject/ai-server`, port 32168) : Paramètres → Intelligence
artificielle → « CodeProject.AI / DeepStack », bouton « Tester l'adresse ».
Essayé sur une VM avec un iGPU Intel HD Graphics 530 : son module YOLOv5 6.2
calcule sur CPU (≈ 1 s par image), plus lent et plus gourmand que la
reconnaissance intégrée ; il n'a d'intérêt qu'avec une carte NVIDIA (image
`cuda`).

**Carte graphique.** Pour que RustNVR voie une carte et en affiche
l'occupation (« GPU » dans la barre du haut), transmettez-la au conteneur :
lignes `devices` / `group_add` (Intel, AMD) ou `gpus: all` (NVIDIA) en
commentaire dans `compose.yaml`. Dans une VM, la carte doit d'abord être
transmise à la VM. L'IA sur GPU passe par CodeProject.AI : voir
[AI.md](AI.md#carte-graphique-gpu).

Une carte transmise sert aussi aux **conversions H.264** (direct, relecture et
exports pour les navigateurs sans H.265) : Paramètres → Réseau → « Convertir
sur la carte graphique quand c'est possible » (`live.hwaccel`, actif par
défaut). Au démarrage, des essais choisissent ce qui marche :

- **NVIDIA** (NVENC) : `gpus: all` et `NVIDIA_DRIVER_CAPABILITIES:
  compute,video,utility` (en commentaire dans `compose.yaml`), avec
  nvidia-container-toolkit sur l'hôte ;
- **Intel, AMD** (VAAPI) : `devices` / `group_add`. L'image contient les
  pilotes (Intel `iHD` et `i965`, AMD) ; sur un processeur Intel jusqu'à la
  9ᵉ génération, c'est `i965` qui encode le H.264.

Sans carte, ou si les essais échouent, le processeur convertit comme avant.

**Mot de passe perdu** : un nouveau mot de passe pour le compte `admin`
(tiré au hasard et affiché ; le conteneur peut rester en marche) :

```sh
docker exec rustnvr rustnvr reset-password -c /config/config.toml
```

## 6. Mise à jour, arrêt

```sh
docker compose pull && docker compose up -d   # image publiée
docker compose up -d --build                  # construite : après git pull
docker compose down            # arrête (les volumes sont gardés)
```

Avec l'image publiée sous Portainer : « Update the stack » en cochant
« Re-pull image ».

`docker stop` envoie SIGTERM : RustNVR s'arrête proprement, comme avec systemd.

L'image a un contrôle de santé : toutes les 30 s, `rustnvr health` interroge le
serveur (`GET /api/v1/health`). Portainer et `docker ps` affichent alors
« healthy », ou « unhealthy » après 3 échecs (port déjà pris, configuration
invalide, serveur bloqué). Docker ne redémarre pas seul un conteneur
« unhealthy » ; `restart: unless-stopped` le relance s'il s'arrête.

### Être prévenu des nouvelles versions

À propos → Mises à jour : indiquez l'adresse du dépôt (par exemple
`https://gitea.exemple.org/proprietaire/RustNVR`) et enregistrez. RustNVR
compare sa version à la dernière publication du dépôt (au démarrage puis
toutes les 12 heures) ; un bandeau annonce une nouvelle version, avec ses notes
et la procédure ci-dessus. Rien n'est installé tout seul.

### Publier une version (sur votre dépôt)

1. Changez `version` dans `Cargo.toml` (`[workspace.package]`). Versions
   « calendrier » : année.mois.numéro de la publication dans le mois, sans zéro
   en tête pour Cargo (`2026.12.1`). Validez et envoyez (`git push`).
2. Dans Gitea : Publications (« Releases ») → Nouvelle publication →
   étiquette `v2026.12.01` sur `main` (le zéro en tête est permis ici), un titre et les nouveautés → Publier.

Les RustNVR qui surveillent le dépôt l'annoncent à la vérification suivante
(« Vérifier maintenant » pour ne pas attendre). L'étiquette doit
correspondre à la version de `Cargo.toml`, sinon la version installée se
croira toujours en retard.
