// The pages the Worker itself serves on public URLs: small, self-contained
// HTML with inline CSS and no external assets, in Localify's look: neutral
// greys, Geist when the visitor has it, a near-black button, light and dark.

/** Where the password form posts, on every protected host. */
export const ACCESS_PATH = "/__localify/access";
/** Where the warning page's `Continue` posts, on every host. */
export const NOTICE_PATH = "/__localify/notice";
/** Linked from every page, for reports about a public URL. */
export const ABUSE_EMAIL = "abuse@localify.dev";
/** The owner limiter counts per minute. */
export const RETRY_AFTER_SECONDS = 60;

const STYLE = `
:root{color-scheme:light dark;--bg:#fafafa;--card:#fff;--text:#171717;--muted:#737373;--line:#e5e5e5;--soft:#f5f5f5;--button:#171717;--button-text:#fafafa;--ring:#3b82f6;--error:#dc2626;--shadow:0 1px 0 rgba(13,13,13,.03),0 24px 48px -24px rgba(13,13,13,.14)}
@media (prefers-color-scheme:dark){:root{--bg:#0a0a0a;--card:#171717;--text:#fafafa;--muted:#a3a3a3;--line:#262626;--soft:#1f1f1f;--button:#fafafa;--button-text:#171717;--error:#f87171;--shadow:0 1px 0 rgba(0,0,0,.4),0 24px 48px -24px rgba(0,0,0,.6)}}
*{box-sizing:border-box}
html{-webkit-text-size-adjust:100%}
body{margin:0;min-height:100vh;min-height:100dvh;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:20px;padding:32px 16px;background:var(--bg);color:var(--text);font:15px/1.6 "Geist Variable","Geist",ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif;-webkit-font-smoothing:antialiased}
.brand{display:flex;align-items:center;gap:8px;font-weight:600;font-size:15px;letter-spacing:-.01em;color:var(--text)}
.brand svg{width:20px;height:20px}
main{width:100%;max-width:400px;background:var(--card);border:1px solid var(--line);border-radius:20px;padding:28px;box-shadow:var(--shadow)}
.title{position:relative;display:flex;align-items:center;gap:6px}
h1{margin:0;font-size:20px;line-height:1.3;font-weight:600;letter-spacing:-.025em}
p{margin:12px 0 0;color:var(--muted)}
strong{color:var(--text);font-weight:600}
.host{margin:2px 0 0;color:var(--muted);font:13px/1.5 ui-monospace,"Geist Mono",SFMono-Regular,Menlo,monospace;word-break:break-all}
.help summary{display:grid;place-items:center;width:22px;height:22px;border-radius:50%;color:var(--muted);cursor:pointer;list-style:none}
.help summary::-webkit-details-marker{display:none}
.help summary svg{width:16px;height:16px}
.help summary:hover,.help[open] summary{color:var(--text)}
.tip{position:absolute;left:0;right:0;top:calc(100% + 10px);z-index:1;padding:12px 14px;border:1px solid var(--line);border-radius:12px;background:var(--card);box-shadow:var(--shadow);font-size:13px;line-height:1.55}
.tip p{margin:0}
.tip p+p{margin-top:6px}
form{margin-top:20px}
.visually-hidden{position:absolute;width:1px;height:1px;overflow:hidden;clip-path:inset(50%);white-space:nowrap}
input{width:100%;font:inherit;padding:10px 12px;border:1px solid var(--line);border-radius:10px;background:var(--card);color:var(--text)}
input::placeholder{color:var(--muted)}
input[aria-invalid="true"]{border-color:var(--error)}
button{display:block;margin-top:12px;width:100%;font:inherit;font-weight:500;padding:10px 14px;border:0;border-radius:10px;background:var(--button);color:var(--button-text);cursor:pointer;transition:opacity .15s}
button:hover{opacity:.88}
input:focus-visible,button:focus-visible,a:focus-visible,summary:focus-visible{outline:2px solid var(--ring);outline-offset:2px}
.error{color:var(--error);margin:8px 0 0;font-size:14px}
a{color:var(--text);text-decoration:underline;text-decoration-color:var(--line);text-decoration-thickness:1.5px;text-underline-offset:4px}
a:hover{text-decoration-color:currentColor}
footer{font-size:13px;color:var(--muted)}
footer a{color:inherit}
.signing{display:none;align-items:center;gap:12px;color:var(--muted)}
.spinner{flex:none;width:18px;height:18px;border:2px solid var(--line);border-top-color:var(--text);border-radius:50%;animation:spin .7s linear infinite}
@keyframes spin{to{transform:rotate(360deg)}}
@media (prefers-reduced-motion:reduce){.spinner{animation:none}}
[data-signing-in] main>:not(.signing){display:none}
[data-signing-in] .signing{display:flex}
`;

