#!/usr/bin/env bash
# Inside a Debian container (scripts/release.sh): installs the previous
# release (/old), starts it (its database is created), updates it to the new
# archive (/dist) and checks that the new version opens that database.
set -euo pipefail
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends curl ca-certificates ffmpeg >/dev/null

start() {
    set -a; . /etc/rustnvr/rustnvr.env; set +a
    : > /tmp/run.log
    runuser -u rustnvr -- /usr/local/bin/rustnvr run -c /etc/rustnvr/config.toml >/tmp/run.log 2>&1 &
    local pid=$!
    for _ in $(seq 1 30); do curl -fsk https://127.0.0.1:8080/api/v1/health >/dev/null 2>&1 && break; sleep 1; done
    sleep 2
    local version; version=$(/usr/local/bin/rustnvr --version)
    if ! curl -fsk https://127.0.0.1:8080/api/v1/health | grep -q '"ok"' || grep -q "cannot open the database" /tmp/run.log; then
        cat /tmp/run.log
        echo "ÉCHEC : $version ne démarre pas sur cette base"
        exit 1
    fi
    echo "  $version : base ouverte, API en marche"
    kill "$pid"; wait "$pid" 2>/dev/null || true
}

OLD=$(ls /old/*.tar.gz)
tar -xzf "$OLD" -C /tmp
RUSTNVR_WEB_PASSWORD=essai-local-1234 bash /tmp/rustnvr-*/install.sh --archive "$OLD" --no-start >/dev/null
start
NEW=$(ls /dist/*.tar.gz)
bash /usr/local/bin/rustnvr-update --archive "$NEW" --no-start >/dev/null
# The units of one-click updates: installed by an updater of 2026.11.10 or
# later; after an older one, reinstalling once (as the interface says)
# brings them.
if [ ! -f /etc/systemd/system/rustnvr-update.path ]; then
    echo "  service de mise à jour absent après l'ancien rustnvr-update : réinstallation"
    bash /usr/local/bin/rustnvr-update --archive "$NEW" --no-start >/dev/null
fi
for unit in rustnvr-update.path rustnvr-update.service; do
    [ -f "/etc/systemd/system/$unit" ] || { echo "ÉCHEC : $unit pas installée"; exit 1; }
done
start
echo "OK : mise à jour depuis $(basename "$OLD")"
