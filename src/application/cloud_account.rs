use anyhow::{Result, bail};
use serde::Serialize;

use super::ports::{
    CloudAccountStore, CloudApi, CloudApiError, DaemonConnectionError, DeviceAuthorization,
    DeviceTokenPoll, PublicTunnel, SignInRequest, TunnelState, TunnelStatus,
};
use crate::branding::BRANDING;

/// Whether this machine is signed in to Localify Cloud, as whom, and how its
/// tunnel is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub signed_in: bool,
    pub login: Option<String>,
    pub machine_name: Option<String>,
    pub tunnel: TunnelStatus,
}

/// What happened when the daemon was asked to fetch this machine's public
/// URLs again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelRefresh {
    Done,
    /// The daemon is not running; the tunnel starts with it.
    DaemonStopped,
    /// The daemon runs but did not take the request, with why.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInPoll {
    /// The person has not confirmed the code yet.
    Pending,
    /// Wait longer between polls: `interval` seconds when Localify Cloud
    /// says, else 5 more than before.
    SlowDown { interval: Option<u64> },
    SignedIn {
        status: AccountStatus,
        refresh: TunnelRefresh,
    },
    /// The person declined on GitHub, with why.
    Denied(String),
    /// This GitHub account may not sign in, with why.
    NotAllowed(String),
    /// The code expired, with why.
    Expired(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedOut {
    /// Who was signed in; `None` when nobody was.
    pub login: Option<String>,
    /// Set when Localify Cloud could not revoke the token and this machine
    /// forgot it anyway.
    pub warning: Option<String>,
    pub refresh: TunnelRefresh,
}

/// Use case: sign this machine in to Localify Cloud and out again (ADR 0020).
pub struct CloudAccounts<'a> {
    api: &'a dyn CloudApi,
    store: &'a dyn CloudAccountStore,
    tunnel: &'a dyn PublicTunnel,
}

impl<'a> CloudAccounts<'a> {
    pub fn new(
        api: &'a dyn CloudApi,
        store: &'a dyn CloudAccountStore,
        tunnel: &'a dyn PublicTunnel,
    ) -> Self {
        Self { api, store, tunnel }
    }

    /// Reads the saved credentials and asks the daemon about the tunnel. It
    /// never calls Localify Cloud, so it works offline.
    pub fn status(&self) -> Result<AccountStatus> {
        let account = self.store.load()?;
        Ok(AccountStatus {
            signed_in: account.is_some(),
            login: account.as_ref().map(|account| account.login.clone()),
            machine_name: account.map(|account| account.machine_name),
            tunnel: tunnel_status(self.tunnel),
        })
    }

    /// Starts GitHub's device flow. A signed-in machine signs out first, so
    /// it never registers twice.
    pub fn start_sign_in(&self) -> Result<DeviceAuthorization> {
        if let Some(account) = self.store.load()? {
            bail!(
                "Already signed in as {}. Run `{} logout` first.",
                account.login,
                BRANDING.cli_command()
            );
        }
        Ok(self.api.start_sign_in()?)
    }

    /// Polls the device flow once. When the person confirmed the code, saves
    /// the credentials and tells the daemon, which then starts the tunnel.
    pub fn poll_sign_in(&self, device_code: &str, machine_name: &str) -> Result<SignInPoll> {
        let device_code = device_code.trim();
        if device_code.is_empty() {
            bail!("The sign-in has no device code. Start the sign-in again.");
        }
        let request = SignInRequest {
            device_code: device_code.to_string(),
            machine_name: machine_name.to_string(),
            ingress_port: BRANDING.public_ingress_port(),
        };
        let poll = match self.api.poll_sign_in(&request) {
            Ok(poll) => poll,
            Err(CloudApiError::Rejected { code, message, .. }) if code == "provisioning_failed" => {
                bail!("{message} Nothing was saved on this machine: sign in again in a moment.")
            }
            Err(error) => return Err(error.into()),
        };
        Ok(match poll {
            DeviceTokenPoll::Pending => SignInPoll::Pending,
            DeviceTokenPoll::SlowDown { interval } => SignInPoll::SlowDown { interval },
            DeviceTokenPoll::Denied(message) => SignInPoll::Denied(message),
            DeviceTokenPoll::NotAllowed(message) => SignInPoll::NotAllowed(message),
            DeviceTokenPoll::Expired(message) => SignInPoll::Expired(message),
            DeviceTokenPoll::Granted(account) => {
                self.store.save(&account)?;
                SignInPoll::SignedIn {
                    refresh: refresh_tunnel(self.tunnel),
                    status: self.status()?,
                }
            }
        })
    }

