use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DomainName(String);

#[derive(Debug, thiserror::Error)]
pub enum DomainNameError {
    #[error("Domain name too short: {0}")]
    TooShort(String),

    #[error("Domain name contains invalid characters: {0}")]
    InvalidCharacters(String),

    #[error("Domain name has an invalid label: {0}")]
    InvalidLabel(String),
}

impl DomainName {
    pub fn new(name: impl Into<String>) -> Result<Self, DomainNameError> {
        let name = name.into().trim().trim_end_matches('.').to_lowercase();

        if name.len() < 3 || !name.contains('.') {
            return Err(DomainNameError::TooShort(name));
        }

        if name.len() > 253
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        {
            return Err(DomainNameError::InvalidCharacters(name));
        }

        for label in name.split('.') {
            if label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
            {
                return Err(DomainNameError::InvalidLabel(name));
            }
        }

        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DomainName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl serde::Serialize for DomainName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for DomainName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        DomainName::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_domain_names() {
        assert!(DomainName::new("app.localify").is_ok());
        assert!(DomainName::new("my-app.localify").is_ok());
        assert!(DomainName::new("sub.domain.localify").is_ok());
        assert!(DomainName::new("dev.store.fr").is_ok());
        assert!(DomainName::new("tenant.dev.store.fr").is_ok());
        assert!(DomainName::new("APP.LOCALIFY").is_ok());
    }

    #[test]
    fn test_invalid_domain_names() {
        assert!(DomainName::new("app").is_err());
        assert!(DomainName::new(".localify").is_err());
        assert!(DomainName::new("-app.localify").is_err());
        assert!(DomainName::new("app-.localify").is_err());
        assert!(DomainName::new("app_name.localify").is_err());
        assert!(DomainName::new("app..localify").is_err());
    }
}
