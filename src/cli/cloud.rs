//! `localify login`, `logout`, `whoami`, `subscribe` and `billing`: this
//! machine's Localify Cloud account (ADR 0020), its plan and its add-ons
//! (ADR 0024).

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::Serialize;

use super::output::{can_prompt, note, print_json, wants_json};
use crate::application::cloud_account::{AccountStatus, SignInPoll, TunnelRefresh};
use crate::application::cloud_errors::{no_plan_line, raise_limit_line};
use crate::application::ports::{
    Addon, BillingPage, BillingPageKind, DeviceAuthorization, Plan, PlanInterval, PlanStatus,
    PublicUrlState, RuntimeState, TunnelState, TunnelStatus,
};
use crate::branding::BRANDING;
use crate::domain::AddonUnits;
use crate::infrastructure::cloud_adapters::{CloudAdapters, computer_name};

/// `subscribe` asks for the plan this often after opening the checkout, for
/// this long.
const PLAN_POLL: Duration = Duration::from_secs(5);
const PLAN_WAIT: Duration = Duration::from_secs(15 * 60);

/// Signs in with GitHub's device flow: shows the code, opens the browser for
/// a person at a terminal, and waits until the code is confirmed.
pub fn login(json_flag: bool, cloud: &CloudAdapters) -> Result<()> {
    let json = wants_json(json_flag);
    let accounts = cloud.accounts();
    let device = accounts.start_sign_in()?;
    print_code(&device, json);
    if can_prompt() {
        open_in_browser(&device.verification_uri);
    }

    let machine_name = computer_name();
    let deadline = Instant::now() + Duration::from_secs(device.expires_in);
    let mut interval = device.interval.max(1);
    let cli = BRANDING.cli_command();
    loop {
        thread::sleep(Duration::from_secs(interval));
        match accounts.poll_sign_in(&device.device_code, &machine_name)? {
            SignInPoll::Pending => {}
            // GitHub's rule for a missing interval: five seconds more.
            SignInPoll::SlowDown { interval: next } => interval = next.unwrap_or(interval + 5),
            SignInPoll::SignedIn { status, refresh } => {
                return print_signed_in(&status, &refresh, json);
            }
            SignInPoll::Denied(message) => {
                bail!("{message} To sign in after all, run `{cli} login` again.")
            }
            SignInPoll::NotAllowed(message) => {
                bail!("{message} To sign in with another GitHub account, run `{cli} login` again.")
            }
            SignInPoll::Expired(message) => {
                bail!("{message} Run `{cli} login` again for a new code.")
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "The code expired before it was confirmed. Run `{cli} login` again for a new code."
            );
        }
    }
}

pub fn logout(json_flag: bool, cloud: &CloudAdapters) -> Result<()> {
    let json = wants_json(json_flag);
    let accounts = cloud.accounts();
    let signed_out = accounts.sign_out()?;
    if let Some(warning) = &signed_out.warning {
        eprintln!("Warning: {warning}");
    }
    if let TunnelRefresh::Failed(reason) = &signed_out.refresh {
        eprintln!(
            "Warning: the daemon did not stop the tunnel ({reason}). Restart it with: {}",
            BRANDING.admin_command("restart")
        );
    }
    if json {
        return print_json(&accounts.status()?);
    }
    match signed_out.login {
        Some(login) => println!(
            "Signed out {login}. This machine's public URLs are offline until a machine claims them."
        ),
        None => println!("This machine was not signed in to Localify Cloud."),
    }
    Ok(())
}

/// `whoami --json`: the account, the tunnel, and the plan when Localify
/// Cloud reports one.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WhoAmI<'a> {
    #[serde(flatten)]
    status: &'a AccountStatus,
    plan: Option<&'a Plan>,
    /// Why the plan could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_error: Option<String>,
}

