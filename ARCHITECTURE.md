# Architecture de RustNVR

Ce document décrit l'architecture **cible** et indique explicitement ce qui
est **implémenté** aujourd'hui. Les choix techniques structurants sont
justifiés dans [docs/adr/0001-choix-techniques-fondamentaux.md](docs/adr/0001-choix-techniques-fondamentaux.md).

## Principes

- **Monolithe modulaire** : un seul binaire `rustnvr`, découpé en crates Cargo
  aux dépendances orientées (les crates métier ne dépendent que de
  `rustnvr-core`, jamais les unes des autres sauf mention).
- **Séparation stricte** ingestion → enregistrement → détection → événements → IA.
  L'enregistrement **ne dépend jamais** de la détection ni de l'IA.
- **Aucun décodage ni réencodage** dans le chemin d'enregistrement : les NAL
  H.264/H.265 reçues par RTP sont recopiées telles quelles dans des fichiers fMP4.
- **Files bornées partout**, aucun consommateur lent ne peut bloquer
  l'ingestion ni faire croître la mémoire.
- **Le runtime Tokio n'exécute que de l'I/O réseau non bloquante.** Les
  écritures disque et, plus tard, le décodage pour la détection s'exécutent sur
  des threads dédiés.
- **Aucun secret dans les journaux** : identifiants séparés de l'URL, type
  `Secret` dont `Debug`/`Display` masquent la valeur, URL journalisées sous la
  forme `hôte:port` uniquement.

## Crates

| Crate | Rôle | État |
|---|---|---|
| `rustnvr-core` | Configuration (TOML, validation), identifiants, secrets, types média partagés, événements, bus d'événements | **Implémenté** |
| `rustnvr-stream` | Client RTSP (Retina), supervision par caméra, reconnexion avec backoff, chien de garde, diffusion bornée des images, sonde de connexion, métriques de flux | **Implémenté** |
| `rustnvr-recorder` | Écrivain fMP4, segmentation, récupération après crash, thread d'enregistrement par caméra | **Implémenté** |
| `rustnvr-app` | Binaire `rustnvr` : CLI, assemblage, signaux, arrêt propre, métriques processus | **Implémenté** |
| `rustnvr-testkit` | Serveur RTSP factice H.264/H.265 pour les tests (déconnexion, blocage, auth) | **Implémenté** (dev uniquement) |
| `rustnvr-storage` | SQLite (SQLx 0.9, WAL, migrations) : événements de détection, vignettes, réglages de détection par caméra. Index des segments : prévu | **Implémenté en partie** |
| `rustnvr-mqtt` | Client MQTT (rumqttc), Home Assistant Discovery, commande de la détection (voir docs/MQTT.md) | **Implémenté** (sans TLS) |
| `rustnvr-remote` | Points de stockage distants : envoi des segments terminés vers S3 (object_store) ou SFTP (russh), dossier d'attente local, vérification de la clé SSH | **Implémenté** |
| `rustnvr-webrtc` | Direct WebRTC (prototype) avec str0m : signalisation WHEP, candidats « host », une socket UDP par adresse, H.264 en Annex B depuis le `LiveTap` sans réencodage, son Opus (caméras G.711) | **Prototype** (vérifié dans Edge) |
| `rustnvr-ai` | IA optionnelle : trait `ObjectDetector`, filtres (`Analyzer`), client de l'API HTTP DeepStack / CodeProject.AI, modèles YOLOX exécutés par tract (fonctionnalité `onnx`) ; aucun modèle fourni | **Implémenté** (désactivé par défaut) |
| `rustnvr-restream` | Republication RTSP : serveur RTSP/1.0 (RTP entrelacé en TCP ; identifiant et mot de passe facultatifs, Digest ou Basic, généraux ou par caméra) qui renvoie les images d'un `LiveTap` en paquets RTP (H.264, H.265, AAC, Opus), sans réencodage ; une tâche par client, rien sans client | **Implémenté** (vérifié avec Retina et FFmpeg) |
| `rustnvr-discovery` | Recherche des caméras du réseau : WS-Discovery ONVIF, balayage des ports RTSP, client ONVIF (modèle, profils, `GetStreamUri`, authentification WS-Security), adresses RTSP habituelles des marques ; rien n'est enregistré | **Implémenté** (ONVIF avec identifiants : vérifié contre une caméra simulée seulement) |
| `rustnvr-motion` | Détection de mouvement sans IA : décodage du sous-flux par FFmpeg, fond adaptatif, zones polygonales, événements, captures JPEG | **Implémenté** |
| `rustnvr-api` | Axum : REST `/api/v1` en lecture seule, WebSocket, service des segments MP4 (Range), interface statique, auth Basic, CPU/RAM hôte | **Implémenté en partie** (voir API_CONTRACT.md) |
| `web/` | Interface React/TypeScript compilée en fichiers statiques servis par `rustnvr-api` | **Premier test** (voir FRONTEND.md) |

