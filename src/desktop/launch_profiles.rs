//! Launch profile summaries for the desktop app, read from the same
//! `localify.toml` the CLI uses.

use std::path::Path;

use crate::desktop::dto::LaunchProfileInfoDto;
use crate::infrastructure::launch_profiles::LaunchProfiles;

/// Lists the launch profiles of a checkout. A broken `localify.toml` is
/// reported as an error string so the UI can show it next to the service.
pub fn environment_launch_profiles(
    root_path: Option<&str>,
) -> (Vec<LaunchProfileInfoDto>, Option<String>) {
    let Some(root_path) = root_path else {
        return (Vec::new(), None);
    };

    match LaunchProfiles::load(Path::new(root_path)) {
        Ok(Some(profiles)) => (
            profiles
                .profiles()
                .map(|profile| LaunchProfileInfoDto {
                    name: profile.name.to_string(),
                    danger: profile.danger,
                    max_duration: profile
                        .max_duration
                        .map(|duration| humantime::format_duration(duration).to_string()),
                })
                .collect(),
            None,
        ),
        Ok(None) => (Vec::new(), None),
        Err(error) => (Vec::new(), Some(format!("{error:#}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_profiles_and_reports_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        assert_eq!(environment_launch_profiles(Some(root)), (Vec::new(), None));

        std::fs::write(
            dir.path().join("localify.toml"),
            "[profiles.prod]\ndanger = true\nmax_duration = \"4h\"\n",
        )
        .unwrap();
        let (profiles, error) = environment_launch_profiles(Some(root));
        assert_eq!(error, None);
        assert_eq!(
            profiles,
            vec![LaunchProfileInfoDto {
                name: "prod".into(),
                danger: true,
                max_duration: Some("4h".into()),
            }]
        );

        std::fs::write(
            dir.path().join("localify.toml"),
            "[profiles.prod]\ndangr = 1\n",
        )
        .unwrap();
        let (profiles, error) = environment_launch_profiles(Some(root));
        assert!(profiles.is_empty());
        assert!(error.unwrap().contains("unknown field"));
    }
}
