# Home Assistant

RustNVR s'intègre à Home Assistant par **MQTT avec découverte automatique** :
pas d'intégration personnalisée à installer. Vérifié avec une installation
Home Assistant réelle (caméra « Jardin ») : les entités apparaissent
d'elles-mêmes. Les exemples de tableau de bord et d'automatisations
ci-dessous utilisent ces entités ; ils n'ont pas été exécutés par nos soins :
vérifiez les identifiants dans **Paramètres → Appareils et services →
Entités**.

## 1. Connecter RustNVR

1. Dans Home Assistant : installez le module complémentaire **Mosquitto
   broker** et l'intégration **MQTT** ; créez un utilisateur Home Assistant
   dédié (par exemple `rustnvr`), que le broker accepte.
2. Dans RustNVR : **Paramètres → MQTT / Home Assistant**, cochez « Se
   connecter », saisissez l'adresse de Home Assistant, le port `1883` (ou
   `8883` avec TLS), l'utilisateur et le mot de passe, puis « Enregistrer et
   se connecter ». L'état passe à « Connecté ».
3. Dans Home Assistant : **Paramètres → Appareils et services → MQTT** :
   un appareil « RustNVR » et un appareil par caméra.

Détails (topics, TLS, comportement en cas de panne) : [MQTT.md](MQTT.md).

## 2. Entités

Home Assistant forme les identifiants à partir du nom de l'appareil et de
l'entité. Pour une caméra nommée « Jardin » :

| Entité | Identifiant observé | Usage |
|---|---|---|
| Mouvement | `binary_sensor.jardin_mouvement` | déclencheur d'automatisations |
| Connexion | `binary_sensor.jardin_connexion` | caméra hors ligne |
| Enregistrement continu | `binary_sensor.jardin_enregistrement_continu` | état |
| Détection de mouvement | `switch.jardin_detection_de_mouvement` | activer / couper la détection |
| Enregistrement du son | `switch.jardin_enregistrement_du_son` (attendu) | enregistrer ou non le son |
| Personne | `binary_sensor.jardin_personne` (attendu ; avec l'IA optionnelle) | personne reconnue pendant le mouvement |
| Dernier mouvement | `camera.jardin_dernier_mouvement` | image du début du dernier événement |
| Dernière personne | `camera.jardin_derniere_personne` (attendu ; avec l'IA optionnelle) | image de la personne reconnue, avec son cadre bleu |
| Mode d'enregistrement | `select.jardin_mode_d_enregistrement` (attendu) | « Continu », « Sur mouvement », « Arrêté » |
| Créer un événement | `button.jardin_creer_un_evenement` (attendu) | événement manuel de 30 s (sonnette, alarme…) |
| Prendre une capture | `button.jardin_prendre_une_capture` (attendu) | image tout de suite, sur l'entité « Capture » |
| Capture | `camera.jardin_capture` (attendu) | dernière image prise par le bouton |

Appareil « RustNVR » : `sensor.rustnvr_cameras_en_ligne`,
`sensor.rustnvr_disque_des_enregistrements` (% utilisé), « Espace disque
faible » (`binary_sensor`, classe « problème ») et
`switch.rustnvr_detection_toutes_les_cameras` (attendu) : détection en marche
ou en pause sur toutes les caméras.

Les identifiants marqués « attendu » suivent la règle de Home Assistant mais
n'ont pas encore été relevés sur une vraie installation.

Toutes ces entités passent à « indisponible » quand RustNVR s'arrête.

## 3. Tableau de bord

```yaml
type: vertical-stack
cards:
  - type: picture-entity
    entity: camera.jardin_dernier_mouvement
    name: Jardin — dernier mouvement
    camera_view: auto
  - type: entities
    entities:
      - binary_sensor.jardin_mouvement
      - binary_sensor.jardin_connexion
      - switch.jardin_detection_de_mouvement
      - sensor.rustnvr_cameras_en_ligne
      - sensor.rustnvr_disque_des_enregistrements
```

**Direct vidéo dans Home Assistant : non fourni.** L'entité `camera` est
l'image fixe du dernier mouvement ; RustNVR ne republie pas les flux en RTSP.
Pour un direct dans Home Assistant, ajoutez la caméra directement à Home
Assistant (intégration « Generic Camera » avec l'URL RTSP de la caméra, de
préférence son sous-flux) : c'est une connexion de plus à la caméra,
indépendante de RustNVR. Pour le direct, la relecture et les événements
complets, ouvrez l'interface RustNVR (par exemple avec une carte bouton et
une action `url`).

## 4. Automatisations

Notification avec l'image du mouvement, pour l'application mobile Home
Assistant (remplacez `mobile_app_mon_telephone` par votre appareil). Le
délai laisse à RustNVR le temps de publier l'image (prise au début de
l'événement, en général moins d'une seconde après) :

