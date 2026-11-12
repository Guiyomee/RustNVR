# Journal des versions

Résumé de chaque version ; les notes complètes (captures, commandes,
précautions) sont sur la page de chaque
[release](https://github.com/Guiyomee/RustNVR/releases). Versions
« calendrier » : année.mois.numéro dans le mois.

## Non publiée

### Corrections
- L'image Docker de la 2026.11.07 n'avait pas été publiée sur GitHub : le
  contrôle du code échouait avec Rust 1.100, sorti entre-temps.

## 2026.11.07 (2026-11-12)

### Nouveautés
- **Fiche de la caméra** (onglet « Informations techniques ») : fabricant,
  modèle, firmware et numéro de série, lus par ONVIF ; Home Assistant les
  affiche aussi sur l'appareil de la caméra.
- **Redémarrer une caméra** par ONVIF, depuis sa page ou un bouton dans Home
  Assistant (port ONVIF réglé, compte administrateur de la caméra).
- **Vision nocturne** : RustNVR voit sur l'image quand une caméra passe en
  infrarouge (noir et blanc), quelle que soit sa marque ; étiquette dans
  l'interface et capteur « Vision nocturne » dans Home Assistant.
- **Mettre une caméra en pause** (page de la caméra, liste des caméras, et
  interrupteur « Caméra active » dans Home Assistant) : plus de connexion,
  d'enregistrement ni de détection jusqu'à la reprise ou au prochain
  redémarrage de RustNVR. Demandé sur le forum HACF.
- **Droits par caméra** (Paramètres → Utilisateurs) : un compte lecteur peut
  être limité à certaines caméras. Les autres n'existent pas pour lui : ni
  direct, ni enregistrements, ni événements, ni images, ni exports. Appliqué
  tout de suite, sans reconnexion.
- **Notifications ntfy et Telegram** (Paramètres → Webhooks) : chaque webhook
  a maintenant un type, choisi en tête du formulaire — webhook JSON, Discord,
  **ntfy** (une notification sur le téléphone, depuis ntfy.sh ou votre propre
  serveur) ou **Telegram** (un message d'un bot). Titre lisible (« Personne ·
  Jardin »), détail et image jointe, pour tous les types d'appel. Les
  webhooks Discord existants sont reconnus tels quels.
- **Notifications personnalisées** (Paramètres → Webhooks → « Personnaliser
  les messages ») : titre et texte au choix pour chaque type d'appel, avec
  des variables (`{objet} devant {camera} à {heure}`), priorité ntfy et
  envoi sans son sur Telegram. Et pour tous les webhooks : **au plus une
  notification toutes les X minutes par caméra**, pour ne pas en recevoir dix
  quand quelqu'un traîne devant la caméra (les pannes passent toujours).
- **Webhook « Erreur d'enregistrement »** : un appel quand l'enregistrement
  d'une caméra échoue (disque plein, erreur d'écriture…), un seul même si
  RustNVR réessaie, puis un autre quand il reprend. Message Discord aussi.
- **Activité des comptes** (Paramètres, administrateurs) : qui s'est connecté,
  depuis quelle adresse, les connexions refusées, et qui a modifié quoi
  (réglages, caméras, comptes, exports). Gardé 180 jours.

