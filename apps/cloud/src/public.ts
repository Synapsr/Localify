// Public traffic on `*.<PUBLIC_BASE_DOMAIN>` (docs/public-urls.md, "Public
// traffic"): find the URL, apply its owner's traffic limit, refuse
// cross-origin writes, show the warning page, check access, forward to the
// machine's tunnel, and keep the app's cookies on their host.

import { ConfigError, setting } from "./config";
import {
  ACCESS_COOKIE,
  accessCookieHeader,
  NOTICE_COOKIE,
  noticeCookieHeader,
  readCookie,
  signAccess,
  signNotice,
  verifyAccess,
  verifyNotice,
} from "./cookie";
import { verifyPassword } from "./crypto";
import { upstreamHeaders, withHostOnlyCookies, type LocalifyHeaders } from "./headers";
import { readLimitedText } from "./http";
import { isOriginOfUrl, localHostFor, parsePublicHost } from "./names";
import {
  ACCESS_PATH,
  NOTICE_PATH,
  notFoundPage,
  offlinePage,
  passwordPage,
  RETRY_AFTER_SECONDS,
  tooManyRequestsPage,
  unavailablePage,
  unreachablePage,
  warningPage,
} from "./pages";
import { findSite, type Site } from "./sites";

const MAX_FORM_BYTES = 8 * 1024;
const SAFE_METHODS = new Set(["GET", "HEAD", "OPTIONS"]);

export async function handlePublic(request: Request, env: Env): Promise<Response> {
  try {
    return await servePublic(request, env);
  } catch (error) {
    if (error instanceof ConfigError)
      console.error("Public traffic is not configured:", error.message);
    else console.error("Public traffic failed", error);
    return unavailablePage(request.method);
  }
}

async function servePublic(request: Request, env: Env): Promise<Response> {
  const url = new URL(request.url);
  const host = url.hostname;
  const method = request.method;
  const baseDomain = setting(env, "PUBLIC_BASE_DOMAIN");

  // 1. Find the URL.
  const target = parsePublicHost(host, baseDomain);
  if (!target) return notFoundPage(method);
  const site = await findSite(env, target.slug);
  if (!site || (target.label !== null && !site.wildcard)) return notFoundPage(method);
  const localHost = localHostFor(site.localDomain, target.label);
  if (!localHost) return notFoundPage(method);
  if (site.machineId === null || site.ingressSecret === null) return offlinePage(method);

  // 2. The owner's traffic limit, across all their URLs.
  if (!(await withinOwnerLimit(env, site))) {
    await request.body?.cancel();
    if ((method === "GET" || method === "HEAD") && isNavigation(request)) {
      return tooManyRequestsPage(method);
    }
    return plain(429, "Too many requests to this owner's public URLs. Retry in a minute.", {
      "retry-after": String(RETRY_AFTER_SECONDS),
    });
  }

  // Nothing below, the password least of all, travels over plain HTTP.
  if (url.protocol !== "https:") {
    url.protocol = "https:";
    return Response.redirect(url.toString(), 308);
  }

  // 3. Refuse what another URL's pages could send with this URL's cookies.
  const webSocket = request.headers.get("upgrade")?.toLowerCase() === "websocket";
  if ((webSocket || !SAFE_METHODS.has(method)) && !fromItsOwnPages(request, site, baseDomain)) {
    return plain(
      403,
      "Requests to this public URL that can change something must come from its own pages.",
    );
  }

  // Localify's own paths, on every URL. A link shared while the URL had a
  // password still carries it after `#`, and redirecting to `/#` empties the
  // fragment, which the browser would carry to the app. The access page shows
  // the warning itself, so a link that carries the password takes one click.
  if (url.pathname === ACCESS_PATH) {
    if (site.protected) return handleAccess(request, env, site, host);
    await request.body?.cancel();
    return redirect("/#");
  }
  if (url.pathname === NOTICE_PATH) return handleNotice(request, env, site, host);

  // 4. The warning page, on a browser's first visit to this host. Assets,
  // `fetch`, HMR, WebSockets and webhooks are never navigations.
  if (
    method === "GET" &&
    !webSocket &&
    isNavigation(request) &&
    !(await acknowledgedWarning(request, env, site, host))
  ) {
    return warningPage({
      host,
      login: site.ownerLogin,
      next: safeNext(url.pathname + url.search, host),
      method,
    });
  }

  // 5. Check access.
  if (site.protected) {
    if (!(await hasAccess(request, env, site, host))) {
      if ((method === "GET" || method === "HEAD") && !webSocket) {
        return passwordPage({ host, next: url.pathname + url.search, method });
      }
      return plain(401, "This public URL needs a password. Open it in a browser to sign in.");
    }
  }

  // 6. and 7. Forward, then keep the app's cookies on this host.
  return forward(
    request,
    env,
    site.machineId,
    {
      edgeKey: setting(env, "EDGE_KEY"),
      ingressSecret: site.ingressSecret,
      publicHost: host,
      localHost,
      urlId: site.id,
      clientIp: request.headers.get("cf-connecting-ip"),
    },
    url,
  );
}

