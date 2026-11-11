# RustNVR Proxy

Affiche l'interface de RustNVR dans Home Assistant (panneau « RustNVR » de la
barre latérale), sans ouvrir de port supplémentaire : Home Assistant authentifie
l'accès (ingress) et l'app relaie vers votre serveur RustNVR.

Il faut une version de RustNVR **postérieure à 2026.11.06** : les précédentes
ne savent pas s'afficher sous ingress.

## Réglage

| Option   | Valeur                                                                      |
| -------- | --------------------------------------------------------------------------- |
| `server` | Adresse de RustNVR vue depuis Home Assistant, sans chemin : `http://192.168.1.10:8080` (ou `https://…` si RustNVR sert lui-même le HTTPS). |

Démarrez l'app, puis cochez « Afficher dans la barre latérale ».

## Connexion

Si des comptes sont configurés dans RustNVR, connectez-vous une fois dans le
panneau : la session est gardée comme dans un navigateur.

Les échecs de connexion bloquent l'adresse d'origine pendant un moment. Pour que
ce soit celle de la personne et non Home Assistant tout entier, déclarez Home
Assistant comme proxy de confiance dans RustNVR (`config.toml`) :

```toml
[web]
trusted_proxies = ["192.168.1.5", "172.30.32.0/23"]   # IP de Home Assistant, réseau interne de ses apps
```

## Dépannage

- **Page blanche ou « 502 »** : RustNVR injoignable depuis Home Assistant ;
  vérifiez `server` (journal de l'app).
- **Page de RustNVR mais vidéo absente** : la vue en direct passe par un
  WebSocket ; un reverse proxy placé devant Home Assistant doit les laisser
  passer (c'est déjà nécessaire pour Home Assistant lui-même).
- **« cross-origin request refused »** : un reverse proxy devant Home Assistant
  doit transmettre l'en-tête `Host` d'origine (ou `X-Forwarded-Host`).
