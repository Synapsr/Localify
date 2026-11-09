use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use super::cloud_account::{TunnelRefresh, refresh_tunnel};
use super::cloud_errors::{with_next_step, with_upgrade_command};
use super::ports::{
    Addon, AddonPrices, BillingPage, CheckoutProduct, CloudAccount, CloudAccountStore, CloudApi,
    CloudApiError, Currency, Plan, PlanInterval, PublicTunnel,
};
use crate::branding::BRANDING;
use crate::domain::AddonUnits;

/// The API asks to retry a checkout another request is creating after 2 s.
const CHECKOUT_RETRY: Duration = Duration::from_secs(2);
const CHECKOUT_ATTEMPTS: u32 = 3;

/// The add-ons' list prices, in minor units, for when Localify Cloud cannot
/// give the live ones: $3 a serving machine, $2 a pack of 10 URLs, or the
/// same in euros.
const LIST_PRICE_MACHINE: u32 = 300;
const LIST_PRICE_URL_PACK: u32 = 200;

/// The add-ons' prices, and whether Localify Cloud gave them.
#[allow(dead_code)] // Only the app shows prices; the CLI target does not build the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddonPricing {
    pub prices: AddonPrices,
    /// False for the list prices, shown while the cloud cannot answer.
    pub live: bool,
}

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
    /// entitled, where the interval is switched.
    pub fn subscribe(&self, interval: PlanInterval) -> Result<BillingPage> {
        self.checkout(CheckoutProduct::Pro(interval), billing_error)
    }

    /// A checkout for `units` of an add-on, or the customer portal when the
    /// account already has that add-on, where its quantity is changed.
    pub fn buy_addon(&self, addon: Addon, units: AddonUnits) -> Result<BillingPage> {
        self.checkout(CheckoutProduct::Addon(addon, units), addon_error)
    }

    /// The add-ons' monthly prices: live from Localify Cloud, or the list
    /// prices in `fallback` when it cannot say. Never fails, since a price
    /// only helps the sentence.
    #[allow(dead_code)] // Only the app shows prices.
    pub fn addon_prices(&self, fallback: Currency) -> AddonPricing {
        let live = self
            .store
            .load()
            .ok()
            .flatten()
            .and_then(|account| self.api.addon_prices(&account).ok());
        match live {
            Some(prices) => AddonPricing { prices, live: true },
            None => AddonPricing {
                prices: AddonPrices {
                    currency: fallback,
                    machine: LIST_PRICE_MACHINE,
                    url_pack: LIST_PRICE_URL_PACK,
                },
                live: false,
            },
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

    /// Waits out another checkout being created for the same account, and
    /// says any other refusal with `refusal`.
    fn checkout(
        &self,
        product: CheckoutProduct,
        refusal: fn(CloudApiError) -> anyhow::Error,
    ) -> Result<BillingPage> {
        let account = self.account()?;
        let mut attempt = 1;
        loop {
            match self.api.checkout(&account, product) {
                Err(CloudApiError::Rejected { code, .. })
                    if code == "checkout_in_progress" && attempt < CHECKOUT_ATTEMPTS =>
                {
                    attempt += 1;
                    thread::sleep(self.checkout_retry);
                }
                result => return result.map_err(refusal),
            }
        }
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

/// For an add-on, the API's own sentence stands: `addons_unavailable` while
/// sales are off, `402 subscription_required` without Pro, with the command
/// its `upgrade` names.
fn addon_error(error: CloudApiError) -> anyhow::Error {
    match &error {
        CloudApiError::Rejected {
            message,
            upgrade: upgrade @ Some(_),
            ..
        } => anyhow!(with_upgrade_command(message, upgrade.as_ref())),
        CloudApiError::Rejected { code, message, .. } if code == "addons_unavailable" => {
            anyhow!("{message}")
        }
        CloudApiError::Rejected { code, message, .. } if code == "checkout_in_progress" => {
            anyhow!("{message} Try again in a few seconds.")
        }
        _ => billing_error(error),
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
    use crate::application::ports::{BillingPageKind, PlanStatus, UpgradeHint};
    use crate::application::testkit::*;

    fn rejected(status: u16, code: &str) -> CloudApiError {
        CloudApiError::Rejected {
            status,
            code: code.into(),
            message: "The API's sentence.".into(),
            upgrade: None,
        }
    }

    fn signed_in() -> (InMemoryCloudAccountStore, InMemoryPublicTunnel) {
        (
            InMemoryCloudAccountStore::signed_in(),
            InMemoryPublicTunnel::running(),
        )
    }

    fn units(units: u32) -> AddonUnits {
        AddonUnits::new(units).unwrap()
    }

    #[test]
    fn an_addon_checkout_asks_for_its_units() {
        let api = InMemoryCloudApi::new();
        api.set_plan(Some(plan(PlanStatus::Active, true)));
        let (store, tunnel) = signed_in();

        let page = CloudBilling::new(&api, &store, &tunnel)
            .buy_addon(Addon::Urls, units(3))
            .unwrap();

        assert_eq!(page.kind, BillingPageKind::Checkout);
        assert_eq!(api.calls(), ["checkout:addon_urls"]);
        assert_eq!(api.last_units(), Some(3));
    }

    #[test]
    fn an_addon_the_account_has_opens_the_portal() {
        let api = InMemoryCloudApi::new();
        let mut held = plan(PlanStatus::Active, true);
        held.addons.machines = 2;
        api.set_plan(Some(held));
        let (store, tunnel) = signed_in();

        let page = CloudBilling::new(&api, &store, &tunnel)
            .buy_addon(Addon::Machine, units(1))
            .unwrap();

        assert_eq!(page.kind, BillingPageKind::Portal);
    }

    #[test]
    fn addon_refusals_keep_the_api_sentence_and_its_command() {
        let (store, tunnel) = signed_in();
        let api = InMemoryCloudApi::new().with_error(CloudApiError::Rejected {
            status: 402,
            code: "subscription_required".into(),
            message: "Add-ons extend Localify Cloud Pro.".into(),
            upgrade: Some(UpgradeHint {
                kind: "pro".into(),
                command: "localify subscribe".into(),
            }),
        });
        let error = CloudBilling::new(&api, &store, &tunnel)
            .buy_addon(Addon::Machine, units(1))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Add-ons extend Localify Cloud Pro. Run: localify subscribe"
        );

        let api = InMemoryCloudApi::new().with_error(rejected(409, "addons_unavailable"));
        let error = CloudBilling::new(&api, &store, &tunnel)
            .buy_addon(Addon::Urls, units(1))
            .unwrap_err();
        assert_eq!(error.to_string(), "The API's sentence.");
    }

    #[test]
    fn prices_fall_back_to_the_list_prices_in_the_local_currency() {
        let (store, tunnel) = signed_in();
        let api = InMemoryCloudApi::new();
        let live = CloudBilling::new(&api, &store, &tunnel).addon_prices(Currency::Usd);
        assert!(live.live);
        assert_eq!(live.prices.currency, Currency::Eur);

        let api = InMemoryCloudApi::new().with_error(rejected(503, "pricing_unavailable"));
        let fallback = CloudBilling::new(&api, &store, &tunnel).addon_prices(Currency::Eur);
        assert_eq!(
            fallback,
            AddonPricing {
                prices: AddonPrices {
                    currency: Currency::Eur,
                    machine: 300,
                    url_pack: 200,
                },
                live: false,
            }
        );
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
