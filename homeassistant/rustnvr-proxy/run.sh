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

# Page the panel opens on: the root of the interface goes there. Only the
# values of the schema's list reach nginx.
case "$(bashio::config 'page')" in
    wall) page=/mur ;;
    events) page=/events ;;
    images) page=/images ;;
    playback) page=/playback ;;
    *) page= ;;
esac
if [[ -n ${page} ]]; then
    printf 'location = / {\n    absolute_redirect off;\n    return 302 $http_x_ingress_path%s;\n}\n' "${page}" > /etc/nginx/rustnvr-home.conf
    bashio::log.info "Page d'accueil du panneau : ${page}"
else
    : > /etc/nginx/rustnvr-home.conf
fi

# Who may open the panel. It shows for every Home Assistant user
# (panel_admin: false), and the supervisor names the user in
# X-Remote-User-Name. With a `users` list, only those pass. Without one,
# everyone passes as long as RustNVR asks for a login: with no accounts there,
# anyone seeing the panel could change everything.
users=()
while IFS= read -r u; do
    [[ -z ${u} ]] && continue
    if [[ ! ${u} =~ ^[A-Za-z0-9._@-]+$ ]]; then
        bashio::exit.nok "users : nom de compte Home Assistant attendu (lettres, chiffres, . _ @ -), reçu « ${u} »"
    fi
    users+=("${u,,}")
done < <(bashio::config 'users')

# $1: value of $rustnvr_denied for users not in the list (0 lets them in,
# 1 not listed, 2 no accounts in RustNVR, 3 RustNVR unreachable).
write_access() {
    {
        printf 'map $http_x_remote_user_name $rustnvr_denied {\n    default %s;\n' "$1"
        for u in "${users[@]}"; do printf '    "%s" 0;\n' "${u}"; done
        printf '}\n'
    } > /etc/nginx/rustnvr-access.conf
}

# "open", "closed" (no login in RustNVR) or "unreachable".
login_state() {
    local status
    status=$(curl -fsSk --max-time 5 "${server}/api/v1/auth/status" 2> /dev/null) || { echo unreachable; return; }
    if [[ $(jq -r '.required' <<< "${status}" 2> /dev/null) == true ]]; then echo open; else echo closed; fi
}

apply_state() {
    case "$1" in
        open) write_access 0 ;;
        closed) write_access 2 ;;
        *) write_access 3 ;;
    esac
}

if (( ${#users[@]} > 0 )); then
    write_access 1
    bashio::log.info "Comptes Home Assistant autorisés : ${users[*]}"
else
    state=$(login_state)
    apply_state "${state}"
    case "${state}" in
        open) bashio::log.info "Panneau ouvert à tous les comptes Home Assistant ; RustNVR demande une connexion" ;;
        closed) bashio::log.warning "RustNVR n'a pas de comptes : le panneau reste fermé. Créez un compte administrateur dans RustNVR, ou listez les comptes autorisés dans « users »" ;;
        *) bashio::log.warning "RustNVR injoignable à ${server} ; nouvel essai toutes les 30 s" ;;
    esac
    # Accounts can be created (or removed) later in RustNVR. Started from a
    # subshell that exits at once, so that the loop is not a child of nginx
    # (which would log "unknown process … exited" when the app stops).
    ( (
        while sleep 30; do
            now=$(login_state)
            [[ ${now} == "${state}" || ( ${now} == unreachable && ${state} != unreachable ) ]] && continue
            state=${now}
            apply_state "${state}"
            nginx -s reload 2> /dev/null || true
            if [[ ${state} == open ]]; then
                bashio::log.info "RustNVR demande maintenant une connexion : panneau ouvert à tous les comptes Home Assistant"
            else
                bashio::log.warning "RustNVR n'a plus de comptes : panneau fermé"
            fi
        done
    ) & )
fi

exec nginx -g 'daemon off;'