/**
 * A request without `Origin` is not a browser's cross-origin request. A page
 * whose referrer policy is `no-referrer` sends `Origin: null`, even to its own
 * origin, so the browser's `Sec-Fetch-Site` decides then.
 */
function fromItsOwnPages(request: Request, site: Site, baseDomain: string): boolean {
  const origin = request.headers.get("origin");
  if (origin === null) return true;
  if (origin === "null") return request.headers.get("sec-fetch-site") === "same-origin";
  return isOriginOfUrl(origin, site.slug, site.wildcard, baseDomain);
}

/**
 * `OWNER_LIMITER`, keyed by the owner's user id. A failing limiter lets the
 * request through: it caps cost, and must not take every URL down.
 */
async function withinOwnerLimit(env: Env, site: Site): Promise<boolean> {
  try {
    return (await env.OWNER_LIMITER.limit({ key: site.userId })).success;
  } catch (error) {
    console.error("The owner limiter failed", error);
    return true;
  }
}

/**
 * A browser's navigation: `Sec-Fetch-Dest` says a document or a frame, or,
 * from a browser too old to send it, `Accept` asks for HTML.
 */
function isNavigation(request: Request): boolean {
  const dest = request.headers.get("sec-fetch-dest");
  if (dest !== null) return dest === "document" || dest === "iframe" || dest === "frame";
  return (request.headers.get("accept") ?? "").includes("text/html");
}

/**
 * A valid notice cookie for this host, or the `Localify-Skip-Warning` header,
 * which a browser cannot add to a navigation.
 */
async function acknowledgedWarning(
  request: Request,
  env: Env,
  site: Site,
  host: string,
): Promise<boolean> {
  if (request.headers.has("localify-skip-warning")) return true;
  const key = setting(env, "COOKIE_KEY");
  const scope = { host, urlId: site.id };
  for (const value of readCookie(request.headers.get("cookie"), NOTICE_COOKIE)) {
    if (await verifyNotice(key, value, scope)) return true;
  }
  return false;
}

/** The warning page's `Continue`: sets the notice cookie, and redirects to `next`. */
async function handleNotice(
  request: Request,
  env: Env,
  site: Site,
  host: string,
): Promise<Response> {
  if (request.method !== "POST") {
    await request.body?.cancel();
    return redirect("/");
  }
  const form = await readForm(request);
  if (!form) return plain(413, "The form is too large.");
  const response = redirect(safeNext(form.get("next"), host));
  response.headers.append("set-cookie", await noticeCookie(env, site, host));
  return response;
}

async function noticeCookie(env: Env, site: Site, host: string): Promise<string> {
  const value = await signNotice(setting(env, "COOKIE_KEY"), { host, urlId: site.id });
  return noticeCookieHeader(value);
}

async function hasAccess(request: Request, env: Env, site: Site, host: string): Promise<boolean> {
  const key = setting(env, "COOKIE_KEY");
  const scope = { host, urlId: site.id, version: site.passwordVersion };
  for (const value of readCookie(request.headers.get("cookie"), ACCESS_COOKIE)) {
    if (await verifyAccess(key, value, scope)) return true;
  }
  return false;
}

/**
 * `GET /__localify/access` is the page a shared link opens, with the password
 * after `#`. Never a redirect: the browser would carry the fragment to the
 * app's page. It posts the password by itself once the warning is
 * acknowledged; before that, it shows the warning and waits for `Continue`,
 * which posts the acknowledgement with the password. `POST` checks the
 * password, sets the cookies, and redirects to `next`.
 */
