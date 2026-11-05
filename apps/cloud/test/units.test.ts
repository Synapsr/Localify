import { describe, expect, it } from "vitest";

import {
  ACCESS_COOKIE,
  accessCookieHeader,
  readCookie,
  signAccess,
  verifyAccess,
  withoutCookie,
} from "../src/cookie";
import { hashPassword, hmacHex, verifyPassword } from "../src/crypto";
import { upstreamHeaders, withHostOnlyCookies, withoutDomainAttribute } from "../src/headers";
import {
  generateSlug,
  isOriginOfUrl,
  isValidLabel,
  isValidLocalDomain,
  isValidSlug,
  localHostFor,
  parsePublicHost,
  slugBase,
} from "../src/names";
import { generatePassword, WORDS } from "../src/passwords";
import { safeNext } from "../src/public";

describe("public hosts", () => {
  const base = "localify.fyi";

  it("reads a slug, and a slug with a label after the first --", () => {
    expect(parsePublicHost("journal-a8f3.localify.fyi", base)).toEqual({
      slug: "journal-a8f3",
      label: null,
    });
    expect(parsePublicHost("journal-a8f3--web.localify.fyi", base)).toEqual({
      slug: "journal-a8f3",
      label: "web",
    });
    expect(parsePublicHost("journal-a8f3--web--feature.localify.fyi", base)).toEqual({
      slug: "journal-a8f3",
      label: "web--feature",
    });
    expect(parsePublicHost("Journal-A8F3.Localify.FYI", base)).toEqual({
      slug: "journal-a8f3",
      label: null,
    });
  });

  it("refuses other domains, nested names, and invalid slugs or labels", () => {
    for (const host of [
      "localify.fyi",
      "journal.localify.dev",
      "a.b.localify.fyi",
      "ab.localify.fyi",
      "1journal.localify.fyi",
      "journal-.localify.fyi",
      "journal-a8f3--.localify.fyi",
      "journal-a8f3---web.localify.fyi",
      "journal-a8f3--web-.localify.fyi",
      "--web.localify.fyi",
      "journal_a8f3.localify.fyi",
      `journal-a8f3--${"a".repeat(50)}.localify.fyi`,
      "evillocalify.fyi",
    ]) {
      expect(parsePublicHost(host, base), host).toBeNull();
    }
  });

  it("maps a label to a subdomain of the local domain", () => {
    expect(localHostFor("journal.localify", null)).toBe("journal.localify");
    expect(localHostFor("journal.localify", "web--feature")).toBe("web--feature.journal.localify");
    expect(localHostFor(`${"a".repeat(250)}.x`, "web")).toBeNull();
  });
});

describe("slugs and labels", () => {
  it("follows the slug rules", () => {
    for (const slug of ["abc", "journal-a8f3", "a1-b2", "a".repeat(32)])
      expect(isValidSlug(slug), slug).toBe(true);
    for (const slug of [
      "ab",
      "a".repeat(33),
      "1abc",
      "-abc",
      "abc-",
      "ab--c",
      "Abc",
      "ab_c",
      "ab.c",
      "",
    ]) {
      expect(isValidSlug(slug), slug).toBe(false);
    }
  });

  it("follows the label rules", () => {
    for (const label of ["a", "web", "web--feature", "1", "a-1"])
      expect(isValidLabel(label), label).toBe(true);
    for (const label of ["", "-web", "web-", "Web", "we.b", "a".repeat(64)])
      expect(isValidLabel(label), label).toBe(false);
  });

  it("checks local domains", () => {
    expect(isValidLocalDomain("journal.localify")).toBe(true);
    expect(isValidLocalDomain("web.journal.localify")).toBe(true);
    for (const domain of [
      "journal",
      "Journal.localify",
      "journal..localify",
      "-x.localify",
      "journal.localify.",
      "",
    ]) {
      expect(isValidLocalDomain(domain), domain).toBe(false);
    }
  });

  it("generates a slug from the normalized first label and 4 hex characters", () => {
    expect(generateSlug("journal.localify")).toMatch(/^journal-[0-9a-f]{4}$/);
    for (let i = 0; i < 20; i++) expect(isValidSlug(generateSlug("journal.localify"))).toBe(true);
  });

  it("normalizes the first label as the contract says", () => {
    expect(slugBase("journal.localify")).toBe("journal");
    expect(slugBase("My_App.localify")).toBe("my-app");
    expect(slugBase("web--feature.app.localify")).toBe("web-feature");
    expect(slugBase("2fa-app.localify")).toBe("fa-app");
    expect(slugBase("--42--x.localify")).toBe("x");
    expect(slugBase("42.localify")).toBe("url");
    expect(slugBase("---.localify")).toBe("url");
    // Cut to 20 characters, then no trailing hyphen.
    expect(slugBase("abcdefghijklmnopqrs-tuvwxyz.localify")).toBe("abcdefghijklmnopqrs");
    expect(slugBase(`${"x".repeat(60)}.localify`)).toBe("x".repeat(20));
    const longest = generateSlug(`${"x".repeat(60)}.localify`);
    expect(longest).toHaveLength(25);
    expect(isValidSlug(longest)).toBe(true);
  });
});