/** The Localify mark, drawn inline: the pages load nothing else. */
const LOGO = `<svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor"><path d="M11.1482 0.0185158C11.1968 0.0158905 11.2455 0.0135133 11.2938 0.011413C14.4354 -0.126765 17.5022 0.99433 19.8139 3.126C21.632 4.79945 22.8742 7.0053 23.3626 9.42776C23.5121 10.1899 23.5596 10.807 23.5933 11.5786C22.2809 11.5971 20.9458 11.5833 19.6304 11.5849C17.613 11.5877 15.4027 11.5998 13.4254 11.1936C11.6061 10.8199 10.2772 10.1075 9.25171 8.5354C8.77168 7.72548 8.49165 6.8926 8.28579 5.97924C8.25282 5.83291 8.20208 5.32793 8.11241 5.27506C8.00004 5.31928 7.96326 5.49565 7.94607 5.60489C7.58277 7.91266 6.44363 9.99929 4.18273 10.9262C2.77989 11.5015 1.51714 11.5418 0 11.5628C0.00588639 8.89663 1.10703 6.04532 2.8665 4.06404C5.10629 1.54196 7.82992 0.266877 11.1482 0.0185158Z"/><path d="M17.3598 12.4284C18.0767 12.3913 18.9906 12.4157 19.7218 12.4159L23.5938 12.4237C23.5145 15.4384 22.3931 18.2794 20.2646 20.4374C18.0543 22.7033 15.0265 23.9867 11.8609 23.9997C8.71918 24.0234 5.69806 22.7914 3.46888 20.5773C1.23371 18.3631 0.099189 15.5683 0.0012207 12.4408C1.56045 12.4485 2.94101 12.5104 4.39083 13.1638C6.40425 14.0511 7.54284 16.0608 7.89937 18.1583C7.92665 18.3189 7.95997 18.6549 8.11465 18.7224C8.20579 18.5903 8.29434 17.9887 8.34038 17.7949C8.61262 16.6489 9.08345 15.484 9.89292 14.6113C11.7205 12.6406 14.8568 12.5312 17.3598 12.4284Z"/></svg>`;

/** A question mark in a circle: the explanation of a page sits behind it. */
const HELP_ICON = `<svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"/><path d="M12 17h.01"/></svg>`;

/** Every page refuses to be framed, except the framed warning (see `framedWarningPage`). */
function csp(frameAncestors = "'none'"): string {
  return `default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors ${frameAncestors}; base-uri 'none'`;
}

const SECURITY_HEADERS: Record<string, string> = {
  "content-type": "text/html; charset=utf-8",
  "cache-control": "no-store",
  "content-security-policy": csp(),
  // `no-referrer` would make the browser send `Origin: null` with the form.
  "referrer-policy": "same-origin",
  "x-content-type-options": "nosniff",
  "x-robots-tag": "noindex",
};

/**
 * Signs in with the password a shared link carries after `#`. The fragment
 * never reaches a server, so it is read here, then cleared from the address
 * bar and history before the form posts it. It runs in `<head>`, so the page
 * shows "Signing in" from its first paint instead of the form. A page brought
 * back from the history cache shows the form again.
 */
const SUBMIT_LINK_PASSWORD = `const hash = location.hash.slice(1);
if (hash) {
  history.replaceState(null, "", location.pathname + location.search);
  const root = document.documentElement;
  root.dataset.signingIn = "";
  addEventListener("pageshow", (event) => {
    if (event.persisted) delete root.dataset.signingIn;
  });
  addEventListener("DOMContentLoaded", () => {
    try {
      const form = document.forms[0];
      form.password.value = decodeURIComponent(hash);
      form.from.value = "link";
      form.submit();
    } catch {
      delete root.dataset.signingIn;
    }
  });
}`;

