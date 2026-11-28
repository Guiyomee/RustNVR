# Fonctionnalités de RustNVR

Liste détaillée, avec ce qui est vérifié et ce qui ne l'est pas encore. Le
résumé est dans le [README](../README.fr.md) ; les limites connues aussi.

## Fonctions

- Caméras déclarées dans un fichier TOML validé au démarrage (toutes les
  erreurs listées en une fois, clés inconnues refusées).
- Client RTSP [Retina](https://crates.io/crates/retina) : TCP (défaut) ou UDP
  (expérimental), H.264 et H.265, authentification Basic (testée) et Digest
  (fournie par Retina, pas encore testée). Retina est une copie corrigée
  (`crates/retina/PATCH.md`) : les zéros non échappés dans un NAL (Tapo
  C200 v2) sont gardés comme le fait FFmpeg ; un serveur qui change
  d'identifiant de session à chaque SETUP (go2rtc) est accepté.
- Une supervision par caméra : délais d'attente, détection de flux bloqué,
  reconnexion avec backoff exponentiel et gigue.
- Enregistrement continu en fMP4 (`avc1`/`hvc1`), segments coupés sur image
  clé, nouveaux segments sur changement de paramètres, reconnexion, perte
  d'images ou saut d'horodatage.
- Écriture sûre : `.mp4.part` → `fsync` → renommage ; récupération
  automatique au démarrage (troncature au dernier fragment complet), et 5 min
  après le retour d'un NAS pour le segment interrompu par sa panne ; la fin
  de ce segment continue sur le disque local (synchronisation du NAS toutes
  les 5 s, ces 5 s gardées en mémoire). Après un
  arrêt brutal, un événement en cours se termine au dernier signe de vie de
  RustNVR (à 15 s près) — testés ; ⚠ pas encore avec une vraie coupure.
- Erreurs d'écriture (disque plein) : segment sauvé jusqu'au dernier fragment
  complet, reprise différée.
- Commandes `probe` (test de connexion), `check-config`, `recover`, `health`
  (le serveur en marche répond-il ? Contrôle de santé de l'image Docker),
  `reset-password` (mot de passe perdu : nouveau mot de passe d'un compte,
  rendu administrateur).
- Sauvegarde et restauration de la configuration (Paramètres → Sauvegarde) :
  un fichier chiffré par une phrase secrète (caméras et leurs mots de passe,
  stockage, MQTT, IA, webhooks, zones et lignes, mur, flèches des trajets),
  restauré avec un aperçu,
  fichier par fichier. Sauvegarde automatique (chaque jour, semaine…) dans
  le dossier `.rustnvr-backups` du NAS ou du disque local, les dernières
  gardées ; jamais dans le point de montage d'un NAS absent (testé sur un
  dossier temporaire, ⚠ pas encore sur le NAS).
- Ajout, modification et suppression de caméras depuis l'interface Web, sans
  redémarrage, avec test de connexion et aperçu en direct. Elles sont stockées
  dans `cameras.ui.toml` (à côté de `config.toml`, lisible par son seul
  propriétaire, car il contient les mots de passe des caméras) ; les caméras de
  `config.toml` restent modifiables uniquement dans ce fichier. Une caméra
  ajoutée depuis l'interface n'enregistre pas par défaut.
- Caméras motorisées (ONVIF PTZ) : flèches et zoom à maintenir, vitesse et
  positions enregistrées (rappel, ajout, suppression) dans le direct, réservés
  aux administrateurs ; sélecteur « Position PTZ » et boutons « PTZ gauche /
  droite / haut / bas / zoom » (un court mouvement par appui) dans Home
  Assistant (⚠ boutons pas encore essayés dans un vrai Home Assistant). Il
  suffit de régler le port ONVIF de la caméra (essayé sur deux caméras
  motorisées, dont une EZVIZ, gauche / droite compris depuis le 21/11).
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
  Signets sur la timeline (une note sur un moment, liste « Signets du
  jour ») ; objets reconnus marqués en vert sur la timeline, vignette au
  survol (⚠ pas encore essayés dans un navigateur).
- Page Vidéos (depuis la 2026.11.11) : les enregistrements d'une caméra
  par jour, en vignettes (première image faite par FFmpeg une fois, gardée
  une semaine), à regarder ou ouvrir dans Lecture — vignette testée sur un
  vrai enregistrement ; ⚠ page pas encore essayée dans un navigateur.
- Suppression à la main (administrateurs, avec confirmation, depuis la
  2026.11.11) : un enregistrement (Vidéos), un événement (Événements, sa
  vidéo reste), une image gardée de l'IA (Images) ; jamais dans une période
  protégée — testé par l'API et le stockage ; ⚠ corbeilles pas encore
  essayées dans un navigateur.
- Ordre des caméras gardé par le serveur (glisser les vignettes de la vue
  d'ensemble) : le même sur tous les appareils, pour tous les comptes et sur
  toutes les pages — testé par l'API ; ⚠ glisser-déposer pas encore essayé
  dans un navigateur.
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
  SQLite avec vignette, page Événements (jusqu'à 30 jours, recherche par
  objet et par zone faite par le serveur), timeline, captures JPEG à la
  demande. Programmation horaire par caméra : jours et heures où la
  détection compte (et où une caméra « sur mouvement » enregistre).
  Coût mesuré : 2,4 % d'un cœur pour FFmpeg sur un sous-flux H.265 768×432.
- IA optionnelle, désactivée par défaut et sans modèle fourni : l'image de
  chaque événement est analysée par un modèle YOLOX exécuté par RustNVR
  (31 ms par image, sans autre programme ; avec ONNX Runtime, livré dans
  l'archive Debian / Proxmox, 2 à 4 fois plus vite que tract : testé sur la
  VM et un Raspberry Pi 4, ⚠ pas encore sur le LXC) ou envoyée à un serveur DeepStack /
  CodeProject.AI (objets reconnus dans l'API, MQTT et l'interface) ; un seuil
  plus exigeant la nuit est possible, à heures fixes ou dès que la caméra
  passe en vision nocturne ([AI.md](AI.md)).
- Lecture des plaques d'immatriculation (facultative, deux petits modèles
  MIT à télécharger) : sur une image du flux principal, la plaque de chaque
  véhicule reconnu va dans l'événement, l'API, MQTT et les webhooks
  (`{plaque}`), et se cherche dans Événements. Essayée sur 89 photos de
  plaques françaises (86 lectures, toutes justes sauf deux incomplètes ;
  146 ms par image avec ONNX Runtime) ; ⚠ pas encore sur de vraies caméras.
- Stockage S3 (Amazon, MinIO, Backblaze, Wasabi…) et SFTP : les segments
  sont écrits dans un dossier d'attente local puis envoyés un par un
  (reprise automatique après une coupure, rien n'est perdu au redémarrage) ;
  clé du serveur SFTP vérifiée (empreinte fixée ou retenue à la première
  connexion).
- Vision nocturne mesurée sur l'image (noir et blanc = infrarouge), chaque
  minute et pour toutes les marques : étiquette dans l'interface, capteur
  « Vision nocturne » dans Home Assistant — mesure vérifiée avec FFmpeg sur
  une mire en couleur et en noir et blanc ; ⚠ seuils pas encore vérifiés sur
  de vraies caméras de nuit.
- Redémarrer une caméra par ONVIF (bouton de la page de la caméra, bouton
  Home Assistant) et fiche de la caméra (fabricant, modèle, firmware, numéro
  de série, dans « Informations techniques » et sur l'appareil Home
  Assistant) — testés de bout en bout contre une caméra ONVIF simulée ; ⚠ pas
  encore sur une vraie caméra.
- Photo pleine résolution de la caméra (ONVIF `GetSnapshotUri`, au choix par
  caméra) pour l'image des événements, l'IA et les notifications, avec repli
  sur l'image du sous-flux ; la photo de chaque profil est essayée et la
  plus grande gardée ; taille au choix (1280, 1920 px ou taille d'origine) ; bouton « Essayer » — tests (adresse de la
  photo, plus grande photo, HTTP Digest de la RFC 2617 et Basic, refus, réponse qui n'est pas
  un JPEG, réduction à 1920 px et masques avec FFmpeg) ; essayé sur une
  EZVIZ H8c, qui ne donne que 768×432 en ONVIF (limite de la caméra), et
  sur une sonnette Reolink PoE : 1920×1440 (port HTTP à ouvrir) ; ⚠ Tapo et
  Imou pas encore.
- Pause d'une caméra (page de la caméra, liste des caméras, interrupteur
  « Caméra active » de Home Assistant) : ni connexion, ni enregistrement, ni
  détection jusqu'à la reprise ou au redémarrage de RustNVR (jamais
  enregistrée) — test de bout en bout avec une caméra simulée ; ⚠ pas encore
  dans un vrai Home Assistant.
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
- Notifications coupées pour une seule caméra (onglet Détection, ou
  interrupteur « Notifications » dans Home Assistant), sans toucher à son
  enregistrement ; les pannes passent toujours (⚠ pas encore essayé avec
  un vrai webhook ni dans Home Assistant).
