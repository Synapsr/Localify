use std::process::Command;

#[test]
fn a_closed_stdout_ends_the_command_quietly() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        "'{}' --config '{}' status | true; echo ${{PIPESTATUS[0]}}",
        env!("CARGO_BIN_EXE_localify"),
        dir.path().join("config.toml").display()
    );

    let output = Command::new("bash")
        .args(["-c", &script])
        .env("HOME", dir.path())
        .output()
        .unwrap();

    let code = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // 0 if the command finished writing before the reader went away.
    assert!(
        code.trim() == "141" || code.trim() == "0",
        "exit code {code}, stderr: {stderr}"
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
}
