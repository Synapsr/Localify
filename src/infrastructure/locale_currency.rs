//! The currency to show prices in when Localify Cloud cannot say which.

use crate::application::ports::Currency;

/// The currency Localify Cloud would pick for this machine's region, from
/// its locale: euros in the European Economic Area, dollars elsewhere, as
/// `GET /v1/pricing` picks them from the country. Only for the list prices
/// shown while that route cannot answer.
pub fn local_currency() -> Currency {
    let locale = macos_locale()
        .or_else(|| std::env::var("LC_ALL").ok())
        .or_else(|| std::env::var("LANG").ok())
        .unwrap_or_default();
    currency_for_locale(&locale)
}

/// The EU's 27 countries, Iceland, Liechtenstein and Norway, as the cloud's
/// `pricing.ts` lists them.
const EURO_REGIONS: [&str; 30] = [
    "AT", "BE", "BG", "HR", "CY", "CZ", "DK", "EE", "FI", "FR", "DE", "GR", "HU", "IE", "IT", "LV",
    "LT", "LU", "MT", "NL", "PL", "PT", "RO", "SK", "SI", "ES", "SE", "IS", "LI", "NO",
];

/// `fr_FR`, `en_FR@currency=EUR` or `de_DE.UTF-8`: the region follows `_`.
fn currency_for_locale(locale: &str) -> Currency {
    let region = locale
        .split(['@', '.'])
        .next()
        .and_then(|name| name.rsplit_once(['_', '-']))
        .map(|(_, region)| region.to_ascii_uppercase());
    match region {
        Some(region) if EURO_REGIONS.contains(&region.as_str()) => Currency::Eur,
        _ => Currency::Usd,
    }
}

/// A macOS app gets no `LANG`: the region is in the user's defaults.
#[cfg(target_os = "macos")]
fn macos_locale() -> Option<String> {
    let output = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .ok()?;
    let locale = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !locale.is_empty()).then_some(locale)
}

#[cfg(not(target_os = "macos"))]
fn macos_locale() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_currency_follows_the_locale_region() {
        assert_eq!(currency_for_locale("fr_FR"), Currency::Eur);
        assert_eq!(currency_for_locale("en_DE@currency=EUR"), Currency::Eur);
        assert_eq!(currency_for_locale("nb_NO.UTF-8"), Currency::Eur);
        assert_eq!(currency_for_locale("en_US.UTF-8"), Currency::Usd);
        assert_eq!(currency_for_locale("en_GB"), Currency::Usd);
        assert_eq!(currency_for_locale("de_CH"), Currency::Usd);
        assert_eq!(currency_for_locale("C"), Currency::Usd);
        assert_eq!(currency_for_locale(""), Currency::Usd);
    }
}
