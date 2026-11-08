// The pages the Worker itself serves on public URLs: small, self-contained
// HTML with inline CSS and no external assets.

/** Where the password form posts, on every protected host. */
export const ACCESS_PATH = "/__localify/access";
/** Where the warning page's `Continue` posts, on every host. */
export const NOTICE_PATH = "/__localify/notice";
/** Linked from every page, for reports about a public URL. */
export const ABUSE_EMAIL = "abuse@localify.dev";
/** The owner limiter counts per minute. */
export const RETRY_AFTER_SECONDS = 60;

const STYLE = `
:root{color-scheme:light dark;--bg:#f6f6f4;--card:#fff;--text:#1b1b18;--muted:#5d5d57;--line:#d8d8d2;--accent:#1f5eff;--error:#b42318}
@media (prefers-color-scheme:dark){:root{--bg:#121211;--card:#1c1c1a;--text:#ededea;--muted:#a3a39c;--line:#383834;--accent:#7a9dff;--error:#ff8a80}}
*{box-sizing:border-box}
body{margin:0;min-height:100vh;display:grid;place-items:center;padding:16px;background:var(--bg);color:var(--text);font:16px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}
main{width:100%;max-width:380px;background:var(--card);border:1px solid var(--line);border-radius:12px;padding:28px}
h1{font-size:1.25rem;margin:0 0 8px}
p{margin:0 0 16px;color:var(--muted)}
.host{color:var(--text);font-weight:600;word-break:break-all}
label{display:block;font-weight:600;margin-bottom:6px}
input{width:100%;font:inherit;padding:10px 12px;border:1px solid var(--line);border-radius:8px;background:var(--bg);color:var(--text)}
input:focus-visible,button:focus-visible{outline:3px solid var(--accent);outline-offset:2px}
button{margin-top:16px;width:100%;font:inherit;font-weight:600;padding:10px 12px;border:0;border-radius:8px;background:var(--accent);color:#fff;cursor:pointer}
.error{color:var(--error);margin:12px 0 0}
a{color:var(--accent)}
a:focus-visible{outline:3px solid var(--accent);outline-offset:2px}
footer{margin-top:20px;font-size:.85rem;color:var(--muted)}
footer a{color:inherit}
`;

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
 * bar and history before the form posts it.
 */
const SUBMIT_LINK_PASSWORD = `const hash = location.hash.slice(1);
if (hash) {
  history.replaceState(null, "", location.pathname + location.search);
  try {
    const form = document.forms[0];
    form.password.value = decodeURIComponent(hash);
    form.submit();
  } catch {}
}`;

/**
 * The same for a browser that has not acknowledged the warning: it fills the
 * password in, and the visitor's `Continue` posts it with the acknowledgement.
 */
const FILL_LINK_PASSWORD = `const hash = location.hash.slice(1);
if (hash) {
  history.replaceState(null, "", location.pathname + location.search);
  try {
    document.forms[0].password.value = decodeURIComponent(hash);
  } catch {}
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
</head>
<body>
<main>
${body}
<footer>Shared with Localify · <a href="mailto:${ABUSE_EMAIL}">Report abuse</a></footer>
</main>
${nonce ? `<script nonce="${nonce}">${script}</script>\n` : ""}</body>
</html>
`;
  return new Response(method === "HEAD" ? null : html, { status, headers });
}

/** What the warning page says (ADR 0026), also on a shared link's page. */
function noticeText(host: string, login: string): string {
  return `<h1>Before you continue</h1>
<p><span class="host">${escapeHtml(host)}</span> is a development preview, served from a personal computer through Localify by GitHub user <a href="https://github.com/${encodeURIComponent(login)}">${escapeHtml(login)}</a>.</p>
<p>Continue only if you trust whoever sent you this link. Never enter passwords or payment details for another site here.</p>`;
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
<button type="submit">Continue</button>
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
  /** The page a shared link opens: it posts the password the link carries. */
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
  const intro = notice
    ? noticeText(host, notice.login)
    : `<h1>Password required</h1>
<p><span class="host">${escapeHtml(host)}</span> is protected. Enter the password its owner shared with you.</p>`;
  let script: string | undefined;
  if (fromLink) script = notice ? FILL_LINK_PASSWORD : SUBMIT_LINK_PASSWORD;
  return page(
    status,
    notice ? "Before you continue" : "Password required",
    `${intro}
<form method="post" action="${ACCESS_PATH}">
<input type="hidden" name="next" value="${escapeHtml(next)}">
${notice ? `<input type="hidden" name="notice" value="${escapeHtml(notice.token)}">\n` : ""}<label for="password">Password</label>
<input id="password" name="password" type="password" autocomplete="current-password" required autofocus${errorId}>
${error ? `<p id="password-error" class="error" role="alert">${escapeHtml(error)}</p>` : ""}
<button type="submit">Continue</button>
</form>`,
    method,
    script,
  );
}

/** Over the owner's traffic limit, for a browser's navigation. */
export function tooManyRequestsPage(method?: string): Response {
  const response = page(
    429,
    "Too many requests",
    `<h1>Too many requests</h1>
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
    `<h1>No public URL here</h1>
<p>Nothing is shared at this address. Check the link, or ask the person who sent it whether the URL still exists.</p>`,
    method,
  );
}

export function offlinePage(method?: string): Response {
  return page(
    503,
    "Offline",
    `<h1>This public URL is offline</h1>
<p>The computer that serves it is not connected right now. Ask its owner to start Localify on it, then reload this page.</p>`,
    method,
  );
}

export function unreachablePage(method?: string): Response {
  return page(
    502,
    "Not reachable",
    `<h1>Could not reach this public URL</h1>
<p>The computer that serves it did not answer. Reload the page in a moment. If it keeps failing, tell its owner.</p>`,
    method,
  );
}

export function unavailablePage(method?: string): Response {
  return page(
    503,
    "Unavailable",
    `<h1>Public URLs are unavailable</h1>
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