### Documentation
- Le guide d'installation et d'utilisation est maintenant le
  **[wiki](https://github.com/Guiyomee/RustNVR/wiki)**, en français et en anglais : Docker, Proxmox,
  Debian / Ubuntu, caméras, stockage, détection et IA, notifications, Home
  Assistant, comptes et sécurité, dépannage. `docs/DOCKER.md`,
  `docs/INSTALLATION.md` et `docs/HOME_ASSISTANT.md` y renvoient.

### Corrections
- La création d'un compte affichait « Réponse du serveur illisible » alors
  que le compte était bien créé.

### Sécurité
- Un compte lecteur n'a plus les pages Caméras et Stockage, et ses
  Paramètres se limitent à l'apparence et à son mot de passe ; le serveur ne
  lui donne plus les réglages (général, MQTT, points de stockage).

## 2026.11.06 (2026-11-11)

### Nouveautés
- **Republication RTSP** (Paramètres → Réseau, désactivée par défaut) : Home
  Assistant, Frigate ou VLC lisent les caméras depuis RustNVR
  (`rtsp://machine:8554/<caméra>`, `/<caméra>/sub` pour le sous-flux) : une
  seule connexion à la caméra. Flux renvoyés tels quels (H.264 / H.265, son
  AAC ou Opus), sans conversion ; RTSP sur TCP. Identifiant et mot de passe
  facultatifs, généraux ou propres à une caméra ; écoute sur `127.0.0.1` par
  défaut. Merci cddu33 (#5) !

### Sécurité
- **Derrière un reverse proxy** : `web.trusted_proxies` déclare le proxy ;
  les échecs de connexion bloquent alors le client réel, et non plus tout le
  monde derrière le proxy.
- `settings.ui.toml` est désormais écrit lisible par son seul propriétaire.
- **Dépendances** : audit automatique (`cargo audit`, `npm audit`) à chaque
  changement et chaque semaine, mises à jour proposées par Dependabot,
  actions GitHub épinglées par empreinte.

### Corrections
- La recherche de caméras ne liste plus la republication RTSP de RustNVR
  comme une caméra.
- **PTZ** : une caméra non motorisée qui a un port ONVIF (sonnette, caméra
  fixe) n'affiche plus de panneau « Pilotage PTZ » en erreur.
- Un sous-flux ajouté (ou remis) à une caméra existante sans retaper le mot
  de passe utilise maintenant celui qui est enregistré, comme le formulaire
  l'annonce (« inchangé ») ; il partait sans mot de passe et la caméra le
  refusait indéfiniment.

### Autres
- Dépendances à jour (jiff, toml, md-5, lucide-react, tailwindcss), proposées
  par Dependabot ; quatre bibliothèques devenues inutiles disparaissent.

## 2026.11.05 (2026-11-09)

### Nouveautés
- **Home Assistant** : sélecteur « Mode d'enregistrement » (continu, sur
  mouvement, arrêté), boutons « Créer un événement » (événement manuel de
  30 s, enregistré même en mode sur mouvement) et « Prendre une capture »
  (image tout de suite, entité « Capture »), interrupteur « Détection (toutes
  les caméras) » pour un mode présent / absent. La pause est affichée dans
  l'interface, avec un bouton pour reprendre.
- **Son dans le direct WebRTC** (« Faible latence ») pour les caméras G.711 ;
  bouton son, coupé au départ.
- **Démo en ligne** sur GitHub Pages (https://guiyomee.github.io/RustNVR/) ;
  l'interface servie par RustNVR n'a plus de mode démo (`?demo=1`).
- **Mises à jour** : le dépôt officiel est vérifié sans rien saisir ; un
  interrupteur dans À propos coupe la vérification. Un autre dépôt se règle
  dans `config.toml`.
- **README** en anglais et en français ; liste détaillée des fonctions dans
  `docs/FONCTIONNALITES.md`, compilation et tests dans `docs/DEVELOPPEMENT.md`.

### Corrections
- **Son G.711** : le direct et la relecture avec le son ne marchaient pas dans
  Chromium (Edge, Chrome).
- **Derrière Nginx Proxy Manager** : le direct est expliqué quand les WebSockets
  ne passent pas (case « Websockets Support ») ; documenté.
- **Port sous 1024** (`--port 80`) : le service peut écouter dessus, et
  `install.sh` vérifie que l'interface répond.
- **Mémoire de l'IA intégrée** : les analyses passent une à une ; YOLOX-l
  617 → 474 Mio.
- **`rustnvr` introuvable** dans `pct enter` / `pct exec` (Proxmox), dont le
  PATH n'a pas `/usr/local/bin` : `install.sh` ajoute des liens dans
  `/usr/bin` (installation neuve ; pour une mise à jour depuis la 2026.11.04,
  à la mise à jour suivante, car c'est le script déjà installé qui la fait).
- **Journal** : une erreur de connexion à une caméra tient sur une ligne (le
  client RTSP ajoutait deux lignes « conn: » et « msg: » sans date ni caméra).
- **PTZ** : une caméra motorisée qui ne gère pas les positions enregistrées
  (Imou Cruiser SE+) n'affichait pas ses flèches ; elle se pilote maintenant,
  sans positions (merci edouardkleinhans, #4).

### Sécurité (audit du 2026-11-08)
- Authentification Basic soumise au même blocage que la page de connexion ;
  calculs de mot de passe limités à quatre à la fois.
- WebSocket refusé depuis une page d'une autre origine.
- Blocage : 5 essais faux par adresse, 20 par compte (un inconnu ne peut plus
  bloquer l'administrateur avec 5 essais).
- En-tête Content-Security-Policy ; un compte lecteur ne voit plus la liste des
  comptes ni les identifiants MQTT, S3, SFTP.
- Certificat auto-signé valable 10 ans ; mots de passe du script LXC par
  variables d'environnement.

### Autres
- Script de release `scripts/release.sh` (export LF, compilation Debian 12,
  tests d'installation et de mise à jour) ; nettoyage du code (dépendances et
  fonctions inutilisées).

## 2026.11.04 (2026-11-08)

- **Son G.711** (PCMU, PCMA) des caméras converti en Opus et enregistré avec la
  vidéo.
- **Script LXC Proxmox** : menu complet (tous les réglages des Proxmox VE
  Helper-Scripts, retour arrière, résumé), réglages enregistrés, partage SMB
  monté sur l'hôte pour les enregistrements (#1).
- **IA plus exigeante la nuit** (seuil et plage horaire).
- **Aperçu d'une webcam USB** dans le formulaire ; commande
  `rustnvr reset-password`.
- Corrections : paquets vidéo mal formés ignorés (Reolink E1), formulaire de
  caméra qui se fermait par erreur, service systemd qui ne pouvait pas écrire
  sur un partage monté, version affichée `2026.11.04`, causes réelles des
  échecs de conversion FFmpeg.

## 2026.11.03 (2026-11-07)

- **Installation sans Docker** : archive précompilée à chaque release,
  `install.sh` et `rustnvr-update` (Debian, Ubuntu).
- **Conteneur LXC Proxmox VE** en une commande (#1).
- **Caméras motorisées** : pilotage PTZ en ONVIF, positions enregistrées (#3).
- **Webcams USB** sans go2rtc ni MediaMTX.
- **Aperçu en direct** au test de connexion d'une caméra.
- Docker : `RUSTNVR_WEB_LISTEN` ; sauvegardes restaurables d'une installation à
  une autre.
- Correction : conversions sur la carte graphique avec les pilotes VAAPI qui
  n'acceptent que la qualité constante (#2).

## 2026.11.02 (2026-11-06)

- **Image Docker publiée** (`ghcr.io/guiyomee/rustnvr`).
- **Passages de ligne** vérifiés par l'IA et comptage des présences.
- **Nouvelles alertes** : passage de ligne, caméra hors ligne / rétablie,
  stockage dégradé (cloche, webhooks, Discord, MQTT).
- **Page Images** et rejeu de l'IA ; les sept modèles YOLOX téléchargeables.
- **Sauvegarde et restauration** de la configuration, chiffrée.
- Changement : avec des zones, un objet reconnu hors de toutes les zones ne
  compte plus.

## 2026.11.01 (2026-11-04)

Première version publiée : enregistrement continu ou sur mouvement des flux
H.264 / H.265 sans réencodage, interface web (direct, relecture, événements,
stockage), détection de mouvement avec zones, reconnaissance d'objets
optionnelle, MQTT et Home Assistant, stockage local, NAS, S3 et SFTP, comptes
administrateur / lecteur, HTTPS intégré.