- Notifications (Paramètres → Webhooks) : webhook JSON, Discord, ntfy et
  Telegram, pour les mouvements, objets reconnus, passages de ligne et les
  pannes (stockage, caméra hors ligne, image figée ou masquée, erreur
  d'enregistrement, IA en panne après 3 échecs de suite ; ⚠ l'image figée ou
  masquée et l'IA en panne pas encore essayées sur de vraies caméras) ; messages
  personnalisés, au plus une notification toutes les X minutes par caméra,
  programmation horaire (les pannes passent toujours) et, pour Discord, ntfy
  et Telegram, une courte vidéo de l'événement en second message (H.264,
  masques compris ; conversion vérifiée avec FFmpeg, ⚠ pas encore reçue sur
  un vrai téléphone).
- Interface dans la barre latérale de Home Assistant (ingress), avec l'app
  « RustNVR Proxy » (dépôt RustNVR-HomeAssistant depuis la 2026.11.11)
  ([wiki](https://github.com/Guiyomee/RustNVR/wiki/FR-Home-Assistant)) ;
  depuis l'app 1.1.0, page d'ouverture au choix (`page`) et panneau visible
  par les comptes Home Assistant non administrateurs, ouvert seulement si
  RustNVR demande une connexion ou aux comptes de l'option `users` (testé
  avec nginx dans un conteneur, ⚠ pas encore dans un vrai Home Assistant).
- App Home Assistant « RustNVR » (bêta, x86-64 et ARM 64 bits, dépôt
  RustNVR-HomeAssistant) : RustNVR lui-même dans Home Assistant OS, sur
  l'image Docker de la même version ; mot de passe admin en option,
  enregistrements dans `/media/rustnvr`, MQTT de Mosquitto branché au
  premier démarrage, interface par ingress (essayée dans un vrai Home
  Assistant OS x86-64 avec l'image de test Gitea le 26/11 ; ⚠ pas encore
  sur un Raspberry Pi).
- Mur d'écrans : toucher une caméra l'affiche seule, avec son son, et la
  toucher encore ramène au mur (⚠ pas encore essayé dans un navigateur).
- Mur d'écrans, réglage général gardé sur le serveur (caméras, nombre par
  ligne, agrandissement), le même sur tous les écrans et dans Home
  Assistant ; une adresse avec ses propres réglages reste possible (⚠ pas
  encore essayé dans un navigateur).
