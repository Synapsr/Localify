import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ACCESS_COOKIE, NOTICE_COOKIE } from "../src/cookie";
import { hmacHex } from "../src/crypto";
import { api, createContext, rateLimit, serve, signIn, visit, type TestContext } from "./support";

let context: TestContext;
let token: string;
let machineId: string;
let urlId: string;
let slug: string;
const PASSWORD = "open sesame please";

beforeEach(async () => {
  context = createContext();
  ({ token, machineId } = await signIn(context));
  slug = `site-${Math.random().toString(16).slice(2, 8)}`;
  const response = await api(context, "POST", "/v1/public-urls", {
    token,
    body: {
      localDomain: "journal.localify",
      wildcard: true,
      slug,
      protection: { mode: "password", password: PASSWORD },
    },
  });
  urlId = ((await response.json()) as { publicUrl: { id: string } }).publicUrl.id;
  await serve(context, token);
  context.routes.set(
    `GET https://m-${machineId}.localify.dev/`,
    () => new Response("from the machine"),
  );
  context.routes.set(`POST https://m-${machineId}.localify.dev/`, () => new Response("posted"));
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function host(label?: string): string {
  return label ? `${slug}--${label}.localify.fyi` : `${slug}.localify.fyi`;
}

async function signInWithPassword(publicHost = host(), next = "/docs?page=2"): Promise<Response> {
  return visit(context, `https://${publicHost}/__localify/access`, {
    method: "POST",
    headers: {
      "content-type": "application/x-www-form-urlencoded",
      "cf-connecting-ip": "203.0.113.7",
    },
    body: new URLSearchParams({ password: PASSWORD, next }).toString(),
  });
}

async function accessCookie(publicHost = host()): Promise<string> {
  const response = await signInWithPassword(publicHost);
  const setCookie = response.headers.get("set-cookie")!;
  return setCookie.split(";")[0]!;
}

function machineCalls() {
  return context.calls.filter((call) => call.url.startsWith(`https://m-${machineId}.`));
}

describe("finding the URL", () => {
  it("answers 404 for an unknown slug, a malformed host, or a label on a plain URL", async () => {
    for (const url of [
      "https://nobody-here.localify.fyi/",
      "https://a.b.localify.fyi/",
      `https://${slug}--.localify.fyi/`,
    ]) {
      const response = await visit(context, url);
      expect(response.status, url).toBe(404);
      expect(response.headers.get("content-type")).toContain("text/html");
      expect(await response.text()).toContain("No public URL here");
    }
    const plain = await api(context, "POST", "/v1/public-urls", {
      token,
      body: { localDomain: "plain.localify", slug: "plain-site", protection: { mode: "public" } },
    });
    expect(plain.status).toBe(201);
    expect((await visit(context, "https://plain-site--web.localify.fyi/")).status).toBe(404);
    expect((await visit(context, "https://plain-site.localify.fyi/")).status).toBe(200);
  });

  it("answers 503 while the machine's tunnel is not ready", async () => {
    context.sqlite
      .prepare("UPDATE machines SET tunnel_state = 'releasing' WHERE id = ?")
      .run(machineId);
    const response = await visit(context, `https://${host()}/`);
    expect(response.status).toBe(503);
    expect(machineCalls()).toHaveLength(0);
  });

  it("answers 503 when no machine serves the URL", async () => {
    await api(context, "POST", "/v1/auth/logout", { token });
    const response = await visit(context, `https://${host()}/`);
    expect(response.status).toBe(503);
    expect(await response.text()).toContain("offline");
  });

  it("answers 404 once the owner's GitHub id leaves the allowlist", async () => {
    context.env.ALLOWED_GITHUB_IDS = "2";
    expect((await visit(context, `https://${host()}/`)).status).toBe(404);
  });

  it("answers 404 for a deleted URL", async () => {
    await api(context, "DELETE", `/v1/public-urls/${urlId}`, { token });
    expect((await visit(context, `https://${host()}/`)).status).toBe(404);
  });

  it("redirects plain HTTP to HTTPS before asking for the password", async () => {
    const response = await visit(context, `http://${host()}/docs?x=1`);
    expect(response.status).toBe(308);
    expect(response.headers.get("location")).toBe(`https://${host()}/docs?x=1`);
  });
});

describe("the password", () => {
  it("shows the password page for GET and HEAD, and a plain 401 otherwise", async () => {
    const page = await visit(context, `https://${host()}/docs?page=2`);
    expect(page.status).toBe(401);
    const html = await page.text();
    expect(html).toContain('action="/__localify/access"');
    expect(html).toContain('name="next" value="/docs?page=2"');
    expect(html).not.toMatch(/<(script|link|img)\b/);
    expect(page.headers.get("cache-control")).toBe("no-store");
    expect(page.headers.get("content-security-policy")).not.toContain("script-src");
    expect(page.headers.get("referrer-policy")).toBe("same-origin");

    const head = await visit(context, `https://${host()}/`, { method: "HEAD" });
    expect(head.status).toBe(401);

    const post = await visit(context, `https://${host()}/api`, { method: "POST", body: "x" });
    expect(post.status).toBe(401);
    expect(post.headers.get("content-type")).toContain("text/plain");
    expect(machineCalls()).toHaveLength(0);
  });

  it("sets a host-only cookie and redirects with 303 on the right password", async () => {
    const response = await signInWithPassword();
    expect(response.status).toBe(303);
    expect(response.headers.get("location")).toBe("/docs?page=2");
    const cookie = response.headers.get("set-cookie")!;
    expect(cookie).toMatch(
      new RegExp(
        `^${ACCESS_COOKIE}=\\d+\\.1\\.[0-9a-f]{64}; Max-Age=604800; Path=/; Secure; HttpOnly; SameSite=Lax$`,
      ),
    );
  });

  it("opens the page a shared link leads to, which posts the password after #", async () => {
    const page = await visit(context, `https://${host()}/__localify/access`);
    expect(page.status).toBe(200);
    const html = await page.text();
    expect(html).toContain('name="next" value="/"');
    const nonce = /<script nonce="([0-9a-f-]{36})">/.exec(html)?.[1];
    expect(nonce).toBeDefined();
    expect(html).toContain("location.hash");
    expect(page.headers.get("content-security-policy")).toContain(`script-src 'nonce-${nonce}'`);

    const again = await visit(context, `https://${host()}/__localify/access`);
    expect(again.headers.get("content-security-policy")).not.toContain(nonce);

    const head = await visit(context, `https://${host()}/__localify/access`, { method: "HEAD" });
    expect(head.status).toBe(200);
    expect(machineCalls()).toHaveLength(0);
  });

  it("redirects only to a path on the same host", async () => {
    const response = await signInWithPassword(host(), "https://evil.example/");
    expect(response.headers.get("location")).toBe("/");
  });

  it("shows the page again with an error on a wrong password", async () => {
    const response = await visit(context, `https://${host()}/__localify/access`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: "password=wrong&next=%2Fdocs",
    });
    expect(response.status).toBe(401);
    expect(response.headers.get("set-cookie")).toBeNull();
    const html = await response.text();
    expect(html).toContain('role="alert"');
    expect(html).toContain('value="/docs"');
  });

  it("answers 429 once the rate limit is reached", async () => {
    context.limiter.allowed = false;
    const response = await signInWithPassword();
    expect(response.status).toBe(429);
    expect(response.headers.get("set-cookie")).toBeNull();
  });

  it("forwards with a valid cookie, without the cookie", async () => {
    const cookie = await accessCookie();
    const response = await visit(context, `https://${host()}/docs?page=2`, {
      headers: {
        cookie: `theme=dark; ${cookie}`,
        "cf-connecting-ip": "203.0.113.7",
        "x-forwarded-host": "spoofed",
        "x-localify-edge": "forged",
      },
    });
    expect(response.status).toBe(200);
    expect(await response.text()).toBe("from the machine");
    const [call] = machineCalls();
    expect(call!.url).toBe(`https://m-${machineId}.localify.dev/docs?page=2`);
    expect(call!.redirect).toBe("manual");
    expect(call!.headers.get("cookie")).toBe("theme=dark");
    expect(call!.headers.get("x-forwarded-host")).toBeNull();
    expect(call!.headers.get("x-localify-edge")).toBe("edge-key");
    expect(call!.headers.get("x-localify-ingress")).toBe(await hmacHex("ingress-key", machineId));
    expect(call!.headers.get("x-localify-public-host")).toBe(host());
    expect(call!.headers.get("x-localify-local-host")).toBe("journal.localify");
    expect(call!.headers.get("x-localify-url-id")).toBe(urlId);
    expect(call!.headers.get("x-localify-client-ip")).toBe("203.0.113.7");
  });

  it("asks each wildcard label to sign in separately", async () => {
    const cookie = await accessCookie(host());
    const response = await visit(context, `https://${host("web")}/`, { headers: { cookie } });
    expect(response.status).toBe(401);

    const labelCookie = await accessCookie(host("web--feature"));
    const forwarded = await visit(context, `https://${host("web--feature")}/`, {
      headers: { cookie: labelCookie },
    });
    expect(forwarded.status).toBe(200);
    expect(machineCalls()[0]!.headers.get("x-localify-local-host")).toBe(
      "web--feature.journal.localify",
    );
  });

  it("signs every browser out when the password changes", async () => {
    const cookie = await accessCookie();
    expect((await visit(context, `https://${host()}/`, { headers: { cookie } })).status).toBe(200);
    await api(context, "PUT", `/v1/public-urls/${urlId}/protection`, {
      token,
      body: { protection: { mode: "password", password: "another password" } },
    });
    expect((await visit(context, `https://${host()}/`, { headers: { cookie } })).status).toBe(401);
  });

  it("lets everyone in once the URL is public", async () => {
    await api(context, "PUT", `/v1/public-urls/${urlId}/protection`, {
      token,
      body: { protection: { mode: "public" } },
    });
    expect((await visit(context, `https://${host()}/`)).status).toBe(200);
  });

  it("keeps the access path from the app once the URL is public", async () => {
    await api(context, "PUT", `/v1/public-urls/${urlId}/protection`, {
      token,
      body: { protection: { mode: "public" } },
    });
    // A link shared while it had a password: `/#` empties its fragment.
    for (const method of ["GET", "POST"]) {
      const response = await visit(context, `https://${host("web")}/__localify/access`, {
        method,
        body: method === "POST" ? "password=x" : undefined,
      });
      expect(response.status, method).toBe(303);
      expect(response.headers.get("location"), method).toBe("/#");
    }
    expect(machineCalls()).toHaveLength(0);
  });
});

describe("cross-origin requests", () => {
  it("refuse writes from another public URL, even with the cookie", async () => {
    const cookie = await accessCookie();
    for (const origin of [
      "https://other-site.localify.fyi",
      `https://other-site--${slug}.localify.fyi`,
      "null",
    ]) {
      const response = await visit(context, `https://${host()}/api`, {
        method: "POST",
        headers: { cookie, origin },
        body: "x",
      });
      expect(response.status, origin).toBe(403);
    }
    expect(machineCalls()).toHaveLength(0);
  });

  it("take Origin: null only when the browser says the request is same-origin", async () => {
    // A page with `Referrer-Policy: no-referrer` sends `Origin: null` even to itself.
    const form = await visit(context, `https://${host()}/__localify/access`, {
      method: "POST",
      headers: {
        "content-type": "application/x-www-form-urlencoded",
        origin: "null",
        "sec-fetch-site": "same-origin",
      },
      body: new URLSearchParams({ password: PASSWORD, next: "/" }).toString(),
    });
    expect(form.status).toBe(303);
    const cookie = form.headers.get("set-cookie")!.split(";")[0]!;

    const write = await visit(context, `https://${host()}/api`, {
      method: "POST",
      headers: { cookie, origin: "null", "sec-fetch-site": "same-origin" },
      body: "x",
    });
    expect(write.status).toBe(200);

    for (const site of ["same-site", "cross-site", "none"]) {
      const response = await visit(context, `https://${host()}/api`, {
        method: "POST",
        headers: { cookie, origin: "null", "sec-fetch-site": site },
        body: "x",
      });
      expect(response.status, site).toBe(403);
    }
    expect(machineCalls()).toHaveLength(1);
  });

  it("refuse the password form from another public URL", async () => {
    const response = await visit(context, `https://${host()}/__localify/access`, {
      method: "POST",
      headers: {
        "content-type": "application/x-www-form-urlencoded",
        origin: "https://other-site.localify.fyi",
      },
      body: new URLSearchParams({ password: PASSWORD, next: "/" }).toString(),
    });
    expect(response.status).toBe(403);
    expect(response.headers.get("set-cookie")).toBeNull();
  });

  it("allow writes from the URL itself and from its other labels", async () => {
    for (const label of [undefined, "web"]) {
      const cookie = await accessCookie(host(label));
      for (const origin of [`https://${host()}`, `https://${host("api")}`]) {
        const response = await visit(context, `https://${host(label)}/api`, {
          method: "POST",
          headers: { cookie, origin },
          body: "x",
        });
        expect(response.status, `${origin} → ${label}`).toBe(200);
      }
    }
  });

  it("let reads and requests without Origin through to the access check", async () => {
    const cookie = await accessCookie();
    const read = await visit(context, `https://${host()}/`, {
      headers: { cookie, origin: "https://other-site.localify.fyi" },
    });
    expect(read.status).toBe(200);
    const preflight = await visit(context, `https://${host()}/`, {
      method: "OPTIONS",
      headers: { origin: "https://other-site.localify.fyi" },
    });
    expect(preflight.status).toBe(401);
    const noOrigin = await visit(context, `https://${host()}/api`, {
      method: "POST",
      headers: { cookie },
      body: "x",
    });
    expect(noOrigin.status).toBe(200);
  });

  it("apply to unprotected URLs too", async () => {
    await api(context, "PUT", `/v1/public-urls/${urlId}/protection`, {
      token,
      body: { protection: { mode: "public" } },
    });
    const response = await visit(context, `https://${host()}/api`, {
      method: "POST",
      headers: { origin: "https://other-site.localify.fyi" },
      body: "x",
    });
    expect(response.status).toBe(403);
  });
});

describe("WebSockets", () => {
  const upgrade = { upgrade: "websocket", connection: "Upgrade" };

  it("get a plain 401 without the cookie", async () => {
    const response = await visit(context, `https://${host()}/hmr`, { headers: upgrade });
    expect(response.status).toBe(401);
    expect(response.headers.get("content-type")).toContain("text/plain");
    expect(machineCalls()).toHaveLength(0);
  });

  it("need an Origin of the same URL when one is sent", async () => {
    const cookie = await accessCookie();
    const foreign = await visit(context, `https://${host()}/hmr`, {
      headers: { ...upgrade, cookie, origin: "https://evil-site.localify.fyi" },
    });
    expect(foreign.status).toBe(403);

    for (const origin of [`https://${host()}`, `https://${host("web")}`]) {
      const response = await visit(context, `https://${host()}/hmr`, {
        headers: { ...upgrade, cookie, origin },
      });
      expect(response.status, origin).toBe(200);
    }
    expect(machineCalls()[0]!.headers.get("upgrade")).toBe("websocket");

    const none = await visit(context, `https://${host()}/hmr`, { headers: { ...upgrade, cookie } });
    expect(none.status).toBe(200);
  });

  it("need the Origin rule on unprotected URLs too", async () => {
    await api(context, "PUT", `/v1/public-urls/${urlId}/protection`, {
      token,
      body: { protection: { mode: "public" } },
    });
    const foreign = await visit(context, `https://${host()}/hmr`, {
      headers: { ...upgrade, origin: "https://evil-site.localify.fyi" },
    });
    expect(foreign.status).toBe(403);
  });
});

describe("forwarding", () => {
  it("keeps a path that starts with // on the machine's host", async () => {
    const cookie = await accessCookie();
    await visit(context, `https://${host()}//evil.example/x`, { headers: { cookie } });
    expect(machineCalls()[0]!.url).toBe(`https://m-${machineId}.localify.dev//evil.example/x`);
    expect(context.calls.some((call) => call.url.includes("://evil.example"))).toBe(false);
  });

  it("shows the offline page when the tunnel has no connector", async () => {
    context.routes.set(
      `GET https://m-${machineId}.localify.dev/`,
      () => new Response("error 1033", { status: 530 }),
    );
    const cookie = await accessCookie();
    const response = await visit(context, `https://${host()}/`, { headers: { cookie } });
    expect(response.status).toBe(503);
  });

  it("removes the Domain attribute from the app's cookies", async () => {
    context.routes.set(`GET https://m-${machineId}.localify.dev/`, () => {
      const headers = new Headers();
      headers.append("set-cookie", "session=abc; Domain=localify.fyi; Path=/; HttpOnly");
      headers.append("set-cookie", "theme=dark; Path=/");
      return new Response("ok", { headers });
    });
    const cookie = await accessCookie();
    const response = await visit(context, `https://${host()}/`, { headers: { cookie } });
    expect(response.headers.getSetCookie()).toEqual([
      "session=abc; Path=/; HttpOnly",
      "theme=dark; Path=/",
    ]);
    expect(await response.text()).toBe("ok");
  });

  it("passes redirects and errors from the app through unchanged", async () => {
    context.routes.set(
      `GET https://m-${machineId}.localify.dev/`,
      () => new Response(null, { status: 302, headers: { location: "/login" } }),
    );
    const cookie = await accessCookie();
    const response = await visit(context, `https://${host()}/`, { headers: { cookie } });
    expect(response.status).toBe(302);
    expect(response.headers.get("location")).toBe("/login");
  });

  it("shows a 502 page when the machine cannot be reached", async () => {
    context.routes.set(`GET https://m-${machineId}.localify.dev/`, () => {
      throw new Error("network down");
    });
    const cookie = await accessCookie();
    const response = await visit(context, `https://${host()}/`, { headers: { cookie } });
    expect(response.status).toBe(502);
  });
});

const navigation = { "sec-fetch-dest": "document", accept: "text/html,*/*;q=0.8" };

/** The form token in the page a navigation to `url` gets. */
async function formToken(url: string, cookie?: string): Promise<string> {
  const headers: Record<string, string> = { ...navigation };
  if (cookie) headers.cookie = cookie;
  const page = await visit(context, url, { headers });
  const token = /name="notice" value="([^"]+)"/.exec(await page.text())?.[1];
  if (!token) throw new Error(`No form token in the page of ${url}`);
  return token;
}

