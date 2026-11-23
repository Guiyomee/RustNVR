# Security policy

RustNVR records cameras, stores passwords and controls who sees what: a
security problem matters. Thank you for reporting it privately first.

*Français : les signalements en français sont bienvenus, de la même façon.*

## Supported versions

Only the **latest release** gets fixes. Update first if you can: the problem
may already be solved (see [CHANGELOG.md](CHANGELOG.md)).

## How to report

**Please do not open a public issue, discussion or Discord message** for a
security problem.

Use GitHub's private reporting instead: the **Security** tab of this
repository → **Report a vulnerability**. Only the maintainer sees it.

Useful to include:

- the RustNVR version and how it is installed (Docker, Proxmox LXC,
  Debian / Ubuntu service);
- what an attacker can do, and from where (local network, Internet, with or
  without an account, with a viewer account…);
- the steps to reproduce it, or a proof of concept;
- never real passwords, tokens or recordings: replace them.

## What happens next

RustNVR is maintained by one person on their free time, so this is best
effort:

- an answer within a few days;
- a fix in a new release as soon as possible, announced in the release notes
  and the changelog once users can update;
- credit in the notes if you wish.

## Good to know

These are expected behaviours, not vulnerabilities:

- **Cameras** often speak RTSP and HTTP without encryption on the local
  network; RustNVR cannot change that. Keep cameras on your local network
  (ideally a separate VLAN without Internet access).
- **Exposing RustNVR to the Internet** without HTTPS and strong passwords
  is not supported. Prefer a VPN; otherwise HTTPS (built in, or a reverse
  proxy), two-step login, and no default password.
- Without any account, anyone who reaches the interface can use it. That
  is why RustNVR then only listens on `127.0.0.1`, unless
  `web.allow_unauthenticated_remote = true` says otherwise: that setting is
  meant for a trusted network only.

Problems in the security of RustNVR itself are welcome, for instance:
access without the right account or role, a viewer reaching a camera that
is not theirs, an API token doing more than allowed, reading or writing
files outside the expected folders, a password or a camera address leaking
in the interface, the logs, MQTT or an export, or the update service being
made to install something else.