    /// Revokes the token on Localify Cloud, forgets it here, then tells the
    /// daemon, which stops the tunnel. When the API cannot be reached, it
    /// forgets the token anyway, with a warning: Localify Cloud cleans up
    /// without it, and being stuck signed in helps nobody.
    pub fn sign_out(&self) -> Result<SignedOut> {
        let Some(account) = self.store.load()? else {
            return Ok(SignedOut {
                login: None,
                warning: None,
                refresh: refresh_tunnel(self.tunnel),
            });
        };
        let warning = match self.api.sign_out(&account) {
            Ok(()) => None,
            // The token is already revoked: there is nothing left to undo.
            Err(CloudApiError::Rejected { status: 401, .. }) => None,
            Err(error @ CloudApiError::Unreachable(_)) => Some(format!(
                "Signed out on this machine only: its token stays valid on Localify Cloud. {error}"
            )),
            Err(error) => bail!(
                "{error} This machine is still signed in; try again: {} logout",
                BRANDING.cli_command()
            ),
        };
        self.store.delete()?;
        Ok(SignedOut {
            login: Some(account.login),
            warning,
            refresh: refresh_tunnel(self.tunnel),
        })
    }
}

/// The tunnel as the daemon reports it, or `Unknown` when it cannot say.
pub fn tunnel_status(tunnel: &dyn PublicTunnel) -> TunnelStatus {
    let unknown = |error: Option<String>| TunnelStatus {
        state: TunnelState::Unknown,
        connections: 0,
        error,
    };
    match tunnel.status() {
        Ok(status) => status,
        Err(DaemonConnectionError::NotRunning) => unknown(None),
        Err(error) => unknown(Some(format!(
            "The daemon did not report the tunnel: {error}. Restart it with: {}",
            BRANDING.admin_command("restart")
        ))),
    }
}

