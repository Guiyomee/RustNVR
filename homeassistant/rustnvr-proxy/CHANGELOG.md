# Journal des versions

## 1.2.0

- **L'app a déménagé** dans le dépôt
  [RustNVR-HomeAssistant](https://github.com/Guiyomee/RustNVR-HomeAssistant),
  où arrivera aussi l'app « RustNVR » (RustNVR dans Home Assistant). Cette
  copie ne recevra plus de mises à jour : ajoutez le dépôt
  `https://github.com/Guiyomee/RustNVR-HomeAssistant` (Paramètres → Apps →
  Magasin d'apps → ⋮ → Dépôts), installez « RustNVR Proxy » depuis lui avec
  les mêmes options, puis désinstallez celle-ci et retirez l'ancien dépôt.

## 1.1.0

- Le panneau apparaît aussi pour les comptes Home Assistant qui ne sont pas
  administrateurs. L'app ne l'ouvre que si RustNVR demande une connexion
  (les droits sont ceux du compte RustNVR, par exemple « lecteur ») ; sinon
  une page explique comment faire. Option `users` pour n'autoriser que
  certains comptes Home Assistant ; un compte refusé voit son nom
  d'utilisateur, à recopier dans la liste.
- Option `page` : la page ouverte par le panneau (vue d'ensemble, mur
  d'écrans, événements, images ou lecture).
- **À savoir avant la mise à jour** : si RustNVR n'a aucun compte, le
  panneau reste fermé tant qu'un compte n'y est pas créé (ou que `users`
  n'est pas rempli).

## 1.0.0

- Première version : l'interface de RustNVR dans la barre latérale de Home
  Assistant (ingress), RustNVR tournant sur une autre machine.
