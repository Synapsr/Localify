# Infisical Is Read Through Its CLI, With Implicit Profiles

Launch profiles gain an `infisical` source. Like Proton Pass, Localify calls the official CLI (`infisical export --format json`) rather than the API: the CLI already holds the login, the instance domain (self-hosted included) and the project link in `.infisical.yaml`, so Localify stores no token and adds no dependency.

A project linked to Infisical needs no `localify.toml`. Without one, Localify offers `default`, `staging` and `prod` (dangerous), which read the environments `dev`, `staging` and `prod`: the environments Infisical creates for a new project. Each service reads the folder named after its own folder (`apps/web` reads `/web`), the layout we found in practice. Writing `localify.toml` replaces the implicit profiles entirely, so there is one source of truth at a time. Implicit profiles stop at these three, without listing the project's environments over the network, so the desktop menu stays offline and instant.

A failed Infisical call is a missing source, not an error, unlike an unusable `pass-cli`. Since implicit profiles make every plain `localify dev` read Infisical, an expired session must not stop a development launch; a dangerous profile still refuses to start on any missing source, including a key whose value the account may not read. Each CLI call is stopped after 20 seconds, because an expired session can make the CLI wait for a browser login.

The CLI uses one login profile per organization. When the selected one belongs to another organization than the project, the API answers 403, and Localify retries with each saved login profile and shows the one that worked. It does not bind the folder to that profile itself: changing the CLI's configuration is left to `infisical profile bind`.
