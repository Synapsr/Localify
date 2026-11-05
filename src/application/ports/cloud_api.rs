use std::fmt;

use serde::Serialize;

use crate::domain::{DomainName, PublicSlug};

/// A signed-in machine: where its API is, its token, and who it is.
#[derive(Clone, PartialEq, Eq)]
pub struct CloudAccount {
    pub api_base: String,
    pub token: String,
    pub login: String,
    pub machine_id: String,
    pub machine_name: String,
}

/// Leaves the token out, so an account can appear in logs and errors.
impl fmt::Debug for CloudAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloudAccount")
            .field("api_base", &self.api_base)
            .field("token", &"<redacted>")
            .field("login", &self.login)
            .field("machine_id", &self.machine_id)
            .field("machine_name", &self.machine_name)
            .finish()
    }
}

/// The code a person confirms on github.com to sign this machine in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds to wait between two polls.
    pub interval: u64,
    /// Seconds before the code expires.
    pub expires_in: u64,
}

/// Registers this machine once the person confirmed the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInRequest {
    pub device_code: String,
    pub machine_name: String,
    /// Where the daemon takes public traffic from the tunnel.
    pub ingress_port: u16,
}

/// One poll of the device flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceTokenPoll {
    /// The person has not confirmed the code yet.
    Pending,
    /// Polling too fast: wait longer between polls, `interval` seconds when
    /// the API says.
    SlowDown {
        interval: Option<u64>,
    },
    Granted(CloudAccount),
    /// The person declined on GitHub, with the API's sentence.
    Denied(String),
    /// This GitHub account may not sign in, with the API's sentence.
    NotAllowed(String),
    /// The code expired, with the API's sentence.
    Expired(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtectionMode {
    Password,
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Protection {
    pub mode: ProtectionMode,
}

/// The machine that serves a public URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServingMachine {
    pub id: String,
    pub name: String,
}

/// A public URL as Localify Cloud describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicUrl {
    pub id: String,
    pub slug: String,
    pub url: String,
    pub local_domain: String,
    pub wildcard: bool,
    /// `None` when no machine serves it.
    pub machine: Option<ServingMachine>,
    pub protection: Protection,
    /// RFC 3339.
    pub created_at: String,
}

/// A public URL and the password Localify Cloud generated for it, which it
/// returns this once only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicUrlWithPassword {
    pub public_url: PublicUrl,
    pub password: Option<String>,
}

/// How a public URL should be protected. A password of `None` asks Localify
/// Cloud to generate one.
#[derive(Clone, PartialEq, Eq)]
pub enum ProtectionChoice {
    Password(Option<String>),
    Public,
}

/// Leaves a chosen password out.
impl fmt::Debug for ProtectionChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Password(Some(_)) => f.write_str("Password(<chosen>)"),
            Self::Password(None) => f.write_str("Password(<generated>)"),
            Self::Public => f.write_str("Public"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPublicUrl {
    pub local_domain: DomainName,
    pub wildcard: bool,
    pub slug: Option<PublicSlug>,
    pub protection: ProtectionChoice,
}

#[derive(Debug, thiserror::Error)]
pub enum CloudApiError {
    /// No answer: offline, DNS, timeout. The sentence says what to check.
    #[error("{0}")]
    Unreachable(String),
    /// The API answered with an error code, such as `not_found`, and a
    /// sentence for a person.
    #[error("{message}")]
    Rejected {
        status: u16,
        code: String,
        message: String,
    },
    /// Anything else, such as a response this version cannot read.
    #[error("{0}")]
    Failed(String),
}

/// Port for the Localify Cloud API (`docs/public-urls.md`, section "API").
/// Every call but the device flow acts as `account`.
pub trait CloudApi {
    fn start_sign_in(&self) -> Result<DeviceAuthorization, CloudApiError>;
    fn poll_sign_in(&self, request: &SignInRequest) -> Result<DeviceTokenPoll, CloudApiError>;
    /// Revokes the token and detaches the machine's public URLs.
    fn sign_out(&self, account: &CloudAccount) -> Result<(), CloudApiError>;
    fn list_public_urls(&self, account: &CloudAccount) -> Result<Vec<PublicUrl>, CloudApiError>;
    /// The calling machine serves the new URL.
    fn create_public_url(
        &self,
        account: &CloudAccount,
        request: &NewPublicUrl,
    ) -> Result<PublicUrlWithPassword, CloudApiError>;
    fn remove_public_url(&self, account: &CloudAccount, id: &str) -> Result<(), CloudApiError>;
    fn set_protection(
        &self,
        account: &CloudAccount,
        id: &str,
        protection: &ProtectionChoice,
    ) -> Result<PublicUrlWithPassword, CloudApiError>;
    /// Moves the URL to the calling machine.
    fn claim_public_url(
        &self,
        account: &CloudAccount,
        id: &str,
    ) -> Result<PublicUrl, CloudApiError>;
}
