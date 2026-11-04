#!/bin/sh
# First start: puts the example configuration in /config, then starts RustNVR.
set -e
if [ ! -f /config/config.toml ]; then
    cp /usr/share/rustnvr/config.toml /config/config.toml
    echo "rustnvr: /config/config.toml created from the example (cameras are added in the interface)"
fi
exec rustnvr "$@"
