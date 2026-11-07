# MQTT et Home Assistant

RustNVR publie l'état des caméras, les mouvements détectés et leurs images sur
un broker MQTT, et annonce ses entités à Home Assistant (découverte MQTT).
Vérifié avec Mosquitto 2 et avec une installation
Home Assistant réelle : toutes les entités ci-dessous sont découvertes.

## Mise en route avec Home Assistant

1. Dans Home Assistant : installez le module complémentaire « Mosquitto
   broker » et l'intégration « MQTT », puis créez un utilisateur Home
   Assistant dédié (par exemple `rustnvr`) ; le broker accepte les comptes
   Home Assistant.
2. Dans RustNVR : **Paramètres → MQTT / Home Assistant**, cochez « Se
   connecter », saisissez l'adresse de Home Assistant, le port `1883`,
   l'utilisateur et le mot de passe, puis « Enregistrer et se connecter ».
   L'état passe à « Connecté ».
3. Dans Home Assistant : chaque caméra apparaît comme un appareil
   (Paramètres → Appareils → MQTT), relié à l'appareil « RustNVR ».

Les réglages saisis dans l'interface sont écrits dans `mqtt.ui.toml`, à côté de
`config.toml` (lisible par son seul propriétaire : il contient le mot de
passe). On peut aussi utiliser la section `[mqtt]` de `config.toml` (voir
`config.example.toml`), avec `password_env` pour garder le mot de passe hors
du fichier.

## Entités Home Assistant

Par caméra (appareil nommé comme la caméra) :

