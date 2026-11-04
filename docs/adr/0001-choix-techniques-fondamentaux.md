# ADR 0001 — Choix techniques fondamentaux : RTSP, conteneur, WebRTC, MQTT

- Statut : **Accepté** pour RTSP et conteneur (implémentés dans la tranche 1) ;
  **Proposé** pour WebRTC et MQTT (à confirmer par prototype lors de leur étape).
- Date : 2026-11-01
- Vérifications effectuées : versions et dates relevées sur l'API crates.io et
  l'API GitHub le 2026-11-01 ; API de Retina lue dans le code source de la
  version 0.4.21 (pas seulement la documentation).

---

## 1. Client RTSP/RTP

### Contexte

Le chemin d'ingestion doit recevoir H.264 et H.265 depuis des caméras IP
hétérogènes (souvent des implémentations RTSP non conformes), sans décoder,
avec reconnexion et sans bloquer Tokio.

### Options

| Option | État vérifié | Avantages | Inconvénients |
|---|---|---|---|
| **Retina** 0.4.21 | Publiée le 2026-10-04, dépôt actif (push 2026-10-28), utilisée en production par Moonfire NVR | Rust pur, Tokio natif, H.264 **et H.265** (feature `h265` activée par défaut), auth Basic/Digest, contournements de caméras buggées (live555, session IDs), sortie `FrameFormat::MP4` (NAL préfixées par longueur 4 octets) et **construction de la `VisualSampleEntry` MP4** (`avc1`/`hvc1` avec `avcC`/`hvcC`) | API annoncée instable (issue #47) ; UDP expérimental (pas de buffer de réordonnancement) ; journalise via `log` et non `tracing` ; erreurs non inspectables programmatiquement (sauf `status_code()`) ; refuse les URL contenant des identifiants |
| FFmpeg/libav (bindings) | Mature | Couvre tout | Dépendance C lourde dans le chemin critique, interdite par le cahier des charges |
| GStreamer | Mature | Pipeline complet | Dépendance système lourde, complexité, empreinte mémoire |
| Implémentation maison | — | Contrôle total | Des mois de travail pour atteindre la robustesse de Retina face aux caméras réelles |

### Décision

**Retina 0.4.x**, transport **TCP interleaved par défaut** (UDP configurable mais
marqué expérimental). Points d'intégration vérifiés dans le code :

- `Session::describe(url, SessionOptions::default().creds(..).session_group(..))`
- `session.setup(i, SetupOptions::default().transport(..).frame_format(FrameFormat::MP4))`
- `session.play(PlayOptions::default())?.demuxed()` → `Stream<Item = Result<CodecItem, Error>>`
- `VideoFrame::{data, timestamp, is_random_access_point, has_new_parameters, loss}`
- `VideoParameters::mp4_sample_entry().build()` → boîte `avc1`/`hvc1` complète.

Conséquences et mesures :

- Retina n'applique **pas de délai d'attente** à la connexion TCP ni au flux :
  RustNVR encapsule `describe/setup/play` dans `tokio::time::timeout` et ajoute
  un **chien de garde de blocage** (aucune image pendant `stall_timeout`).
- Les identifiants sont **retirés de l'URL** et passés via `Credentials` ;
  les journaux n'affichent jamais que `hôte:port`.
- Les logs `log` de Retina sont redirigés vers `tracing` (pont `tracing-log`).
- Retina est isolée dans `rustnvr-stream` : le reste du code ne voit que les
  types `rustnvr_core::media`. Un changement d'API de Retina reste local.
- **H.265 : Retina ne marque comme point d'accès aléatoire que les images IDR**
  (vérifié dans `codec/h265/nal.rs` : CRA et BLA sont classées « non intra »).
  Les encodeurs qui utilisent des CRA pour les images clés suivantes (x265 par
  défaut, certaines caméras) ne seraient alors jamais coupés en segments.
  RustNVR considère donc aussi comme images clés les accès contenant une NAL
  IRAP (types 16 à 21), ce qu'autorisent la spécification HEVC (démarrage du
  décodage sur CRA, images RASL ignorées) et ISO/IEC 14496-15 (échantillons de
  synchronisation). Validé par décodage ffprobe de segments commençant par une CRA.
