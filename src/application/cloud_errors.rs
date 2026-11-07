//! What to do next, for the errors of the Localify Cloud API a person can act
//! on (`docs/public-urls.md`, "API").

use anyhow::anyhow;

use super::ports::CloudApiError;
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

/// The API's sentence, with what to do next when the code calls for it.
pub fn with_next_step(error: CloudApiError) -> anyhow::Error {
    let cli = BRANDING.cli_command();
    let CloudApiError::Rejected { code, message, .. } = &error else {
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
        "quota_exceeded" => anyhow!(
            "{message} To make room, remove what you no longer use: a URL with `{cli} public-url remove <url>`, a machine with `{cli} logout` on it. `{cli} whoami` shows the plan's limits."
        ),
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
}
