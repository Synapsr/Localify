// The pages the Worker itself serves on public URLs: small, self-contained
// HTML with inline CSS and no external assets.

/** Where the password form posts, on every protected host. */
export const ACCESS_PATH = "/__localify/access";

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
footer{margin-top:20px;font-size:.85rem;color:var(--muted)}
`;

const SECURITY_HEADERS: Record<string, string> = {
  "content-type": "text/html; charset=utf-8",
  "cache-control": "no-store",
  "content-security-policy":
    "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
  "referrer-policy": "no-referrer",
  "x-content-type-options": "nosniff",
  "x-robots-tag": "noindex",
};

function page(status: number, title: string, body: string, method = "GET"): Response {
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
<footer>Shared with Localify</footer>
</main>
</body>
</html>
`;
  return new Response(method === "HEAD" ? null : html, { status, headers: SECURITY_HEADERS });
}

export type PasswordPageOptions = {
  host: string;
  next: string;
  error?: string;
  status?: number;
  method?: string;
};

export function passwordPage({
  host,
  next,
  error,
  status = 401,
  method,
}: PasswordPageOptions): Response {
  const errorId = error ? ` aria-describedby="password-error" aria-invalid="true"` : "";
  return page(
    status,
    "Password required",
    `<h1>Password required</h1>
<p><span class="host">${escapeHtml(host)}</span> is protected. Enter the password its owner shared with you.</p>
<form method="post" action="${ACCESS_PATH}">
<input type="hidden" name="next" value="${escapeHtml(next)}">
<label for="password">Password</label>
<input id="password" name="password" type="password" autocomplete="current-password" required autofocus${errorId}>
${error ? `<p id="password-error" class="error" role="alert">${escapeHtml(error)}</p>` : ""}
<button type="submit">Continue</button>
</form>`,
    method,
  );
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
