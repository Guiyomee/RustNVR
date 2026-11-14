# Home Assistant

Ce guide est maintenant dans le **[wiki](https://github.com/Guiyomee/RustNVR/wiki)**, en français et en
anglais :

- [Home Assistant](https://github.com/Guiyomee/RustNVR/wiki/FR-Home-Assistant) : connexion MQTT, entités, direct par la
  republication RTSP, automatisations
- La référence des topics MQTT reste dans [MQTT.md](MQTT.md).

Il y est tenu à jour ; ce fichier reste pour que les anciens liens mènent
quelque part.

## Interface dans Home Assistant

L'interface complète de RustNVR (direct, relecture, événements, réglages)
peut s'afficher dans la barre latérale de Home Assistant, comme Frigate avec
« Frigate Proxy ». C'est l'app **RustNVR Proxy** (Home Assistant OS ou
Supervised), fournie dans ce dépôt ; RustNVR continue de tourner où il est.

1. **Paramètres → Apps → Magasin d'apps → ⋮ → Dépôts** : ajoutez
   `https://github.com/Guiyomee/RustNVR` (ou l'adresse de votre fork).
2. Installez **RustNVR Proxy**. Dans **Configuration**, réglez `server` sur
   l'adresse de RustNVR vue depuis Home Assistant, sans chemin, par exemple
   `http://192.168.1.10:8080`.
3. Démarrez l'app et cochez « Afficher dans la barre latérale ». Le panneau
   « RustNVR » ouvre l'interface ; si des comptes sont configurés, connectez-vous
   une fois.

Si l'authentification de RustNVR est active, déclarez Home Assistant comme
proxy de confiance : les échecs de connexion bloquent alors la personne qui
se trompe, et non tout Home Assistant.

```toml
[web]
trusted_proxies = ["192.168.1.5", "172.30.32.0/23"]   # IP de Home Assistant, réseau interne de ses apps
```

Fonctionnement : Home Assistant sert le panneau sous
`/api/hassio_ingress/<jeton>/` et donne ce préfixe à l'app dans l'en-tête
`X-Ingress-Path`, que l'app transmet à RustNVR. RustNVR l'applique à ses
fichiers, à son API et à ses liens, et n'accepte d'être affiché dans un cadre
que sous ingress, et seulement par une page de la même origine (Home
Assistant). Sans cet en-tête, rien ne change.

---

This guide now lives in the **[wiki](https://github.com/Guiyomee/RustNVR/wiki)**, in French and English:
[Home Assistant](https://github.com/Guiyomee/RustNVR/wiki/EN-Home-Assistant).
