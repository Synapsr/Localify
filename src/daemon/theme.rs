use std::fmt::Write as FmtWrite;

use crate::branding::BRANDING;

/// Escape HTML special characters for safe rendering.
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Percent-encode a path segment for use in href attributes.
pub fn encode_path_segment(s: &str) -> String {
    s.bytes()
        .fold(String::with_capacity(s.len()), |mut out, b| {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char)
                }
                _ => {
                    let _ = write!(out, "%{b:02X}");
                }
            }
            out
        })
}

// ── SVG Icons ───────────────────────────────────────────────

/// Folder icon in brand orange.
pub const FOLDER_ICON: &str = concat!(
    r##"<svg class="ei" viewBox="0 0 20 16" width="18" height="14">"##,
    r##"<path d="M2 2C2 1.4 2.4 1 3 1h4l2 2h8c.6 0 1 .4 1 1"##,
    r##"v9c0 .6-.4 1-1 1H3c-.6 0-1-.4-1-1V2z" fill="#E8853A"/>"##,
    r##"<path d="M2 5h16v8c0 .6-.4 1-1 1H3"##,
    r##"c-.6 0-1-.4-1-1V5z" fill="#F0A050"/>"##,
    "</svg>",
);

/// File icon in brand teal.
pub const FILE_ICON: &str = concat!(
    r##"<svg class="ei" viewBox="0 0 16 20" width="14" height="17">"##,
    r##"<path d="M2 1c0-.6.4-1 1-1h7l5 5v13"##,
    r##"c0 .6-.4 1-1 1H3c-.6 0-1-.4-1-1V1z" fill="#3BB8A2"/>"##,
    r##"<path d="M10 0v4c0 .6.4 1 1 1h4z" fill="#2D9B87"/>"##,
    "</svg>",
);

/// Small home icon for breadcrumb root.
pub const HOME_ICON: &str = concat!(
    r##"<svg class="bc-home" viewBox="0 0 16 16" width="14" height="14">"##,
    r##"<path d="M8 1.5L1.5 7H4v6h3v-4h2v4h3V7h2.5z" fill="currentColor"/>"##,
    "</svg>",
);

// ── Page Shell ──────────────────────────────────────────────

const PAGE_SHELL_TEMPLATE: &str = include_str!("templates/page_shell.html");
const COMMON_CSS: &str = include_str!("templates/common.css");

/// Render a complete themed HTML page.
///
/// Wraps the given body content in the branded page shell
/// with header, footer, and common styles. Pass page-specific
/// CSS and JS via `extra_css` and `extra_js`.
pub fn render_page(title: &str, body: &str, extra_css: &str, extra_js: &str) -> String {
    let extra_css_block = if extra_css.is_empty() {
        String::new()
    } else {
        format!("\n{extra_css}")
    };
    let extra_js_block = if extra_js.is_empty() {
        String::new()
    } else {
        format!("<script>\n{extra_js}\n</script>")
    };

    render_template(
        PAGE_SHELL_TEMPLATE,
        &[
            ("{{PAGE_TITLE}}", &html_escape(title)),
            ("{{APP_NAME}}", BRANDING.app_name),
            ("{{COMMON_CSS}}", COMMON_CSS),
            ("{{EXTRA_CSS_BLOCK}}", &extra_css_block),
            ("{{BODY}}", body),
            ("{{EXTRA_JS_BLOCK}}", &extra_js_block),
        ],
    )
}

/// Render an embedded HTML template by replacing explicit placeholders.
///
/// This keeps daemon-served pages easy to edit as HTML files without pulling
/// in a heavier templating engine for a handful of static pages.
pub fn render_template(template: &str, replacements: &[(&str, &str)]) -> String {
    let mut html = template.to_string();

    for (placeholder, value) in replacements {
        html = html.replace(placeholder, value);
    }

    html
}

/// Shared styles for branded daemon error pages.
pub const ERROR_PAGE_CSS: &str = include_str!("templates/error_page.css");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("hello"), "hello");
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a&b"), "a&amp;b");
        assert_eq!(html_escape(r#"say "hi""#), "say &quot;hi&quot;");
        assert_eq!(html_escape("it's"), "it&#x27;s");
    }

    #[test]
    fn test_encode_path_segment() {
        assert_eq!(encode_path_segment("hello"), "hello");
        assert_eq!(encode_path_segment("hello world"), "hello%20world");
        assert_eq!(encode_path_segment("file#1"), "file%231");
        assert_eq!(encode_path_segment("a+b"), "a%2Bb");
    }

    #[test]
    fn test_render_page_contains_structure() {
        let html = render_page("Test", "<p>body</p>", "", "");
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<title>Test - Localify</title>"));
        assert!(html.contains("roxy-header"));
        assert!(html.contains("<p>body</p>"));
        assert!(html.contains("roxy-footer"));
        assert!(html.contains("local-first development proxy"));
    }

    #[test]
    fn test_render_page_includes_extra_css_and_js() {
        let html = render_page("T", "", ".custom{color:red}", "alert(1)");
        assert!(html.contains(".custom{color:red}"));
        assert!(html.contains("alert(1)"));
    }

    #[test]
    fn test_render_page_escapes_title() {
        let html = render_page("<script>", "", "", "");
        assert!(html.contains("&lt;script&gt; - Localify"));
    }

    #[test]
    fn test_render_page_omits_empty_script() {
        let html = render_page("T", "", "", "");
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn test_render_template_replaces_placeholders() {
        let html = render_template(
            "<div>{{A}} {{B}}</div>",
            &[("{{A}}", "hello"), ("{{B}}", "world")],
        );
        assert_eq!(html, "<div>hello world</div>");
    }

    #[test]
    fn test_error_page_css_contains_shared_error_classes() {
        assert!(ERROR_PAGE_CSS.contains(".error-container"));
        assert!(ERROR_PAGE_CSS.contains(".error-card"));
        assert!(ERROR_PAGE_CSS.contains(".command"));
    }
}
