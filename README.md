# RustNVR

<img src="web/public/favicon.svg" width="96" height="96" alt="RustNVR logo: a camera lens with a copper ring">

**English** · [Français](README.fr.md)

Open-source network video recorder (NVR) written in Rust, built to use little
CPU and memory: H.264/H.265 streams from IP cameras are received over RTSP and
recorded **without decoding or re-encoding** into fragmented MP4 segments.

**[Online demo](https://guiyomee.github.io/RustNVR/)**: the interface with
fictional cameras and events, nothing to install.

<a href='https://ko-fi.com/Q4Z62AA835' target='_blank'><img height='36' style='border:0px;height:36px;' src='https://storage.ko-fi.com/cdn/kofi6.png?v=6' border='0' alt='Buy Me a Coffee at ko-fi.com' /></a>

![Overview: cameras, recent events, storage](docs/images/demo-overview.jpg)

| Events | A camera |
|---|---|
| ![Events with their thumbnails](docs/images/demo-events.jpg) | ![Camera page, Live tab](docs/images/demo-camera.jpg) |

## Why RustNVR

- **Light**: a single program; video is never decoded to be recorded. Next to
  Frigate on the same machine with the same cameras: 13 times less memory,
  2.7 times less CPU (see below).
- **Everything in the interface**: cameras (network discovery, connection test
  with a live preview), storage, detection, AI, MQTT, accounts, backup.
- **Your disks**: local disk, NAS (SMB, NFS), S3 or SFTP, chosen per camera,
  with retention per destination and a fallback to the local disk while the
  NAS is away.
- **Home Assistant** over MQTT, with no integration to install: motion,
  recognized objects, status, recording, last picture, PTZ position.
- **Wherever you like**: Docker, a Proxmox LXC container in one command, or a
  service on Debian / Ubuntu.

## Install

**Docker** (image `ghcr.io/guiyomee/rustnvr`, Linux x86-64):

```sh
mkdir rustnvr && cd rustnvr
curl -fsSLO https://raw.githubusercontent.com/Guiyomee/RustNVR/main/compose.yaml
echo 'RUSTNVR_WEB_PASSWORD=a-strong-password' > .env && chmod 600 .env
docker compose up -d
```

**Proxmox VE** (LXC container, from the host's shell; a menu offers every
setting):

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/Guiyomee/RustNVR/main/packaging/proxmox/rustnvr-lxc.sh)"
```

**Debian / Ubuntu** (systemd service):

```sh
sudo apt install curl ffmpeg
curl -fsSLO https://raw.githubusercontent.com/Guiyomee/RustNVR/main/packaging/install.sh
sudo bash install.sh
```

Then open `https://<server>:8080/`, account `admin`. Details (in French for
now): [Docker](docs/DOCKER.md) · [Install and Proxmox](docs/INSTALLATION.md).
The interface itself is in English and French.

## Features

- Continuous or motion-triggered recording (with the seconds before), H.264
  and H.265, AAC and G.711 sound; safe writes and automatic repair after a
  power cut.
- Live view in the browser (≈ 0.5 s behind), video wall, playback with a
  timeline and several cameras in sync, MP4 exports.
- Motion detection with zones, optional object recognition (built-in YOLOX,
  or DeepStack / CodeProject.AI), stricter at night if you wish.
- PTZ cameras (ONVIF), USB webcams, conversions on the graphics card (VAAPI,
  NVENC) for browsers without H.265.
- Administrator / viewer accounts, built-in HTTPS, webhooks, REST API, new
  version check.

Detailed list, with what is verified and what is not yet (in French):
[docs/FONCTIONNALITES.md](docs/FONCTIONNALITES.md).

## Next to Frigate

Same Debian virtual machine, same real cameras, 4 cameras online on each
side; recording, motion detection and object recognition enabled everywhere
(RustNVR: built-in YOLOX-s). Measured with `docker stats` (100 % = one core):

| | Memory | CPU |
|---|---|---|
| RustNVR | 162 MiB | 16 % |
| Frigate | 2.17 GiB | 45 % |

This is a snapshot: RustNVR's AI only works during motion, and a 24-hour
average is still to be done. Frigate also does more: object tracking frame by
frame, accelerators (Coral…), faces and license plates, stream re-streaming.
RustNVR aims at reliable, frugal recording.

## Known limits

- Sound: AAC and G.711 (converted to Opus); others (G.726, G.722…) are
  recorded without sound. Opus sound plays in current browsers (Safari 17 and
  later); not checked over HLS on iPhone. In the WebRTC live view, only the
  sound of G.711 cameras (as Opus), not AAC.
- B-frames / non-monotonic timestamps: the segment is cut (no reordering).
- `rtsps://` not supported (limit of the RTSP client).
- Malformed video packets (fragments without their start, as on a Reolink
  E1): skipped, the picture stays damaged until the next key frame. On a
  camera that also does not mark the end of its frames (RTP marker bit), the
  RTSP client fails on the next packet: the session is restarted.
- Retention per storage destination and per camera (age, size). The SQLite
  index of recordings is rebuilt from the files (at startup, then every
  10 min): a file copied by hand shows up with that delay.
- Motion detection: needs FFmpeg; an event lasts ≈ 1 s after an object
  leaves (the background adapts again); an object that stops moving is no
  longer detected after ≈ 1 s; shadows, foliage and rain can trigger it: tune
  the sensitivity and the zones. An event running during a hard stop is
  closed at its start time.
- Remote storage: NAS (written directly); S3 and SFTP by uploading finished
  segments (a minute behind with 60 s segments), without playback from the
  remote side nor retention there (use the bucket's lifecycle rules, or a
  clean-up on the SFTP server). The segment being written when the NAS fails
  is lost.
- WebRTC live view: prototype, no STUN/TURN: it needs a direct UDP path
  between the browser and the server (local network or VPN); an H.265 camera
  goes through the H.264 conversion (FFmpeg). Tested in Edge only.
- Interface: H.265 live and playback are converted to H.264 when the browser
  does not decode H.265 (FFmpeg; a few seconds of wait per file played,
  picture reduced to 1280 px wide); no live view on iPhone (HLS playback is
  planned there but untested); network (address, HTTPS) and the cameras of
  config.toml are only changed in `config.toml`. A gap between two segments
  (reconnection, motion recording) stays visible: the player jumps to the
  next block.

## Documentation

In French for now: **[Docker](docs/DOCKER.md)** ·
**[Install, Proxmox](docs/INSTALLATION.md)** ·
**[Home Assistant](docs/HOME_ASSISTANT.md)** · [MQTT](docs/MQTT.md) ·
[AI](docs/AI.md) · [Features](docs/FONCTIONNALITES.md) ·
[Development](docs/DEVELOPPEMENT.md) · [Changelog](CHANGELOG.md) · [Architecture](ARCHITECTURE.md) ·
[Interface](FRONTEND.md) · [API](API_CONTRACT.md) ·
[Technical choices](docs/adr/0001-choix-techniques-fondamentaux.md)

## Support

Feedback, a camera that does not work, an idea: open an
[issue](https://github.com/Guiyomee/RustNVR/issues), in English or French.
And if RustNVR is useful to you: [a coffee on Ko-fi](https://ko-fi.com/Q4Z62AA835).

## License

Your choice: [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).

The compiled program contains libopus (Xiph.Org and others, 3-clause BSD
license: [LICENSE-libopus](LICENSE-libopus)), shipped with the release
archives.

Unless stated otherwise, any contribution submitted for inclusion in RustNVR
is licensed under both, without any additional terms.