- Trajets entre caméras (Paramètres → Trajets) : des flèches « de → vers »
  avec un délai maximal ; les événements où l'IA reconnaît la même sorte
  d'objet sur des caméras reliées forment un trajet, une seule carte dans
  Événements, relu caméras côte à côte dans Lecture ; au choix de chaque
  notification, un seul message par trajet avec sa vidéo (caméras à la
  suite, bandeau « caméra · heure », son des caméras qui en ont) ; export en un seul MP4 ; MQTT
  `<p>/journey` (tests du
  regroupement, des notifications et une vraie conversion FFmpeg de la
  vidéo ; ⚠ pas encore avec de vraies caméras, un vrai téléphone ni dans un
  navigateur).
- Mise à jour en un clic pour une installation faite par `install.sh`
  (Debian, Proxmox) : un service root séparé télécharge, vérifie et installe
  la version demandée ; sous Docker, les commandes à lancer (⚠ unité
  systemd pas encore essayée sur un vrai LXC).
- ARM 64 bits (2026.11.11) : archive `linux-aarch64` compilée en croisé
  depuis Debian 12 par `release.sh` (mise à jour d'un Raspberry Pi 4 sous
  Raspberry Pi OS 13 essayée, ONNX Runtime compris), image Docker
  amd64 + arm64 (programme compilé en croisé, image finale par
  architecture) ; ⚠ pas encore : installation neuve sur un Pi, hôte
  Proxmox ARM, workflows GitHub (runner ARM, image arm64 publiée).
- Page d'accueil au choix par navigateur (Paramètres → Général : vue
  d'ensemble, mur d'écrans, événements, images ou lecture ; ⚠ pas encore
  essayée dans un navigateur).
- API REST `/api/v1` et WebSocket temps réel
  ([API_CONTRACT.md](../API_CONTRACT.md)) ; double authentification (code
  TOTP d'une application, codes de secours) et jetons d'API par appareil, en
  lecture seule ou complets (⚠ interface pas encore essayée dans un
  navigateur) ; comptes administrateur / lecteur
  avec page de connexion (mots de passe Argon2, sessions par cookie, blocage
  après 5 essais manqués), HTTP Basic pour les scripts. Un lecteur peut être
  limité à certaines caméras (les autres n'existent pas pour lui) et n'a ni
  les pages Caméras et Stockage, ni les réglages. Journal d'activité pour les
  administrateurs (Paramètres → Activité des comptes) : connexions, refusées
  comprises, et tout ce qui modifie quelque chose, avec le compte et l'adresse.
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
tel quel, ou converti en H.264 pour un lecteur sans H.265). Par caméra, un
time-lapse quotidien au choix (une image par minute, 720p, gardé 30 jours dans
`.timelapse/`) se regarde dans l'onglet Enregistrements — testé par les tests
unitaires seulement, ⚠ pas encore sur une vraie journée. La lecture dans le
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
