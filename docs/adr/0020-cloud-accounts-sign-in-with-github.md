# Cloud Accounts Sign In With GitHub

Localify Cloud identifies people by their GitHub account, through GitHub's device flow, and replaces the license identity of ADR 0002. `localify login` and the desktop app show a code to confirm on github.com, which works the same in a terminal, over SSH on a remote machine and in the app. The Worker runs the flow with the OAuth App's client ID alone, keeps no GitHub token, and only reads the user's id and login. No email has to be sent and no password stored.

Signing in also registers the machine: the cloud creates its tunnel and returns a Localify token, which the machine keeps in a 0600 file in its data directory. The daemon runs as the user but outside the login session, so the Keychain is not used. The token is stored hashed on the server and revoked by `localify logout`.

V1 is invite-only. Only logins on an allowlist kept in the Worker's configuration can sign in, starting with the Synapsr team, so nobody else can host pages on `localify.fyi` and get the domain flagged. Opening sign-up comes with quotas and the paid offer.
