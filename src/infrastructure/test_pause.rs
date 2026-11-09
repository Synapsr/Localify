//! Barriers for the integration tests, which run the real binary: a debug
//! build stops at a named point while a test holds it there. Release builds
//! compile every point away.

/// The folder a test arms points in: `<folder>/<point>` holds a process at
/// `point`, which writes its PID to `<folder>/<point>.reached` and waits
/// until the test removes `<folder>/<point>`.
pub const PAUSE_DIR_ENV: &str = "LOCALIFY_TEST_PAUSE_DIR";

/// Waits at `point` while a test holds it.
pub fn pause_at(point: &str) {
    #[cfg(debug_assertions)]
    {
        use std::path::Path;
        use std::time::Duration;

        let Some(dir) = std::env::var_os(PAUSE_DIR_ENV) else {
            return;
        };
        let armed = Path::new(&dir).join(point);
        if !armed.exists() {
            return;
        }
        let _ = std::fs::write(
            Path::new(&dir).join(format!("{point}.reached")),
            std::process::id().to_string(),
        );
        while armed.exists() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = point;
}
