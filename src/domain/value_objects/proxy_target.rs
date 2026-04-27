use super::port::{Port, PortError};
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyTarget {
    scheme: ProxyTargetScheme,
    host: String,
    port: Port,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyTargetScheme {
    Http,
    Https,
}

#[derive(Debug, Error)]
pub enum ProxyTargetError {
    #[error("Invalid port: {0}")]
    InvalidPort(#[from] PortError),

    #[error("Invalid port number: {0}")]
    ParsePort(#[from] std::num::ParseIntError),

    #[error("Empty target string")]
    Empty,

    #[error("Unsupported proxy target scheme: {0}")]
    UnsupportedScheme(String),

    #[error("Invalid proxy target: expected host:port, got '{0}'")]
    InvalidHostPort(String),
}

impl ProxyTarget {
    pub fn new(host: impl Into<String>, port: Port) -> Self {
        Self {
            scheme: ProxyTargetScheme::Http,
            host: host.into(),
            port,
        }
    }

    pub fn with_scheme(scheme: ProxyTargetScheme, host: impl Into<String>, port: Port) -> Self {
        Self {
            scheme,
            host: host.into(),
            port,
        }
    }

    pub fn localhost(port: Port) -> Self {
        Self::new("127.0.0.1", port)
    }

    /// Parse from string: "3000", "hostname:3000", or "https://hostname:3000".
    pub fn parse(s: &str) -> Result<Self, ProxyTargetError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ProxyTargetError::Empty);
        }

        if let Some((scheme, rest)) = s.split_once("://") {
            let scheme = ProxyTargetScheme::parse(scheme)?;
            let (host, port) = parse_host_port(rest)?;
            return Ok(Self::with_scheme(scheme, host, port));
        }

        // Try to split by colon
        if let Some((host_part, port_str)) = s.rsplit_once(':') {
            // Check if host_part looks like a hostname/IP (contains letters or dots)
            // This distinguishes "192.168.1.50:3000" from just "3000"
            if host_part.chars().any(|c| c.is_alphabetic() || c == '.') {
                let port = port_str.parse::<u16>()?;
                return Ok(Self::new(host_part, Port::new(port)?));
            }
        }

        // Just a port number
        let port = s.parse::<u16>()?;
        Ok(Self::localhost(Port::new(port)?))
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> Port {
        self.port
    }

    pub fn scheme(&self) -> ProxyTargetScheme {
        self.scheme
    }

    pub fn uri_scheme(&self) -> &'static str {
        self.scheme.as_str()
    }
}

impl ProxyTargetScheme {
    fn parse(value: &str) -> Result<Self, ProxyTargetError> {
        match value {
            "http" => Ok(Self::Http),
            "https" => Ok(Self::Https),
            other => Err(ProxyTargetError::UnsupportedScheme(other.to_string())),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

fn parse_host_port(s: &str) -> Result<(String, Port), ProxyTargetError> {
    let (host, port_str) = s
        .rsplit_once(':')
        .ok_or_else(|| ProxyTargetError::InvalidHostPort(s.to_string()))?;
    if host.trim().is_empty() {
        return Err(ProxyTargetError::InvalidHostPort(s.to_string()));
    }
    let port = port_str.parse::<u16>()?;
    Ok((host.trim().to_string(), Port::new(port)?))
}

impl fmt::Display for ProxyTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.scheme {
            ProxyTargetScheme::Http => write!(f, "{}:{}", self.host, self.port),
            ProxyTargetScheme::Https => write!(f, "https://{}:{}", self.host, self.port),
        }
    }
}

impl Serialize for ProxyTarget {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize as "host:port" string
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ProxyTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        ProxyTarget::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_port_only() {
        let target = ProxyTarget::parse("3000").unwrap();
        assert_eq!(target.host(), "127.0.0.1");
        assert_eq!(target.port().value(), 3000);
    }

    #[test]
    fn test_parse_ip_port() {
        let target = ProxyTarget::parse("192.168.1.50:3000").unwrap();
        assert_eq!(target.host(), "192.168.1.50");
        assert_eq!(target.port().value(), 3000);
    }

    #[test]
    fn test_parse_hostname_port() {
        let target = ProxyTarget::parse("localhost:8080").unwrap();
        assert_eq!(target.host(), "localhost");
        assert_eq!(target.port().value(), 8080);
        assert_eq!(target.scheme(), ProxyTargetScheme::Http);
    }

    #[test]
    fn test_parse_https_target() {
        let target = ProxyTarget::parse("https://localhost:8443").unwrap();
        assert_eq!(target.host(), "localhost");
        assert_eq!(target.port().value(), 8443);
        assert_eq!(target.scheme(), ProxyTargetScheme::Https);
        assert_eq!(target.to_string(), "https://localhost:8443");
    }

    #[test]
    fn test_parse_http_target_is_normalized_like_legacy_targets() {
        let target = ProxyTarget::parse("http://localhost:8080").unwrap();
        assert_eq!(target.host(), "localhost");
        assert_eq!(target.port().value(), 8080);
        assert_eq!(target.scheme(), ProxyTargetScheme::Http);
        assert_eq!(target.to_string(), "localhost:8080");
    }

    #[test]
    fn test_reject_unsupported_scheme() {
        assert!(matches!(
            ProxyTarget::parse("ftp://localhost:3000"),
            Err(ProxyTargetError::UnsupportedScheme(_))
        ));
    }

    #[test]
    fn test_display() {
        let target = ProxyTarget::parse("3000").unwrap();
        assert_eq!(target.to_string(), "127.0.0.1:3000");

        let target = ProxyTarget::parse("192.168.1.50:8080").unwrap();
        assert_eq!(target.to_string(), "192.168.1.50:8080");
    }

    #[test]
    fn test_invalid_port() {
        assert!(ProxyTarget::parse("80").is_err()); // Privileged
        assert!(ProxyTarget::parse("0").is_err());
        assert!(ProxyTarget::parse("").is_err());
        assert!(ProxyTarget::parse("abc").is_err());
    }
}
