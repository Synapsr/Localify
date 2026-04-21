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

/// Render a complete themed HTML page.
///
/// Wraps the given body content in the branded page shell
/// with header, footer, and common styles. Pass page-specific
/// CSS and JS via `extra_css` and `extra_js`.
pub fn render_page(title: &str, body: &str, extra_css: &str, extra_js: &str) -> String {
    let mut html = String::with_capacity(8192 + body.len());
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    html.push_str("<meta charset=\"utf-8\">\n");
    html.push_str(
        "<meta name=\"viewport\" \
         content=\"width=device-width, initial-scale=1\">\n",
    );
    html.push_str("<title>");
    html.push_str(&html_escape(title));
    html.push_str(" - ");
    html.push_str(BRANDING.app_name);
    html.push_str("</title>\n<style>\n");
    html.push_str(COMMON_CSS);
    if !extra_css.is_empty() {
        html.push('\n');
        html.push_str(extra_css);
    }
    html.push_str("\n</style>\n</head>\n<body>\n");
    html.push_str(HEADER_HTML);
    html.push_str("<main class=\"roxy-main\">\n");
    html.push_str(body);
    html.push_str("\n</main>\n");
    html.push_str(FOOTER_HTML);
    if !extra_js.is_empty() {
        html.push_str("<script>\n");
        html.push_str(extra_js);
        html.push_str("\n</script>\n");
    }
    html.push_str("</body>\n</html>");
    html
}

/// Shared styles for branded daemon error pages.
pub const ERROR_PAGE_CSS: &str = "\
.error-container{\
    display:flex;flex-direction:column;align-items:center;\
    gap:28px;max-width:720px;margin:24px auto;\
    animation:fadeInUp .5s ease-out;\
}\
.error-image{display:none}\
.error-card{\
    background:var(--surface);border-radius:var(--radius-lg);\
    border:1px solid var(--border);padding:40px;\
    box-shadow:var(--shadow-sm);\
    text-align:left;width:100%;\
}\
.error-title{\
    color:var(--ink);font-size:1.8rem;margin-bottom:12px;\
    font-weight:700;letter-spacing:-.02em;\
}\
.error-message{\
    margin-bottom:20px;font-size:1rem;line-height:1.65;\
    color:var(--ink-soft);\
}\
.help-section{margin-top:28px;text-align:left}\
.help-label{\
    font-size:.72rem;color:var(--ink-muted);\
    margin-bottom:10px;\
    font-weight:600;text-transform:uppercase;letter-spacing:.12em;\
    font-family:ui-monospace,'SF Mono',Menlo,Consolas,monospace;\
}\
.command{\
    background:#0F1917;color:#E8F0EC;\
    padding:16px 18px;\
    border-radius:var(--radius-md);\
    margin:8px 0 22px 0;\
    font-family:ui-monospace,'SF Mono',Menlo,Consolas,monospace;\
    font-size:.85rem;line-height:1.75;\
    border:1px solid rgba(255,255,255,.06);\
    box-shadow:0 6px 18px rgba(15,25,23,.18);\
    overflow-x:auto;\
}\
.command code{background:transparent;color:inherit;padding:0;border:0}\
.comment{color:#86A195;font-size:.92em}\
@keyframes fadeInUp{from{opacity:0;transform:translateY(8px)}to{opacity:1;transform:translateY(0)}}\
@media(max-width:600px){.error-card{padding:28px 22px}.error-title{font-size:1.5rem}}\
";

// ── Private Constants ───────────────────────────────────────