- Images B / PTS non monotones : Retina expose des PTS ; les caméras IP
  utilisent rarement des images B. Le recorder **coupe le segment** si le PTS
  recule (limitation documentée, pas de réordonnancement DTS pour l'instant).

---

## 2. Conteneur d'enregistrement

### Critères

1. robustesse à une coupure (crash, perte de courant, disque plein) ;
2. récupération après crash ;
3. lecture dans le navigateur sans transcodage ;
4. remuxage sans réencodage (export, HLS) ;
5. surcoût et coût CPU d'écriture ;
6. disponibilité d'un écrivain Rust fiable.

### Comparaison

| Critère | MPEG-TS | Matroska | MP4 fragmenté (fMP4) |
|---|---|---|---|
| Coupure brutale | **Excellent** : paquets de 188 octets autonomes, un fichier tronqué reste lisible jusqu'à la coupure | Bon si écrit en clusters de taille inconnue ; lecteurs tolérants | Bon : seul le fragment en cours (1 à 2 × la durée de fragment) est perdu |
| Récupération | Rien à faire | Réparation des tailles d'éléments / index `Cues` absent → recherche lente | **Tronquer après le dernier couple `moof`+`mdat` complet** : algorithme simple et déterministe |
| Navigateur | Non natif hors Safari (HLS). Chrome/Firefox nécessitent un transmuxage JS (hls.js / mpegts.js) côté client | Pas de support standard (MSE n'accepte que WebM VP8/VP9/AV1/Opus) | **Natif** via `<video>` et MSE pour H.264 ; H.265 selon navigateur/OS/matériel ; HLS fMP4 (CMAF) natif Safari, hls.js ailleurs |
| Remux sans réencodage | `ffmpeg -c copy` | `ffmpeg -c copy` | `ffmpeg -c copy` ; **HLS par plages d'octets sur les fragments existants sans réécrire les données** |
| Surcoût | ~2–5 % (en-têtes TS/PES, PAT/PMT/PCR) | < 1 % | < 1 % |
| Horodatage | PTS 33 bits à 90 kHz (rebouclage ~26,5 h à gérer) | Timecodes de cluster relatifs | `tfdt` 64 bits + durées explicites par échantillon |
| Écrivain Rust | crate `mpeg2ts` 0.6.1 (maintenue) | crate `matroska` = lecteur ; pas d'écrivain pur Rust mûr identifié | crate `mp4` 0.14 non maintenue depuis 2023 ; mais un écrivain fMP4 minimal est petit (~400 lignes) et **Retina fournit déjà les sample entries** |

### Décision

**fMP4, un fichier par segment (60 s par défaut), fragments courts (~1 s,
alignés sur les images clés quand c'est possible)**, avec ce protocole
d'écriture :

1. le segment est créé sous `NOM.mp4.part` (`create_new`, jamais d'écrasement) ;
2. en-tête d'initialisation (`ftyp` + `moov` avec `mvex`) écrit immédiatement ;
3. chaque fragment (`moof` + `mdat`) est écrit d'un bloc puis vidé vers l'OS ;
4. à la clôture : `fsync` du fichier, renommage atomique en `NOM.mp4`, `fsync`
   du répertoire ;
5. au démarrage, la récupération parcourt les `*.mp4.part`, **tronque après le
   dernier fragment complet**, et renomme — ou supprime un fichier sans aucun
   fragment complet.

Garanties : un crash du processus perd au plus le fragment en mémoire, soit
environ 2 × `fragment_millis` (un fragment est écrit à la première image clé
après la durée cible, ou de force au double de celle-ci ; mesuré : ≤ 2 s avec un
GOP de 2 s et des fragments de 1 s). Une perte de courant peut perdre en plus ce qui n'a pas été
`fsync` (cache de pages), borné par la durée de segment. Un `fsync` par
fragment n'est pas activé par défaut (coût sur carte SD/eMMC) ; il pourra être
rendu configurable.

Pourquoi pas MPEG-TS malgré sa robustesse supérieure : la lecture navigateur
exigerait un transmuxage permanent (serveur ou client), et la robustesse de
fMP4 avec des fragments d'une seconde et la troncature à la récupération est
suffisante pour un NVR. **MPEG-TS reste l'alternative documentée** si les
essais terrain montrent des problèmes ; l'écrivain est isolé dans le module
`rustnvr-recorder::fmp4` pour permettre un second format.

Matroska est écarté : pas d'avantage navigateur, pas d'écrivain Rust mûr.

Limites connues de la décision :

- pas de boîte `sidx` : la recherche temporelle dans un fichier repose sur le
  parcours des fragments ; l'index SQLite (étape suivante) stockera les offsets
  de fragments pour la timeline et pour générer des playlists HLS à plages
  d'octets ;
- audio non enregistré dans la tranche 1 (AAC passthrough prévu ; G.711 n'est
  pas lisible nativement par les navigateurs dans MP4) ;
- PTS non monotones (images B) → coupure de segment.

---

## 3. Direct vidéo (live)

### Options vérifiées

| Option | État vérifié | Notes |
|---|---|---|
| **str0m** 0.24.1 | Publiée 2026-10-03, dépôt actif (push 2026-11-01) | WebRTC **Sans-I/O** ; packetiseurs H.264 **et H.265** présents dans `src/packet/` ; « RTP mode » pour relayer des paquets ; pas de runtime imposé |
| webrtc-rs `webrtc` 0.21.1 | Publiée 2026-10-11, réécrite sur le cœur Sans-I/O `rtc`, backend Tokio | API asynchrone de haut niveau type Pion ; dépendances plus nombreuses |
| MSE via WebSocket (fMP4) | Réutilise l'écrivain fMP4 de RustNVR | Latence ~0,5–2 s, aucune dépendance, aucune négociation ICE |
| HLS / LL-HLS (fMP4) | Réutilise les fragments | Latence plus élevée (secondes), compatibilité maximale (iOS) |

### Décision (proposée)

- **WebRTC avec str0m** comme voie principale à faible latence : modèle
  Sans-I/O cohérent avec une tâche par caméra, pas de réencodage (les images
  H.264/H.265 de Retina sont repacketisées en RTP par str0m).
- **Repli MSE/WebSocket en fMP4**, très peu coûteux puisqu'il réutilise le
  muxer d'enregistrement, et **HLS fMP4** pour la relecture et les clients
  sans WebRTC.
- Le support H.265 en WebRTC/MSE dépend du navigateur, du système et du
  matériel : il sera **testé** et documenté navigateur par navigateur plutôt
  que supposé.

À confirmer par un prototype (négociation SDP, ICE en LAN et derrière NAT,
coût CPU mesuré sur N100) avant adoption définitive.

**Résultat du prototype (2026-11-02).** L'ordre a été inversé dans la mise en
œuvre : le direct MSE/WebSocket est la voie par défaut (aucune contrainte
réseau, fonctionne en HTTPS derrière un proxy), WebRTC est une option
« Faible latence » (crate `rustnvr-webrtc`, WHEP). Négociation SDP et ICE en
réseau local validés dans Edge : 0,3 à 0,7 s d'avance sur MSE, CPU serveur
négligeable. H.264 seulement en WebRTC (le H.265 passe
par la conversion H.264). Non traité : ICE derrière NAT (STUN/TURN), mesure
sur N100.

---

## 4. MQTT

### Options vérifiées

| Option | État vérifié | Notes |
|---|---|---|
| **rumqttc** 0.25.1 | Publiée 2025-11-21 ; dernier commit sur `rumqttc` 2026-05-01 ; 187 issues ouvertes | Rust pur, Tokio, MQTT 3.1.1 et v5, TLS rustls (feature par défaut `use-rustls`) ou native-tls, Last Will ; la reconnexion se fait en continuant à appeler `EventLoop::poll()` (backoff à implémenter par l'appelant) |
| paho-mqtt | Binding de la bibliothèque C Paho | Dépendance C, build plus lourd |
| Clients bas niveau (`mqttbytes`, `minimq`) | — | Trop de travail protocolaire à notre charge |

### Décision (proposée)

**rumqttc**, MQTT 3.1.1 par défaut (le plus répandu avec Mosquitto/Home
Assistant), TLS via rustls, encapsulé derrière l'interface de
`rustnvr-mqtt` pour pouvoir changer de client. Risque : rythme de maintenance
ralenti — à réévaluer au moment de l'implémentation.

Exigences Home Assistant retenues (détaillées à l'étape MQTT) : `unique_id`
stables dérivés de l'identifiant caméra, topics de disponibilité avec Last Will,
publication `retain` des configurations Discovery, et suppression d'une caméra
par publication d'un payload vide `retain` sur ses topics de configuration.