Les crates « prévues » **n'existent pas encore** dans le dépôt : elles ne sont
pas créées vides pour éviter de donner une fausse impression d'avancement.

## Flux de données (implémenté)

```
                 ┌──────────────────────────── tâche Tokio par flux ──────────────────────────┐
 caméra RTSP ──► │ StreamSupervisor                                                           │
   (TCP)         │   connexion (timeout) → DESCRIBE/SETUP/PLAY → demux Retina                 │
                 │   chien de garde (stall_timeout) · backoff exponentiel + gigue             │
                 │   watch<StreamState> · StreamMetrics (atomiques) · EventBus                │
                 │        │                                                                   │
                 │        ▼  FrameFanout : un mpsc borné par abonné, try_send non bloquant    │
                 └────────┼───────────────────────────────────────────────────────────────────┘
                          │  StreamItem::Frame(VideoFrame)   (Bytes + Arc<VideoParams>, sans copie)
                          │  StreamItem::SessionEnd
        ┌─────────────────┼───────────────────┬──────────────────────────┐
        ▼                 ▼                   ▼                          ▼
  thread recorder    (motion, prévu)     (LiveTap → direct WS/MSE,   (snapshots, prévu)
                                          WebRTC, republication RTSP)
  RecorderCore        sous-flux           sans coût sans spectateur
  → fMP4 *.mp4.part
  → fsync + rename *.mp4
  → NvrEvent::SegmentFinished ──► EventBus (broadcast) ──► journal / (SQLite, MQTT, WebSocket : prévus)
```

### Direct vidéo

Le fan-out possède aussi une `LiveTap` : un canal broadcast borné, alimenté
seulement s'il y a des spectateurs, plus le cache des images depuis la
dernière image clé (≤ 4 Mio par caméra). Chaque spectateur WebSocket a son
propre muxer fMP4 (une image = un fragment) ; un spectateur trop lent est
déconnecté et repart d'une image clé, sans effet sur l'enregistrement.

### Republication RTSP

`rustnvr-restream` s'abonne à la même `LiveTap` que le direct : un client
RTSP coûte une tâche Tokio et une recopie des images en paquets RTP, rien
d'autre (ni décodage, ni cache en plus, ni connexion de plus à la caméra).
Un client trop lent reprend à l'image clé suivante ; s'il ne lit plus rien
pendant 10 s, il est déconnecté. Les horodatages RTP restent continus quand
la caméra se reconnecte ; un changement de codec ou de résolution ferme la
connexion du client, qui redemande la description du flux.

### Contre-pression et pertes

- Chaque abonné possède sa propre file `mpsc` bornée. L'ingestion utilise
  `try_send` : si la file d'un abonné est pleine, **l'image est abandonnée
  pour cet abonné seulement**, un compteur est incrémenté, et les autres
  abonnés ne sont pas affectés.
- Chaque image porte un numéro de séquence `seq` par session. Un abonné qui
  constate un trou (`seq` non contigu) sait qu'il a perdu des images : le
  recorder clôt alors le segment courant et **attend la prochaine image clé**
  pour en ouvrir un nouveau (jamais de fichier avec une référence cassée).
