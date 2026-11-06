use std::fmt;

/// The name of a public URL: the first label of `<slug>.localify.fyi`.
///
/// The first `--` of a public host separates the slug from an environment
/// label (`<slug>--<label>`), so a slug never contains `--` and never ends with
/// `-`. It starts with a letter, so `--` never lands at positions 3 and 4 of
/// the DNS label, which IDNA reserves (ADR 0022).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PublicSlug(String);

/// The longest a public DNS label may be.
const MAX_DNS_LABEL: usize = 63;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PublicSlugError {
    #[error("The slug '{0}' must be 3 to 32 characters long.")]
    Length(String),
    #[error("The slug '{0}' may only use the letters a-z, the digits 0-9 and '-'.")]
    Characters(String),
    #[error("The slug '{0}' must start with a letter.")]
    Start(String),
    #[error("The slug '{0}' cannot end with '-'.")]
    End(String),
    #[error("The slug '{0}' cannot contain '--', which separates a slug from an environment.")]
    DoubleHyphen(String),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EnvironmentLabelError {
    #[error(
        "'{0}' is not a single DNS label of a-z, 0-9 and '-' that starts and ends with a letter or digit."
    )]
    Invalid(String),
    #[error("'{public_label}' is longer than the 63 characters a DNS label allows.")]
    TooLong { public_label: String },
}

impl PublicSlug {
    /// Accepts upper case and surrounding spaces, as a person may type them.
    pub fn new(value: &str) -> Result<Self, PublicSlugError> {
        let slug = value.trim().to_ascii_lowercase();
        if !(3..=32).contains(&slug.len()) {
            return Err(PublicSlugError::Length(slug));
        }
        if !slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(PublicSlugError::Characters(slug));
        }
        if !slug.starts_with(|c: char| c.is_ascii_lowercase()) {
            return Err(PublicSlugError::Start(slug));
        }
        if slug.ends_with('-') {
            return Err(PublicSlugError::End(slug));
        }
        if slug.contains("--") {
            return Err(PublicSlugError::DoubleHyphen(slug));
        }
        Ok(Self(slug))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The public DNS label for an environment label under a wildcard URL,
    /// `<slug>--<label>`. Names that do not fit are refused, never truncated:
    /// a truncated name would point at another environment.
    pub fn environment_label(&self, label: &str) -> Result<String, EnvironmentLabelError> {
        let valid = !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if !valid {
            return Err(EnvironmentLabelError::Invalid(label.to_string()));
        }
        let public_label = format!("{}--{label}", self.0);
        if public_label.len() > MAX_DNS_LABEL {
            return Err(EnvironmentLabelError::TooLong { public_label });
        }
        Ok(public_label)
    }
}

impl fmt::Display for PublicSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_generated_slug_and_normalizes_case() {
        assert_eq!(
            PublicSlug::new("journal-a8f3").unwrap().as_str(),
            "journal-a8f3"
        );
        assert_eq!(PublicSlug::new("  Journal  ").unwrap().as_str(), "journal");
    }

    #[test]
    fn length_is_3_to_32() {
        assert!(PublicSlug::new("abc").is_ok());
        assert!(PublicSlug::new(&"a".repeat(32)).is_ok());
        assert!(matches!(
            PublicSlug::new("ab"),
            Err(PublicSlugError::Length(_))
        ));
        assert!(matches!(
            PublicSlug::new(&"a".repeat(33)),
            Err(PublicSlugError::Length(_))
        ));
    }

    #[test]
    fn refuses_what_breaks_the_public_host() {
        assert!(matches!(
            PublicSlug::new("my_app"),
            Err(PublicSlugError::Characters(_))
        ));
        assert!(matches!(
            PublicSlug::new("my.app"),
            Err(PublicSlugError::Characters(_))
        ));
        assert!(matches!(
            PublicSlug::new("1app"),
            Err(PublicSlugError::Start(_))
        ));
        assert!(matches!(
            PublicSlug::new("-app"),
            Err(PublicSlugError::Start(_))
        ));
        assert!(matches!(
            PublicSlug::new("app-"),
            Err(PublicSlugError::End(_))
        ));
        assert!(matches!(
            PublicSlug::new("my--app"),
            Err(PublicSlugError::DoubleHyphen(_))
        ));
    }

    #[test]
    fn environment_labels_may_contain_a_double_hyphen() {
        let slug = PublicSlug::new("journal-a8f3").unwrap();
        assert_eq!(
            slug.environment_label("web--feature").unwrap(),
            "journal-a8f3--web--feature"
        );
    }

    #[test]
    fn environment_labels_are_refused_rather_than_truncated() {
        let slug = PublicSlug::new(&format!("a{}", "b".repeat(31))).unwrap();
        assert!(slug.environment_label(&"c".repeat(29)).is_ok());
        assert_eq!(
            slug.environment_label(&"c".repeat(30)),
            Err(EnvironmentLabelError::TooLong {
                public_label: format!("{slug}--{}", "c".repeat(30)),
            })
        );
        assert!(matches!(
            slug.environment_label("feat.web"),
            Err(EnvironmentLabelError::Invalid(_))
        ));
        assert!(matches!(
            slug.environment_label("-feat"),
            Err(EnvironmentLabelError::Invalid(_))
        ));
    }
}
