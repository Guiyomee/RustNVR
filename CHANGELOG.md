# Journal des versions

Résumé de chaque version ; les notes complètes (captures, commandes,
précautions) sont sur la page de chaque
[release](https://github.com/Guiyomee/RustNVR/releases). Versions
« calendrier » : année.mois.numéro dans le mois.

## 2026.11.10 (2026-11-20)

### Licence
- **RustNVR passe sous la licence PolyForm Noncommercial 1.0.0** (fichier
  `LICENSE`) : toujours gratuit pour tout usage non commercial (maison,
  homelab, associations, enseignement), mais un usage commercial (vente,
  service payant, entreprise, produit) demande l'accord de l'auteur. Les
  versions jusqu'à la 2026.11.09 restent sous MIT ou Apache 2.0.

### Nouveautés
- **Double authentification** : en plus du mot de passe, un code à 6
  chiffres d'une application d'authentification (Google Authenticator,
  Aegis, 2FAS…), activé par chaque compte dans Paramètres → Utilisateurs,
  avec 10 codes de secours. Un administrateur peut la retirer d'un compte
  (téléphone perdu), et `reset-password` aussi.
- **Jetons d'API** : un script, une tablette ou Home Assistant se connecte
  avec un jeton (en lecture seule ou complet) au lieu du mot de passe ; la
  liste montre leur dernière utilisation, on les révoque d'un clic.
- **Mise à jour en un clic** (Debian, Proxmox) : À propos → Mises à jour →
  « Mettre à jour vers … ». Un service séparé, en root, télécharge la version,
  vérifie son empreinte, l'installe et redémarre RustNVR ; la page se
  recharge toute seule. Ce service est installé par une mise à jour à la
  main faite avec le `rustnvr-update` de cette version ou d'une plus
  récente : depuis la 2026.11.09, il en faut donc une seconde, qui
  réinstalle la version en place (la commande exacte est affichée dans À
  propos). Sous Docker, les commandes à lancer restent affichées.
- **Bandeau « mis à jour »** : après une mise à jour (bouton, à la main ou
  Docker), un bandeau vert « RustNVR a été mis à jour : … → … » s'affiche
  une fois, avec un lien vers les nouveautés ; la cloche le note aussi.
- **Time-lapse quotidien** : une case par caméra (onglet Détection) ; chaque
  nuit, une vidéo d'environ 48 s de la veille (une image par minute, masques
  appliqués, 720p), gardée 30 jours à côté des enregistrements. Dans
  l'onglet Enregistrements de la caméra : lecture, téléchargement,
  « Préparer » pour un autre jour, l'avancement dans la liste (« En
  attente », « En préparation… 42 % » avec une barre), un bouton pour
  l'arrêter et un autre pour supprimer une vidéo faite.
- **Trajets entre caméras** (Paramètres → Trajets) : on indique quelle
  caméra peut suivre quelle autre (Sonnette → Entrée → Salon, avec un délai
  maximal). Quand l'IA reconnaît la même sorte d'objet sur une caméra puis
  sur la suivante (même si elle reconnaît la personne plus tard sur la
  première), les événements forment un seul trajet : une seule carte
  dans Événements, relue caméras côte à côte dans Lecture. Il faut l'IA.
  Au choix pour chaque notification (« Regrouper les trajets ») : la
  première caméra prévient, les suivantes se taisent, et un seul message
  résume le trajet à la fin, avec sa vidéo (les caméras à la suite, bandeau
  « caméra · heure », avec le son des caméras qui en ont) ; il part dès qu'une des caméras du trajet n'a pas
  ses notifications coupées. MQTT : `<préfixe>/journey`. Un bouton « Exporter »
  fait un seul MP4 du trajet.
- **Photo pleine résolution de la caméra** (au choix, onglet Détection) :
  l'image des événements (vignette, IA, notifications, images gardées) est
  demandée à la caméra elle-même par ONVIF, en pleine définition (1280 px,
  1920 px ou taille d'origine, au choix ; masques appliqués), au lieu d'une
  image du sous-flux. Si la
  caméra ne la donne pas en 3 s, l'image habituelle est prise. Un bouton
  « Essayer » montre la photo, sa taille et son temps. Il faut le port
  ONVIF de la caméra.
- **L'IA continue de regarder pendant un mouvement long** : pluie, vent ou
  phares peuvent faire durer un mouvement de longues minutes ; elle ne
  s'arrête plus après les premières images, mais en analyse une toutes les
  30 s tant qu'il dure (réglable : 30 s, 1 min ou plus du tout, Paramètres
  → Intelligence artificielle). Quelqu'un qui passe pendant ce temps est
  reconnu : alerte « personne » et trajet.
- **Image figée ou masquée** : une caméra qui envoie encore son flux mais
  dont l'image ne change plus depuis 5 min (caméra bloquée), ou devenue
  d'un coup uniforme depuis 1 min (objectif couvert, caméra masquée,
  lumière éteinte), est signalée comme une caméra hors ligne (notification
  « Caméra », badge sur la fiche), puis « rétablie ». La nuit qui tombe
  n'est pas signalée. Vu par la détection de mouvement, pendant qu'elle
  tourne ; une case par caméra pour s'en passer (onglet Détection).
