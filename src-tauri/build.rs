fn main() {
    println!("cargo:rerun-if-env-changed=LOCALIFY_UPDATER_ENDPOINT");
    println!("cargo:rerun-if-env-changed=LOCALIFY_UPDATER_BASE_URL");

    let endpoint = std::env::var("LOCALIFY_UPDATER_ENDPOINT")
        .ok()
        .or_else(|| {
            std::env::var("LOCALIFY_UPDATER_BASE_URL")
                .ok()
                .map(|base| format!("{}/latest.json", base.trim_end_matches('/')))
        })
        .filter(|value| !value.trim().is_empty());

    if let Some(endpoint) = endpoint {
        println!("cargo:rustc-env=LOCALIFY_UPDATER_ENDPOINT={endpoint}");
    }

    tauri_build::build()
}