const COMMON_CSS: &str = "\
:root{\
    --bg:#F5F2EA;\
    --surface:#FFFFFF;\
    --surface-soft:#FBF8F2;\
    --ink:#13211E;\
    --ink-soft:rgba(19,33,30,.72);\
    --ink-muted:rgba(19,33,30,.54);\
    --border:rgba(27,44,39,.10);\
    --border-strong:rgba(27,44,39,.18);\
    --accent:#256F5A;\
    --accent-strong:#0F513F;\
    --accent-soft:rgba(37,111,90,.10);\
    --danger:#B14A2A;\
    --danger-strong:#8A3518;\
    --danger-soft:rgba(177,74,42,.08);\
    --shadow-sm:0 1px 2px rgba(15,25,23,.04),0 6px 24px rgba(15,25,23,.06);\
    --shadow-md:0 12px 40px rgba(15,25,23,.08);\
    --radius-sm:10px;\
    --radius-md:14px;\
    --radius-lg:20px;\
}\
*,*::before,*::after{margin:0;padding:0;box-sizing:border-box}\
html{scroll-behavior:smooth}\
body{\
    font-family:ui-sans-serif,-apple-system,BlinkMacSystemFont,'Inter','Segoe UI',Roboto,'Helvetica Neue',Arial,sans-serif;\
    background:\
        radial-gradient(circle at 0% -10%,rgba(84,148,125,.14),transparent 38%),\
        radial-gradient(circle at 100% 0%,rgba(214,193,151,.22),transparent 30%),\
        var(--bg);\
    color:var(--ink);\
    line-height:1.6;min-height:100vh;\
    display:flex;flex-direction:column;\
    -webkit-font-smoothing:antialiased;\
    text-rendering:optimizeLegibility;\
}\
a{color:var(--accent-strong);text-decoration:none;transition:color .18s ease}\
a:hover{color:var(--ink)}\
code{\
    font-family:ui-monospace,'SF Mono',Menlo,Consolas,'Cascadia Code',monospace;\
    font-size:.88em;\
    background:var(--accent-soft);color:var(--accent-strong);\
    padding:.12em .5em;border-radius:6px;\
}\
.roxy-header{padding:1.25rem 1.5rem 0}\
.roxy-header-inner{\
    max-width:960px;margin:0 auto;\
    display:flex;align-items:center;justify-content:space-between;gap:1rem;\
    padding:.7rem 1.1rem;\
    border-radius:999px;\
    border:1px solid rgba(255,255,255,.6);\
    background:rgba(255,250,240,.72);\
    backdrop-filter:blur(18px);-webkit-backdrop-filter:blur(18px);\
    box-shadow:0 10px 28px rgba(20,34,29,.06);\
}\
.roxy-brand{\
    display:inline-flex;align-items:center;gap:.7rem;\
    font-weight:700;font-size:.95rem;letter-spacing:-.01em;\
    color:var(--ink);\
}\
.roxy-brand__mark{\
    width:1.85rem;height:1.85rem;border-radius:50%;\
    display:flex;flex-direction:column;\
    align-items:center;justify-content:center;gap:2px;\
    background:linear-gradient(135deg,#15332C,#2D6D59);\
    box-shadow:inset 0 1px 0 rgba(255,255,255,.2),0 4px 10px rgba(20,46,39,.2);\
}\
.roxy-brand__mark i{\
    display:block;height:1.5px;border-radius:2px;\
    background:rgba(255,250,240,.94);\
}\
.roxy-brand__mark i:nth-child(1){width:.7rem}\
.roxy-brand__mark i:nth-child(2){width:.95rem}\
.roxy-brand__mark i:nth-child(3){width:.55rem}\
.roxy-header-version{\
    font-family:ui-monospace,'SF Mono',Menlo,Consolas,monospace;\
    font-size:.7rem;letter-spacing:.14em;text-transform:uppercase;\
    color:var(--ink-muted);\
}\
.roxy-main{\
    max-width:960px;width:100%;margin:0 auto;\
    padding:2.25rem 1.5rem 3.5rem;flex:1;\
}\
.roxy-footer{\
    max-width:960px;margin:0 auto;width:100%;\
    padding:1.25rem 1.5rem 2rem;\
    color:var(--ink-muted);font-size:.8rem;\
    display:flex;align-items:center;justify-content:space-between;gap:1rem;\
}\
.roxy-footer a{color:var(--ink-muted)}\
.roxy-footer a:hover{color:var(--ink)}\
@media(max-width:640px){\
    .roxy-header-inner{border-radius:18px}\
    .roxy-footer{flex-direction:column;text-align:center}\
}\
";

const HEADER_HTML: &str = concat!(
    r##"<header class="roxy-header"><div class="roxy-header-inner">"##,
    r##"<span class="roxy-brand">"##,
    r##"<span class="roxy-brand__mark" aria-hidden="true"><i></i><i></i><i></i></span>"##,
    r##"<span>Localify</span>"##,
    r##"</span>"##,
    r##"<span class="roxy-header-version">Local proxy</span>"##,
    "</div></header>\n",
);

const FOOTER_HTML: &str = concat!(
    r##"<footer class="roxy-footer">"##,
    r##"<span>Localify &middot; local-first development proxy</span>"##,
    r##"<a href="https://github.com/rbas">github.com/rbas</a>"##,
    "</footer>\n",
);

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
    fn test_error_page_css_contains_shared_error_classes() {
        assert!(ERROR_PAGE_CSS.contains(".error-container"));
        assert!(ERROR_PAGE_CSS.contains(".error-card"));
        assert!(ERROR_PAGE_CSS.contains(".command"));
    }
}
