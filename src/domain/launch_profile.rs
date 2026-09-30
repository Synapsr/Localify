//! Launch profiles choose where a service's variables come from at launch.
//!
//! A launch profile never changes the service, its domain or its port. It only
//! adds variables to the process Localify starts, below the variables Localify
//! needs for routing and below what the user exported in their shell.

use std::fmt;

/// Name of a launch profile, as typed after `--profile`.
///
/// Restricted to lowercase letters, digits, `-` and `_` because the name is
/// shown in an HTTP header, a terminal title and an environment variable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LaunchProfileName(String);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LaunchProfileNameError {
    #[error(
        "Launch profile name '{0}' is invalid: use 1 to 32 lowercase letters, digits, '-' or '_', starting with a letter or digit"
    )]
    Invalid(String),
}

impl LaunchProfileName {
    /// Profile used when `--profile` is omitted. It can never be dangerous.
    pub const DEFAULT: &'static str = "default";

    pub fn new(value: &str) -> Result<Self, LaunchProfileNameError> {
        let valid_start = value
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        let valid_chars = value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'));

        if !valid_start || !valid_chars || value.len() > 32 {
            return Err(LaunchProfileNameError::Invalid(value.to_string()));
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_default(&self) -> bool {
        self.0 == Self::DEFAULT
    }
}

impl fmt::Display for LaunchProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Variables Localify owns for routing and profile signaling.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuntimeVariables {
    /// Always set on the service, even over the shell.
    pub set: Vec<(String, String)>,
    /// Removed from the service environment so a value inherited from a
    /// parent launch cannot leak into this one.
    pub unset: Vec<String>,
}

/// Environment changes to apply to the service process, plus what the user
/// should know about keys that lost a conflict. Only key names are kept for
/// reporting, never values.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LaunchEnvironment {
    pub set: Vec<(String, String)>,
    pub unset: Vec<String>,
    /// Profile keys the shell already defines; the shell value wins.
    pub shadowed_by_shell: Vec<String>,
    /// Profile keys Localify rewrites so routing keeps working.
    pub replaced_by_runtime: Vec<String>,
}

/// Layers the service environment, strongest first:
///
/// 1. Localify runtime variables (`PORT`, `HOST`, URL variables, CA, profile
///    markers). A profile must never break routing.
/// 2. Variables already exported in the shell, inherited untouched.
/// 3. Profile variables, already merged so the first source wins.
/// 4. `defaults`, set only when nothing above defines them.
///
/// Files the framework loads itself come last by construction: Next.js, Vite
/// and dotenv never override a variable already present in the process.
pub fn layer_environment(
    runtime: &RuntimeVariables,
    shell_defines: impl Fn(&str) -> bool,
    profile: &[(String, String)],
    defaults: &[(String, String)],
) -> LaunchEnvironment {
    let runtime_defines = |key: &str| runtime.set.iter().any(|(existing, _)| existing == key);
    let mut environment = LaunchEnvironment {
        set: runtime.set.clone(),
        unset: runtime.unset.clone(),
        ..LaunchEnvironment::default()
    };

    for (key, value) in profile {
        if runtime_defines(key) {
            environment.replaced_by_runtime.push(key.clone());
        } else if shell_defines(key) {
            environment.shadowed_by_shell.push(key.clone());
        } else {
            environment.set.push((key.clone(), value.clone()));
        }
    }

    for (key, value) in defaults {
        let profile_defines = profile.iter().any(|(existing, _)| existing == key);
        if !runtime_defines(key) && !shell_defines(key) && !profile_defines {
            environment.set.push((key.clone(), value.clone()));
        }
    }

    environment
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
        values
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn value<'a>(environment: &'a LaunchEnvironment, key: &str) -> Option<&'a str> {
        environment
            .set
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn profile_names_are_validated() {
        assert!(LaunchProfileName::new("prod").is_ok());
        assert!(LaunchProfileName::new("staging-eu_1").is_ok());
        assert!(LaunchProfileName::new("Prod").is_err());
        assert!(LaunchProfileName::new("-prod").is_err());
        assert!(LaunchProfileName::new("").is_err());
        assert!(LaunchProfileName::new("prod env").is_err());
        assert!(LaunchProfileName::new(&"a".repeat(33)).is_err());
        assert!(LaunchProfileName::new("default").unwrap().is_default());
    }

    #[test]
    fn runtime_variables_win_over_shell_and_profile() {
        let runtime = RuntimeVariables {
            set: pairs(&[
                ("PORT", "3000"),
                ("NEXT_PUBLIC_APP_URL", "https://web.localify"),
            ]),
            unset: vec!["LOCALIFY_DANGER".into()],
        };
        let profile = pairs(&[
            ("PORT", "80"),
            ("NEXT_PUBLIC_APP_URL", "https://prod.example"),
        ]);

        let environment = layer_environment(&runtime, |key| key == "PORT", &profile, &[]);

        assert_eq!(value(&environment, "PORT"), Some("3000"));
        assert_eq!(
            value(&environment, "NEXT_PUBLIC_APP_URL"),
            Some("https://web.localify")
        );
        assert_eq!(
            environment.replaced_by_runtime,
            vec!["PORT".to_string(), "NEXT_PUBLIC_APP_URL".to_string()]
        );
        assert_eq!(environment.unset, vec!["LOCALIFY_DANGER".to_string()]);
    }

    #[test]
    fn shell_variables_win_over_profile() {
        let profile = pairs(&[("DATABASE_URL", "postgres://prod"), ("API_KEY", "k")]);

        let environment = layer_environment(
            &RuntimeVariables::default(),
            |key| key == "DATABASE_URL",
            &profile,
            &[],
        );

        assert_eq!(value(&environment, "DATABASE_URL"), None);
        assert_eq!(value(&environment, "API_KEY"), Some("k"));
        assert_eq!(
            environment.shadowed_by_shell,
            vec!["DATABASE_URL".to_string()]
        );
    }

    #[test]
    fn profile_variables_are_set_so_framework_files_cannot_override_them() {
        let profile = pairs(&[("DATABASE_URL", "postgres://prod")]);

        let environment = layer_environment(&RuntimeVariables::default(), |_| false, &profile, &[]);

        assert_eq!(value(&environment, "DATABASE_URL"), Some("postgres://prod"));
    }

    #[test]
    fn defaults_apply_only_when_nothing_else_defines_the_key() {
        let defaults = pairs(&[("FORCE_COLOR", "1"), ("NO_COLOR_HINT", "x")]);
        let profile = pairs(&[("NO_COLOR_HINT", "profile")]);

        let environment =
            layer_environment(&RuntimeVariables::default(), |_| false, &profile, &defaults);
        assert_eq!(value(&environment, "FORCE_COLOR"), Some("1"));
        assert_eq!(value(&environment, "NO_COLOR_HINT"), Some("profile"));

        let shell_colored = layer_environment(
            &RuntimeVariables::default(),
            |key| key == "FORCE_COLOR",
            &[],
            &defaults,
        );
        assert_eq!(value(&shell_colored, "FORCE_COLOR"), None);
    }
}
