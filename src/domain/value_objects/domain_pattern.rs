use std::fmt;

use super::domain_name::{DomainName, DomainNameError};

/// Value object representing how a domain is matched — either
/// exactly or as a wildcard pattern covering one-level subdomains.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DomainPattern {
    /// Matches only the exact domain (e.g. `myapp.localify`).
    Exact(DomainName),
    /// Matches the base domain **and** any single-level subdomain
    /// (e.g. `myapp.localify` + `*.myapp.localify`).
    Wildcard(DomainName),
}

impl DomainPattern {
    /// Build a `DomainPattern` from a raw domain name string and a
    /// wildcard flag. Validates the domain name and returns the
    /// appropriate variant.
    pub fn from_name(name: &str, wildcard: bool) -> Result<Self, DomainNameError> {
        let domain = DomainName::new(name)?;
        Ok(if wildcard {
            Self::Wildcard(domain)
        } else {
            Self::Exact(domain)
        })
    }

    /// The underlying base domain regardless of pattern type.
    pub fn base_domain(&self) -> &DomainName {
        match self {
            Self::Exact(d) | Self::Wildcard(d) => d,
        }
    }

    pub fn is_wildcard(&self) -> bool {
        matches!(self, Self::Wildcard(_))
    }

    /// Single source of truth for hostname matching.
    ///
    /// - `Exact` matches only when hostname equals the domain.
    /// - `Wildcard` matches the base domain itself **and**
    ///   any single-label subdomain (e.g. `blog.myapp.localify`
    ///   but **not** `a.b.myapp.localify`).
    pub fn matches_hostname(&self, hostname: &str) -> bool {
        match self {
            Self::Exact(domain) => hostname == domain.as_str(),
            Self::Wildcard(base) => {
                let base_str = base.as_str();
                if hostname == base_str {
                    return true;
                }

                let suffix = format!(".{}", base_str);
                if !hostname.ends_with(&suffix) {
                    return false;
                }

                // Only allow a single label before the base domain.
                let prefix = &hostname[..hostname.len() - suffix.len()];
                !prefix.is_empty() && !prefix.contains('.')
            }
        }
    }

    /// Human-readable display pattern (e.g. `*.myapp.localify`).
    pub fn display_pattern(&self) -> String {
        match self {
            Self::Exact(d) => d.as_str().to_string(),
            Self::Wildcard(d) => format!("*.{}", d.as_str()),
        }
    }

    /// Specificity score for "most specific wins" ordering.
    /// Longer base domains are more specific.
    pub fn specificity(&self) -> usize {
        self.base_domain().as_str().len()
    }
}

impl fmt::Display for DomainPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_pattern())
    }
}

impl serde::Serialize for DomainPattern {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.display_pattern())
    }
}

impl<'de> serde::Deserialize<'de> for DomainPattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        if let Some(base) = s.strip_prefix("*.") {
            let domain = DomainName::new(base).map_err(serde::de::Error::custom)?;
            Ok(DomainPattern::Wildcard(domain))
        } else {
            let domain = DomainName::new(&s).map_err(serde::de::Error::custom)?;
            Ok(DomainPattern::Exact(domain))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(name: &str) -> DomainPattern {
        DomainPattern::Exact(DomainName::new(name).unwrap())
    }

    fn wildcard(name: &str) -> DomainPattern {
        DomainPattern::Wildcard(DomainName::new(name).unwrap())
    }

    // --- from_name ---

    #[test]
    fn from_name_creates_exact_pattern() {
        let p = DomainPattern::from_name("myapp.localify", false).unwrap();
        assert_eq!(p, exact("myapp.localify"));
    }

    #[test]
    fn from_name_creates_wildcard_pattern() {
        let p = DomainPattern::from_name("myapp.localify", true).unwrap();
        assert_eq!(p, wildcard("myapp.localify"));
    }

    #[test]
    fn from_name_rejects_invalid_domain() {
        assert!(DomainPattern::from_name("invalid", false).is_err());
    }

    // --- matches_hostname ---

    #[test]
    fn exact_matches_same_hostname() {
        assert!(exact("myapp.localify").matches_hostname("myapp.localify"));
    }

    #[test]
    fn exact_does_not_match_subdomain() {
        assert!(!exact("myapp.localify").matches_hostname("blog.myapp.localify"));
    }

    #[test]
    fn exact_does_not_match_different_domain() {
        assert!(!exact("myapp.localify").matches_hostname("other.localify"));
    }

    #[test]
    fn wildcard_matches_base_domain() {
        assert!(wildcard("myapp.localify").matches_hostname("myapp.localify"));
    }

    #[test]
    fn wildcard_matches_single_level_subdomain() {
        assert!(wildcard("myapp.localify").matches_hostname("blog.myapp.localify"));
        assert!(wildcard("myapp.localify").matches_hostname("api.myapp.localify"));
    }

    #[test]
    fn wildcard_does_not_match_multi_level_subdomain() {
        assert!(!wildcard("myapp.localify").matches_hostname("a.b.myapp.localify"));
    }

    #[test]
    fn wildcard_does_not_match_unrelated_domain() {
        assert!(!wildcard("myapp.localify").matches_hostname("other.localify"));
    }

    #[test]
    fn wildcard_does_not_match_suffix_overlap() {
        assert!(!wildcard("myapp.localify").matches_hostname("notmyapp.localify"));
    }

    #[test]
    fn wildcard_does_not_match_empty_prefix() {
        assert!(!wildcard("myapp.localify").matches_hostname(".myapp.localify"));
    }

    // --- display_pattern ---

    #[test]
    fn exact_display_pattern() {
        assert_eq!(exact("myapp.localify").display_pattern(), "myapp.localify");
    }

    #[test]
    fn wildcard_display_pattern() {
        assert_eq!(
            wildcard("myapp.localify").display_pattern(),
            "*.myapp.localify"
        );
    }

    // --- specificity ---

    #[test]
    fn longer_base_domain_is_more_specific() {
        let broad = wildcard("myapp.localify");
        let specific = wildcard("sub.myapp.localify");
        assert!(specific.specificity() > broad.specificity());
    }

    // --- Display trait ---

    #[test]
    fn display_trait_matches_display_pattern() {
        let p = wildcard("myapp.localify");
        assert_eq!(format!("{}", p), p.display_pattern());
    }

    // --- is_wildcard ---

    #[test]
    fn is_wildcard_returns_correctly() {
        assert!(!exact("myapp.localify").is_wildcard());
        assert!(wildcard("myapp.localify").is_wildcard());
    }

    // --- serde round-trip ---

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Wrapper {
        pattern: DomainPattern,
    }

    #[test]
    fn serde_roundtrip_exact() {
        let w = Wrapper {
            pattern: exact("myapp.localify"),
        };
        let toml_str = toml::to_string(&w).unwrap();
        let deserialized: Wrapper = toml::from_str(&toml_str).unwrap();
        assert_eq!(w, deserialized);
    }

    #[test]
    fn serde_roundtrip_wildcard() {
        let w = Wrapper {
            pattern: wildcard("myapp.localify"),
        };
        let toml_str = toml::to_string(&w).unwrap();
        let deserialized: Wrapper = toml::from_str(&toml_str).unwrap();
        assert_eq!(w, deserialized);
    }

    #[test]
    fn serde_deserializes_wildcard_from_string() {
        let toml_str = r#"pattern = "*.myapp.localify""#;
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(w.pattern, wildcard("myapp.localify"));
    }

    #[test]
    fn serde_deserializes_exact_from_string() {
        let toml_str = r#"pattern = "myapp.localify""#;
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(w.pattern, exact("myapp.localify"));
    }
}