describe("the origin rule", () => {
  const base = "localify.fyi";

  it("accepts the URL's own host, and its labels when it is a wildcard URL", () => {
    expect(isOriginOfUrl("https://journal-a8f3.localify.fyi", "journal-a8f3", false, base)).toBe(
      true,
    );
    expect(
      isOriginOfUrl("https://journal-a8f3--web.localify.fyi", "journal-a8f3", true, base),
    ).toBe(true);
    expect(
      isOriginOfUrl("https://journal-a8f3--web--x.localify.fyi", "journal-a8f3", true, base),
    ).toBe(true);
  });

  it("refuses other URLs, labels of a plain URL, other schemes and ports, and junk", () => {
    for (const origin of [
      "https://other-b7c1.localify.fyi",
      "https://other-b7c1--journal-a8f3.localify.fyi",
      "https://journal-a8f3.localify.fyi.evil.example",
      "https://journal-a8f3.evil.example",
      "http://journal-a8f3.localify.fyi",
      "https://journal-a8f3.localify.fyi:8443",
      "https://journal-a8f3.localify.fyi/",
      "null",
      "",
    ]) {
      expect(isOriginOfUrl(origin, "journal-a8f3", true, base), origin).toBe(false);
    }
    expect(
      isOriginOfUrl("https://journal-a8f3--web.localify.fyi", "journal-a8f3", false, base),
    ).toBe(false);
  });
});

describe("access cookie", () => {
  const scope = { host: "journal-a8f3.localify.fyi", urlId: "url-1", version: 3 };
  const now = Date.UTC(2026, 10, 5);

  it("verifies for the same host, URL and password version within 7 days", async () => {
    const value = await signAccess("key", scope, now);
    expect(value).toMatch(/^\d+\.3\.[0-9a-f]{64}$/);
    expect(await verifyAccess("key", value, scope, now)).toBe(true);
    expect(await verifyAccess("key", value, scope, now + 7 * 24 * 3600 * 1000 - 1000)).toBe(true);
  });

  it("refuses an expired cookie", async () => {
    const value = await signAccess("key", scope, now);
    expect(await verifyAccess("key", value, scope, now + 7 * 24 * 3600 * 1000)).toBe(false);
  });

  it("refuses another host, URL, version or key", async () => {
    const value = await signAccess("key", scope, now);
    expect(
      await verifyAccess("key", value, { ...scope, host: "journal-a8f3--web.localify.fyi" }, now),
    ).toBe(false);
    expect(await verifyAccess("key", value, { ...scope, urlId: "url-2" }, now)).toBe(false);
    expect(await verifyAccess("key", value, { ...scope, version: 4 }, now)).toBe(false);
    expect(await verifyAccess("other", value, scope, now)).toBe(false);
  });

  it("refuses a tampered version or expiry", async () => {
    const value = await signAccess("key", scope, now);
    const [expiry, , signature] = value.split(".");
    expect(
      await verifyAccess("key", `${expiry}.4.${signature}`, { ...scope, version: 4 }, now),
    ).toBe(false);
    expect(await verifyAccess("key", `${Number(expiry) + 100}.3.${signature}`, scope, now)).toBe(
      false,
    );
    expect(await verifyAccess("key", "garbage", scope, now)).toBe(false);
  });

  it("is set host-only, secure, HTTP-only and Lax", () => {
    const header = accessCookieHeader("v");
    expect(header).toBe(
      `${ACCESS_COOKIE}=v; Max-Age=604800; Path=/; Secure; HttpOnly; SameSite=Lax`,
    );
    expect(header).not.toMatch(/domain=/i);
  });

  it("is read from and removed from a Cookie header", () => {
    const header = `a=1; ${ACCESS_COOKIE}=x; b=2; ${ACCESS_COOKIE}=y`;
    expect(readCookie(header, ACCESS_COOKIE)).toEqual(["x", "y"]);
    expect(withoutCookie(header, ACCESS_COOKIE)).toBe("a=1; b=2");
    expect(withoutCookie(`${ACCESS_COOKIE}=x`, ACCESS_COOKIE)).toBeNull();
    expect(withoutCookie(null, ACCESS_COOKIE)).toBeNull();
  });
});

