// Public traffic on `*.<PUBLIC_BASE_DOMAIN>` (docs/public-urls.md, "Public
// traffic"): find the URL, refuse cross-origin writes, check access, forward
// to the machine's tunnel, and keep the app's cookies on their host.

import { ConfigError, setting } from "./config";
import { ACCESS_COOKIE, accessCookieHeader, readCookie, signAccess, verifyAccess } from "./cookie";
import { verifyPassword } from "./crypto";
import { upstreamHeaders, withHostOnlyCookies, type LocalifyHeaders } from "./headers";
import { readLimitedText } from "./http";
import { isOriginOfUrl, localHostFor, parsePublicHost } from "./names";
import {
  ACCESS_PATH,
  notFoundPage,
  offlinePage,
  passwordPage,
  unavailablePage,
  unreachablePage,
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

  // Nothing below, the password least of all, travels over plain HTTP.
  if (url.protocol !== "https:") {
    url.protocol = "https:";
    return Response.redirect(url.toString(), 308);
  }

  // 2. Refuse what another URL's pages could send with this URL's cookies.
  const webSocket = request.headers.get("upgrade")?.toLowerCase() === "websocket";
  if ((webSocket || !SAFE_METHODS.has(method)) && !fromItsOwnPages(request, site, baseDomain)) {
    return plain(
      403,
      "Requests to this public URL that can change something must come from its own pages.",
    );
  }

  // 3. Check access. The access path is Localify's on every URL: a link shared
  // while the URL had a password still carries it after `#`, and redirecting
  // to `/#` empties the fragment, which the browser would carry to the app.
  if (url.pathname === ACCESS_PATH) {
    if (site.protected) return handleAccess(request, env, site, host);
    await request.body?.cancel();
    return redirect("/#");
  }
  if (site.protected) {
    if (!(await hasAccess(request, env, site, host))) {
      if ((method === "GET" || method === "HEAD") && !webSocket) {
        return passwordPage({ host, next: url.pathname + url.search, method });
      }
      return plain(401, "This public URL needs a password. Open it in a browser to sign in.");
    }
  }

  // 4. and 5. Forward, then keep the app's cookies on this host.
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
 * app's page. `POST` checks the password, sets the cookie, and redirects to
 * `next`.
 */
async function handleAccess(
  request: Request,
  env: Env,
  site: Site,
  host: string,
): Promise<Response> {
  const method = request.method;
  if (method === "GET" || method === "HEAD") {
    return passwordPage({ host, next: "/", status: 200, method, fromLink: true });
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

  const password = form.get("password") ?? "";
  const valid =
    site.passwordHash !== null &&
    site.passwordSalt !== null &&
    password !== "" &&
    (await verifyPassword(password, { hash: site.passwordHash, salt: site.passwordSalt }));
  if (!valid)
    return passwordPage({
      host,
      next,
      error: "That password is not right. Check it and try again.",
    });

  const value = await signAccess(setting(env, "COOKIE_KEY"), {
    host,
    urlId: site.id,
    version: site.passwordVersion,
  });
  const response = redirect(next);
  response.headers.append("set-cookie", accessCookieHeader(value));
  return response;
}

async function readForm(request: Request): Promise<URLSearchParams | null> {
  const text = await readLimitedText(request, MAX_FORM_BYTES);
  return text === null ? null : new URLSearchParams(text);
}

/** A relative path on this same host to return to after the password; `/` otherwise. */
export function safeNext(next: string | null, host: string): string {
  if (!next || !next.startsWith("/") || next.startsWith("//") || next.includes("\\")) return "/";
  let parsed: URL;
  try {
    parsed = new URL(next, `https://${host}`);
  } catch {
    return "/";
  }
  if (parsed.origin !== `https://${host}` || parsed.pathname === ACCESS_PATH) return "/";
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

function plain(status: number, text: string): Response {
  return new Response(text, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" },
  });
}
