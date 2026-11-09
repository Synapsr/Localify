import { getCloudflareContext } from "@opennextjs/cloudflare";

import { routing } from "@/i18n/routing";

// What /api/track and /api/contact share: the Worker's bindings, per-IP limits, a
// bounded JSON body, and the post to the team's Discord webhook. The webhook URL is
// a secret: it never reaches the browser, a log line, or a response.

type RateLimiter = { limit: (options: { key: string }) => Promise<{ success: boolean }> };

declare global {
  interface CloudflareEnv {
    // A secret, set with `wrangler secret put`; locally, in apps/web/.dev.vars.
    DISCORD_CONTACT_WEBHOOK_URL?: string;
    // Rate-limit bindings from wrangler.jsonc.
    TRACK_LIMITER?: RateLimiter;
    CONTACT_LIMITER?: RateLimiter;
  }
}

export const getEnv = () => getCloudflareContext().env;

export const emptyResponse = (status: number) => new Response(null, { status });

// Browsers say where a request comes from; only the site's own pages may post. A
// client that sends no Sec-Fetch-Site header isn't a browser, and the limits cover it.
export const isCrossSite = (request: Request) => {
  const site = request.headers.get("sec-fetch-site");
  return site !== null && site !== "same-origin";
};

// Counts this request against the client IP's budget. Without the binding there is
// nothing to count against, so the request goes through.
export const isRateLimited = async (limiter: RateLimiter | undefined, request: Request) => {
  if (!limiter) return false;
  const key = request.headers.get("cf-connecting-ip") ?? "unknown";
  const { success } = await limiter.limit({ key });
  return !success;
};

// The body as JSON, or undefined when it isn't JSON or is larger than maxBytes. It
// stops reading at the limit, so a large body costs no more than that.
export const readJson = async (request: Request, maxBytes: number): Promise<unknown> => {
  if (Number(request.headers.get("content-length") ?? 0) > maxBytes) return undefined;
  const reader = request.body?.getReader();
  if (!reader) return undefined;

  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > maxBytes) {
      await reader.cancel();
      return undefined;
    }
    chunks.push(value);
  }

  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  try {
    return JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    return undefined;
  }
};

// Control characters out, except newlines; surrounding space trimmed. Null when the
// value isn't a string or is longer than max.
export const cleanText = (value: unknown, max: number): string | null => {
  if (typeof value !== "string") return null;
  // eslint-disable-next-line no-control-regex
  const text = value.replace(/[\u0000-\u0009\u000B-\u001F\u007F]/g, "").trim();
  return text.length <= max ? text : null;
};

export const isInterval = (value: unknown): value is "monthly" | "yearly" =>
  value === "monthly" || value === "yearly";

export const isLocale = (value: unknown): value is string =>
  typeof value === "string" && (routing.locales as readonly string[]).includes(value);

// A site path such as "/" or "/pricing": nothing Discord could render as a link or
// a mention.
export const isPage = (value: unknown): value is string =>
  typeof value === "string" && value.length <= 100 && /^\/[a-z0-9\-/]*$/i.test(value);

// Posts to the webhook. Mentions are off, so nothing a visitor types can ping anyone.
export const postToDiscord = async (webhook: string, message: object) => {
  try {
    const response = await fetch(webhook, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ ...message, allowed_mentions: { parse: [] } }),
      signal: AbortSignal.timeout(5000),
    });
    if (!response.ok) console.error(`Discord webhook answered ${response.status}`);
    return response.ok;
  } catch (error) {
    console.error(`Discord webhook failed: ${error instanceof Error ? error.name : "unknown error"}`);
    return false;
  }
};