async function acknowledge(
  publicHost = host(),
  next = "/docs?page=2",
  options: { origin?: string | null; notice?: string } = {},
): Promise<Response> {
  const notice = options.notice ?? (await formToken(`https://${publicHost}/`));
  const headers: Record<string, string> = { "content-type": "application/x-www-form-urlencoded" };
  const origin = options.origin === undefined ? `https://${publicHost}` : options.origin;
  if (origin !== null) headers.origin = origin;
  return visit(context, `https://${publicHost}/__localify/notice`, {
    method: "POST",
    headers,
    body: new URLSearchParams({ next, notice }).toString(),
  });
}

async function noticeCookie(publicHost = host()): Promise<string> {
  const response = await acknowledge(publicHost);
  return response.headers.get("set-cookie")!.split(";")[0]!;
}

async function makePublic(): Promise<void> {
  await api(context, "PUT", `/v1/public-urls/${urlId}/protection`, {
    token,
    body: { protection: { mode: "public" } },
  });
}

describe("the warning page", () => {
  it("comes before the password on a browser's first navigation", async () => {
    const page = await visit(context, `https://${host()}/docs?page=2`, { headers: navigation });
    expect(page.status).toBe(200);
    expect(page.headers.get("content-type")).toContain("text/html");
    expect(page.headers.get("cache-control")).toBe("no-store");
    expect(page.headers.get("content-security-policy")).toContain("frame-ancestors 'none'");
    expect(page.headers.get("content-security-policy")).not.toContain("script-src");
    expect(page.headers.get("referrer-policy")).toBe("same-origin");
    const html = await page.text();
    expect(html).toContain("development preview");
    expect(html).toContain('<a href="https://github.com/teo-goulois">teo-goulois</a>');
    expect(html).toContain("Never enter passwords or payment details for another site");
    expect(html).toContain('href="mailto:abuse@localify.dev"');
    expect(html).toContain('action="/__localify/notice"');
    expect(html).toContain('name="next" value="/docs?page=2"');
    expect(html).toMatch(/name="notice" value="\d+\.[0-9a-f]{64}"/);
    expect(html).not.toContain('name="password"');
    expect(html).not.toMatch(/<(script|link|img)\b/);
    expect(machineCalls()).toHaveLength(0);
  });

  it("shows on open URLs, and to browsers without Sec-Fetch-Dest", async () => {
    await makePublic();
    const navigations: Record<string, string>[] = [
      navigation,
      { accept: "text/html,application/xhtml+xml" },
    ];
    for (const headers of navigations) {
      const page = await visit(context, `https://${host("web")}/`, { headers });
      expect(page.status, JSON.stringify(headers)).toBe(200);
      expect(await page.text()).toContain('action="/__localify/notice"');
    }
    expect(machineCalls()).toHaveLength(0);
  });

  it("comes before a form a navigation posts, which comes back as a GET", async () => {
    await makePublic();
    const post = await visit(context, `https://${host()}/signup?step=1`, {
      method: "POST",
      headers: {
        ...navigation,
        origin: `https://${host()}`,
        "content-type": "application/x-www-form-urlencoded",
      },
      body: "email=a%40example.com",
    });
    expect(post.status).toBe(200);
    const html = await post.text();
    expect(html).toContain('action="/__localify/notice"');
    expect(html).toContain('name="next" value="/signup?step=1"');

    const head = await visit(context, `https://${host()}/`, {
      method: "HEAD",
      headers: navigation,
    });
    expect(head.status).toBe(200);
    expect(head.headers.get("content-type")).toContain("text/html");
    expect(machineCalls()).toHaveLength(0);

    const back = await acknowledge(host(), "/signup?step=1");
    expect(back.status).toBe(303);
    expect(back.headers.get("location")).toBe("/signup?step=1");
  });

  it("shows inside a frame without a form, linking to the URL in a new tab", async () => {
    await makePublic();
    for (const dest of ["iframe", "frame", "object", "embed"]) {
      for (const method of ["GET", "POST"]) {
        const page = await visit(context, `https://${host("web")}/docs?page=2`, {
          method,
          headers: { "sec-fetch-dest": dest, origin: `https://${host()}` },
          body: method === "POST" ? "x=1" : undefined,
        });
        expect(page.status, `${method} ${dest}`).toBe(200);
        const policy = page.headers.get("content-security-policy")!;
        expect(policy).toContain("frame-ancestors https://*.localify.fyi;");
        expect(policy).not.toContain("frame-ancestors 'none'");
        const html = await page.text();
        expect(html).toContain("teo-goulois");
        expect(html).toContain(
          `<a href="https://${host("web")}/docs?page=2" target="_blank" rel="noopener">`,
        );
        expect(html).not.toContain("<form");
      }
    }
    expect(machineCalls()).toHaveLength(0);
  });

  it("sets a signed host-only cookie on Continue, then shows the password page", async () => {
    const response = await acknowledge();
    expect(response.status).toBe(303);
    expect(response.headers.get("location")).toBe("/docs?page=2");
    const setCookie = response.headers.get("set-cookie")!;
    expect(setCookie).toMatch(
      new RegExp(
        `^${NOTICE_COOKIE}=\\d+\\.[0-9a-f]{64}; Max-Age=604800; Path=/; Secure; HttpOnly; SameSite=Lax$`,
      ),
    );
    expect(setCookie).not.toMatch(/domain=/i);
    const cookie = setCookie.split(";")[0]!;
    const page = await visit(context, `https://${host()}/docs?page=2`, {
      headers: { ...navigation, cookie },
    });
    expect(page.status).toBe(401);
    expect(await page.text()).toContain("Password required");
    expect(machineCalls()).toHaveLength(0);
  });

  it("is acknowledged once for the host, and never for another label", async () => {
    const notice = await noticeCookie(host());
    const access = await accessCookie(host());
    const cookie = `${notice}; ${access}; theme=dark`;
    const response = await visit(context, `https://${host()}/`, {
      headers: { ...navigation, cookie },
    });
    expect(response.status).toBe(200);
    expect(await response.text()).toBe("from the machine");
    expect(machineCalls()[0]!.headers.get("cookie")).toBe("theme=dark");

    const other = await visit(context, `https://${host("web")}/`, {
      headers: { ...navigation, cookie },
    });
    expect(other.status).toBe(200);
    expect(await other.text()).toContain("Before you continue");
    expect(machineCalls()).toHaveLength(1);
  });

  it("refuses a forged or expired notice cookie", async () => {
    await makePublic();
    const notice = await noticeCookie();
    const [name, value] = notice.split("=") as [string, string];
    const [expiry, signature] = value.split(".") as [string, string];
    const past = Math.floor(Date.now() / 1000) - 1;
    const pastSignature = await hmacHex("cookie-key", `notice|${host()}|${urlId}|${past}`);
    for (const forged of [
      `${name}=${Number(expiry) + 60}.${signature}`,
      `${name}=${past}.${pastSignature}`,
      `${name}=garbage`,
    ]) {
      const page = await visit(context, `https://${host()}/`, {
        headers: { ...navigation, cookie: forged },
      });
      expect(await page.text(), forged).toContain("Before you continue");
    }
    expect(machineCalls()).toHaveLength(0);
  });

  it("returns only to a path on the same host, outside Localify's paths", async () => {
    for (const next of [
      "https://evil.example/",
      "//evil.example/",
      "/\\evil.example/",
      "/a/..//evil.example/phish",
      "/a/%2e%2e//evil.example/phish",
      "/__localify/notice",
    ]) {
      const response = await acknowledge(host(), next);
      expect(response.status, next).toBe(303);
      expect(response.headers.get("location"), next).toBe("/");
    }
  });

  it("keeps its path from the app, and takes Continue only from this exact host", async () => {
    const get = await visit(context, `https://${host()}/__localify/notice`, {
      headers: navigation,
    });
    expect(get.status).toBe(303);
    expect(get.headers.get("location")).toBe("/");
    expect(get.headers.get("set-cookie")).toBeNull();

    const tokenHere = await formToken(`https://${host()}/`);
    const tokenOfSibling = await formToken(`https://${host("web")}/`);
    const refused: { origin?: string | null; notice?: string }[] = [
      { origin: "https://other-site.localify.fyi", notice: tokenHere },
      // A sibling label of the same URL passes the Origin rule, not this one.
      { origin: `https://${host("web")}`, notice: tokenHere },
      { origin: null, notice: tokenHere },
      { notice: "" },
      { notice: tokenOfSibling },
      { notice: tokenHere.replace(/.$/, (c) => (c === "0" ? "1" : "0")) },
    ];
    for (const options of refused) {
      const response = await acknowledge(host(), "/", options);
      expect(response.status, JSON.stringify(options)).toBe(403);
      expect(response.headers.get("set-cookie"), JSON.stringify(options)).toBeNull();
    }
    expect(machineCalls()).toHaveLength(0);
  });

  it("shows the warning again, with a fresh token, after its form expired", async () => {
    const expiry = Math.floor(Date.now() / 1000) - 1;
    const expired = `${expiry}.${await hmacHex("cookie-key", `notice-form|${host()}|${expiry}`)}`;
    const response = await acknowledge(host(), "/docs", { notice: expired });
    expect(response.status).toBe(403);
    expect(response.headers.get("set-cookie")).toBeNull();
    const html = await response.text();
    expect(html).toContain('name="next" value="/docs"');
    const fresh = /name="notice" value="([^"]+)"/.exec(html)![1]!;
    expect((await acknowledge(host(), "/docs", { notice: fresh })).status).toBe(303);
  });

  it("never interrupts assets, fetch, HMR, WebSockets, webhooks or tools", async () => {
    await makePublic();
    const requests: { method?: string; headers: Record<string, string>; body?: string }[] = [
      { headers: { "sec-fetch-dest": "script", accept: "*/*" } },
      { headers: { "sec-fetch-dest": "style", accept: "text/css,*/*;q=0.1" } },
      { headers: { "sec-fetch-dest": "image", accept: "image/avif,*/*" } },
      // `fetch` and XHR, even when they ask for HTML.
      { headers: { "sec-fetch-dest": "empty", accept: "text/html" } },
      { headers: { "sec-fetch-dest": "websocket", upgrade: "websocket", connection: "Upgrade" } },
      { headers: { upgrade: "websocket", connection: "Upgrade", accept: "text/html" } },
      // Webhooks, even one that asks for HTML.
      { method: "POST", headers: { "content-type": "application/json" }, body: "{}" },
      { method: "POST", headers: { accept: "text/html" }, body: "{}" },
      { headers: {} },
      { headers: { accept: "application/json" } },
      { headers: { ...navigation, "localify-skip-warning": "1" } },
    ];
    for (const init of requests) {
      const response = await visit(context, `https://${host()}/`, init);
      expect(response.status, JSON.stringify(init)).toBe(200);
    }
    expect(machineCalls()).toHaveLength(requests.length);
    expect(machineCalls().at(-1)!.headers.has("localify-skip-warning")).toBe(false);
  });
});

