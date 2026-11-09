#!/usr/bin/env bash
# Inside a Debian container (scripts/release.sh): installs the archive of
# /dist as a first installation, starts it as the service would, checks the
# API and the interface, then updates it in place (configuration kept).
set -euo pipefail
ARCHIVE=$(ls /dist/*.tar.gz)
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends curl ca-certificates ffmpeg >/dev/null
tar -xzf "$ARCHIVE" -C /tmp
RUSTNVR_WEB_PASSWORD=essai-local-1234 bash /tmp/rustnvr-*/install.sh --archive "$ARCHIVE" --no-start | tail -n 3
set -a; . /etc/rustnvr/rustnvr.env; set +a
runuser -u rustnvr -- /usr/local/bin/rustnvr run -c /etc/rustnvr/config.toml >/tmp/run.log 2>&1 &
for _ in $(seq 1 30); do curl -fsk https://127.0.0.1:8080/api/v1/health >/dev/null 2>&1 && break; sleep 1; done
curl -fsk https://127.0.0.1:8080/api/v1/health | grep -q '"ok"' || { cat /tmp/run.log; echo "ÉCHEC : API muette"; exit 1; }
curl -fsk https://127.0.0.1:8080/ | grep -q "<title>" || { echo "ÉCHEC : interface absente"; exit 1; }
if grep -q "cannot open the database" /tmp/run.log; then echo "ÉCHEC : base refusée"; exit 1; fi
echo "# kept" >> /etc/rustnvr/config.toml
bash /usr/local/bin/rustnvr-update --archive "$ARCHIVE" --no-start | tail -n 1
tail -n 1 /etc/rustnvr/config.toml | grep -q "# kept" || { echo "ÉCHEC : configuration perdue à la mise à jour"; exit 1; }
# Found without /usr/local/bin in the PATH (as under `pct enter`).
env PATH=/sbin:/bin:/usr/sbin:/usr/bin rustnvr --version >/dev/null || { echo "ÉCHEC : rustnvr hors de /usr/bin"; exit 1; }
echo "OK : $(/usr/local/bin/rustnvr --version), $(. /etc/os-release; echo "$PRETTY_NAME")"