| Entité | Type | Rôle |
|---|---|---|
| Mouvement | `binary_sensor` (`motion`) | événement de mouvement en cours |
| Connexion | `binary_sensor` (`connectivity`, diagnostic) | flux principal reçu |
| Enregistrement continu | `binary_sensor` | enregistrement activé |
| Détection de mouvement | `switch` (configuration) | active ou coupe la détection de la caméra |
| Enregistrement du son | `switch` (configuration) | enregistre ou non le son de la caméra (caméras de l'interface) |
| Personne | `binary_sensor` (`occupancy`) | personne reconnue pendant le mouvement (seulement avec l'IA optionnelle, [AI.md](AI.md)) |
| Dernier mouvement | `camera` | image prise au début du dernier événement |
| Dernière personne | `camera` | image où une personne a été reconnue, cadres bleus dessinés (seulement avec l'IA optionnelle) |
| Position PTZ | `select` | caméras motorisées (port ONVIF réglé) : choisir une position enregistrée y envoie la caméra ; les options sont les noms des positions, lus au démarrage et après chaque ajout ou suppression depuis l'interface (une position créée dans l'appli du fabricant apparaît au prochain démarrage) |

Pour RustNVR (appareil « RustNVR ») : « Caméras en ligne », « Espace disque
faible » (problème) et « Disque des
enregistrements » (% utilisé du disque local), mis à jour toutes les minutes.

Toutes les entités deviennent « indisponibles » quand RustNVR s'arrête ou perd
la connexion (message `offline` de dernière volonté). Les identifiants uniques
sont `<client_id>_<caméra>_<entité>` : ils restent stables d'un redémarrage à
l'autre, et deux RustNVR peuvent partager un broker avec des `client_id`
différents. Une caméra supprimée est retirée de Home Assistant (messages
retenus vides).

## Topics

`<p>` est le préfixe des topics (`rustnvr` par défaut).

| Topic | Contenu | Retenu |
|---|---|---|
| `<p>/availability` | `online` / `offline` | oui |
| `<p>/<caméra>/status` | `online` / `offline` | oui |
| `<p>/<caméra>/motion` | `ON` / `OFF` | oui |
| `<p>/<caméra>/motion_detection` | `ON` / `OFF` (détection activée) | oui |
| `<p>/<caméra>/motion_detection/set` | commande `ON` / `OFF` | — |
| `<p>/<caméra>/audio` | `ON` / `OFF` (son enregistré) | oui |
| `<p>/<caméra>/audio/set` | commande `ON` / `OFF` | — |
| `<p>/<caméra>/person` | `ON` / `OFF` (personne reconnue, IA optionnelle) | oui |
| `<p>/<caméra>/recording` | `ON` / `OFF` | oui |
| `<p>/<caméra>/snapshot` | JPEG du dernier événement | oui |
| `<p>/<caméra>/<objet>/snapshot` | JPEG de la dernière image où un objet de ce type (`person`, `car`, `dog`…) a été reconnu, cadres bleus de ces objets dessinés ; publié avant le passage de `person` à `ON` | oui |
| `<p>/<caméra>/crossing` | JSON de chaque passage d'une ligne : `line`, `direction` (`forward` / `backward`), `at_ms`, `today`, `object` (objet vu sur la ligne, lignes limitées à des objets ; sinon `null`) | non |
| `<p>/<caméra>/line/<ligne>/today` | passages de la ligne depuis minuit | oui |
| `<p>/<caméra>/line/<ligne>/present` | qui est à l'intérieur (lignes qui comptent les présences : entrées − sorties depuis minuit ou la remise à zéro) ; entité « Présents · ligne » | oui |
| `<p>/<caméra>/line/<ligne>/presence/reset` | n'importe quel message (commande) : remet le compte à zéro ; bouton « Remettre à zéro · ligne » | — |
| `<p>/<caméra>/ptz/preset/set` | nom d'une position enregistrée sur la caméra (commande) : la caméra motorisée y va ; sélecteur « Position PTZ » | — |
| `<p>/<caméra>/event` | JSON à chaque début, mise à jour et fin d'événement | non |
| `<p>/cameras_online` | nombre | oui |
| `<p>/disk_used_percent` | nombre | oui |
| `<p>/disk_low` | `ON` / `OFF` (un point de stockage à court d'espace) | oui |
| `<p>/storage_problem` | `ON` / `OFF` (un point de stockage inutilisable : partage non monté, en lecture seule, qui ne répond pas…) ; entité « Stockage dégradé » | oui |

Exemple de message `event` :

```json
{"id":2,"camera":"jardin","kind":"motion","start_ms":1793607714212,"end_ms":null,
 "score":0.17,"zones":["Allée"],"objects":[],"ongoing":true}
```

`objects` : objets reconnus par l'IA optionnelle (voir [AI.md](AI.md)), par
exemple `[{"label":"person","score":0.87,"bbox":[0.25,0.25,0.5,0.75],"zone":"Allée"}]` (`zone` : zone de détection où se trouve l'objet, s'il y en a une) ; un
message `event` supplémentaire est publié quand l'analyse se termine.

Les messages de découverte sont publiés (retenus) sous
`homeassistant/<composant>/<client_id>/<caméra>_<entité>/config`.

## Comportement

- QoS 1 ; reconnexion automatique (1 s puis jusqu'à 30 s entre les essais) ;
  à chaque connexion, tout l'état est republié.
- Les messages ne bloquent jamais l'enregistrement : ils passent par une file
  bornée (1024) ; si le broker ne suit pas, des messages sont abandonnés et un
  avertissement est journalisé.
- Une commande `motion_detection/set` a le même effet que la case « Détecter
  les mouvements » de l'interface (réglage enregistré).

## Connexion chiffrée (TLS)

Cochez « Connexion chiffrée (TLS) » (ou `tls = true`) et utilisez le port TLS
du broker (8883 en général). Le certificat du broker est vérifié :

- avec les autorités de certification du système (certificat Let's Encrypt,
  etc.) ;
- ou avec le fichier PEM indiqué dans « Certificat d'autorité » (`ca_file`),
  pour un broker à certificat auto-signé.

Un certificat non reconnu est refusé (erreur « UnknownIssuer » dans l'état de
la connexion) : il n'existe pas d'option pour désactiver la vérification.
Testé avec Mosquitto 2 (listener TLS + mot de passe).

## Limites

- Pas de certificat client (authentification mutuelle TLS).
- Pas de commande pour l'enregistrement continu (lecture seule).