describe("caching of the app's responses", () => {
  const cacheable = { "cache-control": "public, max-age=600" };

  /** The app answers HTML marked cacheable, with `Expires`; `.js` is a script, `/missing` a 404 page. */
  function cacheableApp(): void {
    context.routes.set(`GET https://m-${machineId}.localify.dev/`, (_body, url) => {
      if (url.endsWith(".js")) {
        return new Response("export {}", {
          headers: { "content-type": "text/javascript", ...cacheable },
        });
      }
      return new Response("<!doctype html><p>the app</p>", {
        status: url.endsWith("/missing") ? 404 : 200,
        headers: {
          "content-type": "text/html; charset=utf-8",
          ...cacheable,
          expires: "Thu, 01 Jan 2099 00:00:00 GMT",
          vary: "Accept-Encoding",
        },
      });
    });
  }

  it("never stores HTML fetched without the cookie, and varies on the fetch metadata", async () => {
    await makePublic();
    cacheableApp();
    // Another page of the same site prefetches this host's HTML.
    const prefetch = await visit(context, `https://${host()}/`, {
      headers: { "sec-fetch-dest": "empty", "sec-fetch-mode": "cors" },
    });
    expect(prefetch.status).toBe(200);
    expect(prefetch.headers.get("cache-control")).toBe("no-store");
    expect(prefetch.headers.get("expires")).toBeNull();
    expect(prefetch.headers.get("vary")).toBe("Accept-Encoding, Sec-Fetch-Dest, Sec-Fetch-Mode");
    expect(await prefetch.text()).toContain("the app");
  });

  it("never stores HTML for an acknowledged visitor either, whatever its status", async () => {
    await makePublic();
    cacheableApp();
    const cookie = await noticeCookie();
    for (const path of ["/", "/missing"]) {
      const response = await visit(context, `https://${host()}${path}`, {
        headers: { ...navigation, cookie },
      });
      expect(response.headers.get("cache-control"), path).toBe("no-store");
      expect(response.headers.get("expires"), path).toBeNull();
      expect(response.headers.get("vary"), path).toBe("Accept-Encoding");
      expect(await response.text()).toContain("the app");
    }
  });

  it("keeps the app's caching on other responses", async () => {
    await makePublic();
    cacheableApp();
    const script = await visit(context, `https://${host()}/main.js`, {
      headers: { "sec-fetch-dest": "script" },
    });
    expect(script.headers.get("cache-control")).toBe("public, max-age=600");
    expect(script.headers.get("vary")).toBe("Sec-Fetch-Dest, Sec-Fetch-Mode");

    const cookie = await noticeCookie();
    const acknowledged = await visit(context, `https://${host()}/main.js`, {
      headers: { "sec-fetch-dest": "script", cookie },
    });
    expect(acknowledged.headers.get("cache-control")).toBe("public, max-age=600");
    expect(acknowledged.headers.get("vary")).toBeNull();
    expect(await acknowledged.text()).toBe("export {}");
  });
});

