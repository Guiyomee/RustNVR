# Développer RustNVR

Compiler, lancer depuis le dépôt, outils en ligne de commande et tests.
Installer RustNVR sur un serveur : voir le [wiki](https://github.com/Guiyomee/RustNVR/wiki) (Docker, Proxmox,
Debian / Ubuntu).

## Construire

Prérequis vérifiés : Linux x86_64 (testé sous Ubuntu 26.04 / WSL2), Rust
stable (testé avec 1.99.1), un compilateur C (`gcc`) et `cmake` : libopus
(conversion du son G.711) est compilée avec RustNVR et liée statiquement.
Aucune bibliothèque système n'est requise à l'exécution (pas d'OpenSSL ;
l'enregistrement n'utilise pas FFmpeg). **FFmpeg est optionnel** : détection
de mouvement, captures, vignettes d'événements, et conversion du direct en
H.264 pour les navigateurs qui ne décodent pas le H.265 (Firefox).
Pour l'interface : Node.js 24 et npm (testé avec 24.19 / 11.17).

```sh
cargo build --release              # binaire : target/release/rustnvr
cd web && npm ci && npm run build  # interface : web/dist
cd web && npm run build:demo       # démo en ligne (données fictives) : web/dist-demo
```

## Lancer depuis le dépôt

```sh
cp config.example.toml config.toml        # puis adapter les caméras
export RUSTNVR_ENTREE_PASSWORD='...'      # si password_env est utilisé
./target/release/rustnvr check-config -c config.toml
./target/release/rustnvr run -c config.toml
```

`run` s'arrête proprement sur `SIGINT`/`SIGTERM` : les segments ouverts sont
terminés et renommés avant la sortie.

### Interface Web

Lancez `rustnvr run` depuis la racine du dépôt (ou réglez `web.static_dir`)
puis ouvrez `http://127.0.0.1:8080`. Par défaut, l'interface n'écoute que sur
la machine locale. Pour y accéder depuis le réseau, activez l'authentification
(`web.username` + `web.password_env`) et changez `web.listen` (par exemple
`0.0.0.0:8080`) ; sans authentification, RustNVR refuse de démarrer sur une
adresse réseau. Activez aussi HTTPS (`web.tls_self_signed = true`, ou votre
certificat avec `web.tls_cert` / `web.tls_key`) : sinon les mots de passe et la
vidéo circulent en clair sur le réseau. Détails :
[FRONTEND.md](../FRONTEND.md).

## Outils en ligne de commande

Tester une caméra avant de l'ajouter (le mot de passe est lu dans une variable
d'environnement pour ne pas apparaître dans l'historique du shell) :

```sh
export CAM_PW='...'
./target/release/rustnvr probe --url rtsp://192.168.1.20/stream1 --username admin --password-env CAM_PW
# ou une caméra déjà configurée :
./target/release/rustnvr probe --camera entree -c config.toml
```

Exemple de sortie réelle (contre le serveur d'interopérabilité ci-dessous) :

```
probing rtsp://127.0.0.1:8554
* stream 0: video/h265 clock 90000 Hz 25 fps
  server tool: "GStreamer"
OK: h265 (hvc1.1.6.L93.90) 1280x720, first key frame after 42 ms
```

Réparer manuellement des segments interrompus (fait aussi au démarrage) :

```sh
./target/release/rustnvr recover --dir /var/lib/rustnvr/recordings
```

Journaux : niveau via `RUST_LOG` (par défaut `info`, ex. `RUST_LOG=debug`),
format JSON via `--log-format json`.

## Tests

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd web && npm run build   # inclut la vérification TypeScript
```

Les tests incluent un serveur RTSP factice (`crates/rustnvr-testkit`) servant
de vrais flux H.264/H.265 (fixtures de 80 Ko régénérables par
`scripts/gen-fixtures.sh`, nécessite ffmpeg avec libx264/libx265), avec une
piste son G.711 si besoin : reconnexion, flux bloqué, authentification,
paquets mal formés, et test de bout en bout jusqu'aux fichiers. Si `ffprobe`
est installé, les tests de bout en bout décodent chaque segment et exigent
zéro erreur ; sinon cette vérification est ignorée (un message l'indique).

### Interopérabilité avec un serveur RTSP indépendant

`scripts/interop-gst-rtsp-server.py` sert une mire H.264 et H.265 avec
GStreamer (Ubuntu : `apt install python3-gi gir1.2-gst-rtsp-server-1.0
gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly`) :

```sh
python3 scripts/interop-gst-rtsp-server.py --width 1920 --height 1080 --fps 25 --bitrate 4000
./target/release/rustnvr probe --url rtsp://127.0.0.1:8554/h264
```

## Préparer une version

`scripts/release.sh` (Linux, ou Git Bash avec Docker Desktop) compile et teste
une release sans rien publier : export du commit en fins de ligne LF,
interface (node:24), programme compilé sur Debian 12 (il tourne donc sur
Debian 12 et 13), archive et `.sha256`, installation neuve sur Debian 12 et 13,
mise à jour depuis la dernière release publiée.

```sh
scripts/release.sh --rehearse          # répétition avec la version actuelle
scripts/release.sh 2026.11.05 --notes notes.md
```

La version vient de `[workspace.package] version` (Cargo.toml, `2026.11.5`
pour `2026.11.05`) et de `web/package.json`. Le résumé de chaque version est
dans [CHANGELOG.md](../CHANGELOG.md).

## Publier une version sur son propre dépôt

1. Changez `version` dans `Cargo.toml` (`[workspace.package]`). Versions
   « calendrier » : année.mois.numéro de la publication dans le mois, sans zéro
   en tête pour Cargo (`2026.12.1`). Validez et envoyez (`git push`).
2. Dans Gitea : Publications (« Releases ») → Nouvelle publication →
   étiquette `v2026.12.01` sur `main` (le zéro en tête est permis ici), un titre et les nouveautés → Publier.

Les RustNVR qui surveillent le dépôt l'annoncent à la vérification suivante
(« Vérifier maintenant » pour ne pas attendre). L'étiquette doit
correspondre à la version de `Cargo.toml`, sinon la version installée se
croira toujours en retard.
