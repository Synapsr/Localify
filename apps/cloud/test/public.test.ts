import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ACCESS_COOKIE } from "../src/cookie";
import { hmacHex } from "../src/crypto";
import { api, createContext, serve, signIn, visit, type TestContext } from "./support";

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
