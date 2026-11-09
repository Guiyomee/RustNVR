# Interface Web de RustNVR

Application React + TypeScript compilée en fichiers statiques (`web/dist`),
servis par le serveur Axum de RustNVR. Il n'y a pas de serveur Node en
production.

## Démarrer

Prérequis vérifiés : Node.js 24 et npm 11 (sur Windows, Node.js 24.19).

```sh
cd web
npm ci            # ou npm install
npm run build     # vérifie les types (tsc) puis produit web/dist
```

Puis lancer RustNVR depuis la racine du dépôt (`web.static_dir = "web/dist"`
par défaut) et ouvrir `http://127.0.0.1:8080`.

Développement avec rechargement à chaud (l'API est relayée vers un `rustnvr`
en cours d'exécution, par défaut `http://127.0.0.1:8080`) :

```sh
cd web
RUSTNVR_URL=http://127.0.0.1:8080 npm run dev
```

`npm run typecheck` vérifie les types sans construire.

## Technologies

| Outil | Version | Rôle |
|---|---|---|
| React | 19.3.1 | rendu |
| TypeScript | 7.0.2 | typage strict (`noUncheckedIndexedAccess`) |
| Vite | 8.3.5 | build et serveur de développement |
| Tailwind CSS | 4.3.3 | styles, thème dans `src/index.css` |
| lucide-react | 1.66.0 | icônes |

Pas de bibliothèque de graphiques, de routage ni de composants : un routeur
History de 70 lignes (`src/lib/router.tsx`), des graphiques SVG/CSS et des
composants maison suffisent. 42 paquets npm au total.

Taille du build (mesurée) : bundle principal 265 Ko (83 Ko gzip), CSS
32 Ko (7 Ko gzip) ; chaque page autre que la vue d'ensemble est chargée à la
demande (2 à 6 Ko gzip).

## Architecture

```
src/
  types/api.ts          miroir des DTO Rust (crates/rustnvr-api/src/dto.rs)
  api/client.ts         unique point d'appel HTTP : délais, erreurs typées (ApiError, UnavailableError)
  api/endpoints.ts      routes existantes uniquement
  api/ws.ts             WebSocket avec reconnexion (backoff exponentiel + gigue)
  state/liveStore.ts    état temps réel immuable + useSyncExternalStore, notifications
  data/source.ts        interface DataSource
  data/liveSource.ts    source réelle : API + WebSocket, repli en polling 10 s
  data/demo/            source DÉMO isolée (données fictives déterministes)
  data/DataContext.tsx  choix de la source, capacités du serveur, hooks useLive/useCapability
  hooks/                useAsync (annulation), useInView (chargement paresseux), useStorage…
  components/
    layout/             AppLayout, Sidebar, Topbar, Logo
    ui/                 Button, Card, StatCard, Sparkline, Meter, StatusIndicator,
                        Modal/ConfirmDialog, Tabs/Segmented, Popover, Loading/Empty/Error/Unavailable
    cameras/            CameraCard/CameraGrid, CameraPlayer, CameraForm, ZoneEditor, MotionPanel
    events/             EventCard, EventRow
    timeline/           Timeline (réutilisable), TimelineCard, useTimelineData, periods
    playback/           RecordingPlayer (HLS : hls.js chargé à la demande, HLS natif, ou fichier par fichier)
    storage/            StorageCard
    settings/           SettingsSection, StorageSettings (points de stockage, rétention), MqttSettings
  pages/                Overview, Cameras, CameraDetail, Events, Images, Playback, Storage, Settings, About
```

Règles :

- **Aucun appel `fetch` hors de `api/`.** Les pages passent par la
  `DataSource` du contexte.
- **Aucune route inventée.** Une fonction absente du serveur est signalée par
  `UnavailableError` (avec la route prévue, voir API_CONTRACT.md) et affichée
  par le composant `Unavailable`. Les capacités viennent de
  `GET /api/v1/capabilities`.
- **Chaque bouton agit ou explique pourquoi il est désactivé** (`disabledReason`,
  visible au survol et lu par les lecteurs d'écran).
- **Les données temps réel** (système, caméras) arrivent par WebSocket dans
  `LiveStore` ; les composants ne s'abonnent qu'à la tranche qu'ils lisent.

## Démo

La démo est une **construction à part** de l'interface, publiée sur GitHub
Pages (https://guiyomee.github.io/RustNVR/) : `npm run build:demo` écrit
`dist-demo/` (adresses sous `/RustNVR/`, `404.html` pour les liens directs),
`npm run dev:demo` la lance en développement. L'interface servie par RustNVR
n'a **pas** de mode démo : `DEMO_SITE` (`src/lib/build.ts`) vaut `false`, le
code et les photos de la démo n'y sont pas inclus (`?demo=1` ne fait rien).

Garanties :

- les données fictives sont dans `src/data/demo/` et ne sont jamais lues par
  la source réelle ; la démo n'appelle jamais l'API (ni l'écran de connexion) ;
- bandeau orange permanent avec un lien « Installer RustNVR », badge « Mode
  démo » dans la barre latérale,
  « (démo) » dans les noms et les états, badge DÉMO sur chaque image ;
- les images de démonstration sont des photos fixes libres de droits (CC0,
  Wikimedia Commons, `public/demo/`, crédits dans `public/demo/CREDITS.md`)
  portant la mention « Image fixe · démo », jamais une fausse vidéo (dessin SVG
  si une photo manque) ;
- les enregistrements fictifs n'ont pas de fichier : la lecture est
  désactivée avec une explication.

## Direct vidéo

`CameraPlayer` est le seul endroit où le direct est rendu ; il monte
`LivePlayer` (fMP4 par WebSocket + Media Source Extensions,
`/api/v1/cameras/{id}/live`) uniquement si la caméra est en ligne, **visible à
l'écran** (`useInView`) et l'onglet affiché (`usePageVisible`) : une caméra
cachée ne coûte ni réseau, ni décodage, ni travail serveur.

`LivePlayer` vérifie le codec avec `MediaSource.isTypeSupported` avant de
créer le tampon (message explicite sinon, typiquement H.265), suit le bord du
direct (saut si plus de 1,5 s de retard), purge le tampon au-delà de 10 s
derrière la lecture, se reconnecte avec backoff (1 s → 15 s) et relance la
connexion si rien n'arrive pendant 10 s. Le badge « LIVE » n'apparaît que
lorsque l'image défile réellement.

**Conversion H.264.** Si le navigateur ne décode pas le codec reçu (H.265 dans
Firefox, ou Edge sans l'extension HEVC) et que le serveur annonce
`capabilities.live_transcode`, `LivePlayer` se reconnecte avec
`?transcode=h264`, retient ce choix pour la session (pas de nouvel essai
inutile) et affiche le badge « converti H.264 ». Les vignettes utilisent le
sous-flux ; la page d'une caméra propose « Sous-flux / Principal (HD) ».

**Détection en pause.** Si une caméra a `motion.paused`, un bandeau en haut
de chaque page le dit, avec « Reprendre » (`PUT /api/v1/detection/pause`) pour
un administrateur ; l'état du NVR passe à « Détection en pause » et l'onglet
Détection d'une caméra le rappelle. Les événements `manual` sont libellés
« Manuel » (cartes, liste, marqueurs de la timeline).

**WebRTC (faible latence, expérimental).** Si `capabilities.live_webrtc`, la
page d'une caméra propose « Standard / Faible latence ». `WebrtcPlayer` est un
client WHEP minimal : transceiver vidéo en réception seule (et audio, en grand,
quand la caméra a un son Opus enregistré : bouton son, coupé au départ), attente de la fin
du rassemblement des candidats ICE (2 s au plus), `POST
/api/v1/cameras/{id}/live/webrtc` avec l'offre SDP, puis
`setRemoteDescription`. Le badge « WEBRTC » distingue ce mode. Si ICE échoue
ou ne se connecte pas en 15 s (pas de chemin UDP direct vers le serveur), un
message invite à revenir au direct standard ; pas de bascule automatique. Le
choix n'est pas mémorisé : chaque ouverture de la page repart en standard.
Les vignettes de la vue d'ensemble restent en MSE.

Mesuré dans Edge (Chromium) contre une source H.264 720p 25 i/s : lecture
continue, environ 0,5 s de vidéo en tampon devant la position de lecture. Non
disponible sur iPhone (pas de Media Source Extensions classique).

## Lecture des enregistrements

`RecordingPlayer` lit les segments MP4 fragmentés servis par
`/api/v1/recordings/…` (requêtes Range). Vérifié dans Edge (Chromium) contre
un serveur réel :

- positionnement à un instant donné, y compris au milieu d'un segment ;
- passage automatique au segment suivant, saut des trous ;
- vitesses ×1, ×2, ×4, ×8 (`playbackRate`) ;
- jusqu'à 4 caméras sur une horloge commune (la première pilote, les autres
  sont recalées au-delà de 1,5 s d'écart).

Les segments qui se suivent sont lus en HLS (hls.js, ou HLS natif sur Safari)
sans pause entre fichiers. **Le H.265 dépend du navigateur et du matériel** :
quand le navigateur ne le décode pas (`decodesHevc()`), le lecteur passe aux
copies H.264 faites par le serveur (`?transcode=h264`, fichier par fichier).

## Mur d'écrans

`WallPage` (`/mur`, hors de la mise en page : ni menu ni bandeau) : les caméras
choisies en direct sur tout l'écran. Réglages dans l'adresse
(`?cameras=a,b&cols=3&focus=0`), pour un favori de tablette ; par défaut toutes
les caméras actives, dans l'ordre de la vue d'ensemble. La caméra en
mouvement occupe 2 × 2 cases pendant le mouvement et 10 s après ; elle est
placée en tête par la propriété CSS `order`, sans déplacer les éléments
`<video>` (un `<video>` déplacé dans la page perd son flux). Verrou d'écran
(Wake Lock) quand le navigateur l'accepte ; rechargement de la page après
2 min sans connexion au serveur ; commandes affichées au mouvement de la
souris ou au toucher.

`ExportDialog` (bouton « Exporter ») : caméra, heures de début et de fin
(raccourcis 30 s à 15 min, « position actuelle »), case « Convertir en H.264 »
cochée d'office pour une caméra H.265 si le navigateur ne la décode pas.
`POST /cameras/{id}/export` prépare le fichier ; le téléchargement démarre
ensuite par un lien `download` (le lien reste affiché 15 minutes).

## Événements et IA optionnelle

`EventCard` / `EventRow` affichent les objets reconnus (`event.objects`,
fournisseur d'IA optionnel) : étiquettes traduites (« Personne 87 % ») et
cadres dessinés sur la vignette à partir de `bbox` (fractions de l'image).
Les cases « Personne », « Véhicule », « Animal » de la page Événements sont
actives quand `capabilities.ai` est vrai ; elles filtrent côté navigateur les
événements chargés selon les étiquettes COCO (`liveSource.ts`,
`OBJECT_KINDS`). « Mouvement » coché affiche tous les événements.

## Palette et visualisations

Thème clair (Paramètres → Général → Thème : Sombre, Clair ou Système, retenu
par le navigateur, appliqué avant le premier affichage par `lib/theme.ts`) :
mêmes variables redéfinies sous `:root[data-theme="light"]`, textes à 4,5:1 au
moins sur les cartes.

Palette imposée (`src/index.css`) : fonds `#111827` / `#0B1220`, cartes
`#1F2937`, bordures `#293548`, bleu `#3B82F6`, vert `#22C55E`, orange
`#F59E0B`, rouge `#EF4444`, textes `#F9FAFB` / `#9CA3AF`. Les couleurs
d'état sont toujours accompagnées d'une icône ou d'un libellé.

Logo (`components/layout/Logo.tsx`, même dessin que `public/favicon.svg`) :
un objectif à anneau cuivre `#C8642B` sur une tuile ardoise `#1C2128`, reflet
ivoire `#F4F1EC` ; couleurs fixes, identiques sur les deux thèmes. Dans le nom,
« NVR » est en `brand` : `#E08A4E` en thème sombre (6,6:1 sur `#111827`),
`#A9521F` en thème clair (5,4:1 sur blanc), le cuivre du logo étant trop peu
contrasté pour du texte de cette taille. Le bleu reste la couleur des actions.

Timeline : la série « mouvement » utilise `#D97706` (pas plus sombre de
l'orange) car `#F59E0B` sort de la plage de luminosité exigée pour une série
et se confond avec le rouge pour les daltoniens ; les événements sont des
losanges couleur texte. Couple bleu/`#D97706` validé sur les deux fonds
(séparation CVD ΔE 30, vision normale ΔE 34). Le curseur de lecture est blanc
plein, « maintenant » en tirets gris.

## Langues

Français (par défaut) ou anglais, choisi dans Paramètres → Général → Langue
et retenu par le navigateur (`localStorage` `rustnvr.lang`) ; changer de
langue recharge la page. Chaque texte porte ses deux versions au même endroit :
`t('Texte français', 'English text')` (`lib/i18n.ts`), sans fichier de
traduction séparé ni bibliothèque. Dates et nombres suivent `locale()`
(`fr-FR` ou `en-GB`) ; les noms d'objets COCO sont traduits par
`lib/objects.ts`. Les messages qui viennent du serveur (erreurs, journal)
restent en français. Pour un nouveau texte : toujours écrire les deux versions.

## Accessibilité et performance

- navigation clavier (focus visible), `aria-current`, rôles `tablist`,
  `radiogroup`, `meter`, boîtes de dialogue natives `<dialog>` ;
- `prefers-reduced-motion` respecté ;
- vignettes et lecteurs chargés seulement à l'écran (IntersectionObserver) ;
- graphiques mis à jour au rythme du serveur (2 à 5 s), pas plus ;
- pas de polling quand le WebSocket est ouvert.

## Responsive

Vérifié par captures (Edge headless) à 1600 px et 492 px (largeur minimale
de la fenêtre headless) : barre latérale repliable en bureau, tiroir sur
mobile, cartes empilées, timeline défilable horizontalement. Non vérifié sur
un vrai smartphone ni une tablette.