function page(
  status: number,
  title: string,
  body: string,
  method = "GET",
  script?: string,
  frameAncestors?: string,
): Response {
  const nonce = script ? crypto.randomUUID() : null;
  const policy = csp(frameAncestors) + (nonce ? `; script-src 'nonce-${nonce}'` : "");
  const headers = { ...SECURITY_HEADERS, "content-security-policy": policy };
  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>${escapeHtml(title)}</title>
<style>${STYLE}</style>
${nonce ? `<script nonce="${nonce}">${script}</script>\n` : ""}</head>
<body>
<div class="brand">${LOGO}Localify</div>
<main>
${body}
</main>
<footer>Shared with Localify · <a href="mailto:${ABUSE_EMAIL}">Report abuse</a></footer>
</body>
</html>
`;
  return new Response(method === "HEAD" ? null : html, { status, headers });
}

/**
 * The heading every page starts with. Its explanation, when it has one, sits
 * behind a `?` that a click, a tap or the keyboard opens, without scripts.
 */
function heading(title: string, help?: string): string {
  const tip = help
    ? `<details class="help"><summary aria-label="About this page">${HELP_ICON}</summary><div class="tip">${help}</div></details>`
    : "";
  return `<div class="title"><h1>${escapeHtml(title)}</h1>${tip}</div>`;
}

function hostLine(host: string): string {
  return `<p class="host">${escapeHtml(host)}</p>`;
}

/**
 * What the warning page says (ADR 0026), also on a shared link's page: who
 * shared it in one line, the rest behind the `?`.
 */
function noticeText(host: string, login: string): string {
  const owner = `<a href="https://github.com/${encodeURIComponent(login)}">${escapeHtml(login)}</a>`;
  return `${heading(
    "Before you continue",
    `<p>This is a development preview, served from a personal computer through Localify by GitHub user ${owner}.</p>
<p>Never enter passwords or payment details for another site here.</p>`,
  )}
${hostLine(host)}
<p>Shared by ${owner}. Continue only if you trust the sender.</p>`;
}

export type WarningPageOptions = {
  host: string;
  login: string;
  next: string;
  /** The form token (`signNoticeForm`) that `Continue` posts as `notice`. */
  token: string;
  status?: number;
  method?: string;
};

/**
 * Shown before anything else, the password included, on a browser's first
 * visit to a host. `Continue` comes back to `next` with a `GET`: the body of
 * a form posted before the warning is not kept.
 */
export function warningPage({
  host,
  login,
  next,
  token,
  status = 200,
  method,
}: WarningPageOptions): Response {
  return page(
    status,
    "Before you continue",
    `${noticeText(host, login)}
<form method="post" action="${NOTICE_PATH}">
<input type="hidden" name="next" value="${escapeHtml(next)}">
<input type="hidden" name="notice" value="${escapeHtml(token)}">
<button type="submit" autofocus>Continue</button>
</form>`,
    method,
  );
}

export type FramedWarningPageOptions = {
  host: string;
  login: string;
  next: string;
  /** Who may frame it: the public URLs, `https://*.<PUBLIC_BASE_DOMAIN>`. */
  frameAncestors: string;
  method?: string;
};

/**
 * The warning inside a frame. It has nothing to click through: its link opens
 * the URL in a new tab, where the top-level warning, which no page can frame,
 * takes the acknowledgement. Reloading the frame then loads the app.
 */
export function framedWarningPage({
  host,
  login,
  next,
  frameAncestors,
  method,
}: FramedWarningPageOptions): Response {
  const target = `https://${host}${next}`;
  return page(
    200,
    "Before you continue",
    `${noticeText(host, login)}
<p><a href="${escapeHtml(target)}" target="_blank" rel="noopener">Open it in a new tab to continue</a>, then reload this page.</p>`,
    method,
    undefined,
    frameAncestors,
  );
}

