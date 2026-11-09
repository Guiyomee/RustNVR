# IA optionnelle : reconnaissance d'objets

RustNVR fonctionne entièrement sans IA : la détection de mouvement compare
des images, sans reconnaître ce qui bouge. En option, l'image de chaque
événement de mouvement peut être envoyée à un **fournisseur de
reconnaissance d'objets**, qui indique ce qu'elle contient (personne,
voiture, chien…).

**Désactivée par défaut. Aucun modèle n'est fourni ni téléchargé par
RustNVR.**

## Fournisseurs

| Fournisseur | État |
|---|---|
| API HTTP DeepStack / CodeProject.AI Server (`deepstack_http`) | implémenté ; API vérifiée contre CodeProject.AI Server 2.9.5 (module YOLOv5 6.2, Windows) et un serveur simulé |
| Intégré : modèle YOLOX (ONNX) exécuté par RustNVR (`onnx`) | implémenté ; vérifié sur 32 images réelles (voir ci-dessous) |

`deepstack_http` envoie l'image à `POST <url>/v1/vision/detection`
(formulaire multipart : champ `image`, `min_confidence`, et `api_key` si
configuré) et lit `predictions` (`label`, `confidence`, `x_min`…`y_max`).
C'est l'API de CodeProject.AI Server (port
32168 par défaut) et de DeepStack (projet qui n'est plus maintenu). Ces
serveurs s'installent séparément, sur la même machine ou une autre plus
puissante ; reportez-vous à leur documentation.

## IA intégrée (`onnx`) : sans autre programme

RustNVR peut exécuter lui-même un modèle de détection **YOLOX** au format
ONNX, avec la bibliothèque `tract` (pur Rust : ni Python, ni DLL, ni carte
graphique). Aucun modèle n'est fourni avec le programme. Le plus simple :
Paramètres → Intelligence artificielle → « Intégrée » → **Télécharger**
(YOLOX-nano, 3,7 Mo, licence Apache-2.0) : le serveur le récupère depuis la
publication officielle, vérifie son empreinte SHA-256
(`c789161e…a0b7d`) et l'enregistre dans `<data_dir>/models/` ; cliquez
ensuite sur « Enregistrer ». Ou à la main :

```sh
mkdir -p /var/lib/rustnvr/models
curl -L -o /var/lib/rustnvr/models/yolox_nano.onnx \
  https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.1rc0/yolox_nano.onnx
```

```toml
[ai]
provider = { type = "onnx", model = "/var/lib/rustnvr/models/yolox_nano.onnx" }
min_score = 0.5
labels = ["person"]
```

L'interface propose trois modèles de la même publication, au choix dans une
liste (téléchargés à la demande, empreinte SHA-256 vérifiée). Mesurés sur les
mêmes 32 images réelles (jour et nuit), un cœur de processeur de bureau,
personnes comptées à partir de 50 % :

| Modèle | Fichier | Temps par image | Mémoire (programme d'essai) | Ce qu'il a trouvé |
|---|---|---|---|---|
| YOLOX-nano | 3,7 Mo | 34–54 ms (médiane 38) | 34 Mo | personnes de jour ; rate la personne de nuit (0,42) et le chien |
| YOLOX-tiny | 20 Mo | 101–126 ms (médiane 105) | 70 Mo | plus de choses, mais trois fausses personnes (le chien, deux images de jardin vide) |
| YOLOX-s | 36 Mo | 392–460 ms (médiane 413) | 110 Mo | toutes les personnes (nuit comprise, 0,66), le chien reconnu, aucune fausse personne |

YOLOX-s est le meilleur compromis ; nano reste le choix des petites machines
(N100 : temps probablement doublés, non mesuré). L'analyse n'a lieu que sur
les images des mouvements détectés (5 au plus par événement) et des passages
de ligne vérifiés.

Les plus gros modèles sont aussi proposés. Mesurés sur 5 images (photos de
la démo), un i5-11600K ; un i5-6500 met environ 1,5 fois plus de temps (s y
mesure ≈ 0,5 s par image). mAP : précision moyenne publiée par YOLOX sur les
80 types d'objets (plus haut = plus juste) :

| Modèle | Fichier | mAP | Temps par image | Mémoire |
|---|---|---|---|---|
| YOLOX-s | 36 Mo | 40,5 | ≈ 355 ms | 120 Mo |
| YOLOX-m | 101 Mo | 47,2 | ≈ 830 ms | 240 Mo |
| YOLOX-l | 217 Mo | 50,1 | ≈ 1,6 s | 440 Mo |
| YOLOX-Darknet53 | 255 Mo | 48,0 | ≈ 2,0 s | 560 Mo |
| YOLOX-x | 396 Mo | 51,5 | ≈ 3,2 s | 800 Mo |

Sur les mêmes voitures, m est nettement plus sûr de lui que s (0,87–0,89
contre 0,74–0,78) ; l et x ajoutent peu pour beaucoup plus de calcul, et
Darknet53 est plus lent que l et moins juste. Sans carte graphique, m est le
plus gros raisonnable ; le mieux est de comparer sur ses propres images avec
le rejeu (ci-dessous). Les modèles YOLOv5/YOLOv8 d'Ultralytics ne
sont pas pris en charge (sorties différentes, et licence AGPL).

Mesuré sur 32 images réelles d'une caméra extérieure (640×360, jour et nuit),
processeur de bureau, un seul cœur :

| | CodeProject.AI 2.9.5 (YOLOv5) | Intégré (YOLOX-nano) |
|---|---|---|
| Temps par image | 240–320 ms | **30–33 ms** |
| Chargement | serveur à part | 0,2 s au démarrage |
| Personnes réelles de jour | 0,82–0,88 | 0,68–0,76 |
| Seconde personne, plus petite | trouvée | manquée |
| Fausses « personnes » de nuit | nombreuses (jusqu'à 0,74) | une à 0,42 |

Le modèle intégré est moins sensible aux petits sujets mais bien plus
fiable la nuit ; `min_score = 0.5` lui convient mieux que 0,6.

La fonction est incluse par défaut à la compilation (fonctionnalité Cargo
`onnx`, +22 Mo sur le binaire) ; `cargo build --no-default-features` la
retire.

## Activer

1. La détection de mouvement doit fonctionner (FFmpeg) et être activée sur
   les caméras concernées.
2. **Paramètres → Intelligence artificielle** : choisissez le fournisseur
   (« Intégrée » avec le chemin du modèle, ou « CodeProject.AI / DeepStack »
   avec l'adresse du serveur), la confiance minimale et les **objets à
   reconnaître**, puis « Enregistrer et appliquer ». C'est appliqué aussitôt,
   sans redémarrage ; un modèle introuvable est refusé avec la raison. Le
   bouton **Analyser** montre ce que l'IA voit sur une capture d'une caméra
   (cadres verts : gardés ; en pointillés : écartés par les réglages).
3. **Par caméra** (fiche de la caméra, onglet Détection) : cochez les objets
   qui comptent pour cette caméra (par exemple seulement « Personne » pour le
   jardin) ; rien de coché : tous ceux des réglages.
4. **Zones par objet** (même onglet, zone sélectionnée) : cochez les objets
   qui ne comptent **que dans cette zone**, par exemple « Personne » pour la
   zone Allée. Une personne n'est alors gardée que si le bas de son cadre (ses
   pieds) est dans une zone où « Personne » est cochée : quelqu'un qui passe
   sur le trottoir, même visible au-dessus de la haie, est ignoré. Les objets
   cochés dans aucune zone comptent dans toutes les zones. Avec des zones,
   rien ne compte en dehors (ni mouvement, ni objet reconnu) ; sans zone,
   toute l'image compte. Le nom de la zone est indiqué
   avec l'objet (« Personne 87 % (Allée) »), dans l'API et dans MQTT.

Les réglages saisis dans l'interface sont écrits dans `ai.ui.toml`, à côté de
`config.toml`, et remplacent sa section `[ai]`. On peut aussi tout régler
dans `config.toml` (redémarrage nécessaire) :

```toml
[ai]
provider = { type = "deepstack_http", url = "http://127.0.0.1:32168" }
min_score = 0.5          # confiance minimale (0..1)
# Plus exigeante la nuit (heure locale du serveur ; à cheval sur minuit si
# from est plus tard que to) : insectes, pluie et phares en infrarouge.
night = { min_score = 0.75, from = "20:00", to = "07:00" }
labels = ["person", "car", "dog", "cat"]   # vide : toutes les étiquettes
timeout_seconds = 10
```

Une clé d'API éventuelle se donne par variable d'environnement
(`api_key_env = "RUSTNVR_AI_KEY"`) ; une URL contenant un identifiant ou un
mot de passe est refusée.

## Fonctionnement

- La vignette de l'événement (prise 1 s après le début du mouvement, 640 px
  de large) est analysée, puis une nouvelle image toutes les
  `interval_seconds` (3 s) tant que le mouvement dure et que rien n'est
  reconnu, au plus `max_pictures` images (5) : une personne qui entre dans
  le champ après le début du mouvement est vue. L'image où l'objet est
  reconnu devient la vignette de l'événement. Pas d'analyse en continu du flux.
- Les objets retenus (confiance ≥ `min_score`, étiquette dans `labels`)
  sont enregistrés avec l'événement : API (`objects`), WebSocket, message
  MQTT `event`, capteur Home Assistant « Personne » (`binary_sensor`, allumé
  pendant le mouvement), et interface (étiquettes et cadres sur la vignette ; filtres
  « Personne », « Véhicule », « Animal » de la page Événements).
- Si le fournisseur ne répond pas, l'événement est enregistré normalement,
  sans objets, et un avertissement est journalisé (sans l'URL). La détection
  et l'enregistrement ne dépendent jamais du fournisseur.
- Les images d'événements quittent RustNVR vers l'adresse configurée :
  gardez le fournisseur sur votre réseau (ou en HTTPS).

Format de réponse vérifié contre CodeProject.AI Server 2.9.5 sur 32 images réelles
d'une caméra ; de nuit, des objets sont parfois
vus à tort avec une confiance de 0,4 à 0,5 : `min_score = 0.6` est un bon
point de départ.

Vérifié (serveur simulé, mire en mouvement) :
requête conforme, filtres par étiquette et confiance, cadres convertis en
fractions de l'image, objets dans l'API, MQTT et l'interface, comportement
quand le fournisseur est arrêté.

## Rejouer l'IA sur de vraies images

Paramètres → Intelligence artificielle → « Garder les images analysées » :
chaque image envoyée à l'IA (événements, passages de ligne vérifiés) est
gardée avec ce qu'elle y a trouvé, **même rien**, dans la destination par
défaut (dossier `.captures`, supprimé avec les vidéos au bout du même nombre
de jours). Quelques Mo par jour et par caméra.

Ensuite, « Rejouer l'IA » (même page) analyse de nouveau une journée avec
les réglages actuels et montre, image par image, ce qui change : « Avant :
rien · Maintenant : Personne 78 % ». Pour choisir un modèle (nano, tiny, s,
ou un fichier YOLOX plus gros), un score minimal ou des zones sur ses propres
images, sans attendre de nouveaux événements. 200 images au plus par rejeu.

Vérifié sur une instance de test : images gardées pendant un vrai événement
(mire GStreamer, deux images, rien trouvé), rejeu avec YOLOX-nano (photo d'un
parking : « rien » devient quatre voitures), noms de fichiers piégés refusés.

## Carte graphique (GPU)

La reconnaissance intégrée calcule **toujours sur le processeur**, un cœur
par image : tract, le moteur en Rust pur, n'utilise pas de carte graphique.
Les images sont analysées **une à la fois**, sur un seul fil : quand
plusieurs caméras bougent ensemble, leurs images attendent leur tour. C'est ce
qui garde la mémoire au niveau du modèle (YOLOX-l : 474 Mio contre 617 Mio
avec trois analyses en parallèle ; YOLOX-s : 130 Mio). Si les analyses
prennent trop de retard (plus de 16 images en attente, message « picture
skipped » dans le journal), prenez un modèle plus léger.
Pour l'IA sur GPU, RustNVR passe par **CodeProject.AI** (fournisseur
« CodeProject.AI / DeepStack ») installé avec accès à la carte, par exemple
son image `cuda` pour une carte NVIDIA. Le module de détection de
CodeProject.AI doit lui-même utiliser la carte : c'est un réglage de
CodeProject.AI.

Paramètres → Intelligence artificielle → **Cartes graphiques** liste les
cartes que voit RustNVR, avec leur état :

| État | Signification | Que faire |
|---|---|---|
| Utilisable par RustNVR | le périphérique (`/dev/dri/renderD…`, `/dev/nvidia…`) est là et RustNVR peut l'ouvrir | rien |
| Non transmise au conteneur | la carte existe mais son périphérique n'est pas dans le conteneur | `devices: [/dev/dri:/dev/dri]` (Intel, AMD) ou `gpus: all` (NVIDIA, avec nvidia-container-toolkit) dans `compose.yaml` |
| Accès refusé | le périphérique est là mais le compte de RustNVR ne peut pas l'ouvrir | `group_add` avec le groupe propriétaire (`ls -n /dev/dri` sur l'hôte, souvent `render`) |
| Affichage seulement | carte virtuelle d'une VM, puce d'affichage d'un serveur : aucun calcul | transmettre la vraie carte à la VM (passthrough) |

Dans une machine virtuelle, RustNVR ne voit la carte de l'hôte que si elle a
été transmise à la VM ; sinon la liste ne montre que la carte virtuelle.

Le bouton « Passer l'IA par le GPU (CodeProject.AI) » choisit ce fournisseur ;
après chaque analyse, RustNVR affiche **où elle a tourné** (« Calcul : GPU »),
d'après le champ `inferenceDevice` des réponses de CodeProject.AI.

La barre du haut affiche **GPU x %** quand une carte capable de calculer est
présente : l'occupation de **toute la carte**, tous programmes confondus
(CodeProject.AI compris), sur les 2 dernières secondes. Mesure : temps hors
veille (RC6) pour Intel, `gpu_busy_percent` pour AMD ; inconnue (« — ») pour
NVIDIA.

Vérifié : détection et mesures contre une arborescence `/sys` simulée (Intel
i915, AMD, NVIDIA, carte virtuelle QEMU), affichage dans Edge. ⚠ Pas encore
vérifié sur une vraie carte graphique.

## Limites

- Les filtres « Personne / Véhicule / Animal » s'appliquent dans le
  navigateur aux événements chargés (200 par défaut sur la période).
- Un objet immobile n'est jamais analysé : seule une détection de mouvement
  déclenche l'analyse (au plus `max_pictures` images par événement).
- Coût et précision non mesurés avec un vrai modèle.

## Ajouter un fournisseur

Un fournisseur implémente le trait `ObjectDetector` de la crate
`rustnvr-ai` : il reçoit une image JPEG et rend des `DetectedObject`
(étiquette, confiance, cadre en fractions de l'image). Les filtres, le délai
maximal et l'enregistrement sont communs (`Analyzer`). Un fournisseur dans le
processus (ONNX Runtime) devra rester optionnel à la compilation pour ne pas
alourdir le binaire par défaut.