- **IA en panne** : un nouveau type de notification prévient quand la
  reconnaissance d'objets échoue 3 fois de suite (serveur d'IA arrêté,
  modèle illisible), puis quand elle remarche ; Paramètres → Intelligence
  artificielle affiche aussi les échecs. À cocher dans chaque webhook.
- **Sauvegarde automatique** (Paramètres → Sauvegarde) : la sauvegarde
  chiffrée de la configuration, faite toute seule chaque jour, semaine…
  dans le dossier `.rustnvr-backups` du stockage choisi (le NAS de
  préférence), en gardant les dernières ; un bouton « Sauvegarder
  maintenant ». La phrase secrète est enregistrée sur le serveur.
- La sauvegarde de configuration garde aussi les flèches des trajets et
  les réglages du mur, et les réglages de la sauvegarde automatique.
- La base de données est migrée au premier démarrage : une version plus
  ancienne ne pourra plus la rouvrir.

### Améliorations
- Sécurité : bibliothèque `url` mise à jour (2.5.9, RUSTSEC-2026-0592,
  dépassement de pile sur des adresses `blob:` imbriquées ; pas exploitable
  dans RustNVR, qui n'utilise pas la fonction concernée).
- **IA intégrée 2 à 4 fois plus rapide** : la reconnaissance d'objets tourne
  avec **ONNX Runtime** (bibliothèque officielle de Microsoft, livrée dans
  l'archive Debian / Proxmox) au lieu de tract, sur le même processeur et
  avec les mêmes résultats : YOLOX-m passe de 1,25 s à 0,31 s par image sur
  un i5-6500. Paramètres → IA indique le moteur (« CPU · ONNX Runtime »).
  Sans la bibliothèque (image Docker pour l'instant), rien ne change : tract
  reste utilisé. Elle prend les cœurs que RustNVR a le droit d'utiliser
  (limite d'un conteneur ou d'un LXC comprise : 0,8 s avec 2 cœurs, 0,3 s
  avec 4) ; `RUSTNVR_AI_THREADS` en fixe le nombre (par exemple pour laisser
  un cœur libre sur un Raspberry Pi).
- Le journal note chaque notification envoyée (webhook, type de message,
  titre, réponse du serveur), pas seulement les échecs : on peut vérifier
  après coup qu'une notification est bien partie. Jamais l'adresse du
  webhook.

## 2026.11.09 (2026-11-18)

### Nouveautés
- **Couper les notifications d'une caméra** sans toucher à son
  enregistrement : fiche de la caméra → Détection → « Envoyer les
  notifications de cette caméra », ou l'interrupteur « Notifications » de la
  caméra dans Home Assistant. Une automatisation peut ainsi couper le garage
  quand quelqu'un est à la maison et garder la sonnette. Les pannes sont
  toujours signalées.
- **Mur d'écrans** : toucher une caméra l'affiche seule, en grand et avec son
  son ; la toucher encore, Échap ou « retour » ramène au mur. Merci
  Warrior1300 pour l'idée !
- **Mur d'écrans, réglage général** : les caméras affichées, le nombre par
  ligne et l'agrandissement (bouton ⚙ du mur) sont maintenant gardés sur le
  serveur et valent pour tous les écrans, le panneau Home Assistant compris.
  Un administrateur les change ; une adresse avec ses propres réglages (favori
  d'une tablette) les garde pour cet écran. La base de données est migrée au
  premier démarrage : une version plus ancienne ne pourra plus la rouvrir.
- **Page d'accueil au choix** (Paramètres → Général) : un navigateur peut
  ouvrir RustNVR directement sur le mur d'écrans, les événements, les images
  ou la lecture, par exemple pour un écran réservé au mur. Le choix est
  gardé par ce navigateur ; le menu « Vue d'ensemble » y mène toujours.
- **App Home Assistant « RustNVR Proxy » 1.1.0** (publiée à part le
  2026-11-17, elle marche avec la 2026.11.08) : option `page` pour la
  page ouverte par le panneau, et panneau visible aussi par les comptes Home
  Assistant qui ne sont pas administrateurs. L'app ne l'ouvre que si RustNVR
  demande une connexion : chacun a les droits de son compte RustNVR (par
  exemple « lecteur »). Option `users` pour n'autoriser que certains comptes.
  **Si RustNVR n'a aucun compte**, le panneau reste fermé après la mise à
  jour de l'app, le temps d'en créer un.

### Corrections
- **Page Stockage** : la carte « Disque des enregistrements » montrait le
  disque du dossier local même quand les caméras enregistrent sur un NAS
  par défaut (par exemple 643 Go de vidéos sur un disque de 7,8 Go). Elle
  montre maintenant le point de stockage par défaut, comme la vue
  d'ensemble : son disque, ses vidéos et son autonomie.
- **TP-Link Tapo C200 (ancienne version)** : son flux H.264 contient des
  zéros que la norme interdit ; RustNVR coupait la session toutes les
  quelques secondes (« invalid sequence 00 00 00 21 »), d'où des segments de
  4 à 8 s et des trous. Ces zéros sont maintenant gardés tels quels, comme le
  fait FFmpeg (copie corrigée de Retina dans `crates/retina`, voir
  `PATCH.md`). Merci stefsec pour l'analyse !
- **Son à travers go2rtc** : go2rtc donne un nouvel identifiant de session à
  chaque piste ; RustNVR refusait la piste audio (« session id changed »). Il
  garde maintenant le premier, comme FFmpeg.

## 2026.11.08 (2026-11-13)

### Nouveautés
- **Interface dans Home Assistant** (ingress) : RustNVR s'affiche dans la
  barre latérale de Home Assistant grâce à la nouvelle app « RustNVR Proxy »
  (dossier `homeassistant/` ; ce dépôt s'ajoute tel quel au magasin d'apps).
  RustNVR lit le préfixe `X-Ingress-Path` et l'applique à ses fichiers, à
  son API et à ses liens, et accepte d'être affiché dans une page de Home
  Assistant (même origine seulement). Voir
  [le wiki](https://github.com/Guiyomee/RustNVR/wiki/FR-Home-Assistant).
  Merci cddu33 (#11) !
- **IA plus exigeante quand la caméra est en vision nocturne** (Paramètres →
  IA → « Plus exigeante la nuit ») : au lieu d'heures fixes, le seuil de nuit
  suit chaque caméra dès qu'elle passe en noir et blanc, y compris un jour
  sombre ou dans un garage.
- **Flèches PTZ dans Home Assistant** : boutons « PTZ gauche / droite / haut
  / bas / zoom avant / zoom arrière » pour chaque caméra motorisée, à mettre
  sur un tableau de bord ou dans une automatisation (« tourne vers le portail
  quand on sonne »), en plus du choix des positions enregistrées.
- **Ordre des caméras commun à tous les appareils** : l'ordre choisi en
  glissant les vignettes de la vue d'ensemble est gardé par RustNVR, et non
  plus par chaque navigateur ; ordinateur, téléphone et tous les comptes le
  retrouvent, sur toutes les pages. Seuls les administrateurs le changent ;
  l'ordre déjà choisi dans votre navigateur est repris. Le nombre de
  caméras par ligne reste propre à chaque appareil.
- **Vidéo dans les notifications** (Paramètres → Webhooks, Discord, ntfy et
  Telegram : « Envoyer aussi une courte vidéo ») : quelques secondes de
  l'événement (de 2 s avant à 15 s au plus), en second message environ une
  minute après, le temps que l'enregistrement soit fini ; légère (H.264,
  480 lignes) et masques de confidentialité compris.
- **Recherche dans les événements** : jusqu'à 30 jours d'un coup, par
  objet reconnu et par zone (choisissez une caméra pour filtrer ses zones),
  la recherche étant faite par RustNVR. Sur la timeline, les événements avec
  un objet reconnu sont marqués en vert et nommés (« Personne »), et leur
  vignette s'affiche au survol.
- **Programmation horaire** : la détection d'une caméra (onglet Détection →
  « Programmation ») et les notifications d'un webhook ne fonctionnent que
  les jours et aux heures choisis (par exemple la nuit, ou le week-end). En
  dehors, pas d'événement ni de notification, et une caméra « sur
  mouvement » n'enregistre pas, et la détection s'arrête (aucun calcul) ;
  l'enregistrement continu et les pannes ne changent pas.
- **Signets sur la timeline** (Lecture → « Signet ») : notez un moment
  (« livraison colis ») ; il apparaît sur la timeline et dans la liste
  « Signets du jour », d'où « Lire » y revient, sur tous les appareils.

### Corrections
- **Coupure du NAS** : le segment en cours quand le NAS tombe n'est plus
  perdu. Ses dernières secondes continuent sur le disque local, et ce qui
  était déjà sur le NAS est récupéré quelques minutes après son retour, sans
  attendre le prochain redémarrage de RustNVR.
- **Arrêt brutal** (coupure de courant, plantage) : un événement en cours se
  termine au dernier moment où RustNVR tournait (à 15 s près), et non plus à
  son début (durée nulle).
- La lecture d'une période (playlist HLS) désigne ses fichiers par des
  adresses relatives : elle fonctionne aussi derrière un préfixe.
- Derrière un proxy en HTTP (Home Assistant sur `http://…:8123` avec
  « RustNVR Proxy »), un RustNVR en HTTPS marquait son cookie de session
  `Secure` : le navigateur le refusait et l'interface restait sur la page de
  connexion. Le cookie suit maintenant le protocole du navigateur
  (`X-Forwarded-Proto`).
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
