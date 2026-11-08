// The headers a public request carries to the machine.

import { ACCESS_COOKIE, NOTICE_COOKIE, withoutCookies } from "./cookie";

const STRIPPED = new Set(["host", "forwarded", "x-real-ip", "localify-skip-warning"]);
const STRIPPED_PREFIXES = ["x-forwarded-", "x-localify-", "cf-"];

export type LocalifyHeaders = {
  /** `EDGE_KEY`: the machine zone refuses machine hostnames without it, then removes it. */
  edgeKey: string;
  ingressSecret: string;
  publicHost: string;
  localHost: string;
  urlId: string;
  clientIp: string | null;
};

/**
 * Copies the request's headers without `Host`, `Forwarded`, `X-Real-IP`,
 * `X-Forwarded-*`, `X-Localify-*`, `CF-*`, `Localify-Skip-Warning`, and the
 * access and notice cookies, then adds
 * `X-Localify-Edge` for the machine zone, and the `X-Localify-*` headers the
 * daemon's public ingress reads.
 */
export function upstreamHeaders(incoming: Headers, localify: LocalifyHeaders): Headers {
  const headers = new Headers();
  for (const [name, value] of incoming) {
    const lower = name.toLowerCase();
    if (lower === "cookie" || STRIPPED.has(lower)) continue;
    if (STRIPPED_PREFIXES.some((prefix) => lower.startsWith(prefix))) continue;
    headers.append(name, value);
  }
  const cookies = withoutCookies(incoming.get("cookie"), [ACCESS_COOKIE, NOTICE_COOKIE]);
  if (cookies) headers.set("cookie", cookies);
  headers.set("x-localify-edge", localify.edgeKey);
  headers.set("x-localify-ingress", localify.ingressSecret);
  headers.set("x-localify-public-host", localify.publicHost);
  headers.set("x-localify-local-host", localify.localHost);
  headers.set("x-localify-url-id", localify.urlId);
  if (localify.clientIp) headers.set("x-localify-client-ip", localify.clientIp);
  return headers;
}

/**
 * A `Set-Cookie` value without its `Domain` attribute, so the cookie stays on
 * the public host instead of reaching every URL under the base domain.
 */
export function withoutDomainAttribute(setCookie: string): string {
  const [pair = "", ...attributes] = setCookie.split(";");
  const kept = attributes.filter(
    (attribute) => attribute.split("=")[0]!.trim().toLowerCase() !== "domain",
  );
  return [pair, ...kept].join(";");
}

/**
 * An app's response to a request without the notice cookie, which a browser
 * must not reuse for a navigation: another page of the same site could fetch
 * the app's HTML first, and the navigation would then skip the warning from
 * the browser's cache. It varies on the fetch metadata, and HTML is never
 * stored. A WebSocket upgrade (101) is never cached and passes as it is.
 */
export function withoutNavigationReuse(response: Response): Response {
  if (response.status === 101 || response.webSocket) return response;
  const headers = new Headers(response.headers);
  headers.append("vary", "Sec-Fetch-Dest, Sec-Fetch-Mode");
  if ((headers.get("content-type") ?? "").toLowerCase().includes("text/html")) {
    headers.set("cache-control", "no-store");
  }
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

/** The upstream response, with `Domain` removed from every `Set-Cookie`. The body still streams. */
export function withHostOnlyCookies(response: Response): Response {
  const cookies = response.headers.getSetCookie();
  const rewritten = cookies.map(withoutDomainAttribute);
  if (rewritten.every((cookie, index) => cookie === cookies[index])) return response;
  const headers = new Headers(response.headers);
  headers.delete("set-cookie");
  for (const cookie of rewritten) headers.append("set-cookie", cookie);
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
    // A WebSocket upgrade (101) carries its socket on the response.
    webSocket: response.webSocket ?? null,
  });
}
