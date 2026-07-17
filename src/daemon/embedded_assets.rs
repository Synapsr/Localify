use std::sync::OnceLock;

/// Embedded error mascot image (localify_error_small.png)
const LOCALIFY_ERROR_IMAGE: &[u8] = include_bytes!("../../assets/localify_error_small.png");

/// Embedded 404 mascot image (localify_404_small.png)
const LOCALIFY_404_IMAGE: &[u8] = include_bytes!("../../assets/localify_404_small.png");

static LOCALIFY_ERROR_DATA_URI: OnceLock<String> = OnceLock::new();
static LOCALIFY_404_DATA_URI: OnceLock<String> = OnceLock::new();

fn png_data_uri(bytes: &[u8]) -> String {
    use base64::{Engine as _, engine::general_purpose};
    let b64 = general_purpose::STANDARD.encode(bytes);
    format!("data:image/png;base64,{b64}")
}

pub fn localify_error_data_uri() -> &'static str {
    LOCALIFY_ERROR_DATA_URI
        .get_or_init(|| png_data_uri(LOCALIFY_ERROR_IMAGE))
        .as_str()
}

pub fn localify_404_data_uri() -> &'static str {
    LOCALIFY_404_DATA_URI
        .get_or_init(|| png_data_uri(LOCALIFY_404_IMAGE))
        .as_str()
}
