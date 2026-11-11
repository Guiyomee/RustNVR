#!/usr/bin/with-contenv bashio
# Forwards Home Assistant's ingress to the RustNVR server of the `server` option.
set -e

server=$(bashio::config 'server')
server=${server%/}
# Only scheme://host[:port]: the value goes into the nginx configuration.
if [[ ! ${server} =~ ^https?://[A-Za-z0-9._-]+(:[0-9]+)?$ && ! ${server} =~ ^https?://\[[0-9A-Fa-f:.]+\](:[0-9]+)?$ ]]; then
    bashio::exit.nok "server : adresse de RustNVR attendue, par exemple http://192.168.1.10:8080 (sans chemin), reçu « ${server} »"
fi

printf 'proxy_pass %s;\n' "${server}" > /etc/nginx/rustnvr-server.conf
bashio::log.info "Interface de RustNVR : ${server}"
exec nginx -g 'daemon off;'