describe("a shared link", () => {
  it("shows the warning with the password filled in, for one click", async () => {
    const page = await visit(context, `https://${host()}/__localify/access`, {
      headers: navigation,
    });
    expect(page.status).toBe(200);
    const html = await page.text();
    expect(html).toContain("Before you continue");
    expect(html).toContain("teo-goulois");
    expect(html).toContain('href="mailto:abuse@localify.dev"');
    expect(html).toContain('action="/__localify/access"');
    expect(html).toMatch(/name="notice" value="\d+\.[0-9a-f]{64}"/);
    expect(html).toContain('name="password"');
    expect(html).toContain("location.hash");
    expect(html).toContain("history.replaceState");
    expect(html).not.toContain("submit()");
    const nonce = /<script nonce="([0-9a-f-]{36})">/.exec(html)?.[1];
    expect(nonce).toBeDefined();
    expect(page.headers.get("content-security-policy")).toContain(`script-src 'nonce-${nonce}'`);
    expect(page.headers.get("content-security-policy")).toContain("frame-ancestors 'none'");
    expect(page.headers.get("referrer-policy")).toBe("same-origin");
    expect(machineCalls()).toHaveLength(0);
  });

  async function postLink(origin: string, notice: string, password = PASSWORD): Promise<Response> {
    return visit(context, `https://${host()}/__localify/access`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded", origin },
      body: new URLSearchParams({ next: "/", notice, password }).toString(),
    });
  }

  it("sets both cookies on Continue, and then opens the app", async () => {
    const notice = await formToken(`https://${host()}/__localify/access`);
    const response = await postLink(`https://${host()}`, notice);
    expect(response.status).toBe(303);
    expect(response.headers.get("location")).toBe("/");
    const cookies = response.headers.getSetCookie();
    expect(cookies.map((cookie) => cookie.split("=")[0])).toEqual([ACCESS_COOKIE, NOTICE_COOKIE]);
    const cookie = cookies.map((value) => value.split(";")[0]).join("; ");
    const app = await visit(context, `https://${host()}/`, { headers: { ...navigation, cookie } });
    expect(app.status).toBe(200);
    expect(await app.text()).toBe("from the machine");
  });

  it("acknowledges the warning only from this exact host", async () => {
    const notice = await formToken(`https://${host()}/__localify/access`);
    // A sibling label of the same URL may sign in, but not acknowledge.
    const sibling = await postLink(`https://${host("web")}`, notice);
    expect(sibling.status).toBe(303);
    expect(sibling.headers.getSetCookie().map((cookie) => cookie.split("=")[0])).toEqual([
      ACCESS_COOKIE,
    ]);
    const untokened = await postLink(`https://${host()}`, "1");
    expect(untokened.headers.getSetCookie().map((cookie) => cookie.split("=")[0])).toEqual([
      ACCESS_COOKIE,
    ]);
  });

  it("keeps the acknowledgement when the password is wrong", async () => {
    const notice = await formToken(`https://${host()}/__localify/access`);
    const response = await postLink(`https://${host()}`, notice, "wrong");
    expect(response.status).toBe(401);
    const cookies = response.headers.getSetCookie();
    expect(cookies).toHaveLength(1);
    expect(cookies[0]).toMatch(new RegExp(`^${NOTICE_COOKIE}=`));
    const html = await response.text();
    expect(html).toContain("Password required");
    expect(html).not.toContain('name="notice"');
  });

  it("signs in by itself once the warning is acknowledged", async () => {
    const cookie = await noticeCookie();
    const page = await visit(context, `https://${host()}/__localify/access`, {
      headers: { ...navigation, cookie },
    });
    expect(page.status).toBe(200);
    const html = await page.text();
    expect(html).toContain("Password required");
    expect(html).not.toContain('name="notice"');
    expect(html).toContain("form.submit()");
  });
});

