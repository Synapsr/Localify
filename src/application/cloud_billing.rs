use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use super::cloud_account::{TunnelRefresh, refresh_tunnel};
use super::cloud_errors::with_next_step;
use super::ports::{
    BillingPage, CloudAccount, CloudAccountStore, CloudApi, CloudApiError, Plan, PlanInterval,
    PublicTunnel,
};
use crate::branding::BRANDING;

/// The API asks to retry a checkout another request is creating after 2 s.
const CHECKOUT_RETRY: Duration = Duration::from_secs(2);
const CHECKOUT_ATTEMPTS: u32 = 3;

/// Use case: the account's Localify Cloud plan, and the Polar pages that
/// start or manage it (ADR 0024). Paying is for a person: the CLI and the app
/// call this, never the MCP server.
pub struct CloudBilling<'a> {
    api: &'a dyn CloudApi,
    store: &'a dyn CloudAccountStore,
    tunnel: &'a dyn PublicTunnel,
    checkout_retry: Duration,
}

impl<'a> CloudBilling<'a> {
    pub fn new(
        api: &'a dyn CloudApi,
        store: &'a dyn CloudAccountStore,
        tunnel: &'a dyn PublicTunnel,
    ) -> Self {
        Self {
            api,
            store,
            tunnel,
            checkout_retry: CHECKOUT_RETRY,
        }
    }

    /// The plan from `GET /v1/me`; `None` from a Localify Cloud that does not
    /// report plans yet.
    pub fn plan(&self) -> Result<Option<Plan>> {
        let account = self.account()?;
        self.api.plan(&account).map_err(with_next_step)
    }

    /// A checkout for Pro, or the customer portal when the plan is already
    /// entitled, where the interval is switched. Waits out another checkout
    /// being created for the same account.
    pub fn subscribe(&self, interval: PlanInterval) -> Result<BillingPage> {
        let account = self.account()?;
        let mut attempt = 1;
        loop {
            match self.api.checkout(&account, interval) {
                Err(CloudApiError::Rejected { code, .. })
                    if code == "checkout_in_progress" && attempt < CHECKOUT_ATTEMPTS =>
                {
                    attempt += 1;
                    thread::sleep(self.checkout_retry);
                }
                result => return result.map_err(billing_error),
            }
        }
    }

    /// The Polar customer portal: the card, invoices, the interval and
    /// cancelling.
    pub fn portal(&self) -> Result<String> {
        let account = self.account()?;
        self.api.billing_portal(&account).map_err(billing_error)
    }

    /// Once the plan becomes entitled, the daemon fetches what it may serve.
    pub fn refresh_tunnel(&self) -> TunnelRefresh {
        refresh_tunnel(self.tunnel)
    }

    fn account(&self) -> Result<CloudAccount> {
        self.store.load()?.with_context(|| {
            format!(
                "This machine is not signed in to Localify Cloud. Sign in with: {} login",
                BRANDING.cli_command()
            )
        })
    }
}

/// A cloud that predates billing answers 404 on its routes; `no_billing` is
/// the portal of an account that never subscribed.
fn billing_error(error: CloudApiError) -> anyhow::Error {
    let cli = BRANDING.cli_command();
    match &error {
        CloudApiError::Rejected { code, .. } if code == "no_billing" => anyhow!(
            "This account has no subscription yet, so there is no billing to manage. Start one with: {cli} subscribe"
        ),
        CloudApiError::Rejected { status: 404, .. } => anyhow!(
            "Billing is not available on Localify Cloud yet. Nothing to pay for now: public URLs keep working as they do."
        ),
        CloudApiError::Rejected { code, message, .. } if code == "checkout_in_progress" => {
            anyhow!("{message} Try again in a few seconds: {cli} subscribe")
        }
        _ => with_next_step(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::{BillingPageKind, PlanStatus};
    use crate::application::testkit::*;

    fn rejected(status: u16, code: &str) -> CloudApiError {
        CloudApiError::Rejected {
            status,
            code: code.into(),
            message: "The API's sentence.".into(),
        }
    }

    #[test]
    fn subscribing_asks_for_the_interval_and_returns_the_page() {
        let (api, store, tunnel) = (
            InMemoryCloudApi::new(),
            InMemoryCloudAccountStore::signed_in(),
            InMemoryPublicTunnel::running(),
        );

        let page = CloudBilling::new(&api, &store, &tunnel)
            .subscribe(PlanInterval::Year)
            .unwrap();

        assert_eq!(page.kind, BillingPageKind::Checkout);
        assert_eq!(api.calls(), ["checkout:year"]);
    }

    #[test]
    fn a_checkout_being_created_elsewhere_is_waited_out() {
        let api = InMemoryCloudApi::new().with_error(rejected(409, "checkout_in_progress"));
        let (store, tunnel) = (
            InMemoryCloudAccountStore::signed_in(),
            InMemoryPublicTunnel::running(),
        );
        let mut billing = CloudBilling::new(&api, &store, &tunnel);
        billing.checkout_retry = Duration::ZERO;

        let page = billing.subscribe(PlanInterval::Month).unwrap();

        assert_eq!(page.kind, BillingPageKind::Checkout);
        assert_eq!(api.calls(), ["checkout:month", "checkout:month"]);
    }

    #[test]
    fn a_cloud_without_billing_says_so_plainly() {
        let (store, tunnel) = (
            InMemoryCloudAccountStore::signed_in(),
            InMemoryPublicTunnel::running(),
        );
        let api = InMemoryCloudApi::new().with_error(rejected(404, "http_404"));
        let error = CloudBilling::new(&api, &store, &tunnel)
            .subscribe(PlanInterval::Month)
            .unwrap_err();
        assert!(error.to_string().contains("not available"), "{error}");

        let api = InMemoryCloudApi::new().with_error(rejected(404, "no_billing"));
        let error = CloudBilling::new(&api, &store, &tunnel)
            .portal()
            .unwrap_err();
        assert!(error.to_string().contains("subscribe"), "{error}");
    }

    #[test]
    fn the_plan_needs_a_sign_in_and_tolerates_a_cloud_without_plans() {
        let (api, tunnel) = (InMemoryCloudApi::new(), InMemoryPublicTunnel::running());
        let signed_out = InMemoryCloudAccountStore::signed_out();
        assert!(
            CloudBilling::new(&api, &signed_out, &tunnel)
                .plan()
                .unwrap_err()
                .to_string()
                .contains("login")
        );

        let store = InMemoryCloudAccountStore::signed_in();
        assert_eq!(
            CloudBilling::new(&api, &store, &tunnel).plan().unwrap(),
            None
        );

        api.set_plan(Some(plan(PlanStatus::Trialing, true)));
        let plan = CloudBilling::new(&api, &store, &tunnel).plan().unwrap();
        assert_eq!(plan.map(|plan| plan.status), Some(PlanStatus::Trialing));
    }

    #[test]
    fn a_banned_account_is_told_where_to_write() {
        let api = InMemoryCloudApi::new().with_error(rejected(403, "banned"));
        let (store, tunnel) = (
            InMemoryCloudAccountStore::signed_in(),
            InMemoryPublicTunnel::running(),
        );

        let error = CloudBilling::new(&api, &store, &tunnel).plan().unwrap_err();

        assert!(error.to_string().contains("abuse@localify.dev"), "{error}");
    }
}
