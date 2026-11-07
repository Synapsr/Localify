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
    /// Changes with every change of protection. Set for a password, by a
    /// Localify Cloud recent enough to send it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
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
    /// `None` from a Localify Cloud older than the paid plan.
    pub state: Option<PublicUrlState>,
    /// RFC 3339.
    pub created_at: String,
}

/// Whether a public URL carries traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicUrlState {
    Serving,
    /// Beyond the plan's limits, or without a plan. Kept with its slug and
    /// password until the plan covers it again.
    Suspended,
    /// Removed for abuse: it never serves again.
    TakenDown,
}

/// The account's Localify Cloud plan, as `GET /v1/me` gives it
/// (`docs/public-urls.md`, "Plans").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub status: PlanStatus,
    /// May serve public URLs right now.
    pub entitled: bool,
    pub interval: Option<PlanInterval>,
    /// RFC 3339, while trialing.
    pub trial_ends_at: Option<String>,
    /// RFC 3339: the next charge, when one is due.
    pub renews_at: Option<String>,
    /// RFC 3339: when a canceled plan stops, or an override expires.
    pub ends_at: Option<String>,
    /// False once the account used its trial.
    pub trial_available: bool,
    pub limits: PlanCounts,
    pub usage: PlanCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    None,
    Trialing,
    Active,
    /// The card failed; still entitled for 7 days.
    PastDue,
    /// Canceled, entitled until `ends_at`.
    Canceling,
    /// Access granted by the Localify team instead of a subscription.
    Override,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanInterval {
    Month,
    Year,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanCounts {
    pub serving_machines: u32,
    pub public_urls: u32,
}

/// The page that takes the person to Polar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingPage {
    pub kind: BillingPageKind,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BillingPageKind {
    /// A checkout for Pro, with its trial when the account still has one.
    Checkout,
    /// The customer portal: the plan is already entitled, or the person
    /// manages it.
    Portal,
}

/// The slug a new public URL would get, and its URL. Localify Cloud owns the
/// public domain, so a client takes it from here instead of writing it.
#[allow(dead_code)] // Only the app asks; the CLI target does not build the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlugSuggestion {
    pub slug: String,
    pub url: String,
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
    /// `None` from a Localify Cloud older than the paid plan.
    fn plan(&self, account: &CloudAccount) -> Result<Option<Plan>, CloudApiError>;
    /// A Polar checkout for Pro at `interval`, or the customer portal when
    /// the plan is already entitled.
    fn checkout(
        &self,
        account: &CloudAccount,
        interval: PlanInterval,
    ) -> Result<BillingPage, CloudApiError>;
    /// The URL of a Polar customer portal session.
    fn billing_portal(&self, account: &CloudAccount) -> Result<String, CloudApiError>;
    /// The first free slug that mirrors the local domain's name, as
    /// creating a URL without a slug would pick it.
    #[allow(dead_code)] // Only the app calls it; the CLI target does not build the app.
    fn suggest_slug(
        &self,
        account: &CloudAccount,
        local_domain: &DomainName,
    ) -> Result<SlugSuggestion, CloudApiError>;
    /// Whether the user may create a URL with this slug.
    #[allow(dead_code)] // Only the app calls it.
    fn slug_available(
        &self,
        account: &CloudAccount,
        slug: &PublicSlug,
    ) -> Result<bool, CloudApiError>;
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