- La mémoire par caméra est bornée par `capacité de file × taille d'image`
  plus le fragment en cours (≈ 1 s de flux).

### Images clés

Une image est « clé » si Retina la signale comme point d'accès aléatoire
(IDR) ou, en H.265, si elle contient une NAL IRAP (BLA/IDR/CRA) — Retina ne
signale pas les CRA (voir ADR 0001).

### Sessions et discontinuités

Une « session » est une connexion RTSP réussie. Le recorder ouvre un nouveau
segment à chaque : nouvelle session, changement de paramètres (SPS/PPS/VPS,
résolution, codec), trou de séquence, recul ou saut de PTS supérieur à
`max_timestamp_jump`, ou dépassement de la durée de segment (coupure sur la
première image clé suivante).

## Modèle d'exécution

| Composant | Exécution | Raison |
|---|---|---|
| Supervision RTSP | 1 tâche Tokio par flux | I/O réseau pure, non bloquante |
| Recorder | 1 thread OS nommé `rec-<camera>` par caméra | Écritures et `fsync` bloquants hors du runtime |
| Métriques | 1 tâche Tokio périodique | Lecture de compteurs atomiques et de `/proc/self` |
| HTTP / WebSocket | tâches Tokio (Axum) ; balayage disque en `spawn_blocking` | Les lectures de répertoires et de fragments ne bloquent pas le runtime |
| Détection (prévu) | pool de threads borné | Décodage CPU hors runtime |

### Arrêt propre

1. `SIGINT`/`SIGTERM` → annulation du `CancellationToken` racine ;
2. chaque superviseur ferme sa session RTSP, envoie `SessionEnd` et libère ses
   émetteurs ;
3. chaque recorder voit sa file se fermer, écrit le dernier fragment, `fsync`,
   renomme le segment, puis termine son thread ;
4. le binaire attend tous les recorders (avec délai maximal) avant de quitter.

## Stockage sur disque (implémenté)

```
<recordings_dir>/<camera_id>/<AAAA-MM-JJ>/<AAAAMMJJTHHMMSS.mmmZ>.mp4        segment terminé
<recordings_dir>/<camera_id>/<AAAA-MM-JJ>/<AAAAMMJJTHHMMSS.mmmZ>.mp4.part   segment en cours
```

Dates en UTC. Les fichiers sont la **source de vérité** : l'index SQLite
(étape 2) sera reconstructible par balayage des fichiers et de leurs fragments.

## Configuration

Fichier TOML validé intégralement au démarrage (toutes les erreurs sont
rapportées en une fois). Voir [config.example.toml](config.example.toml).
Les mots de passe peuvent être fournis par variable d'environnement
(`password_env`) pour ne pas figurer dans le fichier.

## Sécurité

- Implémenté : secrets masqués, identifiants séparés de l'URL, URL
  journalisées sans chemin ni identifiants, identifiants de caméra
  restreints à `[a-z0-9_-]` (utilisés dans les chemins et futurs topics MQTT).
- Interface Web : écoute sur `127.0.0.1` par défaut ; authentification HTTP
  Basic (comparaison en temps constant) sur toutes les routes ; avertissement
  au démarrage et bandeau dans l'interface si elle est désactivée ; refus de
  démarrer sur une adresse réseau sans authentification sauf dérogation
  explicite ; chemins de fichiers validés composant par composant ; en-têtes
  `nosniff`, `X-Frame-Options: DENY`, `no-referrer` ; l'API n'expose jamais
  chemins RTSP ni identifiants.
- Prévu : TLS intégré, sessions et comptes, limitation des tentatives,
  contrôle d'accès par rôle.

## Observabilité

- Journaux structurés `tracing` (texte ou JSON), filtrables par `RUST_LOG`.
- Implémenté : par flux, images, images clés, octets, paquets RTP perdus,
  reconnexions, images abandonnées par abonné ; par processus (Linux), RSS et
  CPU, publiés périodiquement dans les journaux.
- Prévu : exposition via l'API et MQTT, FPS analysés par la détection,
  occupation disque.
