//! Remote machines in the app (`docs/remote-machines.md`, "Desktop app").
//! Everything goes through the daemon's management socket, and removing a
//! machine through Localify Cloud, as `localify machines remove` does. The
//! window does no network.

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use super::DesktopApi;
use super::dto::{
    RemoteAccessAnswerRequest, RemoteMachineRequest, RemotePairAnswerRequest,
    RemoteSettingsRequest, RemoteStreamRequest,
};
use crate::application::ports::DaemonConnectionError;
use crate::infrastructure::cloud::api::CloudClient;
use crate::infrastructure::cloud::credentials::CredentialsStore;
use crate::infrastructure::cloud::http::{CloudHttp, block_on};
use crate::infrastructure::mgmt_client::{MgmtSocketClient, MgmtStream};

fn daemon_error(error: DaemonConnectionError) -> anyhow::Error {
    match error {
        DaemonConnectionError::NotRunning => anyhow!(
            "The Localify daemon is not running, and remote machines go through it. Start it in Settings."
        ),
        DaemonConnectionError::ProtocolError(message) => anyhow!(message),
        other => anyhow!("Could not reach the Localify daemon: {other}"),
    }
}

impl DesktopApi {
    fn daemon(&self) -> Result<MgmtSocketClient> {
        let (_, paths) = self.load_config_and_paths()?;
        Ok(MgmtSocketClient::new(&paths.socket_path))
    }

    /// The relay link, this machine's settings and its fingerprint.
    pub fn remote_status(&self) -> Result<Value> {
        self.daemon()?.remote_status().map_err(daemon_error)
    }

    /// The roster merged with pins, presence and sessions, without probing.
    pub fn remote_machines(&self) -> Result<Value> {
        self.daemon()?.remote_machines(false).map_err(daemon_error)
    }

    pub fn change_remote_settings(&self, request: RemoteSettingsRequest) -> Result<Value> {
        self.daemon()?
            .remote_settings(serde_json::to_value(request)?)
            .map_err(daemon_error)
    }

    /// A person's answer to a pairing code, on either side.
    pub fn answer_remote_pairing(&self, request: RemotePairAnswerRequest) -> Result<()> {
        self.daemon()?
            .remote_pair_answer(&request.pairing_id, request.accept)
            .map_err(daemon_error)
    }

    /// Deletes the pin, and tells the machine when a session is open.
    pub fn unpair_remote_machine(&self, request: RemoteMachineRequest) -> Result<bool> {
        self.daemon()?
            .remote_unpair(&request.machine_id)
            .map_err(daemon_error)
    }

    /// A person's answer in the access dialog: `{outcome}`, `granted` only
    /// when the request still waited and nothing changed the settings since.
    pub fn answer_remote_access(&self, request: RemoteAccessAnswerRequest) -> Result<Value> {
        self.daemon()?
            .remote_access_answer(&request.request_id, request.allow, request.suppress)
            .map_err(daemon_error)
    }

    /// The machines whose access requests this Mac declines at once.
    pub fn remote_access_suppressed(&self) -> Result<Value> {
        self.daemon()?
            .remote_access_suppressed()
            .map_err(daemon_error)
    }

    /// Lets a machine ask for access again.
    pub fn restore_remote_access(&self, request: RemoteMachineRequest) -> Result<bool> {
        self.daemon()?
            .remote_access_restore(&request.machine_id)
            .map_err(daemon_error)
    }

    /// Signs another machine out of the account. The relay tells every
    /// connected peer, which drops its pin; this one drops it now.
    pub fn remove_remote_machine(&self, request: RemoteMachineRequest) -> Result<()> {
        let (_, paths) = self.load_config_and_paths()?;
        let Some(credentials) = CredentialsStore::in_data_dir(&paths.data_dir).load()? else {
            bail!("This Mac is not signed in to Localify Cloud.");
        };
        if credentials.machine_id == request.machine_id {
            bail!("This is this Mac: sign it out in Settings → Localify Cloud instead.");
        }
        let http = CloudHttp::new(credentials.api_base, Some(credentials.token))?;
        block_on(CloudClient::new(http).remove_machine(&request.machine_id))??;
        let _ = MgmtSocketClient::new(&paths.socket_path).remote_unpair(&request.machine_id);
        Ok(())
    }

    /// Opens a streaming command; closing the stream ends what it watched.
    pub fn open_remote_stream(&self, request: RemoteStreamRequest) -> Result<MgmtStream> {
        let command = match request {
            RemoteStreamRequest::Events { access } => {
                json!({ "cmd": "remote_events", "access": access })
            }
            RemoteStreamRequest::Pair { machine_id } => {
                json!({ "cmd": "remote_pair", "machineId": machine_id })
            }
            RemoteStreamRequest::Watch { machine_id } => {
                json!({ "cmd": "remote_watch", "machineId": machine_id })
            }
            RemoteStreamRequest::Follow {
                machine_id,
                run_id,
                offset,
            } => json!({
                "cmd": "remote_output_follow",
                "machineId": machine_id,
                "runId": run_id,
                "offset": offset,
            }),
            RemoteStreamRequest::Access { machine_id, wants } => json!({
                "cmd": "remote_access",
                "machineId": machine_id,
                "wants": wants,
            }),
        };
        self.daemon()?.remote_stream(&command).map_err(daemon_error)
    }
}
