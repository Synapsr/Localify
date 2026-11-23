/// <reference types="node" />
// The dashboard's HTTP side: a cache of one read of D1, and the routes. GET
// and HEAD only, no route takes input that reaches SQL, and nothing served
// carries a secret: the reads never select one (`model.ts`).

import type { IncomingMessage, ServerResponse } from "node:http";

import { randomBytes } from "node:crypto";

import type { ReadDb } from "./db.ts";

import { buildDashboard, readRaw, type Dashboard } from "./model.ts";
import { messagePage, overviewPage, userPage, type PageMeta } from "./page.ts";

/** How long a read serves before the next page load reads D1 again. */
export const CACHE_MS = 30_000;
/** The refresh button reads again at most this often, whoever presses it. */
export const MIN_RELOAD_MS = 5_000;

export type Loader = (fresh: boolean) => Promise<Dashboard>;

/**
 * One read shared by every request while it runs, kept `CACHE_MS`. A
 * failed read is not kept: the next request tries again.
 */
export function cachedLoader(db: ReadDb, clock: () => number = Date.now): Loader {
  let cached: Dashboard | null = null;
  let running: Promise<Dashboard> | null = null;
  return (fresh) => {
    const now = clock();
    const age = cached ? now - cached.readAt : Infinity;
    if (cached && age < (fresh ? MIN_RELOAD_MS : CACHE_MS)) return Promise.resolve(cached);
    running ??= readRaw(db)
      .then((raw) => (cached = buildDashboard(raw, clock())))
      .finally(() => (running = null));
    return running;
  };
}

/**
 * Loopback, and `.localify` names: the Localify proxy, and a public URL,
 * whose daemon forwards the local host. Any other `Host` is a page that
 * resolved its own name to this machine, and gets nothing.
 */
export function allowedHost(host: string | undefined): boolean {
  if (!host) return false;
  const name = host.replace(/:\d+$/, "").toLowerCase();
  return (
    name === "localhost" ||
    name === "127.0.0.1" ||
    name === "[::1]" ||
    name === "localify" ||
    name.endsWith(".localify")
  );
}

function headers(nonce: string): Record<string, string> {
  return {
    "content-type": "text/html; charset=utf-8",
    "cache-control": "no-store",
    "content-security-policy": `default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-${nonce}'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`,
    "x-content-type-options": "nosniff",
    "x-frame-options": "DENY",
    "referrer-policy": "no-referrer",
    "x-robots-tag": "noindex, nofollow",
  };
}

export type Options = { source: PageMeta["source"]; clock?: () => number };

/** The request handler over a loader. Errors are logged here, never served. */
export function createHandler(load: Loader, { source, clock = Date.now }: Options) {
  let lastReadAt: number | null = null;
  return async (request: IncomingMessage, response: ServerResponse): Promise<void> => {
    const nonce = randomBytes(16).toString("base64");
    const url = new URL(request.url ?? "/", "http://dashboard");
    const send = (status: number, html: string, extra: Record<string, string> = {}) => {
      response.writeHead(status, { ...headers(nonce), ...extra });
      response.end(request.method === "HEAD" ? undefined : html);
    };
    const meta = (): PageMeta => ({
      source,
      readAt: lastReadAt,
      now: clock(),
      nonce,
      path: url.pathname,
    });

    if (!allowedHost(request.headers.host)) {
      send(421, "Unknown host.", { "content-type": "text/plain; charset=utf-8" });
      return;
    }
    if (request.method !== "GET" && request.method !== "HEAD") {
      send(405, messagePage("Read-only", "This dashboard only reads.", meta()), {
        allow: "GET, HEAD",
      });
      return;
    }
    if (url.pathname === "/favicon.ico") {
      response.writeHead(204, { "cache-control": "max-age=86400" });
      response.end();
      return;
    }
    const userMatch = /^\/users\/([^/]+)$/.exec(url.pathname);
    if (url.pathname !== "/" && !userMatch) {
      send(404, messagePage("Not found", "No such page.", meta()));
      return;
    }

    let dashboard: Dashboard;
    try {
      dashboard = await load(url.searchParams.has("reload"));
      lastReadAt = dashboard.readAt;
    } catch (error) {
      console.error("Reading D1 failed:", error);
      send(
        503,
        messagePage(
          "D1 could not be read",
          "The terminal running the dashboard has the error. Try again in a moment.",
          meta(),
        ),
      );
      return;
    }
    if (!userMatch) {
      send(200, overviewPage(dashboard, meta()));
      return;
    }
    let id: string;
    try {
      id = decodeURIComponent(userMatch[1]!);
    } catch {
      id = "";
    }
    const user = dashboard.users.find((candidate) => candidate.id === id);
    if (!user) {
      send(404, messagePage("Not found", "No account has this id.", meta()));
      return;
    }
    send(200, userPage(user, meta()));
  };
}