pub fn whoami(json_flag: bool, cloud: &CloudAdapters) -> Result<()> {
    let status = cloud.accounts().status()?;
    // Asked only when signed in; the rest of the answer stays offline.
    let plan = status.signed_in.then(|| cloud.billing().plan());
    let (plan, plan_error) = match plan {
        Some(Ok(plan)) => (plan, None),
        Some(Err(error)) => (None, Some(format!("{error:#}"))),
        None => (None, None),
    };
    if wants_json(json_flag) {
        return print_json(&WhoAmI {
            status: &status,
            plan: plan.as_ref(),
            plan_error,
        });
    }
    let (Some(login), Some(machine)) = (&status.login, &status.machine_name) else {
        println!(
            "Not signed in to Localify Cloud. Sign in with: {} login",
            BRANDING.cli_command()
        );
        return Ok(());
    };
    println!("Signed in to Localify Cloud as {login}, on the machine {machine}.");
    match (&plan, &plan_error) {
        (Some(plan), _) => println!("{}", plan_line(plan)),
        (None, Some(error)) => println!("Plan: unknown. {error}"),
        // A Localify Cloud older than the paid plan reports none.
        (None, None) => {}
    }
    println!("{}", tunnel_line(&status));
    if let Some(line) = blocked_line(&status.tunnel, cloud) {
        println!("{line}");
    }
    Ok(())
}

/// Opens the Polar checkout for Pro on a terminal and waits for the plan;
/// otherwise prints the page and returns (`docs/public-urls.md`, "CLI").
pub fn subscribe(yearly: bool, json_flag: bool, cloud: &CloudAdapters) -> Result<()> {
    let json = wants_json(json_flag);
    let billing = cloud.billing();
    let interval = if yearly {
        PlanInterval::Year
    } else {
        PlanInterval::Month
    };
    let page = billing.subscribe(interval)?;
    if json || !can_prompt() {
        return print_page(&page, json);
    }
    if page.kind == BillingPageKind::Portal {
        println!(
            "Localify Cloud Pro is already active. The customer portal, where the interval can be switched:\n\n    {}\n",
            page.url
        );
        open_in_browser(&page.url);
        return Ok(());
    }
    println!(
        "Opening the Polar checkout for Localify Cloud Pro:\n\n    {}\n\nWaiting for the plan (up to 15 min)...",
        page.url
    );
    open_in_browser(&page.url);
    let plan = wait_for_plan(cloud, |plan| plan.entitled, "The plan is not active yet")?;
    println!("Localify Cloud Pro is active. {}", plan_line(&plan));
    print_refresh(&billing.refresh_tunnel(), false);
    Ok(())
}

/// Opens the Polar checkout for an add-on on a terminal, or the customer
/// portal when the account already has it, and waits for the new limit;
/// otherwise prints the page and returns, as `subscribe` does.
pub fn buy_addon(
    addon: Addon,
    units: AddonUnits,
    json_flag: bool,
    cloud: &CloudAdapters,
) -> Result<()> {
    let json = wants_json(json_flag);
    let billing = cloud.billing();
    let waits = !json && can_prompt();
    // The limit before, to tell when the new one arrives.
    let before = if waits {
        billing.plan()?.map(|plan| addon_limit(&plan, addon))
    } else {
        None
    };
    let page = billing.buy_addon(addon, units)?;
    if !waits {
        return print_page(&page, json);
    }
    match page.kind {
        BillingPageKind::Checkout => println!(
            "Opening the Polar checkout for {}:\n\n    {}\n",
            addon_units_label(addon, units),
            page.url
        ),
        BillingPageKind::Portal => println!(
            "This account already has {}. Change how many in the customer portal:\n\n    {}\n",
            addon_label(addon),
            page.url
        ),
    }
    open_in_browser(&page.url);
    let Some(before) = before else {
        return Ok(());
    };
    println!("Waiting for the new limit (up to 15 min)...");
    let plan = wait_for_plan(
        cloud,
        |plan| addon_limit(plan, addon) != before,
        "The new limit does not show yet",
    )?;
    println!("The plan's limits changed. {}", plan_line(&plan));
    print_refresh(&billing.refresh_tunnel(), false);
    Ok(())
}

/// Without a terminal or with `--json`: the page, for whoever runs the
/// command to open.
fn print_page(page: &BillingPage, json: bool) -> Result<()> {
    if json {
        return print_json(page);
    }
    println!("{}", page.url);
    Ok(())
}

/// The limit an add-on raises.
fn addon_limit(plan: &Plan, addon: Addon) -> u32 {
    match addon {
        Addon::Machine => plan.limits.serving_machines,
        Addon::Urls => plan.limits.public_urls,
    }
}

fn addon_label(addon: Addon) -> &'static str {
    match addon {
        Addon::Machine => "extra serving machines",
        Addon::Urls => "extra public URLs",
    }
}

