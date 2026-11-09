# Contrat de l'API RustNVR v1

Base : `/api/v1`. Réponses JSON en `snake_case`, horodatages en millisecondes
Unix (UTC). Les types Rust sont dans
[crates/rustnvr-api/src/dto.rs](crates/rustnvr-api/src/dto.rs) et leur miroir
TypeScript dans [web/src/types/api.ts](web/src/types/api.ts) : les deux
doivent évoluer ensemble.

- [Règles générales](#règles-générales)
- [Routes implémentées](#routes-implémentées)
- [Routes prévues (non implémentées)](#routes-prévues-non-implémentées)

## Règles générales

**Authentification.** Comptes stockés dans la base (mot de passe haché avec
Argon2) et sessions par cookie `rustnvr_session` (`HttpOnly`,
`SameSite=Strict`, 30 jours, `Secure` derrière un proxy qui envoie
`X-Forwarded-Proto: https`). Toutes les routes `/api/` exigent une session ou
des identifiants HTTP Basic d'un compte (scripts), sauf `/auth/status`,
`/auth/login` et `/auth/setup`. Les fichiers de l'interface sont publics (ils
ne contiennent aucune donnée). Rôles : `admin` (tout) et `viewer` (lecture
seule : `GET`, plus sa déconnexion, son mot de passe et l'ouverture du
direct WebRTC `POST /cameras/{id}/live/webrtc` ; sinon 403 ; l'interface grise
alors toutes les commandes de modification). Toute
requête qui modifie quelque chose et porte un en-tête `Origin` d'un autre site
est refusée (403). 5 échecs de connexion en 15 min, par adresse ou par compte,
bloquent les essais 15 min (429). Au démarrage, si la base n'a aucun compte et
que `web.username` / mot de passe sont configurés, ce compte est créé comme
administrateur. Sans compte ni `web.username`, l'accès est libre (et
l'interface propose de créer le premier administrateur) ; le serveur écoute
alors en `127.0.0.1` par défaut et refuse une autre adresse sauf
`web.allow_unauthenticated_remote = true`. Sans base de données, l'ancien mode
s'applique : HTTP Basic avec `web.username` sur toutes les routes. HTTPS
intégré (TLS 1.2/1.3, HTTP/2) avec `web.tls_cert` + `web.tls_key`, ou un
certificat auto-signé généré dans `<data_dir>/tls` (`web.tls_self_signed`) ;
les cookies sont alors `Secure` et le direct passe en `wss://`.

| Route | Accès | Rôle |
|---|---|---|
| `GET /auth/status` | public | `{ "required", "accounts", "setup", "user": { "username", "role" } \| null }` |
| `POST /auth/login` | public | `{ "username", "password" }` → utilisateur + cookie ; 401 `bad_credentials`, 429 |
| `POST /auth/logout` | session | 204, cookie effacé |
| `POST /auth/setup` | public, sans aucun compte | crée le premier administrateur et ouvre sa session ; 409 ensuite |
| `PUT /auth/password` | session | `{ "current", "new" }` (8 caractères min.) ; ferme les sessions du compte |
| `GET /users` · `POST /users` | admin | `[{ "username", "role", "created_ms" }]` ; corps `{ "username", "password", "role" }` |
| `PUT /users/{nom}` · `DELETE /users/{nom}` | admin | `{ "password"?, "role"? }` ; ferme ses sessions ; 409 pour le dernier administrateur |

**Erreurs.** Code HTTP approprié et corps :

```json
{ "error": { "code": "not_found", "message": "no camera \"x\"" } }
```

Codes : `not_found` (404), `bad_request` (400), `unavailable` (503), `internal` (500). Une route
inconnue sous `/api/v1` renvoie toujours un 404 JSON (jamais l'interface).
`401` sans corps JSON, avec `WWW-Authenticate: Basic realm="RustNVR"`.

**En-têtes de sécurité** sur toutes les réponses : `X-Content-Type-Options:
nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`.

**Secrets.** Aucune réponse ne contient de chemin RTSP, d'identifiant ni de
mot de passe de caméra : seul `rtsp://hôte:port` est exposé.

## Routes implémentées

Toutes sont couvertes par `crates/rustnvr-api/tests/api.rs` et ont été
vérifiées contre un serveur réel.

### `GET /api/v1/health`

`{ "status": "ok" }` tant que le serveur tourne. **Sans authentification**,
comptes ou non, et sans aucune information : c'est le contrôle de santé de
l'image Docker (`rustnvr health -c config.toml` l'appelle sur le port de
`web.listen`, en HTTPS si le serveur sert du TLS, certificat non vérifié
puisque c'est la même machine ; code de sortie 0 ou 1).

### `GET /api/v1/capabilities`

Ce que le serveur sait réellement faire. L'interface s'en sert pour afficher
« non disponible » plutôt que de deviner.

```json
{ "api_version": 1, "live_video": true, "live_transcode": true, "live_webrtc": true, "snapshots": true, "events": true,
  "motion_detection": true, "camera_management": true, "camera_probe": true,
  "recordings": true, "export": true, "protect": true, "retention": true, "settings_write": true,
  "mqtt": true, "ai": false, "users": true, "backup": true, "ptz": true,
  "usb_cameras": true }
```

`usb_cameras` : des webcams USB (V4L2) peuvent être ajoutées comme caméras
(`usb` dans `POST /cameras`, liste dans `GET /devices/video`) ; Linux
seulement.

`ptz` : pilotage des caméras motorisées (`/cameras/{id}/ptz`, voir plus bas),
pour les caméras dont `ptz` vaut `true`.

`export` : FFmpeg fonctionne (export d'un extrait, voir plus bas).

`ai` : un fournisseur de reconnaissance d'objets est configuré (`[ai]`) et la
détection de mouvement est possible (base de données et FFmpeg).

### `GET /api/v1/system`

```json
{ "version": "0.1.0", "started_at_ms": 1793558385019, "uptime_seconds": 42,
  "auth_enabled": false, "user": null,
  "cpu_percent": 8.1, "cpu_history": [7.9, 8.4, 8.1],
  "memory_total_bytes": 8255049728, "memory_used_bytes": 836222976,
  "process_cpu_percent": 1.2, "process_rss_bytes": 13631488,
  "service_cpu_percent": 14.8, "service_memory_bytes": 169869312,
  "service_memory_limit_bytes": null, "service_scope": "container",
  "cameras_total": 4, "cameras_enabled": 3, "cameras_online": 2, "low_space": [],
  "storage_problems": [{ "name": "NAS", "problem": "/nas est sur le disque système : le partage réseau n'est pas monté" }],
  "restart_needed": false,
  "gpus": [{ "id": "card0", "name": "Intel Alder Lake-N [UHD Graphics]", "vendor": "intel",
             "driver": "i915", "device": "/dev/dri/renderD128", "status": "usable",
             "busy_percent": 12.5 }],
  "hwaccel": { "mode": "auto", "state": "active", "card": "Intel HD Graphics 530",
               "device": "/dev/dri/renderD128", "driver": "i965", "constant_qp": false,
               "detail": null } }
```

`hwaccel` : conversions H.264 (direct, relecture, exports) sur la carte
graphique, réglage `live.hwaccel`. Avec `auto`, des essais (une seconde de
mire encodée avec les réglages du direct) sont faits au démarrage et à chaque
changement du réglage : **NVENC** (`h264_nvenc`) si une carte NVIDIA est
joignable (`usable` dans `gpus`, `/dev/nvidiactl`, ou `/dev/dxg` sous WSL),
puis **VAAPI** (`h264_vaapi`) sur chaque carte Intel ou AMD `usable`, avec le
pilote par défaut puis, pour Intel, `iHD` et `i965` nommés ; la première qui
réussit est utilisée. Pour VAAPI, chaque pilote est essayé avec un débit
plafonné, puis à QP constant (`-rc_mode CQP`) : certains pilotes n'offrent que
ce mode (`iHD` libre de Debian sur une Intel UHD 630, par exemple) ;
`constant_qp: true` dit alors que le direct est à qualité constante (QP 30),
sans plafond de débit. `detail` donne la ligne de FFmpeg qui dit pourquoi un
essai a échoué, pas seulement son message final. `api` : `nvenc` ou `vaapi` ;
`device` : nœud VAAPI (`null` pour NVENC). `state` : `off` (réglage), `testing`, `active`, `no_card`
(aucune carte NVIDIA/Intel/AMD utilisable : processeur) ou `failed` (`detail` :
les erreurs des essais ; processeur). Décodage par la carte quand elle sait
(CUDA, VAAPI ; sinon par le processeur, automatiquement), filtres (taille,
masques, 8 bits 4:2:0) sur le processeur, encodage par la carte, sans images B ;
qualité constante relevée sur la carte (CQ = CRF + 6 pour NVENC, QP = CRF + 4
pour VAAPI) pour des fichiers de taille comparable. Une conversion qui échoue
sur la carte est refaite sur le processeur, et `detail` le signale.

`gpus` : cartes graphiques de la machine (Linux : `/sys/class/drm` et
`/proc/driver/nvidia`), `[]` sans carte ou hors Linux. `vendor` : `intel`,
`nvidia`, `amd`, ou `other` (adaptateur d'affichage seulement : carte virtuelle
d'une VM, puce de gestion d'un serveur). `status` : `usable` (le périphérique
de la carte est présent et RustNVR peut l'ouvrir), `not_passed` (la carte existe
mais son périphérique n'est pas transmis au conteneur), `no_permission`,
`display_only`. `name` : modèle donné par le pilote NVIDIA ou le fichier
`pci.ids`, sinon fabricant et identifiants PCI. `busy_percent` : part du temps
où la carte a travaillé sur les 2 dernières secondes, tous utilisateurs
confondus (CodeProject.AI compris) : `gpu_busy_percent` pour AMD, temps hors
veille RC6 pour Intel ; `null` pour NVIDIA et au premier échantillon. RustNVR
ne calcule rien lui-même sur ces cartes.

`restart_needed` : le fichier exécutable de RustNVR a été remplacé depuis le
démarrage (mise à jour installée) ; l'interface affiche alors un bandeau
demandant de redémarrer le serveur.

`low_space` : noms des points de stockage à court d'espace (occupation au-delà de
`storage.low_space_percent`, 90 % par défaut, ou espace libre sous leur réserve),
mis à jour chaque minute.

`storage_problems` : points de stockage inutilisables, vérifiés chaque minute
(5 s après le démarrage) : dossier absent, partage réseau non monté
(`require_mount`), écriture refusée (un petit fichier `.rustnvr-write-test`
est écrit puis effacé à la racine : partage monté en lecture seule…) ou
partage qui ne répond pas en 10 s. Chaque changement (dégradé, puis rétabli)
est aussi annoncé aux webhooks (`storage`), à MQTT (`<p>/storage_problem`) et
au journal ; l'interface le notifie (cloche) et le signale dans la barre du
haut.

`cpu_percent` : machine, tous cœurs ; `memory_*` : machine (sous Docker, la
machine hôte). `process_*` : le programme RustNVR seul, CPU en % d'un cœur.
`service_*` : RustNVR avec les FFmpeg qu'il lance, CPU en % d'un cœur ; lu
dans le cgroup v2 quand il ne contient que RustNVR et ses processus
(`service_scope` `container` : mêmes chiffres que `docker stats`, mémoire =
`memory.current` − `inactive_file` ; `service` : unité systemd), sinon somme
des processus (`processes`). `service_memory_limit_bytes` : limite du
conteneur ou du service, `null` sans limite. Échantillonnés toutes les 2 s
(`cpu_history` : 30 derniers points). Champs CPU/mémoire `null` hors Linux.

### `GET /api/v1/cameras` · `GET /api/v1/cameras/{id}`

```json
{ "id": "entree", "name": "Entrée", "enabled": true, "recording_enabled": true,
  "has_sub_stream": false, "endpoint": "rtsp://127.0.0.1:8554",
  "status": "online", "attempt": null, "retry_in_ms": null, "last_error": null,
  "online_since_ms": 1793558385100, "codec": "h264", "width": 1280, "height": 720,
  "fps": 25.0, "bitrate_kbps": 1997,
  "sub_status": null,
  "stream": { "frames": 1150, "key_frames": 23, "bytes": 11500000, "lost_packets": 0,
              "sessions": 1, "failures": 0, "dropped_frames": 0 },
  "recorder": { "segments": 1, "frames_written": 1150, "bytes_written": 11576801,
                "frames_skipped": 0, "write_errors": 0, "discontinuities": 0 } }
```

`status` : `online` | `connecting` | `retrying` | `idle` | `stopped` |
`disabled`. `codec`, `width`, `height`, `fps`, `bitrate_kbps` ne sont
renseignés qu'en `online`. `recorder` est `null` si l'enregistrement est
désactivé. 404 si la caméra n'existe pas.

`motion` : `{ "enabled", "paused", "running", "active", "error" }` — détection
activée pour cette caméra, détection en pause sur toutes les caméras (voir
`PUT /detection/pause`), images en cours d'analyse, événement en cours,
dernière erreur du décodeur.

`managed` vaut `true` pour une caméra ajoutée depuis l'interface (stockée dans
`cameras.ui.toml`) ; `edit` donne alors ses réglages modifiables
(`main_url`, `sub_url`, `username`, `has_password`, `transport`,
`onvif_port`), **jamais le mot de passe**. Pour les caméras de `config.toml`,
`edit` est `null`.

`ptz` : un service ONVIF est réglé pour la caméra (`onvif` dans la
configuration) ; l'interface propose alors le pilotage PTZ. Que la caméra
soit vraiment motorisée ne se sait qu'en l'interrogeant (`GET …/ptz`).

### `POST /api/v1/cameras` · `PUT /api/v1/cameras/{id}` · `DELETE /api/v1/cameras/{id}`

Gestion des caméras sans redémarrage (corps JSON obligatoire : un formulaire
d'un autre site ne peut pas les déclencher sans contrôle CORS préalable).

```json
{ "id": "garage", "name": "Garage", "enabled": true, "record": true,
  "record_destinations": ["local", "nas"], "record_mode": "motion",
  "retention_days": 7, "max_size_gb": null, "record_audio": true,
  "main": { "url": "rtsp://192.168.1.21:554/stream1", "username": "admin",
            "password": "…", "transport": "tcp" },
  "sub":  { "url": "rtsp://192.168.1.21:554/stream2", "username": "admin", "password": "…" } }
```

- `POST` crée et démarre la caméra (201 + caméra) ; `id` obligatoire,
  `[a-z0-9_-]`, `probe` réservé ; 409 `conflict` si déjà utilisé.
- `retention_days`, `max_size_gb` : rétention propre à la caméra (`null` :
  aucune), appliquée sur chaque point de stockage avant celle du point de
  stockage ; la taille s'entend par point de stockage. Renvoyées aussi dans
  `GET /cameras`.
- `record_audio` : enregistrer le son avec la vidéo (par défaut `true` à la
  création ; absent dans `PUT` : inchangé). Le son AAC est enregistré tel
  quel ; le son G.711 (PCMU, PCMA, 8 kHz mono) est converti en Opus (16 kbit/s,
  piste `Opus` du MP4, lue par les navigateurs). Les autres sons (G.726,
  G.722…) ne sont pas enregistrés.
- `record_mode` : `continuous` (défaut) ou `motion` : seulement autour des
  événements de mouvement (`recording.pre_roll_seconds` avant, au moins
  `recording.post_roll_seconds` après), ce qui suppose la détection activée
  pour la caméra. Renvoyé aussi dans `GET /cameras`.
- `PUT` remplace les réglages et redémarre la caméra (`id` dans l'URL) ;
  `password` absent ou `null` **conserve** le mot de passe enregistré.
- `onvif_port` : port du service ONVIF de la caméra, pour le pilotage PTZ
  (`null` ou absent : aucun). Il est joint sur l'adresse du flux principal,
  avec ses identifiants ; une adresse ou un utilisateur ONVIF réglés à la main
  dans `cameras.ui.toml` sont gardés.
- **Webcam USB** : `"main": null, "sub": null, "usb": { "device":
  "/dev/video0", "input_format": "mjpeg", "width": 1280, "height": 720,
  "framerate": 15, "bitrate_kbps": 2000 }` (seul `device` est obligatoire ;
  `input_format` : `mjpeg`, `yuyv422`, `h264`, `nv12`, `yuv420p`, `rgb24`,
  absent : celui de la webcam). FFmpeg lit la webcam et encode en H.264 (sur
  la carte graphique retenue pour les conversions, sinon libx264 « veryfast »,
  une image clé toutes les 2 s, débit plafonné à `bitrate_kbps`) ; avec
  `input_format: "h264"`, le H.264 de la webcam est copié sans encodage. Les
  images entrent ensuite dans le même chemin que celles d'une caméra RTSP
  (enregistrement, direct, détection, captures). Pas de son, pas de sous-flux,
  pas d'ONVIF (`onvif_port` ignoré) ; le test de connexion RTSP ne s'y
  applique pas. Dans `GET /cameras` : `endpoint` vaut `usb:/dev/video0` et
  `edit.usb` reprend ces réglages (`main_url` vide). Sans `main` ni `usb` :
  400.
- `DELETE` arrête et retire la caméra (204) ; ses enregistrements restent sur
  le disque.

### `GET /api/v1/devices/video`

Les périphériques vidéo de la machine (`/dev/videoN`), pour choisir une
webcam dans le formulaire (capacité `usb_cameras`) :

```json
{ "devices": [{ "path": "/dev/video0",
                "by_id": "/dev/v4l/by-id/usb-046d_HD_Pro_Webcam_C920-video-index0",
                "name": "HD Pro Webcam C920",
                "formats": [{ "format": "mjpeg", "compressed": true,
                              "description": "Motion-JPEG",
                              "sizes": ["640x480", "1280x720", "1920x1080"] }],
                "error": null }] }
```

`name` vient de `/sys/class/video4linux`, `by_id` (chemin qui ne change pas
quand la webcam est branchée ailleurs) de `/dev/v4l/by-id`, `formats` de
`ffmpeg -f v4l2 -list_formats all` (5 s au plus par périphérique). Une webcam
a souvent un second nœud sans images : `formats` y est vide. `error` : par
exemple « Permission denied » quand le compte de RustNVR n'est pas dans le
groupe `video`.
- Les caméras de `config.toml` sont en lecture seule : 409 `read_only`.
- Persistance : `cameras.ui.toml` à côté de `config.toml`, écrit de façon
  atomique, permissions 0600 (il contient les mots de passe des caméras).
- Après chaque changement, le WebSocket envoie `{"type":"cameras","cameras":[…]}`.

### `POST /api/v1/discovery/scan` · `POST /api/v1/discovery/streams`

Recherche des caméras du réseau local, pour pré-remplir le formulaire d'ajout
(capacité `discovery`). Rien n'est enregistré ; réservé aux administrateurs
comme toute requête qui n'est pas un `GET`.

`POST /discovery/scan` `{ "network": "192.168.1.0/24" }` (facultatif :
absent, le /24 de l'adresse de RustNVR sur la route par défaut, si elle est
privée ; de /22 à /30 seulement, sinon 400) lance en même temps, ≈ 3 s :

- **WS-Discovery** ONVIF (sonde multicast `239.255.255.250:3702`) : les
  caméras répondent avec leur adresse ONVIF, leur nom et leur modèle. Il faut
  le réseau local : sous Docker `network_mode: host` ; sous WSL (NAT), rien ne
  répond ;
- un **balayage RTSP** (ports 554 et 8554, 128 connexions à la fois, 0,5 s
  chacune) : chaque port ouvert reçoit un `OPTIONS` RTSP pour confirmer, et
  son en-tête `Server` est gardé.

```json
{ "network": "192.168.1.0/24", "took_ms": 3018,
  "devices": [{ "address": "192.168.1.35", "onvif": false, "xaddr": null,
                "name": null, "hardware": null, "rtsp_port": 554,
                "server": null, "camera": null }] }
```

`camera` : identifiant d'une caméra déjà configurée à cette adresse.

`POST /discovery/streams` `{ "address", "xaddr"?, "rtsp_port"?, "server"?,
"username"?, "password"? }` : les flux d'un appareil.

1. **ONVIF** : service de l'appareil (`xaddr`, sinon `/onvif/device_service`
   sur les ports 80, 8000, 2020, 8080, 8899), horloge de la caméra
   (`GetSystemDateAndTime`, sans authentification), puis, avec un
   `UsernameToken` WS-Security (empreinte du mot de passe, datée à l'heure de
   la caméra) : `GetDeviceInformation`, `GetCapabilities` (service média),
   `GetProfiles` et `GetStreamUri` de chaque profil. Identifiants refusés :
   `error` l'explique (certaines marques demandent un utilisateur ONVIF).
2. Sans ONVIF : les **adresses RTSP habituelles** des marques courantes
   (Hikvision, Dahua, Reolink, Tapo, Uniview, Axis, Foscam, génériques ; celle
   que nomme l'en-tête `Server` d'abord), essayées par `DESCRIBE` (5 s chacune),
   flux principal puis sous-flux.

```json
{ "onvif": true, "manufacturer": "Dahua", "model": "IPC-HDW2431T", "error": null,
  "streams": [{ "url": "rtsp://192.168.1.21:554/cam/realmonitor?channel=1&subtype=0&unicast=true",
                "label": "Main", "codec": "H265", "width": 2688, "height": 1520,
                "recordable": true }] }
```

Adresses sans identifiants, la plus grande image d'abord ; `recordable` :
H.264 ou H.265. Le mot de passe ne sert qu'à cette requête. `onvif_port` :
port du service ONVIF qui a répondu (`null` sans ONVIF), repris par le
formulaire pour le pilotage PTZ.

### `POST /api/v1/cameras/probe` · `POST /api/v1/cameras/{id}/probe`

Test de connexion (DESCRIBE, puis attente de la première image clé, 10 s par
étape). Corps de la première forme : `{ "url", "username", "password",
"transport" }` ; la seconde utilise les réglages enregistrés de la caméra.

```json
{ "streams": [ { "media": "video", "encoding": "h264", "clock_rate": 90000 } ],
  "selected": 0,
  "video": { "codec": "h264", "rfc6381_codec": "avc1.42C01F", "width": 640, "height": 360,
             "time_to_key_frame_ms": 86 },
  "preview": "/api/v1/cameras/preview/9485b146293da91afc9d38b4c104fedf" }
```

Échec : 502 `probe_failed` avec un message lisible (« la caméra refuse
l'identifiant ou le mot de passe (401) », « chemin RTSP inconnu (404) »…).

`preview` (quand une image est arrivée et que FFmpeg fonctionne) : adresse
d'un **aperçu en direct**, à usage unique, valable 2 minutes. `GET` dessus
rouvre la caméra avec le client RTSP de l'enregistrement (sans le son) et
renvoie un MJPEG `multipart/x-mixed-replace;boundary=ffmpeg` (4 images par
seconde, 640 px de large au plus, H.264 et H.265 convertis par FFmpeg) pendant
**1 minute** au plus ; une balise `<img>` l'affiche telle quelle. L'adresse et
les identifiants restent sur le serveur (seul le jeton circule). Jeton
inconnu, déjà utilisé ou expiré : 404 ; la connexion à la caméra s'arrête
quand le navigateur ferme l'image.

### `POST /api/v1/cameras/usb/preview` (admin)

Aperçu d'une webcam avant de l'enregistrer comme caméra. Corps : l'objet `usb`
de `POST /cameras` (`device`, `input_format`, `width`, `height`, `framerate`,
`bitrate_kbps`). Réponse `{ "preview": "/api/v1/cameras/preview/<jeton>" }`,
même jeton à usage unique que ci-dessus : `GET` dessus fait lire la webcam par
FFmpeg avec ces réglages (MJPEG, 4 images par seconde, 1 minute au plus).
Réglages invalides : 400 ; webcam déjà lue par une caméra activée : 409
`busy` (une webcam ne s'ouvre qu'une fois) ; FFmpeg absent : 503.

### `GET /api/v1/cameras/{id}/recordings?from={ms}&to={ms}`

Segments **terminés** chevauchant `[from, to)`, triés par début. `to > from`
et au plus 7 jours, sinon 400. Le segment en cours d'écriture n'est pas listé.
La liste vient de l'index SQLite (table `segments`, alimentée à la fin de
chaque segment et réconciliée avec les fichiers au démarrage puis toutes les
10 minutes) ; sans base de données, elle est obtenue en parcourant les
fichiers (durées lues dans les fragments fMP4, mises en cache).

```json
[ { "camera": "entree", "start_ms": 1793558385179, "end_ms": 1793558415179,
    "duration_ms": 30000, "bytes": 7569934,
    "url": "/api/v1/recordings/local/entree/2026-11-01/20261101T183945.179Z.mp4" } ]
```

### `GET /api/v1/cameras/{id}/playlist.m3u8?from={ms}&to={ms}`

Playlist HLS (VOD, version 7, fMP4) des segments terminés de la période, pour
une lecture sans pause entre les fichiers (hls.js, HLS natif de Safari et
d'iOS, VLC, FFmpeg). Chaque fichier est un segment HLS adressé par plages
d'octets (`EXT-X-MAP` = `ftyp`+`moov`, `EXT-X-BYTERANGE` = fragments), sans
copie ni réencodage ; `EXT-X-DISCONTINUITY` entre fichiers (horodatages
remis à zéro), `EXT-X-PROGRAM-DATE-TIME` = début réel de chaque fichier.
Mêmes limites de période que la liste ci-dessus ; 404 s'il n'y a rien.

### `GET /api/v1/recordings/{destination}/{camera}/{day}/{file}`

Fichier MP4 fragmenté d'un segment (`video/mp4`), avec prise en charge des
requêtes `Range` (206) pour le positionnement dans `<video>`. Chaque
composant est validé (point de stockage connu, identifiant de caméra,
`AAAA-MM-JJ`, nom de segment) :
toute autre forme, y compris `..` encodé, renvoie 400.

`?transcode=h264` : copie H.264 du segment (image convertie par FFmpeg, au plus
1280 px de large ; son copié), pour un navigateur qui ne décode pas le H.265.
La copie est faite une fois (une conversion à la fois, ≈ 4 s pour une minute
de H.265 2304×1296 sur un processeur de bureau), gardée en cache (24 h, 2 Go
au plus, dans `<data_dir>/h264-cache`), puis servie avec `Range`. 503 si
`live.transcode = false`. L'interface l'utilise d'elle-même : fichier par
fichier, en préparant le suivant pendant la lecture.

### `GET /api/v1/protected` · `POST /api/v1/protected` · `DELETE /api/v1/protected/{id}`

Enregistrements protégés : les segments d'une période ne sont jamais effacés,
ni par la rétention (par caméra et par point de stockage), ni par la réserve
d'espace libre (`min_free_gb`), sur tous les points de stockage. Nécessite la
base de données (`capabilities.protect`).

```json
POST { "camera": "jardin", "start_ms": 1792497620000, "end_ms": 1792497650000, "note": "colis" }
→ 201 { "id": 1, "camera": "jardin", "start_ms": 1792497620000, "end_ms": 1792497650000,
        "note": "colis", "created_ms": 1793728513827 }
```

- Un segment est gardé s'il peut contenir une image de la période : il
  commence avant la fin de la période et au plus 2 × `segment_seconds` + 60 s
  avant son début.
- 31 jours au plus par protection, note de 200 caractères au plus ; 404 pour
  une caméra inconnue, 400 pour une période invalide.
- `GET` : toutes les protections, les plus récentes d'abord. `DELETE` : 204,
  puis 404. `POST` et `DELETE` sont réservés aux administrateurs.
- Si la liste ne peut pas être lue (base indisponible pendant un passage), rien
  n'est effacé à ce passage.

### `POST /api/v1/cameras/{id}/export` · `GET /api/v1/exports/{file}`

Export d'un extrait en un seul fichier MP4, en deux temps : la première
requête prépare le fichier et ne répond qu'une fois prêt (ou avec la raison de
l'échec), la seconde le télécharge.

```json
{ "from_ms": 1793712322000, "to_ms": 1793712382000, "h264": false }
```

Réponse :

```json
{ "url": "/api/v1/exports/96729-1--jardin_20261103-142522_60s.mp4",
  "name": "jardin_20261103-142522_60s.mp4", "bytes": 7035700,
  "duration_ms": 60000, "start_ms": 1793712321500, "h264": false }
```

Les segments de la période sont mis bout à bout par FFmpeg (les périodes sans
enregistrement sont sautées ; `duration_ms` est la durée réellement
enregistrée). Sans conversion, image et son sont copiés tels quels et l'extrait
commence à l'image clé qui précède `from_ms` (`start_ms` l'indique) : quelques
dizaines de millisecondes. Avec `"h264": true`, l'image est convertie en H.264
(libx264, pleine résolution ; son copié) et l'extrait commence exactement à
`from_ms` : ≈ 4 s pour une minute de H.265 2304×1296.

2 heures au plus par extrait, un export à la fois. 400 si la période est
invalide, 404 sans enregistrement sur la période, 503 sans FFmpeg
(`capabilities.export`). Autorisé au rôle lecteur (rien n'est modifié).

`GET /api/v1/exports/{file}` sert le fichier préparé (`Range` pris en charge,
`Content-Disposition: attachment` avec le nom proposé). Les fichiers sont dans
`<data_dir>/exports` et supprimés au bout de 15 minutes ; 404 ensuite ou pour
tout autre nom.

### `GET /api/v1/cameras/{id}/live` (WebSocket)

Direct vidéo, sans décodage ni réencodage, pour Media Source Extensions :

1. texte `{"type":"init","codec":"avc1.4D401F","codecs":"avc1.4D401F,mp4a.40.2","audio":"mp4a.40.2","width":1920,"height":1080}`
   — `codec` : codec vidéo ; `codecs` : ce que le client passe à
   `MediaSource.isTypeSupported('video/mp4; codecs="…"')` ; `audio` : codec du
   son, ou `null` ;
2. binaire : segment d'initialisation MP4 (`ftyp` + `moov`) ;
3. binaire : un fragment `moof` + `mdat` par image, à partir d'une image clé.

Paramètres :

- `stream=main|sub` : flux de la caméra (par défaut `sub` s'il est configuré,
  sinon `main` ; `sub` sans sous-flux retombe sur `main`) ;
- `audio=1` : inclure le son de la caméra (piste AAC dans le même flux), si
  elle en envoie et que son son est enregistré (`record_audio`) ; avec
  `transcode`, le son contourne FFmpeg (seule l'image est convertie) ;
- `transcode=h264` : recevoir une conversion H.264 faite par FFmpeg, pour un
  navigateur qui ne décode pas le codec d'origine (H.265 dans Firefox). Une
  seule conversion par caméra et par flux, partagée par les spectateurs,
  arrêtée 10 s après le départ du dernier ; 503 si `live.transcode = false`.
  En cas d'échec (FFmpeg absent, erreur), la connexion est fermée avec une
  raison commençant par `transcode:`.

Le serveur conserve les images depuis la dernière image clé (≤ 4 Mio par
caméra) pour qu'un nouveau spectateur voie l'image immédiatement. Il ferme la
connexion (code 1000, raison explicite) si le flux change (reconnexion de la
caméra, nouveaux paramètres) ou si le client prend trop de retard ; le client
se reconnecte. 404 si la caméra n'existe pas ou n'est pas connectée ; 503
`unavailable` au-delà de 8 spectateurs par caméra. Aucun coût côté serveur
quand personne ne regarde.

### `POST /api/v1/cameras/{id}/live/webrtc` (WHEP, prototype)

Direct à faible latence par WebRTC, sans réencodage pour une caméra H.264.
Signalisation WHEP sans « trickle » : le corps est l'offre SDP du navigateur
(`Content-Type: application/sdp`, vidéo en réception seule, et son en réception
seule si voulu, candidats ICE déjà rassemblés) ; la réponse est `201 Created`, `Content-Type: application/sdp`,
avec la réponse SDP du serveur.

- `stream=main|sub` : comme pour `/live` ;
- son : envoyé en Opus quand l'offre contient une piste audio et que le son de
  la caméra est de l'Opus (G.711 converti) ; l'AAC ne passe pas en WebRTC ;
- caméra H.265 : la vidéo passe par la conversion H.264 (`live.transcode`),
  car les navigateurs ne reçoivent pas le H.265 en WebRTC ; 503 si la
  conversion est désactivée ;
- candidats ICE « host » uniquement (pas de STUN/TURN) : l'adresse locale par
  laquelle le serveur joint le client, une socket UDP à port éphémère par
  spectateur. Le navigateur doit pouvoir joindre le serveur en UDP (réseau
  local, VPN) ; sinon ICE échoue et l'interface propose le direct standard ;
- la session se termine quand le navigateur ferme la connexion, quand le flux
  s'arrête, ou après 20 s sans connexion établie ;
- erreurs : 400 (offre SDP invalide ou refusée, flux inconnu), 404 (caméra
  inconnue ou sans flux), 503 (pas d'image récente, conversion indisponible), au format d'erreur habituel.

### `GET /api/v1/storage`

Mis en cache 30 s côté serveur.

```json
{ "recordings_dir": "/var/lib/rustnvr/recordings",
  "destinations": [ … ], "default_destination": "nas",
  "disk": { "total_bytes": 1081101176832, "used_bytes": 11106127872, "free_bytes": 1014891024384 },
  "recordings_bytes": 157800000,
  "cameras": [ { "camera": "entree", "bytes": 78800000, "segments": 11,
                 "oldest_ms": 1793558385179, "newest_ms": 1793558685179 } ],
  "days": [ { "day": "2026-11-01", "bytes": 157800000 } ],
  "fill_rate_bytes_per_day": null, "estimated_days_until_full": null,
  "retention_configured": false, "computed_at_ms": 1793558727000 }
```

`fill_rate_bytes_per_day` et `estimated_days_until_full` restent `null` tant
qu'il y a moins d'une heure d'enregistrements ; ils sont calculés pour le
disque local, à partir de ses seuls enregistrements. `disk` est le disque du
dossier des enregistrements ; `cameras` et `days` comptent tous les points de
stockage. `destinations` donne chaque point de stockage (même forme que
ci-dessous) avec, en plus, `recordings_bytes` et
`estimated_days_until_full` : jours avant que son disque soit plein au rythme
de ses propres enregistrements (`null` avec moins d'une heure
d'enregistrements, et pour S3/SFTP dont le disque n'est que le dossier
d'attente). `default_destination` est le point de stockage par défaut
(`storage.default_destination`, ou `local` s'il n'existe plus), toujours à
jour malgré le cache. `retention_configured` vaut `true` si au moins un point
de stockage a une limite.

### `GET /api/v1/storage/destinations` · `POST` · `PUT /api/v1/storage/destinations/{id}` · `DELETE …/{id}`

Points de stockage : le disque local (`local`, toujours présent) et des
dossiers sur des partages réseau **montés** dans le système (NAS en SMB/CIFS
ou NFS), écrits directement comme un disque local.
Pour un compte lecteur, `remote` ne contient ni `access_key_id` (S3) ni
`username` (SFTP).

```json
[ { "id": "nas", "name": "NAS salon", "kind": "nas", "path": "/mnt/nas/rustnvr",
    "managed": true, "require_mount": true, "available": true, "unavailable_reason": null,
    "disk": { "total_bytes": 4000000000000, "used_bytes": 1200000000000, "free_bytes": 2800000000000 },
    "retention_days": 30, "max_size_gb": null, "min_free_gb": 50, "low_space": false,
    "used_by": ["garage"], "recordings_bytes": null } ]
```

Corps de `POST` / `PUT` : `{ "id" (POST seulement), "name", "path",
"require_mount", "retention_days", "max_size_gb", "min_free_gb" }`. Les trois routes
d'écriture renvoient la liste à jour.

- `require_mount` (défaut `true`) : le dossier n'est utilisé que s'il existe
  **et** se trouve sur un autre système de fichiers que `/`. Un partage
  démonté laisse un dossier vide sur le disque système : sans ce contrôle,
  RustNVR remplirait silencieusement le disque système.
- À la création ou modification, le dossier doit exister, satisfaire
  `require_mount` et être accessible en écriture, sinon 400 `invalid`.
- `local` : seule la rétention est modifiable (`name` et `path` ignorés) ;
  elle remplace celle de `config.toml`. Ne peut pas être supprimé.
- Points de stockage de `config.toml` (`managed: false`) : 409 `read_only`.
- `DELETE` : 409 `conflict` si des caméras l'utilisent encore ; les fichiers
  déjà enregistrés ne sont jamais supprimés.
- Persistance : `storage.ui.toml` à côté de `config.toml` (écriture atomique).

**Rétention** (`retention_days`, `max_size_gb`, `null` = illimité) :
appliquée 30 s après le démarrage puis toutes les 10 minutes, par point de
stockage. Supprime les segments terminés les plus anciens (âge lu dans le nom
du fichier) ; les segments en cours (`.mp4.part`) ne sont jamais touchés.

**Réserve d'espace** (`min_free_gb`, `null` = aucune) : contrôlée chaque minute
et après chaque erreur d'écriture ; si l'espace libre du système de fichiers
passe en dessous, les segments terminés les plus anciens de ce point de
stockage sont supprimés (toutes caméras).

**S3 et SFTP** (`kind` `s3` ou `sftp`) : le corps de `POST` / `PUT` ajoute
`"remote": { "type": "s3", "endpoint", "region", "bucket", "prefix",
"access_key_id", "secret_access_key" }` ou `{ "type": "sftp", "host", "port",
"username", "password" | "private_key", "remote_dir", "host_key" }` ; un secret
absent est conservé. `path` est alors le dossier d'attente local
(`<data_dir>/outbox/<id>` s'il est vide). Les réponses donnent `remote` sans
aucun secret (`has_secret`, `has_password`) et `upload` : `{ "pending",
"uploaded", "uploaded_bytes", "last_upload_ms", "error", "host_key" }`. Les
segments terminés sont envoyés vers `<prefix>/<caméra>/<jour>/<fichier>` puis
supprimés du dossier d'attente ; nouvel essai après 5 s, puis jusqu'à 5 min.
SFTP : clé du serveur comparée à `host_key`, sinon retenue à la première
connexion (fichier `.rustnvr-known-host` du dossier d'attente) et exigée
ensuite. `storage.ui.toml` est écrit en 0600 (il contient les secrets).

**Panne du NAS** : une caméra est contrôlée à chaque nouveau segment. Si son
point de stockage est indisponible et qu'elle n'enregistre pas aussi en
local, elle écrit sur le disque local en attendant (avertissement dans le
journal), puis revient au NAS dès qu'il est de nouveau disponible. Le segment
en cours au moment de la panne est perdu.

### `PUT /api/v1/detection/pause`

Corps `{ "paused": true }` : met la détection de mouvement en pause sur toutes
les caméras (aucune image analysée, réglages de chaque caméra inchangés), ou
la reprend (`false`) ; répond `{ "paused": … }`. Gardé au redémarrage (fichier
`detection-paused` du dossier de données). Aussi par MQTT (`<p>/detection/set`,
interrupteur Home Assistant « Détection (toutes les caméras) »). Visible dans
`motion.paused` de chaque caméra. Administrateur seulement.

### `PUT /api/v1/cameras/{id}/audio`

Corps `{ "enabled": false }` : enregistre le son de la caméra, ou non, et
renvoie la caméra. Coupé, le son disparaît aussitôt des enregistrements ;
rétabli, il revient à la prochaine image clé (nouveau segment). Si le flux
avait été ouvert sans le son, la caméra se reconnecte (quelques secondes sans
enregistrement). 409 `read_only` pour une caméra de `config.toml`
(`record = { audio = … }` dans ce fichier).

Dans `GET /cameras` : `record_audio` (réglage) et `audio` (son reçu de la
caméra, par exemple `"mp4a.40.2 16000 Hz"`, ou `"opus 48000 Hz"` pour un son
G.711 converti, ou `null` hors ligne ou sans son enregistrable).

### `GET /api/v1/cameras/{id}/ptz` · `POST …/ptz/move` · `POST …/ptz/stop` · presets

Pilotage d'une caméra motorisée en **ONVIF PTZ** (capacité `ptz`, caméras
avec `ptz: true`). RustNVR trouve le service PTZ annoncé par
`GetCapabilities` et le premier profil média qui a une configuration PTZ, avec
les identifiants ONVIF (ceux du flux principal par défaut) ; la session est
gardée, et rouverte si la caméra ne répond plus ou si ses réglages changent.
Lire est permis à tous les comptes ; bouger et changer les positions sont des
écritures, **réservées aux administrateurs**.

- `GET …/ptz` → `{ "presets": [{ "token": "1", "name": "Portail" }] }` :
  les positions enregistrées sur la caméra (`GetPresets`). 400 si aucun
  port ONVIF n'est réglé, si la caméra ne répond pas en ONVIF, refuse les
  identifiants ou n'annonce pas de PTZ (le message le dit) ; 404 caméra
  inconnue.
- `POST …/ptz/move` `{ "pan": 0.5, "tilt": 0, "zoom": 0 }` : déplacement
  continu (`ContinuousMove`), vitesses de -1 à 1 (pan : droite > 0 ; tilt :
  haut > 0 ; zoom : avant > 0 ; absentes : 0 ; au-delà : ramenées à ±1),
  jusqu'à `stop`, ou **10 s au plus** (`Timeout` envoyé à la caméra, au cas où
  l'arrêt ne viendrait jamais). 204.
- `POST …/ptz/stop` : arrêt (pan, tilt et zoom). 204.
- `POST …/ptz/presets` `{ "name": "Terrasse" }` : la position actuelle,
  enregistrée sous ce nom (1 à 64 caractères). 204.
- `POST …/ptz/presets/{token}/goto` : aller à une position. 204.
- `DELETE …/ptz/presets/{token}` : supprimer une position. 204.

Les noms des positions sont aussi proposés à Home Assistant (sélecteur MQTT,
voir `docs/MQTT.md`), lus au démarrage et après chaque changement.

### `GET /api/v1/webhooks` · `PUT /api/v1/webhooks` · `POST /api/v1/webhooks/test`

Adresses appelées (HTTP POST, JSON) sur les événements de détection.
Réservé aux administrateurs, lecture comprise (les adresses portent souvent un
jeton). Écrits dans `webhooks.ui.toml` (0600), qui remplace `[[webhooks]]` de
`config.toml` ; appliqués sans redémarrage. 20 au plus.

```json
[{ "name": "n8n", "url": "https://n8n.maison/webhook/abc", "enabled": true,
   "cameras": [], "objects": ["person"], "events": ["motion", "objects", "end"], "image": true }]
```

- `events` : `motion` (une fois l'image de l'événement prise, ≈ 1 s après le
  début), `objects` (objets reconnus, image avec les cadres), `end` (fin, sans
  image), `crossing` (ligne de passage franchie), `storage` (point de
  stockage inutilisable, puis rétabli ; ni caméras ni objets ne le filtrent)
  et `camera` (flux principal d'une caméra coupé depuis **1 minute**, puis de
  nouveau en ligne ; une reconnexion plus courte n'est pas annoncée, et le
  retour ne l'est qu'après une annonce « hors ligne » ; filtré par
  `cameras`, pas par `objects`). Par défaut `motion` et `objects`.
- `cameras` vide : toutes ; `objects` vide : tous les événements. Avec des
  objets, les appels `motion` sont sautés, `objects` / `end` n'ont lieu que
  si l'un de ces objets a été reconnu, et `crossing` seulement pour un passage
  vérifié avec l'un d'eux (ligne limitée à des objets, voir `lines`).
- Corps d'un passage : `{ "type": "crossing", "camera", "camera_name",
  "crossing": { "line", "direction": "forward" | "backward", "at_ms",
  "today", "object": "person" | null }, "image", "rustnvr" }` ; l'image (si
  prise) est celle analysée, l'objet encadré. D'un point de stockage :
  `{ "type": "storage", "storage": { "destination", "name", "available",
  "problem", "at_ms" }, "rustnvr" }`. Discord : « Personne · Portail ·
  Jardin » (ou « Passage · … ») avec le sens et le compte du jour ;
  « Stockage dégradé · NAS » avec la raison, puis « Stockage rétabli · NAS ».
  D'une caméra : `{ "type": "camera", "camera", "camera_name",
  "camera_state": { "online", "at_ms", "offline_ms", "error" }, "rustnvr" }`
  (`offline_ms` : durée sans image ; `error` : dernière erreur de connexion,
  sans identifiants). Discord : « Caméra hors ligne · Garage » avec la durée
  et l'erreur, puis « Caméra rétablie · Garage » avec la durée de la coupure.
- Corps envoyé : `{ "type", "camera", "camera_name", "event": { "id", "kind",
  "start_ms", "end_ms", "score", "zones", "objects" }, "image":
  "data:image/jpeg;base64,…" | null, "rustnvr" }`.
- Adresse Discord (`https://discord.com/api/webhooks/…`, aussi
  `discordapp.com`, `ptb.` et `canary.`) : à la place de ce JSON, que Discord
  refuse, un message Discord. Il contient un embed (titre « Mouvement ·
  Jardin », « Événement manuel · Jardin », « Personne · Jardin » ou « Fin de
  l'événement · Jardin », objets
  et scores, zones, durée, heure de l'événement, numéro). L'image est envoyée
  en pièce jointe (`multipart/form-data`, champs `payload_json` et
  `files[0]` = `evenement.jpg`) et affichée dans l'embed.
- Une réponse en erreur est rapportée avec sa raison quand le serveur en
  donne une (champ `message` d'une réponse JSON, ou début du texte, 160
  caractères au plus) : `"réponse 400 : Cannot send an empty message"`.
- Chaque webhook a sa propre file (50 appels) et son propre envoi : une
  adresse lente ou injoignable ne retarde ni RustNVR ni les autres. Délai de
  10 s, un nouvel essai après 5 s, puis un avertissement dans le journal avec
  le seul nom de la machine (jamais le chemin ni le jeton).
- `POST /test` prend un webhook et l'appelle une fois avec `"type": "test"` :
  `{ "status": 204, "ms": 3, "error": null }` ou `{ "status": null, "error":
  "connexion impossible" }`.

### `POST /api/v1/backup` · `POST /api/v1/backup/inspect` · `POST /api/v1/backup/restore`

Sauvegarde de la configuration, réservée aux administrateurs (elle contient
tous les mots de passe). Contenu : le fichier de configuration, les fichiers
écrits par l'interface (`cameras.ui.toml`, `storage.ui.toml`,
`settings.ui.toml`, `mqtt.ui.toml`, `ai.ui.toml`, `webhooks.ui.toml`,
`updates.ui.toml`) et les réglages de détection de chaque caméra (zones,
lignes, masques). Ni enregistrements, ni événements, ni comptes.

- `POST /backup` `{ "passphrase": "…" }` (8 caractères au moins) : le fichier
  `rustnvr-config-AAAAMMJJ-HHMM.rnvrbak`, entièrement chiffré (clé tirée de la
  phrase par Argon2id, ChaCha20-Poly1305). 400 si la phrase est trop courte.
- `POST /backup/inspect` `{ "data": "<fichier en base64>", "passphrase" }` :
  `{ "created_ms", "rustnvr", "files": [{ "name", "state": "same" | "changed"
  | "new" | "not_in_backup", "problem": null }], "cameras": [{ "id", "name"
  }], "motion": ["jardin"], "problems": [] }`. `cameras` : les caméras une
  fois tout restauré ; `problems` : pourquoi la configuration ne se
  chargerait pas si tout était restauré (elle est chargée à l'essai dans un
  dossier temporaire) ; `files[].problem` : pourquoi elle ne se chargerait
  pas en restaurant ce seul fichier. Sauvegarde d'une autre installation :
  dans `ai.ui.toml`, un modèle `.onnx` introuvable mais présent sous le même
  nom dans `<data_dir>/models` est pris là (Docker `/data/…` → LXC
  `/var/lib/rustnvr/…`). `config.toml` est propre à chaque installation
  (dossiers, port) : l'interface ne le coche pas d'office.
  400 : phrase incorrecte, fichier abîmé ou pas une sauvegarde.
- `POST /backup/restore` `{ "data", "passphrase", "files": ["webhooks.ui.toml"],
  "motion": true }` : écrit les fichiers choisis (de façon atomique, 0600)
  après avoir vérifié que la configuration résultante se charge, et les
  réglages de détection si `motion`. `{ "files": [...], "motion": 2,
  "restart_needed": true }`. Pris en compte au **redémarrage** de RustNVR ;
  d'ici là `GET /system` renvoie `config_restored: true` (bandeau de
  l'interface). Un fichier absent de la sauvegarde n'est jamais supprimé.

### `GET /api/v1/updates` · `PUT /api/v1/updates` · `POST /api/v1/updates/check`

Nouvelles versions : la dernière publication (« release ») du dépôt surveillé
est comparée à la version en cours, au démarrage (30 s après) puis toutes les
12 heures. Rien n'est téléchargé ni installé. Le dépôt est le dépôt officiel
(`https://github.com/Guiyomee/RustNVR`) sauf si `config.toml` en donne un
autre (`[updates] repository`, Gitea, Forgejo ou GitHub) ; `repository` vaut
`null` quand la vérification est désactivée.

```json
{ "current": "2026.11.1", "repository": "https://gitea.example.org/owner/RustNVR",
  "latest": "v2026.12.01", "available": true, "name": "RustNVR 2026.12.01",
  "notes": "## Nouveautés\n- …", "url": "https://gitea.example.org/owner/RustNVR/releases/tag/v2026.12.01",
  "published_at": "2026-12-01T17:00:00Z", "checked_at_ms": 1793726203661,
  "error": null, "install": "docker" }
```

- `latest` : étiquette (`tag_name`) de la dernière publication ; `null` si le
  dépôt n'en a aucune (sans erreur). Comparaison numérique (`v2026.12.01` >
  `v2026.11.02`, un zéro en tête ne compte pas, une version finale passe devant ses `-rc`).
- `notes` : texte Markdown de la publication, affiché tel quel (jamais
  interprété comme HTML) ; 20 000 caractères au plus.
- `install` : `docker` (fichier `/.dockerenv`), `systemd` (variable
  `INVOCATION_ID`) ou `other`, pour la procédure affichée.
- `error` : dépôt injoignable, accès refusé (dépôt privé : jeton par
  `[updates] token_env`), réponse illisible.
- `PUT` (administrateur) : `{ "check": true }` active la vérification,
  `{ "check": false }` la coupe ; écrit dans `updates.ui.toml` (0600), puis
  vérification immédiate. `POST /check` (administrateur) vérifie tout de suite.
- `GET /system` porte `update_available` (version publiée plus récente, ou
  `null`), pour le bandeau de l'interface.

### `GET /api/v1/logs?since=&level=&camera=&limit=`

Derniers messages du journal (les 2 000 derniers depuis le démarrage, gardés
en mémoire), réservés aux administrateurs (403 pour un lecteur). Ce sont les
messages de la console, avec le même filtre (`RUST_LOG`) : jamais de mot de
passe ni d'URL complète de caméra.

```json
{ "entries": [ { "seq": 42, "time_ms": 1793713034100, "level": "warn", "target": "rustnvr_stream::supervisor",
                 "camera": "entree", "message": "stream unavailable attempt=2 retry_in_ms=1966 error=…" } ],
  "last_seq": 42 }
```

- `since` : seulement les messages de `seq` supérieur (lecture incrémentale :
  repasser `last_seq`) ;
- `level` : niveau le moins grave montré, `error`, `warn`, `info` (défaut),
  `debug`, `trace` ;
- `camera` : messages d'une caméra (déduite du message ou de son contexte) ;
- `limit` : les derniers messages au plus (500 par défaut, 2 000 au plus).

### `GET /api/v1/ai` · `PUT /api/v1/ai` · `POST /api/v1/ai/test?camera=…` · `POST /api/v1/ai/probe` · `POST /api/v1/ai/models/{id}`

`POST /ai/probe` (administrateur) : `{ "url": "http://127.0.0.1:32168",
"api_key_env": null }` → vérifie un serveur CodeProject.AI / DeepStack **sans
rien enregistrer**, en lui faisant analyser une mire 320×240 intégrée au
programme (délai 30 s, le premier appel pouvant charger le modèle).
Réponse : `{ "ok": true, "ms": 950, "device": "CPU", "error": null }`, ou
`ok: false` avec la raison en français et ce qu'il faut vérifier (serveur
injoignable, autre chose qu'un serveur de reconnaissance sur ce port, 404 sans
module de détection, détection en échec, délai dépassé, variable de clé
d'API absente). 400 si l'adresse ne commence pas par `http://` ou `https://`.

Réglages de la reconnaissance d'objets (section `[ai]`), modifiables depuis
l'interface et appliqués sans redémarrage :

```json
{ "settings": { "provider": { "type": "onnx", "model": "/var/lib/rustnvr/models/yolox_nano.onnx" },
                "min_score": 0.5, "labels": ["person", "car"], "timeout_seconds": 10,
                "max_pictures": 5, "interval_seconds": 3,
                "night": { "min_score": 0.75, "from": "20:00", "to": "07:00" } },
  "active": "onnx", "device": "CPU", "error": null, "onnx_available": true,
  "detection_ready": true, "managed": true,
  "models": [{ "id": "yolox_nano", "name": "YOLOX-nano", "bytes": 3659407, "license": "Apache-2.0", "hint": "Le plus léger : …",
              "path": "/var/lib/rustnvr/recordings/.rustnvr/models/yolox_nano.onnx", "downloaded": false }] }
```

- `night` (facultatif, `null` : aucun) : de `from` à `to` (heure locale du
  serveur, `HH:MM`, à cheval sur minuit quand `from` est plus tard), un objet
  doit atteindre `night.min_score` au lieu de `min_score`.
- `PUT` prend l'objet `settings` (sans `provider` : reconnaissance coupée ;
  `{"type":"deepstack_http","url":"…","api_key_env":"…"}` pour CodeProject.AI).
  Le fournisseur est démarré avant l'enregistrement : un modèle absent ou
  illisible est refusé (400) et les réglages en place restent actifs. Les
  réglages sont écrits dans `ai.ui.toml`, qui remplace `[ai]` de
  `config.toml` ; les capteurs « Personne » de Home Assistant sont annoncés ou
  retirés aussitôt.
- `POST /ai/test?camera=…` analyse une image actuelle de la caméra avec le
  fournisseur actif et renvoie `{ "ms", "device", "objects": [{ "label", "score", "bbox",
  "kept" }], "image": "data:image/jpeg;base64,…" }` ; `kept` indique si l'objet
  passe les filtres (confiance, `labels`, objets de la caméra). 400 si aucun
  fournisseur n'est actif.
- `device` : où les analyses tournent. Toujours `CPU` pour la reconnaissance
  intégrée (tract n'utilise pas de carte graphique) ; pour CodeProject.AI, le
  champ `inferenceDevice` de sa dernière réponse (`CPU`, `GPU`…), `null` avant
  la première analyse ou avec DeepStack, qui ne le donne pas.
- `models` : les sept modèles YOLOX publiés, du plus léger au plus gros
  (`yolox_nano`, `yolox_tiny`, `yolox_s`, `yolox_m`, `yolox_l`,
  `yolox_darknet`, `yolox_x` ; `hint` : vitesse, mémoire et précision
  mesurées, voir docs/AI.md). Téléchargement abandonné au bout de 30 min.
  `POST /ai/models/{id}` (administrateur) télécharge un modèle de `models` dans
  `<data_dir>/models/` depuis sa publication officielle, vérifie sa taille et
  son empreinte SHA-256 avant de le garder (écrit à côté puis renommé), et
  renvoie l'entrée de `models` (`downloaded: true`). Rien n'est téléchargé si le
  bon fichier est déjà là. 404 pour un modèle inconnu, 500 avec la raison si le
  téléchargement échoue (pas d'accès à Internet, empreinte inattendue). Le
  modèle n'est utilisé qu'après un `PUT /ai` avec son `path`.
- `keep_pictures` (défaut `false`) : chaque image analysée (images des
  événements et des passages de ligne vérifiés) est gardée avec ce qui y a
  été trouvé, rien compris, dans la destination par défaut :
  `<racine>/.captures/<caméra>/<AAAA-MM-JJ>/<heure UTC>-e<événement>-p<n>.jpg`
  (ou `-line-<ligne>.jpg`) et un `.json` à côté (`at_ms`, `event`, `picture`,
  `line`, `provider`, `ms`, `found`, `kept`). Le disque local quand la
  destination par défaut envoie ailleurs (S3/SFTP) ou ne répond pas. Les
  jours plus vieux que `retention_days` de cette destination sont effacés
  (vérifié chaque minute) ; pas de rétention : gardés.

### `GET /api/v1/ai/captures` · `GET /api/v1/ai/captures/{camera}/{day}[/{file}]` · `DELETE /api/v1/ai/captures/{camera}/{day}` · `POST /api/v1/ai/replay`

Images gardées par `keep_pictures` et rejeu de la reconnaissance dessus.

- `GET /ai/captures` : `[{ "camera", "day", "count", "bytes" }]`, le jour le
  plus récent d'abord. 400 dans les toutes premières secondes après le
  démarrage (stockage pas encore vérifié).
- `GET /ai/captures/{camera}/{day}` : les images du jour, dans l'ordre :
  `[{ "file", "at_ms", "event", "picture", "line", "provider", "ms", "found",
  "kept" }]` ; `…/{file}` : le JPEG. Les noms sont vérifiés (identifiant de
  caméra, `AAAA-MM-JJ`, nom de fichier sans `/`) : 400 sinon.
- `DELETE /ai/captures/{camera}/{day}` (administrateur) : `{ "deleted": 12 }`.
- `POST /ai/replay` `{ "camera", "day", "file"? }` (administrateur) : analyse
  de nouveau les images du jour (200 au plus), ou la seule image `file` (404
  si elle n'existe pas ; page « Images » de l'interface), avec le fournisseur et les
  réglages actuels (score, objets, zones, lignes) :
  `{ "provider", "changed", "skipped", "pictures": [{ "before": {…comme
  ci-dessus}, "found", "kept", "ms", "changed" }] }`. `changed` : les objets
  qui comptent (`kept`) diffèrent de l'époque. 400 si aucun fournisseur
  n'est actif.

### `GET /api/v1/cameras/{id}/motion` · `PUT /api/v1/cameras/{id}/motion`

Réglages de détection de mouvement d'une caméra (valeurs par défaut :
désactivée, sensibilité 50, aucune zone). Stockés dans la base SQLite ; ils
valent aussi pour les caméras de `config.toml`.

```json
{ "enabled": true, "sensitivity": 50,
  "zones": [ { "name": "Allée", "points": [[0.1, 0.5], [0.9, 0.5], [0.9, 1], [0.1, 1]], "sensitivity": 60 } ] }
```

- `sensitivity` : 1 (seuls les grands changements contrastés) à 100 (le
  moindre changement) ; sans zone, elle s'applique à toute l'image.
- `objects` : objets à reconnaître pour cette caméra (IA optionnelle), parmi
  les `labels` de `[ai]` ; vide ou absent : tous.
- `zones` : 8 au plus, 3 à 32 points chacune, coordonnées en fractions de
  l'image (0..1). Le mouvement n'est compté que dans les zones.
  Chaque zone peut avoir `objects` (IA optionnelle) : ces objets ne comptent
  que si le bas de leur cadre (les pieds d'une personne) est dans l'une des
  zones qui les citent ; les objets cités nulle part comptent dans toutes les
  zones. Avec des zones, rien ne compte en dehors : ni mouvement, ni objet
  reconnu (un objet sans cadre, qu'on ne peut placer, non plus). L'objet
  gardé porte alors `zone` (nom de la zone) dans l'événement et dans MQTT.
- `masks` : masques de confidentialité, 8 au plus,
  `{ "name": "Voisin", "rect": [x1, y1, x2, y2] }` en fractions de l'image.
  Noircis dans les captures (`/snapshot`), les vignettes, l'image analysée par
  l'IA (donc dans MQTT et les webhooks) et les exports (un export d'une caméra
  masquée est toujours converti en H.264 : les masques y sont incrustés) ;
  ignorés par la détection. Les enregistrements gardent l'image complète (pas
  de réencodage) : l'interface noircit le direct et la relecture à partir de
  `privacy_masks` de la caméra (`GET /cameras`). Ce masque d'affichage n'empêche
  pas de télécharger un segment complet par l'API (`/recordings/…`).
- `lines` : lignes de passage, 4 au plus,
  `{ "name": "Portail", "a": [x, y], "b": [x, y], "direction": "both" | "forward" | "backward" }`.
  Le centre de ce qui bouge (pixels changés hors masques, 5 images/s) est
  suivi d'une image à l'autre ; un passage est compté quand ce pas traverse
  le segment. `forward` : de la gauche de `a → b` (tel que tracé) vers sa
  droite, la flèche de l'interface. Un même passage n'est compté qu'une fois
  par ligne en 2 s ; un saut de plus de 30 % de l'image n'est pas un passage.
  Une seule chose en mouvement à la fois : plusieurs mouvements simultanés se
  confondent. Passages en base (purgés avec les événements), dans MQTT
  (`<p>/<caméra>/crossing`, `<p>/<caméra>/line/<ligne>/today`), dans
  `GET /crossings`, sur le WebSocket (`crossing`) et aux webhooks
  (`crossing`).
  `objects` (facultatif, IA optionnelle), par exemple `["person"]` : à chaque
  passage, l'image du moment est analysée ; le passage ne compte (et n'est
  annoncé, avec `object`) que si l'un de ces objets est sur la ligne (bas ou
  centre de son cadre à moins de 25 % de l'image du segment). Reconnaissance
  arrêtée : tout passage compte, sans `object` ; reconnaissance en échec : le
  passage compte sans `object` (avertissement au journal).
  `presence: true` (demande `direction: "both"`) : compte qui est à
  l'intérieur, la flèche indiquant l'entrée : entrées − sorties depuis minuit
  (heure locale) ou la dernière remise à zéro, jamais moins de 0 (une sortie
  dont l'entrée a été manquée est ignorée). Publié dans `GET /crossings`
  (`present`) et MQTT (`<p>/<caméra>/line/<ligne>/present`, entités Home
  Assistant « Présents · ligne » et bouton « Remettre à zéro · ligne »).
  Remise à zéro : `POST /api/v1/cameras/{id}/presence/reset`
  `{ "line": "Porte du garage" }` → 204 (404 : pas de telle ligne de
  présence), ou MQTT `<p>/<caméra>/line/<ligne>/presence/reset`.
- `PUT` redémarre la détection de cette caméra (pas son flux) et renvoie les
  réglages enregistrés. 400 si invalide ; 503 si `enabled` sans FFmpeg.

Fonctionnement : FFmpeg (`live.ffmpeg`) décode le sous-flux (ou le flux
principal) en images grises de `motion.width` pixels, `motion.fps` fois par
seconde ; chaque image est comparée à un fond qui s'adapte en quelques
secondes. Un changement de plus de 60 % de l'image est pris pour un
changement d'éclairage (bascule jour/nuit) et ignoré. Un événement commence
après 2 images consécutives avec mouvement, se termine après
`motion.post_seconds` sans mouvement (sa fin est le dernier mouvement vu) et
est découpé au-delà de `motion.max_event_seconds`.

### `GET /api/v1/events?camera=&from=&to=&limit=`

Événements chevauchant `[from, to)` (par défaut les dernières 24 h), du plus
récent au plus ancien ; `kind` : `motion`, ou `manual` (bouton Home
Assistant « Créer un événement », commande MQTT `<p>/<caméra>/event/create` :
score 0, sans zones ; non compté dans les statistiques d'activité) ; `limit` 200 par défaut, 1000 au plus ; 31 jours au plus
par requête. Un événement en cours a `end_ms: null`. 503 si la base de données
n'a pas pu être ouverte.

```json
[ { "id": "42", "camera": "jardin", "kind": "motion",
    "start_ms": 1793607714212, "end_ms": 1793607722212, "score": 0.17,
    "zones": ["Allée"], "thumbnail_url": "/api/v1/events/42/thumbnail",
    "objects": [ { "label": "person", "score": 0.87, "bbox": [0.25, 0.25, 0.5, 0.75], "zone": "Allée" } ] } ]
```

`score` : plus grande fraction d'une zone (ou de l'image) qui a changé.
`objects` : objets reconnus dans la vignette par le fournisseur d'IA optionnel
(`[ai]`, capacité `ai`), du plus sûr au moins sûr ; vide sans IA, sans objet
reconnu ou si l'analyse a échoué. `label` est l'étiquette du fournisseur
(classes COCO : `person`, `car`, `dog`…), `score` sa confiance (0..1), `bbox`
(absent si inconnu) le cadre en fractions de l'image `[x_min, y_min, x_max,
y_max]`. L'événement est d'abord publié sans objets, puis de nouveau (message
`event` du WebSocket) quand l'analyse se termine. Si les objets ont été trouvés sur
une image prise après la vignette, celle-ci est remplacée et `thumbnail_url`
prend le suffixe `?objects=1` (les vignettes étant mises en cache).
`GET /api/v1/events/{id}` renvoie un seul événement. Les événements plus
anciens que `motion.events_retention_days` (30 par défaut) sont supprimés
avec leurs vignettes.

### `GET /api/v1/events/stats?days=7|30`

Statistiques d'activité (page Événements → « Activité ») : événements de
détection comptés à l'heure locale du serveur, sur les 7 (défaut) ou 30
derniers jours.

```json
{ "from_ms": …, "to_ms": …, "days": 7, "total": 138,
  "by_day": [{ "key": "2026-10-27", "count": 1 }, …],
  "by_hour": [1, 1, 1, 0, …],
  "by_camera": [{ "key": "jardin", "count": 81 }, …],
  "by_object": [{ "key": "person", "count": 45 }, …],
  "without_object": 60 }
```

`by_day` contient chaque jour de la période, même sans événement ; `by_hour`
les heures 0 à 23 ; `by_camera` et `by_object` sont triés du plus grand au plus
petit (un événement compte une fois par sorte d'objet). 400 pour une autre
période, 503 sans base de données.

### `GET /api/v1/crossings?days=7|30`

Passages des lignes tracées sur les caméras (`lines` des réglages de
détection), comptés à l'heure locale du serveur :

```json
{ "from_ms": …, "to_ms": …, "days": 7,
  "lines": [{ "camera": "test", "line": "Milieu", "total": 5, "forward": 3,
              "backward": 2, "today": 5, "by_day": [{ "key": "2026-11-03", "count": 5 }, …],
              "present": null }] }
```

Seules les lignes ayant au moins un passage sur la période sont listées,
plus les lignes qui comptent les présences (`present` : qui est à
l'intérieur ; `null` pour les autres lignes). 400 pour une autre période,
503 sans base de données.

### `GET /api/v1/events/{id}/thumbnail`

JPEG (640 px de large au plus) pris 1 s après le début de l'événement,
`Cache-Control: private, max-age=86400, immutable`. 404 sans vignette.

### `GET /api/v1/cameras/{id}/snapshot?stream=main|sub`

Image actuelle en JPEG (1920 px de large au plus), `Cache-Control: no-store` ;
flux principal par défaut. FFmpeg décode les images depuis la dernière image
clé gardée en mémoire et encode la dernière : rien n'est décodé tant que
personne ne demande d'image. 2 captures simultanées au plus. 503 si FFmpeg
est absent ou si la caméra n'a pas d'image récente.

### `GET /api/v1/mqtt` · `PUT /api/v1/mqtt`

Réglages MQTT et état de la connexion, **jamais le mot de passe** (ni le
compte, `username` à `null`, pour un compte lecteur) :

```json
{ "enabled": true, "host": "192.168.1.10", "port": 8883, "tls": true, "ca_file": null, "username": "rustnvr",
  "has_password": true, "client_id": "rustnvr", "topic_prefix": "rustnvr",
  "discovery": true, "discovery_prefix": "homeassistant",
  "managed": true, "connected": true, "error": null }
```

Corps de `PUT` : les mêmes champs modifiables, plus `password` (absent ou
`null` : conservé ; `""` : supprimé). Les réglages sont écrits dans
`mqtt.ui.toml` (permissions 0600), qui remplace alors `[mqtt]` de
`config.toml`, et le client se reconnecte. Topics et entités Home Assistant :
voir [docs/MQTT.md](docs/MQTT.md).

### `GET /api/v1/settings` · `PUT /api/v1/settings`

Réglages généraux modifiables depuis l'interface :

```json
{ "recording": { "segment_seconds": 60, "fragment_millis": 1000, "queue_frames": 256,
                 "max_timestamp_jump_seconds": 10, "retry_after_error_seconds": 5,
                 "pre_roll_seconds": 5, "post_roll_seconds": 5 },
  "motion": { "fps": 5, "width": 320, "post_seconds": 10, "max_event_seconds": 600,
              "events_retention_days": 30 },
  "live_transcode": true, "live_max_bitrate_kbps": 2000, "live_hwaccel": "auto",
  "low_space_percent": 90, "default_destination": "local", "ffmpeg": "ffmpeg", "managed": false }
```

`live_hwaccel` (`live.hwaccel`) : `auto` (conversions H.264 sur la carte
graphique si l'essai de démarrage réussit) ou `off` ; facultatif dans `PUT`
(absent : inchangé). Un changement relance l'essai en arrière-plan ; l'état
réel est dans `GET /system` → `hwaccel`.

`PUT` prend les mêmes champs sauf `ffmpeg` (lecture seule) et `managed`, les
valide (400 avec la liste des problèmes), les écrit dans `settings.ui.toml`
(qui remplace ces valeurs de `config.toml`) et les applique : redémarrage des
caméras si `recording` change (quelques secondes sans enregistrement), de la
détection si `motion` change ; les autres valeurs s'appliquent aussitôt.

`default_destination` : point de stockage utilisé quand on active
l'enregistrement d'une caméra sans en choisir (`record_destinations` vide dans
`POST`/`PUT /cameras`), et proposé pour les nouvelles caméras ; facultatif dans
`PUT` (absent : inchangé) ; 400 si le point de stockage n'existe pas. Un point
supprimé ensuite est remplacé par `local`.

### `GET /api/v1/ws` (WebSocket)

Messages serveur → client (le client n'envoie rien) :

| `type` | Contenu | Quand |
|---|---|---|
| `system` | `{ "system": SystemInfo }` | à la connexion puis toutes les 5 s |
| `camera` | `{ "camera": Camera }` | à la connexion (toutes) puis à chaque changement d'état |
| `cameras` | `{ "cameras": [Camera] }` | caméra ou point de stockage ajouté, modifié ou supprimé |
| `segment` | `{ "recording": Recording }` | segment terminé |
| `event` | `{ "event": Event }` | événement commencé, complété (vignette, zones) ou terminé ; suivi du message `camera` (état `motion`) |
| `recorder_error` | `{ "camera": "id", "message": "…" }` | erreur d'écriture |
| `crossing` | `{ "crossing": { "camera", "line", "direction", "at_ms", "today", "object" } }` | ligne de passage franchie (l'interface notifie ceux qui ont un `object`) |

Si le client prend du retard, il reçoit de nouveau l'état complet. Le serveur
ferme proprement les WebSocket à l'arrêt.

### Hors API

Toute autre route sert l'interface (`web.static_dir`), avec repli sur
`index.html` pour les routes côté client. Si l'interface n'est pas compilée,
une page 503 explique comment la construire.

## Routes prévues (non implémentées)

Aucune pour l'instant : l'IA optionnelle (étape 8) ajoutera ses routes quand
un fournisseur existera.

