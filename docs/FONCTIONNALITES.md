# Fonctionnalités de RustNVR

Liste détaillée, avec ce qui est vérifié et ce qui ne l'est pas encore. Le
résumé est dans le [README](../README.fr.md) ; les limites connues aussi.

## Fonctions

- Caméras déclarées dans un fichier TOML validé au démarrage (toutes les
  erreurs listées en une fois, clés inconnues refusées).
- Client RTSP [Retina](https://crates.io/crates/retina) : TCP (défaut) ou UDP
  (expérimental), H.264 et H.265, authentification Basic (testée) et Digest
  (fournie par Retina, pas encore testée).
- Une supervision par caméra : délais d'attente, détection de flux bloqué,
  reconnexion avec backoff exponentiel et gigue.
- Enregistrement continu en fMP4 (`avc1`/`hvc1`), segments coupés sur image
  clé, nouveaux segments sur changement de paramètres, reconnexion, perte
  d'images ou saut d'horodatage.
- Écriture sûre : `.mp4.part` → `fsync` → renommage ; récupération
  automatique au démarrage (troncature au dernier fragment complet).
- Erreurs d'écriture (disque plein) : segment sauvé jusqu'au dernier fragment
  complet, reprise différée.
- Commandes `probe` (test de connexion), `check-config`, `recover`, `health`
  (le serveur en marche répond-il ? Contrôle de santé de l'image Docker),
  `reset-password` (mot de passe perdu : nouveau mot de passe d'un compte,
  rendu administrateur).
- Sauvegarde et restauration de la configuration (Paramètres → Sauvegarde) :
  un fichier chiffré par une phrase secrète (caméras et leurs mots de passe,
  stockage, MQTT, IA, webhooks, zones et lignes), restauré avec un aperçu,
  fichier par fichier.
- Ajout, modification et suppression de caméras depuis l'interface Web, sans
  redémarrage, avec test de connexion et aperçu en direct. Elles sont stockées
  dans `cameras.ui.toml` (à côté de `config.toml`, lisible par son seul
  propriétaire, car il contient les mots de passe des caméras) ; les caméras de
  `config.toml` restent modifiables uniquement dans ce fichier. Une caméra
  ajoutée depuis l'interface n'enregistre pas par défaut.
- Caméras motorisées (ONVIF PTZ) : flèches et zoom à maintenir, vitesse et
  positions enregistrées (rappel, ajout, suppression) dans le direct, réservés
  aux administrateurs ; sélecteur « Position PTZ » dans Home Assistant. Il
  suffit de régler le port ONVIF de la caméra (essayé sur deux caméras
  motorisées ; une EZVIZ n'applique pas le pivotement gauche / droite en ONVIF).
- Webcams USB (V4L2) sans programme en plus : choisies dans la liste des
  webcams du serveur, encodées en H.264 par FFmpeg (sur la carte graphique si
  elle sert aux conversions) ou copiées quand elles envoient du H.264 ;
  enregistrées, vues en direct et analysées comme une caméra RTSP (vérifié
  avec une webcam sous Docker ; ⚠ pas encore dans un LXC ; pas de son).
- Republication RTSP (désactivée par défaut) : Home Assistant, Frigate, VLC…
  lisent les caméras depuis RustNVR (`rtsp://machine:8554/<caméra>`, et
  `/<caméra>/sub` pour le sous-flux) au lieu de s'y connecter : la caméra ne
  voit qu'une connexion par flux. Images renvoyées telles quelles (H.264,
  H.265 ; son AAC, ou l'Opus tiré du G.711), sans conversion ; rien ne
  tourne sans client. RTP dans la connexion RTSP (TCP) seulement, pas d'UDP ;
  identifiant et mot de passe facultatifs (Digest ou Basic), généraux ou
  propres à une caméra ; cinq erreurs bloquent l'adresse 15 minutes, et une
  adresse ne peut pas prendre toutes les places. Écoute sur `127.0.0.1` par
  défaut ; le réseau avec un mot de passe, ou après un accord explicite.
  Réglée dans Paramètres → Réseau et dans la fiche des caméras, sans
  redémarrage. Coût mesuré (`cargo run --release -p rustnvr-restream
  --example cost`, flux de synthèse à 4 Mbit/s, clients sur la même
  machine) : environ 0,1 % d'un cœur et 0,5 Mio par client ; 1,4 % d'un cœur
  et 12 Mio pour 16 clients (vérifié avec Retina et FFmpeg contre une caméra
  simulée ; ⚠ pas encore avec Home Assistant ni Frigate).
- Points de stockage : chaque caméra enregistre sur le disque local et/ou sur
  un partage NAS (SMB/CIFS, NFS) monté dans le système, écrit directement.
  Contrôle que le partage est bien monté (sinon le disque système se
  remplirait) et repli automatique sur le disque local pendant une panne du
  NAS. Rétention par point de stockage (durée en jours, taille maximale),
  appliquée toutes les 10 minutes. Gérés dans Paramètres → Stockage
  (`storage.ui.toml`) ou dans `config.toml`.
- Relecture HLS : les segments qui se suivent sont lus sans pause (hls.js, HLS
  natif sur Safari), synchronisation de plusieurs caméras, vitesses ×1 à ×8.
- Interface Web (React) servie par RustNVR : vue d'ensemble, caméras et état
  RTSP, lecture des enregistrements avec timeline, stockage, paramètres ;
  direct vidéo sans réencodage (≈ 0,5 s de latence mesurée en local) ; option
  « Faible latence » par WebRTC (prototype, H.264, réseau local uniquement :
  0,3 à 0,7 s d'avance sur le direct standard mesurées). Les fonctions
  indisponibles sur le serveur sont affichées comme « non disponibles ». Une
  [démo en ligne](https://guiyomee.github.io/RustNVR/) montre l'interface avec
  des données fictives.
- Détection de mouvement sans IA, activée caméra par caméra (onglet
  « Détection ») : FFmpeg décode le sous-flux en petites images grises
  (320 px, 5 images/s), comparées à un fond adaptatif ; zones polygonales avec
  sensibilité par zone, changements d'éclairage ignorés. Événements en base
  SQLite avec vignette, page Événements, timeline, captures JPEG à la demande.
  Coût mesuré : 2,4 % d'un cœur pour FFmpeg sur un sous-flux H.265 768×432.
- IA optionnelle, désactivée par défaut et sans modèle fourni : l'image de
  chaque événement est analysée par un modèle YOLOX exécuté par RustNVR
  (31 ms par image, sans autre programme) ou envoyée à un serveur DeepStack /
  CodeProject.AI (objets reconnus dans l'API, MQTT et l'interface) ; un seuil
  plus exigeant la nuit est possible ([AI.md](AI.md)).
- Stockage S3 (Amazon, MinIO, Backblaze, Wasabi…) et SFTP : les segments
  sont écrits dans un dossier d'attente local puis envoyés un par un
  (reprise automatique après une coupure, rien n'est perdu au redémarrage) ;
  clé du serveur SFTP vérifiée (empreinte fixée ou retenue à la première
  connexion).
- Son des caméras enregistré avec la vidéo (AAC tel quel, G.711 converti en
  Opus), désactivable par caméra (interface, MQTT / Home Assistant) ; bouton son
  dans la relecture et dans le direct de la page caméra ; dans le direct WebRTC,
  son des caméras G.711 seulement (Opus ; l'AAC n'y passe pas).
- Enregistrement au choix par caméra : en continu, ou seulement sur mouvement
  (5 s avant, depuis l'image clé précédente, et 5 s après chaque événement par
  défaut ; images d'avant gardées en mémoire, 32 Mio au plus).
- MQTT et découverte Home Assistant, configurables dans Paramètres → MQTT :
  mouvement, connexion, enregistrement, interrupteur de détection, image du
  dernier mouvement par caméra ; commandes : mode d'enregistrement, événement
  manuel, capture tout de suite, pause de la détection sur toutes les caméras
  (mode présent / absent) ([MQTT.md](MQTT.md)).
- API REST `/api/v1` et WebSocket temps réel
  ([API_CONTRACT.md](../API_CONTRACT.md)) ; comptes administrateur / lecteur
  avec page de connexion (mots de passe Argon2, sessions par cookie, blocage
  après 5 essais manqués), HTTP Basic pour les scripts.
- Journaux `tracing` texte ou JSON ; métriques périodiques par caméra
  (i/s, débit, pertes, reconnexions) et processus (CPU, RSS).
- Aucun secret journalisé : identifiants retirés de l'URL, URL affichées sous
  la forme `rtsp://hôte:port`, mots de passe possibles par variable
  d'environnement.

## Fichiers produits

```
<recordings_dir>/<camera>/<AAAA-MM-JJ>/<AAAAMMJJTHHMMSS.mmmZ>.mp4
```

Horodatage UTC du début du segment. Les fichiers sont décodés sans erreur par
FFmpeg (vérifié avec `ffprobe -count_frames`) et exportables sans réencodage :
`ffmpeg -i segment.mp4 -c copy export.mp4`. Depuis l'interface, le bouton
« Exporter » de la page Lecture fait un seul MP4 d'une période choisie (copié
tel quel, ou converti en H.264 pour un lecteur sans H.265). La lecture dans le
navigateur est vérifiée dans Edge pour le H.264 (page Lecture) ; Edge sur la
machine de test ne décode pas le H.265.

## Mesures

Relevés réels, chacun sur une seule installation : des ordres de grandeur, pas
un banc d'essai. La comparaison avec Frigate est dans le README.

**Enregistrement seul.** 4 flux 1080p à 25 images/s et 4 Mbit/s (2 en H.264,
2 en H.265, servis par GStreamer), Intel Core i5-11600K sous WSL2 : 1,8 à
2,1 % d'un cœur et 12 Mio de mémoire pour RustNVR, aucune image perdue,
segments décodés sans erreur par `ffprobe`.

**Autres coûts**

- Détection de mouvement : 2,4 % d'un cœur (FFmpeg) par sous-flux H.265
  768×432.
- Reconnaissance d'objets intégrée, sur un cœur de processeur de bureau :
  ≈ 38 ms par image avec YOLOX-nano, ≈ 413 ms avec YOLOX-s.
- Conversion du son G.711 en Opus : 0,22 % d'un cœur par caméra
  (i5-11600K).
- Conversion H.265 → H.264 d'un enregistrement 1080p de 30 s (relecture dans
  un navigateur sans H.265) : 20,8 s de processeur avec libx264, 5,7 s avec
  une carte NVIDIA (NVENC).
