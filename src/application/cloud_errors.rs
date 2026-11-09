//! What to do next, for the errors of the Localify Cloud API a person can act
//! on (`docs/public-urls.md`, "API").

use anyhow::anyhow;

use super::ports::{CloudApiError, UpgradeHint};
use crate::branding::BRANDING;

/// Where to appeal a ban or a takedown.
pub const ABUSE_CONTACT: &str = "abuse@localify.dev";

/// What a person reads when the account has no plan: on `402
/// subscription_required`, and in `whoami`.
pub fn no_plan_line() -> String {
    format!(
        "No plan: public URLs need Localify Cloud Pro. Start a 7-day trial with {} subscribe.",
        BRANDING.cli_command()
    )
}

/// How the sentence that offers to raise a limit starts, so the MCP server
/// can tell an agent the purchase is the user's call.
pub const RAISE_LIMIT: &str = "Or raise the limit with:";

/// What lifts a refusal the account can buy its way out of, with the command
/// Localify Cloud names for it, as it names it.
pub fn raise_limit_line(upgrade: &UpgradeHint) -> String {
    format!("{RAISE_LIMIT} {}", upgrade.command)
}

/// The API's sentence, and the command its `upgrade` names unless the
/// sentence already gives it.
pub fn with_upgrade_command(message: &str, upgrade: Option<&UpgradeHint>) -> String {
    match upgrade {
        Some(upgrade) if !message.contains(&upgrade.command) => {
            format!("{message} Run: {}", upgrade.command)
        }
        _ => message.to_string(),
    }
}

/// The API's sentence, with what to do next when the code calls for it.
pub fn with_next_step(error: CloudApiError) -> anyhow::Error {
    let cli = BRANDING.cli_command();
    let CloudApiError::Rejected {
        code,
        message,
        upgrade,
        ..
    } = &error
    else {
        return error.into();
    };
    match code.as_str() {
        "not_found" => anyhow!("{message} List your public URLs with: {cli} public-url list"),
        "local_domain_taken" => {
            anyhow!("{message} Find the URL it already has with: {cli} public-url list")
        }
        "invalid_local_domain" => anyhow!("{message} Check the name with: {cli} list"),
        "subscription_required" => anyhow!(no_plan_line()),
        "banned" => anyhow!(
            "Localify Cloud suspended this account for breaking its terms. To appeal, write to {ABUSE_CONTACT}."
        ),
        "taken_down" => anyhow!(
            "{message} It was removed for abuse and cannot serve again. To appeal, write to {ABUSE_CONTACT}."
        ),
        "quota_exceeded" => {
            let raise = upgrade
                .as_ref()
                .map(|upgrade| format!(" {}.", raise_limit_line(upgrade)))
                .unwrap_or_default();
            anyhow!(
                "{message} To make room, remove what you no longer use: a URL with `{cli} public-url remove <url>`, a machine with `{cli} logout` on it.{raise} `{cli} whoami` shows the plan's limits."
            )
        }
        "capacity" => anyhow!(
            "{message} Localify Cloud is full for now: serving resumes by itself as tunnels free up, nothing to do."
        ),
        _ => error.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(status: u16, code: &str) -> CloudApiError {
        CloudApiError::Rejected {
            status,
            code: code.into(),
            message: "The API's sentence.".into(),
            upgrade: None,
        }
    }

    fn billing_hint() -> UpgradeHint {
        UpgradeHint {
            kind: "addon_urls".into(),
            command: "localify billing".into(),
        }
    }

    #[test]
    fn each_refusal_says_what_to_do() {
        let cli = BRANDING.cli_command();
        let text = |status, code| with_next_step(rejected(status, code)).to_string();

        assert_eq!(text(402, "subscription_required"), no_plan_line());
        assert!(no_plan_line().contains(&format!("{cli} subscribe")));
        assert!(text(403, "banned").contains(ABUSE_CONTACT));
        assert!(text(403, "taken_down").contains(ABUSE_CONTACT));
        assert!(text(403, "quota_exceeded").contains(&format!("{cli} public-url remove")));
        assert!(text(503, "capacity").contains("resumes by itself"));
        assert_eq!(text(500, "internal"), "The API's sentence.");
    }

    #[test]
    fn a_quota_names_the_upgrade_only_when_the_api_offers_one() {
        let plain = with_next_step(rejected(403, "quota_exceeded")).to_string();
        assert!(!plain.contains("raise the limit"), "{plain}");

        let mut offered = rejected(403, "quota_exceeded");
        if let CloudApiError::Rejected { upgrade, .. } = &mut offered {
            *upgrade = Some(billing_hint());
        }
        let text = with_next_step(offered).to_string();
        assert!(text.starts_with("The API's sentence."), "{text}");
        assert!(
            text.contains("Or raise the limit with: localify billing."),
            "{text}"
        );
    }

    #[test]
    fn the_upgrade_command_is_added_once() {
        let hint = billing_hint();
        assert_eq!(
            with_upgrade_command("Not on sale.", Some(&hint)),
            "Not on sale. Run: localify billing"
        );
        assert_eq!(
            with_upgrade_command("Run `localify billing` first.", Some(&hint)),
            "Run `localify billing` first."
        );
        assert_eq!(with_upgrade_command("Not on sale.", None), "Not on sale.");
    }
}