describe("passwords", () => {
  it("hashes and verifies a password", async () => {
    const stored = await hashPassword("correct-horse");
    expect(stored.hash).toMatch(/^[0-9a-f]{64}$/);
    expect(stored.salt).toMatch(/^[0-9a-f]{32}$/);
    expect(await verifyPassword("correct-horse", stored)).toBe(true);
    expect(await verifyPassword("correct-horsf", stored)).toBe(false);
    expect((await hashPassword("correct-horse")).salt).not.toBe(stored.salt);
  });

  it("generates 4 lowercase words from the list joined by -", () => {
    expect(WORDS.length).toBe(512);
    expect(new Set(WORDS).size).toBe(512);
    for (const word of WORDS) expect(word).toMatch(/^[a-z]{3,7}$/);
    for (let i = 0; i < 50; i++) {
      const password = generatePassword();
      const words = password.split("-");
      expect(words).toHaveLength(4);
      for (const word of words) expect(WORDS).toContain(word);
    }
  });
});

describe("upstream headers", () => {
  it("strips proxy, Cloudflare and Localify headers and the access cookie, then adds Localify's", () => {
    const incoming = new Headers({
      host: "journal-a8f3.localify.fyi",
      forwarded: "for=1.2.3.4",
      "x-real-ip": "1.2.3.4",
      "x-forwarded-for": "1.2.3.4",
      "x-forwarded-host": "evil",
      "x-localify-ingress": "forged",
      "x-localify-local-host": "other.localify",
      "cf-connecting-ip": "5.6.7.8",
      "cf-ray": "abc",
      cookie: `session=1; ${ACCESS_COOKIE}=secret`,
      accept: "text/html",
      upgrade: "websocket",
    });
    const headers = upstreamHeaders(incoming, {
      ingressSecret: "ingress",
      publicHost: "journal-a8f3.localify.fyi",
      localHost: "journal.localify",
      urlId: "url-1",
      clientIp: "5.6.7.8",
    });
    expect(Object.fromEntries(headers)).toEqual({
      accept: "text/html",
      cookie: "session=1",
      upgrade: "websocket",
      "x-localify-ingress": "ingress",
      "x-localify-public-host": "journal-a8f3.localify.fyi",
      "x-localify-local-host": "journal.localify",
      "x-localify-url-id": "url-1",
      "x-localify-client-ip": "5.6.7.8",
    });
  });

  it("drops the Cookie header when only the access cookie was in it", () => {
    const headers = upstreamHeaders(new Headers({ cookie: `${ACCESS_COOKIE}=secret` }), {
      ingressSecret: "i",
      publicHost: "p",
      localHost: "l",
      urlId: "u",
      clientIp: null,
    });
    expect(headers.has("cookie")).toBe(false);
    expect(headers.has("x-localify-client-ip")).toBe(false);
  });
});

describe("upstream Set-Cookie", () => {
  it("loses its Domain attribute and keeps the rest", () => {
    expect(withoutDomainAttribute("sid=1; Domain=localify.fyi; Path=/; HttpOnly")).toBe(
      "sid=1; Path=/; HttpOnly",
    );
    expect(withoutDomainAttribute("sid=1; path=/; DOMAIN=.localify.fyi; Secure")).toBe(
      "sid=1; path=/; Secure",
    );
    expect(withoutDomainAttribute("sid=1;domain=x")).toBe("sid=1");
    expect(withoutDomainAttribute("sid=1; Path=/")).toBe("sid=1; Path=/");
    // A cookie named domain is not the attribute.
    expect(withoutDomainAttribute("domain=1; Path=/")).toBe("domain=1; Path=/");
  });

  it("is rewritten on every cookie of a response, which keeps its status and body", async () => {
    const headers = new Headers({ "content-type": "text/plain" });
    headers.append("set-cookie", "a=1; Domain=localify.fyi; Path=/");
    headers.append("set-cookie", "b=2; Path=/");
    const response = withHostOnlyCookies(
      new Response("body", { status: 201, statusText: "Created", headers }),
    );
    expect(response.headers.getSetCookie()).toEqual(["a=1; Path=/", "b=2; Path=/"]);
    expect(response.status).toBe(201);
    expect(response.headers.get("content-type")).toBe("text/plain");
    expect(await response.text()).toBe("body");
  });

  it("leaves a response without Domain attributes untouched", () => {
    const original = new Response("x", { headers: { "set-cookie": "a=1; Path=/" } });
    expect(withHostOnlyCookies(original)).toBe(original);
  });
});

describe("the next path after the password", () => {
  const host = "journal-a8f3.localify.fyi";

  it("keeps a path and query on the same host", () => {
    expect(safeNext("/docs?page=2", host)).toBe("/docs?page=2");
    expect(safeNext("/", host)).toBe("/");
  });

  it("falls back to / for anything that could leave the host", () => {
    for (const next of [
      null,
      "",
      "https://evil.example/",
      "//evil.example/",
      "/\\evil.example",
      "docs",
      "/__localify/access",
    ]) {
      expect(safeNext(next, host), String(next)).toBe("/");
    }
  });
});

describe("ingress secret", () => {
  it("is the hex HMAC-SHA-256 of the machine id", async () => {
    // RFC 4231 test case 2.
    expect(await hmacHex("Jefe", "what do ya want for nothing?")).toBe(
      "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
    );
  });
});