describe("the owner's traffic limit", () => {
  function ownerId(): string {
    const row = context.sqlite.prepare("SELECT user_id FROM public_urls WHERE id = ?").get(urlId);
    return (row as { user_id: string }).user_id;
  }

  it("is 2,000 requests a minute, keyed by owner", async () => {
    expect(rateLimit("OWNER_LIMITER")).toEqual({ limit: 2000, period: 60 });
    await makePublic();
    await visit(context, `https://${host()}/`);
    expect([...context.ownerLimiter.counts.keys()]).toEqual([ownerId()]);
  });

  it("lets a Vite page load its modules, several times a minute", async () => {
    await makePublic();
    const cookie = await noticeCookie();
    for (let reload = 0; reload < 4; reload++) {
      const page = await visit(context, `https://${host()}/`, {
        headers: { ...navigation, cookie },
      });
      expect(page.status).toBe(200);
      const modules = await Promise.all(
        Array.from({ length: 400 }, (_, index) =>
          visit(context, `https://${host("web")}/src/module-${index}.ts`, {
            headers: { "sec-fetch-dest": "script", origin: `https://${host()}` },
          }),
        ),
      );
      expect(modules.every((response) => response.status === 200)).toBe(true);
    }
    expect(machineCalls()).toHaveLength(4 * 401);
  });

  it("answers 429 with Retry-After across all the owner's URLs, until the next minute", async () => {
    await makePublic();
    context.ownerLimiter.counts.set(ownerId(), 2000);
    const page = await visit(context, `https://${host()}/`, { headers: navigation });
    expect(page.status).toBe(429);
    expect(page.headers.get("retry-after")).toBe("60");
    expect(page.headers.get("content-type")).toContain("text/html");
    expect(await page.text()).toContain("Too many requests");

    const requests: { method?: string; headers: Record<string, string>; body?: string }[] = [
      { headers: { "sec-fetch-dest": "script" } },
      { method: "POST", headers: {}, body: "x" },
    ];
    for (const init of requests) {
      const response = await visit(context, `https://${host("web")}/api`, init);
      expect(response.status).toBe(429);
      expect(response.headers.get("retry-after")).toBe("60");
      expect(response.headers.get("content-type")).toContain("text/plain");
    }
    expect(machineCalls()).toHaveLength(0);

    context.ownerLimiter.counts.clear();
    expect((await visit(context, `https://${host()}/`)).status).toBe(200);
  });

  it("lets traffic through when the limiter fails", async () => {
    await makePublic();
    context.env.OWNER_LIMITER = {
      limit: async () => {
        throw new Error("limiter down");
      },
    } as unknown as RateLimit;
    vi.spyOn(console, "error").mockImplementation(() => {});
    expect((await visit(context, `https://${host()}/`)).status).toBe(200);
  });
});