/// `3 more serving machines`, or `2 packs of 10 public URLs (20 URLs)`.
fn addon_units_label(addon: Addon, units: AddonUnits) -> String {
    let n = units.value();
    match (addon, n) {
        (Addon::Machine, 1) => "1 more serving machine".to_string(),
        (Addon::Machine, n) => format!("{n} more serving machines"),
        (Addon::Urls, 1) => "1 pack of 10 public URLs".to_string(),
        (Addon::Urls, n) => format!("{n} packs of 10 public URLs ({} URLs)", u32::from(n) * 10),
    }
}

/// Opens the Polar customer portal on a terminal; prints its URL otherwise.
pub fn billing(json_flag: bool, cloud: &CloudAdapters) -> Result<()> {
    let json = wants_json(json_flag);
    let url = cloud.billing().portal()?;
    if json {
        return print_json(&serde_json::json!({ "url": url }));
    }
    if can_prompt() {
        println!(
            "Opening the customer portal: card, invoices, interval and cancelling.\n\n    {url}"
        );
        open_in_browser(&url);
    } else {
        println!("{url}");
    }
    Ok(())
}

/// Polls `GET /v1/me` until the plan is `done`. The success page grants
/// nothing: the plan changes when Localify Cloud sees the subscription.
fn wait_for_plan(
    cloud: &CloudAdapters,
    done: impl Fn(&Plan) -> bool,
    not_yet: &str,
) -> Result<Plan> {
    let billing = cloud.billing();
    match poll_plan(|| billing.plan(), done, PLAN_POLL, PLAN_WAIT) {
        Some(plan) => Ok(plan),
        None => bail!(
            "{not_yet}. Once the checkout is done, check with: {} whoami",
            BRANDING.cli_command()
        ),
    }
}

/// Reads the plan every `poll` until it is `done`, for `wait` at most;
/// `None` once `wait` is over. A failed read is retried: the checkout is
/// still open in the browser.
fn poll_plan(
    mut read: impl FnMut() -> Result<Option<Plan>>,
    done: impl Fn(&Plan) -> bool,
    poll: Duration,
    wait: Duration,
) -> Option<Plan> {
    let deadline = Instant::now() + wait;
    loop {
        thread::sleep(poll);
        if let Ok(Some(plan)) = read()
            && done(&plan)
        {
            return Some(plan);
        }
        if Instant::now() >= deadline {
            return None;
        }
    }
}

/// One line, the way `/v1/me` gives the plan, such as
/// `Pro (trial), ends 14 Nov 2026 · 1 of 2 serving machines · 3 of 10 URLs`.
fn plan_line(plan: &Plan) -> String {
    let cli = BRANDING.cli_command();
    let date = |value: &Option<String>| value.as_deref().map(short_date).unwrap_or_default();
    let interval = match plan.interval {
        Some(PlanInterval::Year) => "yearly",
        _ => "monthly",
    };
    let state = match plan.status {
        PlanStatus::None if plan.trial_available => return no_plan_line(),
        PlanStatus::None => {
            return format!(
                "No plan: public URLs need Localify Cloud Pro. Subscribe with {cli} subscribe."
            );
        }
        PlanStatus::Trialing => format!("Pro (trial), ends {}", date(&plan.trial_ends_at)),
        PlanStatus::Active => match &plan.renews_at {
            Some(_) => format!("Pro ({interval}), renews {}", date(&plan.renews_at)),
            None => format!("Pro ({interval})"),
        },
        PlanStatus::Canceling => format!("Pro ({interval}), ending {}", date(&plan.ends_at)),
        PlanStatus::PastDue if plan.entitled => {
            format!("Pro ({interval}): the payment failed, update the card with {cli} billing")
        }
        PlanStatus::PastDue => {
            format!("Pro paused: the payment failed. Update the card with {cli} billing")
        }
        PlanStatus::Override => match &plan.ends_at {
            Some(_) => format!("Team access, until {}", date(&plan.ends_at)),
            None => "Team access".to_string(),
        },
    };
    format!(
        "{state} · {} of {} serving machines · {} of {} URLs",
        plan.usage.serving_machines,
        plan.limits.serving_machines,
        plan.usage.public_urls,
        plan.limits.public_urls
    )
}