/// Tells the daemon to fetch this machine's public URLs again. A daemon that
/// is not running fetches them when it starts, so that is not an error.
pub fn refresh_tunnel(tunnel: &dyn PublicTunnel) -> TunnelRefresh {
    match tunnel.refresh() {
        Ok(()) => TunnelRefresh::Done,
        Err(DaemonConnectionError::NotRunning) => TunnelRefresh::DaemonStopped,
        Err(error) => TunnelRefresh::Failed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::testkit::*;

    fn granted() -> DeviceTokenPoll {
        DeviceTokenPoll::Granted(cloud_account())
    }

    #[test]
    fn a_stopped_daemon_leaves_the_tunnel_unknown() {
        let (api, store, tunnel) = (
            InMemoryCloudApi::new(),
            InMemoryCloudAccountStore::signed_in(),
            InMemoryPublicTunnel::stopped(),
        );
        let status = CloudAccounts::new(&api, &store, &tunnel).status().unwrap();

        assert!(status.signed_in);
        assert_eq!(status.login.as_deref(), Some("teo-goulois"));
        assert_eq!(status.tunnel.state, TunnelState::Unknown);
        assert_eq!(status.tunnel.error, None);
    }

    #[test]
    fn signing_in_saves_the_credentials_and_tells_the_daemon() {
        let api = InMemoryCloudApi::new().with_polls(vec![DeviceTokenPoll::Pending, granted()]);
        let store = InMemoryCloudAccountStore::signed_out();
        let tunnel = InMemoryPublicTunnel::running();
        let accounts = CloudAccounts::new(&api, &store, &tunnel);

        assert_eq!(
            accounts.poll_sign_in("dc", "laptop").unwrap(),
            SignInPoll::Pending
        );
        assert_eq!(store.account(), None);

        let SignInPoll::SignedIn { status, refresh } =
            accounts.poll_sign_in("dc", "laptop").unwrap()
        else {
            panic!("expected a sign-in");
        };
        assert!(status.signed_in);
        assert_eq!(refresh, TunnelRefresh::Done);
        assert_eq!(store.account(), Some(cloud_account()));
        assert_eq!(tunnel.refreshes(), 1);

        let request = api.last_sign_in_request().unwrap();
        assert_eq!(request.machine_name, "laptop");
        assert_eq!(request.ingress_port, BRANDING.public_ingress_port());
    }

    #[test]
    fn a_signed_in_machine_signs_out_before_signing_in_again() {
        let (api, store, tunnel) = (
            InMemoryCloudApi::new(),
            InMemoryCloudAccountStore::signed_in(),
            InMemoryPublicTunnel::running(),
        );
        let error = CloudAccounts::new(&api, &store, &tunnel)
            .start_sign_in()
            .unwrap_err()
            .to_string();

        assert_eq!(
            error,
            format!(
                "Already signed in as teo-goulois. Run `{} logout` first.",
                BRANDING.cli_command()
            )
        );
    }

    #[test]
    fn a_declined_or_slowed_sign_in_saves_nothing() {
        let api = InMemoryCloudApi::new().with_polls(vec![
            DeviceTokenPoll::SlowDown { interval: Some(10) },
            DeviceTokenPoll::Denied("You declined.".into()),
        ]);
        let store = InMemoryCloudAccountStore::signed_out();
        let tunnel = InMemoryPublicTunnel::running();
        let accounts = CloudAccounts::new(&api, &store, &tunnel);

        assert_eq!(
            accounts.poll_sign_in("dc", "laptop").unwrap(),
            SignInPoll::SlowDown { interval: Some(10) }
        );
        assert_eq!(
            accounts.poll_sign_in("dc", "laptop").unwrap(),
            SignInPoll::Denied("You declined.".into())
        );
        assert_eq!(store.account(), None);
        assert_eq!(tunnel.refreshes(), 0);
    }

    #[test]
    fn signing_out_offline_forgets_the_token_with_a_warning() {
        let api = InMemoryCloudApi::new()
            .with_sign_out_error(CloudApiError::Unreachable("No network.".into()));
        let store = InMemoryCloudAccountStore::signed_in();
        let tunnel = InMemoryPublicTunnel::stopped();

        let signed_out = CloudAccounts::new(&api, &store, &tunnel)
            .sign_out()
            .unwrap();

        assert_eq!(store.account(), None);
        assert!(signed_out.warning.unwrap().contains("No network."));
        assert_eq!(signed_out.refresh, TunnelRefresh::DaemonStopped);
    }

    #[test]
    fn a_revoked_token_is_simply_forgotten() {
        let api = InMemoryCloudApi::new().with_sign_out_error(CloudApiError::Rejected {
            status: 401,
            code: "unauthorized".into(),
            message: "Sign in again.".into(),
        });
        let store = InMemoryCloudAccountStore::signed_in();
        let tunnel = InMemoryPublicTunnel::running();

        let signed_out = CloudAccounts::new(&api, &store, &tunnel)
            .sign_out()
            .unwrap();

        assert_eq!(store.account(), None);
        assert_eq!(signed_out.warning, None);
    }

    #[test]
    fn a_failing_api_keeps_the_machine_signed_in() {
        let api = InMemoryCloudApi::new().with_sign_out_error(CloudApiError::Rejected {
            status: 500,
            code: "internal".into(),
            message: "Something broke.".into(),
        });
        let store = InMemoryCloudAccountStore::signed_in();
        let tunnel = InMemoryPublicTunnel::running();

        assert!(
            CloudAccounts::new(&api, &store, &tunnel)
                .sign_out()
                .is_err()
        );
        assert_eq!(store.account(), Some(cloud_account()));
    }
}