```yaml
alias: Mouvement au jardin
triggers:
  - trigger: state
    entity_id: binary_sensor.jardin_mouvement
    to: "on"
actions:
  - delay: "00:00:02"
  - action: notify.mobile_app_mon_telephone
    data:
      title: Mouvement — Jardin
      message: "{{ now().strftime('%H:%M:%S') }}"
      data:
        image: /api/camera_proxy/camera.jardin_dernier_mouvement
mode: single
```

Notification quand une **personne** est reconnue (IA optionnelle, voir
[AI.md](AI.md)) : l'image de la notification est celle où la personne a été
reconnue, avec son cadre bleu (publiée avant que le capteur passe à « on »).

```yaml
alias: Personne au jardin
triggers:
  - trigger: state
    entity_id: binary_sensor.jardin_personne
    to: "on"
actions:
  - action: notify.mobile_app_mon_telephone
    data:
      title: Personne détectée — Jardin
      message: "{{ now().strftime('%H:%M:%S') }}"
      data:
        image: /api/camera_proxy/camera.jardin_derniere_personne
mode: single
```

Couper la détection quand quelqu'un est à la maison, la rétablir au départ
(un groupe ou une `zone.home` selon votre installation) :

```yaml
alias: Détection jardin selon présence
triggers:
  - trigger: state
    entity_id: zone.home
actions:
  - action: >-
      {{ 'switch.turn_off' if states('zone.home') | int(0) > 0 else 'switch.turn_on' }}
    target:
      entity_id: switch.jardin_detection_de_mouvement
```

Mettre toute la détection en pause quand quelqu'un est à la maison (mode
« présent »), la reprendre au départ, sans toucher aux réglages des caméras :

```yaml
alias: RustNVR selon présence
triggers:
  - trigger: state
    entity_id: zone.home
actions:
  - action: >-
      {{ 'switch.turn_off' if states('zone.home') | int(0) > 0 else 'switch.turn_on' }}
    target:
      entity_id: switch.rustnvr_detection_toutes_les_cameras
```

Créer un événement et prendre une image quand on sonne (l'événement apparaît
dans RustNVR avec son extrait vidéo ; `binary_sensor.sonnette` est à
remplacer par votre sonnette) :

```yaml
alias: Sonnette
triggers:
  - trigger: state
    entity_id: binary_sensor.sonnette
    to: "on"
actions:
  - action: button.press
    target:
      entity_id:
        - button.entree_creer_un_evenement
        - button.entree_prendre_une_capture
  - delay: "00:00:03"
  - action: notify.mobile_app_mon_telephone
    data:
      title: On sonne
      message: "{{ now().strftime('%H:%M:%S') }}"
      data:
        image: /api/camera_proxy/camera.entree_capture
mode: single
```

Pour un événement d'une autre durée, publier le nombre de secondes :
`mqtt.publish` sur `rustnvr/entree/event/create` avec `payload: "120"`.

Tout enregistrer quand l'alarme est armée (caméra en « Sur mouvement » le
reste du temps) :

```yaml
alias: Enregistrement selon l'alarme
triggers:
  - trigger: state
    entity_id: alarm_control_panel.maison
actions:
  - action: select.select_option
    target:
      entity_id: select.jardin_mode_d_enregistrement
    data:
      option: >-
        {{ 'Continu' if is_state('alarm_control_panel.maison', 'armed_away') else 'Sur mouvement' }}
```

Alerte quand une caméra se déconnecte plus de 5 minutes, ou quand le disque
est presque plein :

```yaml
alias: Caméra hors ligne
triggers:
  - trigger: state
    entity_id: binary_sensor.jardin_connexion
    to: "off"
    for: "00:05:00"
actions:
  - action: notify.mobile_app_mon_telephone
    data:
      message: La caméra Jardin ne répond plus depuis 5 minutes.
```

```yaml
alias: Disque RustNVR presque plein
triggers:
  - trigger: numeric_state
    entity_id: sensor.rustnvr_disque_des_enregistrements
    above: 90
actions:
  - action: notify.mobile_app_mon_telephone
    data:
      message: "Disque des enregistrements à {{ states('sensor.rustnvr_disque_des_enregistrements') }} %."
```

Le switch de détection a le même effet que la case de l'interface RustNVR :
le réglage est enregistré et survit au redémarrage. Couper la détection
n'arrête pas l'enregistrement continu ; une caméra en mode « sur mouvement »
n'enregistre plus tant que la détection est coupée.

## 5. Limites

- Pas de direct vidéo ni de relecture dans Home Assistant (voir § 3).
- Le mode d'enregistrement ne se change que pour les caméras ajoutées depuis
  l'interface (celles de `config.toml` restent comme le fichier le dit).
- Seules les entités « Personne » et « Dernière personne » sont fournies : les
  autres objets reconnus (véhicule, animal…) figurent dans le message MQTT
  `event`, et leur image dans `rustnvr/<caméra>/<objet>/snapshot` (à déclarer
  soi-même comme caméra MQTT si besoin).