/// `14 Nov 2026` from an RFC 3339 time, in UTC; the value itself when it
/// does not read as one.
fn short_date(value: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = value.get(..10).unwrap_or_default().split('-');
    let (Some(year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return value.to_string();
    };
    let month = month
        .parse::<usize>()
        .ok()
        .and_then(|month| MONTHS.get(month.checked_sub(1)?));
    match (month, day.parse::<u8>()) {
        (Some(month), Ok(day)) if year.len() == 4 => format!("{day} {month} {year}"),
        _ => value.to_string(),
    }
}

/// Why this machine serves nothing although it has URLs, and what to do.
fn blocked_line(tunnel: &TunnelStatus, cloud: &CloudAdapters) -> Option<String> {
    if tunnel.runtime != Some(RuntimeState::Blocked) {
        return None;
    }
    let blocked = tunnel.blocked.as_ref()?;
    let cli = BRANDING.cli_command();
    let next = match blocked.reason.as_str() {
        "subscription_required" => no_plan_line(),
        "serving_slots_full" => {
            let holders = slot_holders(cloud);
            let holders = if holders.is_empty() {
                "your other machines".to_string()
            } else {
                holders.join(", ")
            };
            let raise = blocked
                .upgrade
                .as_ref()
                .map(|upgrade| format!(" {}", raise_limit_line(upgrade)))
                .unwrap_or_default();
            format!(
                "Every serving slot is taken, by {holders}. Remove a URL one of them serves, or serve it from here: {cli} public-url claim <url>{raise}"
            )
        }
        "capacity" => {
            "Localify Cloud is full for now: serving resumes by itself as tunnels free up."
                .to_string()
        }
        _ => String::new(),
    };
    Some(
        format!("Public URLs: not served. {} {next}", blocked.message)
            .trim_end()
            .to_string(),
    )
}

/// The other machines serving this account's URLs: the ones holding its
/// serving slots. Best effort, since it only helps the sentence.
fn slot_holders(cloud: &CloudAdapters) -> Vec<String> {
    let mut names: Vec<String> = cloud
        .public_urls()
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter(|view| !view.served_here && view.url.state == Some(PublicUrlState::Serving))
        .filter_map(|view| view.url.machine.map(|machine| machine.name))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// After a change: what the daemon did with it, when that needs saying.
pub fn print_refresh(refresh: &TunnelRefresh, json: bool) {
    match refresh {
        TunnelRefresh::Done => {}
        TunnelRefresh::DaemonStopped => note(
            json,
            &format!(
                "The daemon is not running: the tunnel starts with it. Start it with: {}",
                BRANDING.admin_command("start")
            ),
        ),
        TunnelRefresh::Failed(reason) => eprintln!(
            "Warning: the daemon did not take the change ({reason}). It checks again within 5 minutes, or restart it: {}",
            BRANDING.admin_command("restart")
        ),
    }
}

/// The code a person needs, even when stdout is for JSON.
fn print_code(device: &DeviceAuthorization, json: bool) {
    let minutes = device.expires_in.div_ceil(60);
    note(
        json,
        &format!(
            "To sign in, open {} and enter the code:\n\n    {}\n\nWaiting for GitHub (the code expires in {minutes} min)...",
            device.verification_uri, device.user_code
        ),
    );
}

fn print_signed_in(status: &AccountStatus, refresh: &TunnelRefresh, json: bool) -> Result<()> {
    if json {
        print_json(status)?;
    } else {
        println!(
            "Signed in as {}, on the machine {}.",
            status.login.as_deref().unwrap_or_default(),
            status.machine_name.as_deref().unwrap_or_default()
        );
        println!(
            "Give a project domain a public URL with: {} public-url create <local-domain>",
            BRANDING.cli_command()
        );
    }
    print_refresh(refresh, json);
    Ok(())
}

fn tunnel_line(status: &AccountStatus) -> String {
    let tunnel = &status.tunnel;
    match tunnel.state {
        TunnelState::Connected => format!(
            "Tunnel: connected to Cloudflare ({} connections).",
            tunnel.connections
        ),
        TunnelState::Starting => "Tunnel: starting.".to_string(),
        TunnelState::Stopped => {
            "Tunnel: stopped. It runs while this machine serves a public URL.".to_string()
        }
        TunnelState::Error => format!(
            "Tunnel: failed. {}",
            tunnel.error.as_deref().unwrap_or("See the daemon logs.")
        ),
        TunnelState::Unknown => match &tunnel.error {
            Some(error) => format!("Tunnel: unknown. {error}"),
            None => format!(
                "Tunnel: unknown, the daemon is not running. Start it with: {}",
                BRANDING.admin_command("start")
            ),
        },
    }
}

/// Best effort: the code and URL are printed anyway, for a machine reached
/// over SSH or without a browser.
fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = Command::new(opener)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::{PlanAddons, PlanCounts};

    fn plan(status: PlanStatus) -> Plan {
        Plan {
            status,
            entitled: status != PlanStatus::None,
            interval: Some(PlanInterval::Month),
            trial_ends_at: Some("2026-11-14T10:00:00Z".into()),
            renews_at: Some("2026-12-14T10:00:00Z".into()),
            ends_at: None,
            trial_available: true,
            limits: PlanCounts {
                serving_machines: 2,
                public_urls: 10,
            },
            usage: PlanCounts {
                serving_machines: 1,
                public_urls: 3,
            },
            addons: PlanAddons::default(),
        }
    }

    #[test]
    fn the_plan_reads_as_the_contract_shows_it() {
        assert_eq!(
            plan_line(&plan(PlanStatus::Trialing)),
            "Pro (trial), ends 14 Nov 2026 · 1 of 2 serving machines · 3 of 10 URLs"
        );
        assert_eq!(plan_line(&plan(PlanStatus::None)), no_plan_line());
        assert!(
            plan_line(&plan(PlanStatus::Active)).starts_with("Pro (monthly), renews 14 Dec 2026")
        );
        let mut used = plan(PlanStatus::None);
        used.trial_available = false;
        assert!(plan_line(&used).contains("subscribe") && !plan_line(&used).contains("trial"));
        assert!(plan_line(&plan(PlanStatus::Override)).starts_with("Team access ·"));
    }

    #[test]
    fn an_addon_waits_for_the_limit_it_raises() {
        let mut plan = plan(PlanStatus::Active);
        assert_eq!(addon_limit(&plan, Addon::Machine), 2);
        plan.limits.public_urls = 30;
        assert_eq!(addon_limit(&plan, Addon::Urls), 30);
    }

    #[test]
    fn polling_stops_once_the_new_limit_shows() {
        let raised = {
            let mut plan = plan(PlanStatus::Active);
            plan.limits.serving_machines = 4;
            plan
        };
        let mut answers = vec![
            Ok(Some(raised)),
            Ok(Some(plan(PlanStatus::Active))),
            Ok(None),
            Err(anyhow::anyhow!("offline")),
        ];
        let mut reads = 0;

        let plan = poll_plan(
            || {
                reads += 1;
                answers.pop().unwrap()
            },
            |plan| addon_limit(plan, Addon::Machine) != 2,
            Duration::ZERO,
            Duration::from_secs(60),
        );

        assert_eq!(plan.map(|plan| plan.limits.serving_machines), Some(4));
        assert_eq!(
            reads, 4,
            "failed and unchanged reads are retried, then it stops"
        );
    }

    #[test]
    fn polling_gives_up_when_the_wait_is_over() {
        let mut reads = 0;

        let plan = poll_plan(
            || {
                reads += 1;
                Ok(Some(plan(PlanStatus::Active)))
            },
            |plan| addon_limit(plan, Addon::Machine) != 2,
            Duration::from_millis(1),
            Duration::from_millis(20),
        );

        assert_eq!(plan, None);
        assert!(reads >= 1);
    }

    #[test]
    fn addon_units_read_as_what_they_buy() {
        let units = |n| AddonUnits::new(n).unwrap();
        assert_eq!(
            addon_units_label(Addon::Machine, units(1)),
            "1 more serving machine"
        );
        assert_eq!(
            addon_units_label(Addon::Machine, units(3)),
            "3 more serving machines"
        );
        assert_eq!(
            addon_units_label(Addon::Urls, units(2)),
            "2 packs of 10 public URLs (20 URLs)"
        );
    }

    #[test]
    fn dates_read_as_day_month_year() {
        assert_eq!(short_date("2026-11-04T23:00:00Z"), "4 Nov 2026");
        assert_eq!(short_date("soon"), "soon");
        assert_eq!(short_date("2026-13-04"), "2026-13-04");
    }
}
