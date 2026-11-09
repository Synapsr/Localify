//! `--machine` as the `localify` binary parses it (docs/remote-machines.md,
//! "CLI"): `dev` on another machine refuses every option but `--project`,
//! `--env` and `--profile` before it asks the daemon anything.

use std::process::{Command, Output};

fn localify(args: &[&str]) -> Output {
    let dir = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_localify"))
        .arg("--config")
        .arg(dir.path().join("config.toml"))
        .args(args)
        .env("HOME", dir.path())
        .output()
        .unwrap()
}

#[test]
fn dev_on_another_machine_refuses_every_other_option_with_exit_2() {
    for (extra, named) in [
        (vec!["--detach"], "--detach"),
        (vec!["--in-app"], "--in-app"),
        (vec!["--env-file", ".env"], "--env-file"),
        (vec!["--confirm-profile", "prod"], "--confirm-profile"),
        (vec!["--domain", "web.localify"], "--domain"),
        (vec!["--", "pnpm", "dev"], "a command after --"),
    ] {
        let mut args = vec!["dev", "web", "--machine", "teo-mini"];
        args.extend(extra);

        let output = localify(&args);

        assert_eq!(output.status.code(), Some(2), "{args:?}");
        // Not a terminal: one JSON document with the code.
        let refused: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stdout)));
        assert_eq!(refused["outcome"], "refused");
        assert_eq!(refused["code"], "refused_argument");
        let message = refused["message"].as_str().unwrap();
        assert!(message.ends_with(&format!("Remove {named}.")), "{message}");
    }
}

#[test]
fn machine_needs_a_service() {
    let output = localify(&["stop", "--machine", "teo-mini"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("<SERVICE>"));
}