async function handleAccess(
  request: Request,
  env: Env,
  site: Site,
  host: string,
): Promise<Response> {
  const method = request.method;
  if (method === "GET" || method === "HEAD") {
    const acknowledged = await acknowledgedWarning(request, env, site, host);
    return passwordPage({
      host,
      next: "/",
      status: 200,
      method,
      fromLink: true,
      noticeFor: acknowledged ? undefined : site.ownerLogin,
    });
  }
  if (method !== "POST") return redirect("/");

  const clientIp = request.headers.get("cf-connecting-ip") ?? "unknown";
  const { success } = await env.PASSWORD_LIMITER.limit({ key: `${site.id}:${clientIp}` });
  // Refused before the body is read, so throttled requests cost no memory.
  if (!success) {
    await request.body?.cancel();
    return passwordPage({
      host,
      next: "/",
      status: 429,
      error: "Too many attempts. Wait a minute, then try again.",
    });
  }
  const form = await readForm(request);
  if (!form) return plain(413, "The form is too large.");
  const next = safeNext(form.get("next"), host);
  // The shared link's page posts the warning's `Continue` with the password.
  // The acknowledgement holds whether or not the password is right.
  const notice = form.get("notice") === "1" ? await noticeCookie(env, site, host) : null;

  const password = form.get("password") ?? "";
  const valid =
    site.passwordHash !== null &&
    site.passwordSalt !== null &&
    password !== "" &&
    (await verifyPassword(password, { hash: site.passwordHash, salt: site.passwordSalt }));
  let response: Response;
  if (valid) {
    const value = await signAccess(setting(env, "COOKIE_KEY"), {
      host,
      urlId: site.id,
      version: site.passwordVersion,
    });
    response = redirect(next);
    response.headers.append("set-cookie", accessCookieHeader(value));
  } else {
    response = passwordPage({
      host,
      next,
      error: "That password is not right. Check it and try again.",
    });
  }
  if (notice) response.headers.append("set-cookie", notice);
  return response;
}

async function readForm(request: Request): Promise<URLSearchParams | null> {
  const text = await readLimitedText(request, MAX_FORM_BYTES);
  return text === null ? null : new URLSearchParams(text);
}

/**
 * A relative path on this same host to return to after the warning or the
 * password, outside Localify's own paths; `/` otherwise.
 */
export function safeNext(next: string | null, host: string): string {
  if (!next || !next.startsWith("/") || next.startsWith("//") || next.includes("\\")) return "/";
  let parsed: URL;
  try {
    parsed = new URL(next, `https://${host}`);
  } catch {
    return "/";
  }
  if (parsed.origin !== `https://${host}`) return "/";
  if (parsed.pathname === ACCESS_PATH || parsed.pathname === NOTICE_PATH) return "/";
  return parsed.pathname + parsed.search;
}

async function forward(
  request: Request,
  env: Env,
  machineId: string,
  localify: LocalifyHeaders,
  url: URL,
): Promise<Response> {
  const headers = upstreamHeaders(request.headers, localify);
  // Concatenated, not resolved against a base: a path like `//other.host/`
  // must stay a path on the machine's hostname.
  const upstream = `https://m-${machineId}.${setting(env, "MACHINE_DOMAIN")}${url.pathname}${url.search}`;
  const hasBody = request.method !== "GET" && request.method !== "HEAD";
  let response: Response;
  try {
    // An `Upgrade: websocket` request passes through this same fetch.
    response = await fetch(upstream, {
      method: request.method,
      headers,
      body: hasBody ? request.body : null,
      redirect: "manual",
    });
  } catch (error) {
    console.error("Forwarding to the machine failed", machineId, error);
    return unreachablePage(request.method);
  }
  // 530 is Cloudflare's answer when the tunnel has no connector (error 1033).
  if (response.status === 530) return offlinePage(request.method);
  return withHostOnlyCookies(response);
}

function redirect(location: string): Response {
  return new Response(null, { status: 303, headers: { location, "cache-control": "no-store" } });
}

function plain(status: number, text: string, extra: Record<string, string> = {}): Response {
  return new Response(text, {
    status,
    headers: {
      "content-type": "text/plain; charset=utf-8",
      "cache-control": "no-store",
      ...extra,
    },
  });
}
