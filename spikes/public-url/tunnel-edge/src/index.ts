// Measures the transport Codex proposed on 2026-11-05: the public Worker checks
// access, then forwards to a named Cloudflare Tunnel on the serving machine.
// No Durable Object on the request path. Spike only.

interface Env {
  PUBLIC_BASE_DOMAIN: string;
  /** Hostname routed to the machine's named tunnel. */
  ORIGIN_HOST: string;
  /** Signs the access cookie. */
  GATE_SECRET: string;
  /** Proves to the machine's ingress that the request came through this Worker. */
  INGRESS_SECRET: string;
}

const ACCESS_COOKIE = "__Host-localify-access";
const STRIPPED_PREFIXES = ["x-localify-", "x-forwarded-", "cf-"];
const STRIPPED = new Set(["host", "forwarded", "x-real-ip"]);

/** `<label>--<slug>.<base>` or `<slug>.<base>`, one DNS label deep. */
export function parsePublicHost(
  host: string,
  baseDomain: string,
): { slug: string; label: string | null } | null {
  const suffix = `.${baseDomain}`;
  if (!host.endsWith(suffix)) return null;
  const name = host.slice(0, -suffix.length);
  if (!name || name.includes(".")) return null;
  const separator = name.lastIndexOf("--");
  if (separator === -1) return { slug: name, label: null };
  const label = name.slice(0, separator);
  const slug = name.slice(separator + 2);
  return label && slug ? { slug, label } : null;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const host = url.hostname.toLowerCase();
    const target = parsePublicHost(host, env.PUBLIC_BASE_DOMAIN);
    if (!target) return new Response("Unknown public URL", { status: 404 });

    const cookies = request.headers.get("cookie") ?? "";
    if (!(await hasValidAccessCookie(cookies, host, env.GATE_SECRET))) {
      return new Response("This public URL is password protected.", { status: 401 });
    }

    const headers = new Headers();
    for (const [name, value] of request.headers) {
      const lower = name.toLowerCase();
      if (STRIPPED.has(lower) || STRIPPED_PREFIXES.some((prefix) => lower.startsWith(prefix))) {
        continue;
      }
      headers.append(name, value);
    }
    const appCookies = withoutAccessCookie(cookies);
    if (appCookies) headers.set("cookie", appCookies);
    else headers.delete("cookie");
    headers.set("x-localify-ingress", env.INGRESS_SECRET);
    headers.set("x-forwarded-host", host);
    headers.set("x-forwarded-proto", "https");
    headers.set("x-localify-label", target.label ?? "");
    const clientIp = request.headers.get("cf-connecting-ip");
    if (clientIp) headers.set("x-forwarded-for", clientIp);

    const upstream = new URL(url.pathname + url.search, `https://${env.ORIGIN_HOST}`);
    // Passing the Upgrade header through makes fetch proxy WebSockets too.
    return fetch(upstream, {
      method: request.method,
      headers,
      body: request.body,
      redirect: "manual",
    });
  },
} satisfies ExportedHandler<Env>;

/** Cookie value: `<expiry seconds>.<hex HMAC of host|expiry>`. */
async function hasValidAccessCookie(cookies: string, host: string, secret: string) {
  const value = readCookie(cookies, ACCESS_COOKIE);
  if (!value) return false;
  const [expiry, signature] = value.split(".");
  if (!expiry || !signature || Number(expiry) * 1000 < Date.now()) return false;
  const expected = await sign(`${host}|${expiry}`, secret);
  return timingSafeEqual(expected, signature);
}

async function sign(message: string, secret: string): Promise<string> {
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const mac = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(message));
  return [...new Uint8Array(mac)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

function timingSafeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

function readCookie(cookies: string, name: string): string | null {
  for (const part of cookies.split(";")) {
    const [key, ...rest] = part.trim().split("=");
    if (key === name) return rest.join("=");
  }
  return null;
}

function withoutAccessCookie(cookies: string): string {
  return cookies
    .split(";")
    .map((part) => part.trim())
    .filter((part) => part && !part.startsWith(`${ACCESS_COOKIE}=`))
    .join("; ");
}