export type PasswordPageOptions = {
  host: string;
  next: string;
  error?: string;
  status?: number;
  method?: string;
  /**
   * The page a shared link opens: it posts the password the link carries,
   * with no click, warning or not (ADR 0036).
   */
  fromLink?: boolean;
  /**
   * When this browser has not acknowledged the warning yet: the owner's login
   * and a form token. The page then says what the warning says, and posts the
   * token as the acknowledgement with the password.
   */
  notice?: { login: string; token: string };
};

export function passwordPage({
  host,
  next,
  error,
  status = 401,
  method,
  fromLink = false,
  notice,
}: PasswordPageOptions): Response {
  const errorId = error ? ` aria-describedby="password-error" aria-invalid="true"` : "";
  const text = notice
    ? noticeText(host, notice.login)
    : `${heading(
        "Password required",
        "<p>Its owner protected this public URL. Enter the password they shared with you, or open the link they sent.</p>",
      )}
${hostLine(host)}`;
  return page(
    status,
    notice ? "Before you continue" : "Password required",
    `${fromLink ? `<div class="signing" role="status"><span class="spinner" aria-hidden="true"></span><span>Signing in to <strong>${escapeHtml(host)}</strong>…</span></div>\n` : ""}${text}
<form method="post" action="${ACCESS_PATH}">
<input type="hidden" name="next" value="${escapeHtml(next)}">
<input type="hidden" name="from" value="">
${notice ? `<input type="hidden" name="notice" value="${escapeHtml(notice.token)}">\n` : ""}<label for="password" class="visually-hidden">Password</label>
<input id="password" name="password" type="password" placeholder="Password" autocomplete="current-password" required autofocus${errorId}>
${error ? `<p id="password-error" class="error" role="alert">${escapeHtml(error)}</p>` : ""}
<button type="submit">Continue</button>
</form>`,
    method,
    fromLink ? SUBMIT_LINK_PASSWORD : undefined,
  );
}

/** Over the owner's traffic limit, for a browser's navigation. */
export function tooManyRequestsPage(method?: string): Response {
  const response = page(
    429,
    "Too many requests",
    `${heading("Too many requests")}
<p>The public URLs of this owner received more requests in the last minute than Localify allows. Wait a minute, then reload this page.</p>`,
    method,
  );
  response.headers.set("retry-after", String(RETRY_AFTER_SECONDS));
  return response;
}

export function notFoundPage(method?: string): Response {
  return page(
    404,
    "No public URL here",
    `${heading("No public URL here")}
<p>Nothing is shared at this address. Check the link, or ask the person who sent it whether the URL still exists.</p>`,
    method,
  );
}

/** A URL taken down for abuse, or one whose owner is banned. */
export function removedPage(method?: string): Response {
  return page(
    410,
    "Removed",
    `${heading("This public URL was removed")}
<p>It was taken offline for breaking the Localify terms. To report abuse, or if you think this is a mistake, write to <a href="mailto:${ABUSE_EMAIL}">${ABUSE_EMAIL}</a>.</p>`,
    method,
  );
}

/** A URL its owner's plan does not cover right now (docs/public-urls.md, "Downgrades"). */
export function pausedPage(method?: string): Response {
  return page(
    402,
    "Paused",
    `${heading("This public URL is paused")}
<p>Its owner's Localify Cloud plan does not cover it right now. Ask them to check their plan, then reload this page.</p>`,
    method,
  );
}

export function offlinePage(method?: string): Response {
  return page(
    503,
    "Offline",
    `${heading("This public URL is offline")}
<p>The computer that serves it is not connected right now. Ask its owner to start Localify on it, then reload this page.</p>`,
    method,
  );
}

export function unreachablePage(method?: string): Response {
  return page(
    502,
    "Not reachable",
    `${heading("Could not reach this public URL")}
<p>The computer that serves it did not answer. Reload the page in a moment. If it keeps failing, tell its owner.</p>`,
    method,
  );
}

export function unavailablePage(method?: string): Response {
  return page(
    503,
    "Unavailable",
    `${heading("Public URLs are unavailable")}
<p>Localify Cloud cannot serve this page right now. Try again in a few minutes.</p>`,
    method,
  );
}

export function escapeHtml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}
