// The JSON API on api.<MACHINE_DOMAIN> (docs/public-urls.md, "API"). The
// token gives the user and the calling machine; every read and change is
// scoped to that user, and an unknown id and someone else's id both answer
// 404.

import { CloudflareApi, CloudflareError } from "./cloudflare";
import { ConfigError, isAllowedGithubId, setting } from "./config";
import { hashPassword, sha256Hex } from "./crypto";
import { isUniqueViolation, urlOfUser, urlsOfMachine, urlsOfUser, type PublicUrl } from "./db";
import { fetchGitHubUser, pollDeviceFlow, startDeviceFlow } from "./github";
import { ApiError, errorResponse, json, noContent, readJson } from "./http";
import { ingressSecret, newId, signInMachine, signOutMachine } from "./machines";
import {
  isValidLocalDomain,
  isValidSlug,
  randomSlug,
  slugCandidates,
  SLUG_RULES,
} from "./names";
import { generatePassword } from "./passwords";
import { forgetSite } from "./sites";

export const MAX_URLS_PER_USER = 10;
const MIN_PASSWORD_LENGTH = 8;
const MAX_PASSWORD_LENGTH = 256;
const MAX_MACHINE_NAME_LENGTH = 64;
const SLUG_ATTEMPTS = 5;
/** GitHub asks for this much more time between polls after `slow_down`. */
const SLOW_DOWN_SECONDS = 5;

/** Who calls, from the bearer token. */
type Caller = {
  userId: string;
  login: string;
  machineId: string;
  machineName: string;
  tunnelId: string | null;
  dnsRecordId: string | null;
};

type Route = {
  method: string;
  pattern: RegExp;
  handle: (request: Request, env: Env, params: string[]) => Promise<Response>;
};

const ROUTES: Route[] = [
  { method: "POST", pattern: /^\/v1\/auth\/device$/, handle: deviceStart },
  { method: "POST", pattern: /^\/v1\/auth\/device\/token$/, handle: deviceToken },
  { method: "GET", pattern: /^\/v1\/me$/, handle: authed(me) },
  { method: "POST", pattern: /^\/v1\/auth\/logout$/, handle: authed(logout) },
  { method: "GET", pattern: /^\/v1\/slugs$/, handle: authed(suggestSlug) },
  { method: "GET", pattern: /^\/v1\/slugs\/([^/]+)$/, handle: authed(checkSlug) },
  { method: "GET", pattern: /^\/v1\/public-urls$/, handle: authed(listUrls) },
  { method: "POST", pattern: /^\/v1\/public-urls$/, handle: authed(createUrl) },
  { method: "DELETE", pattern: /^\/v1\/public-urls\/([^/]+)$/, handle: authed(deleteUrl) },
  {
    method: "PUT",
    pattern: /^\/v1\/public-urls\/([^/]+)\/protection$/,
    handle: authed(setProtection),
  },
  { method: "POST", pattern: /^\/v1\/public-urls\/([^/]+)\/claim$/, handle: authed(claimUrl) },
  { method: "GET", pattern: /^\/v1\/machines\/self\/runtime$/, handle: authed(runtime) },
];

export async function handleApi(request: Request, env: Env): Promise<Response> {
  const path = new URL(request.url).pathname;
  const matching = ROUTES.filter((route) => route.pattern.test(path));
  const route = matching.find((candidate) => candidate.method === request.method);
  try {
    if (!route) {
      throw matching.length > 0
        ? new ApiError(405, "method_not_allowed", `${request.method} is not allowed on ${path}.`)
        : new ApiError(
            404,
            "not_found",
            `There is no ${path} in the Localify Cloud API. Update Localify and try again.`,
          );
    }
    const params = (route.pattern.exec(path) ?? []).slice(1);
    return await route.handle(request, env, params);
  } catch (error) {
    return errorResponse(toApiError(error));
  }
}

function toApiError(error: unknown): ApiError {
  if (error instanceof ApiError) return error;
  if (error instanceof ConfigError) {
    console.error("Localify Cloud is not configured:", error.message);
    return new ApiError(
      500,
      "not_configured",
      "Localify Cloud is not fully set up yet. Tell the Localify team.",
    );
  }
  if (error instanceof CloudflareError) {
    console.error("Cloudflare API call failed:", error.message);
    return new ApiError(
      502,
      "cloudflare_unavailable",
      "Cloudflare did not answer as expected. Try again in a minute.",
    );
  }
  console.error("Unexpected API error", error);
  return new ApiError(
    500,
    "internal_error",
    "Something went wrong on Localify Cloud. Try again in a minute.",
  );
}

function authed(
  handler: (request: Request, env: Env, caller: Caller, params: string[]) => Promise<Response>,
): Route["handle"] {
  return async (request, env, params) =>
    handler(request, env, await authenticate(request, env), params);
}

async function authenticate(request: Request, env: Env): Promise<Caller> {
  const header = request.headers.get("authorization") ?? "";
  const match = /^Bearer\s+(lfy_[0-9a-f]{64})$/.exec(header.trim());
  const signedOut = new ApiError(
    401,
    "unauthorized",
    "You are not signed in to Localify Cloud. Run `localify login`.",
  );
  if (!match?.[1]) throw signedOut;
  const row = await env.DB.prepare(
    `SELECT u.id AS user_id, u.github_id, u.login, m.id AS machine_id, m.name AS machine_name,
            m.tunnel_id, m.dns_record_id
     FROM tokens t
     JOIN users u ON u.id = t.user_id
     JOIN machines m ON m.id = t.machine_id AND m.user_id = t.user_id
     WHERE t.hash = ?1 AND m.status = 'active'`,
  )
    .bind(await sha256Hex(match[1]))
    .first<{
      user_id: string;
      github_id: number;
      login: string;
      machine_id: string;
      machine_name: string;
      tunnel_id: string | null;
      dns_record_id: string | null;
    }>();
  if (!row) throw signedOut;
  if (!isAllowedGithubId(env, row.github_id)) throw notAllowed(row.login);
  return {
    userId: row.user_id,
    login: row.login,
    machineId: row.machine_id,
    machineName: row.machine_name,
    tunnelId: row.tunnel_id,
    dnsRecordId: row.dns_record_id,
  };
}

function notAllowed(login: string): ApiError {
  return new ApiError(
    403,
    "not_allowed",
    `Localify Cloud is invite-only for now, and the GitHub account ${login} is not on the list. Ask the Localify team for an invite.`,
  );
}

// --- Sign-in ---

/** Both device routes are rate limited per client IP. */
async function limitDeviceFlow(request: Request, env: Env): Promise<void> {
  const clientIp = request.headers.get("cf-connecting-ip") ?? "unknown";
  const { success } = await env.DEVICE_LIMITER.limit({ key: clientIp });
  if (!success) {
    throw new ApiError(
      429,
      "rate_limited",
      "Too many sign-in requests from this network. Wait a minute, then try again.",
    );
  }
}

async function deviceStart(request: Request, env: Env): Promise<Response> {
  await limitDeviceFlow(request, env);
  return json(await startDeviceFlow(setting(env, "GITHUB_CLIENT_ID")));
}

async function deviceToken(request: Request, env: Env): Promise<Response> {
  await limitDeviceFlow(request, env);
  const body = await readJson(request);
  const deviceCode = body.deviceCode;
  if (typeof deviceCode !== "string" || deviceCode === "" || deviceCode.length > 256) {
    throw invalid("deviceCode is missing. Run `localify login` again.");
  }
  const machineName = typeof body.machineName === "string" ? body.machineName.trim() : "";
  if (
    machineName === "" ||
    machineName.length > MAX_MACHINE_NAME_LENGTH ||
    /\p{Cc}/u.test(machineName)
  ) {
    throw invalid(
      `machineName must be 1 to ${MAX_MACHINE_NAME_LENGTH} characters without control characters.`,
    );
  }
  const ingressPort = body.ingressPort;
  if (
    typeof ingressPort !== "number" ||
    !Number.isInteger(ingressPort) ||
    ingressPort < 1 ||
    ingressPort > 65535
  ) {
    throw invalid("ingressPort must be a port number between 1 and 65535.");
  }

  const poll = await pollDeviceFlow(setting(env, "GITHUB_CLIENT_ID"), deviceCode);
  switch (poll.kind) {
    case "pending":
      throw new ApiError(
        428,
        "authorization_pending",
        "Waiting for you to enter the code on GitHub.",
      );
    case "slow_down": {
      const interval = (poll.interval ?? SLOW_DOWN_SECONDS) + SLOW_DOWN_SECONDS;
      return json(
        {
          error: "slow_down",
          message: `GitHub asks to check less often. Wait ${interval} seconds between checks.`,
          interval,
        },
        429,
      );
    }
    case "expired":
      throw new ApiError(
        410,
        "expired_token",
        "The sign-in code expired. Run `localify login` again.",
      );
    case "denied":
      throw new ApiError(
        403,
        "access_denied",
        "The sign-in was declined on GitHub. Run `localify login` to try again.",
      );
    case "invalid_code":
      throw new ApiError(
        400,
        "invalid_device_code",
        "GitHub does not know this sign-in code. Run `localify login` again.",
      );
    case "token":
      break;
  }

  // The GitHub token is used for this one read, then dropped.
  const github = await fetchGitHubUser(poll.accessToken);
  if (!isAllowedGithubId(env, github.id)) throw notAllowed(github.login);

  const user = await env.DB.prepare(
    `INSERT INTO users (id, github_id, login, created_at) VALUES (?1, ?2, ?3, ?4)
     ON CONFLICT (github_id) DO UPDATE SET login = excluded.login
     RETURNING id, login`,
  )
    .bind(newId(), github.id, github.login, Date.now())
    .first<{ id: string; login: string }>();
  if (!user) throw new Error("User upsert returned no row");

  const { machineId, token } = await signInMachine(env, user.id, machineName, ingressPort);
  return json({
    token,
    user: { login: user.login },
    machine: { id: machineId, name: machineName },
  });
}

async function me(_request: Request, _env: Env, caller: Caller): Promise<Response> {
  return json({
    user: { login: caller.login },
    machine: { id: caller.machineId, name: caller.machineName },
  });
}

async function logout(_request: Request, env: Env, caller: Caller): Promise<Response> {
  await signOutMachine(env, {
    id: caller.machineId,
    tunnelId: caller.tunnelId,
    dnsRecordId: caller.dnsRecordId,
  });
  return noContent();
}

// --- Slugs ---

/**
 * `GET /v1/slugs?localDomain=`: the slug a new URL for it would get, and that
 * URL, so a client never writes the public domain itself.
 */
async function suggestSlug(request: Request, env: Env, caller: Caller): Promise<Response> {
  const localDomain = readLocalDomain(new URL(request.url).searchParams.get("localDomain"));
  const slug = await freeSlug(env, caller, localDomain);
  return json({ slug, url: `https://${slug}.${baseDomain(env)}` });
}

/** `GET /v1/slugs/<slug>`: whether the caller can create a URL with it. */
async function checkSlug(
  _request: Request,
  env: Env,
  caller: Caller,
  [param]: string[],
): Promise<Response> {
  let slug: string;
  try {
    slug = decodeURIComponent(param ?? "")
      .trim()
      .toLowerCase();
  } catch {
    slug = "";
  }
  if (!isValidSlug(slug)) throw new ApiError(422, "invalid_slug", SLUG_RULES);
  const taken = await takenSlugs(env, caller, [slug]);
  return json({ slug, available: !taken.has(slug) });
}

/**
 * The first free slug for a local domain: its local name, then with the
 * owner's login, then with random characters. A create may still lose a
 * race for it, and then retries with random characters.
 */
async function freeSlug(env: Env, caller: Caller, localDomain: string): Promise<string> {
  const candidates = slugCandidates(localDomain, caller.login);
  for (let attempt = 0; attempt < SLUG_ATTEMPTS; attempt++) candidates.push(randomSlug(localDomain));
  const taken = await takenSlugs(env, caller, candidates);
  return candidates.find((slug) => !taken.has(slug)) ?? randomSlug(localDomain);
}

/**
 * Slugs held by a live URL, or by someone else's deleted one for 30 days.
 * The caller's own deleted slugs are free: a create takes them back.
 */
async function takenSlugs(env: Env, caller: Caller, slugs: string[]): Promise<Set<string>> {
  const placeholders = slugs.map((_, index) => `?${index + 2}`).join(", ");
  const { results } = await env.DB.prepare(
    `SELECT slug FROM public_urls
     WHERE slug IN (${placeholders}) AND (deleted_at IS NULL OR user_id <> ?1)`,
  )
    .bind(caller.userId, ...slugs)
    .all<{ slug: string }>();
  return new Set(results.map((row) => row.slug));
}

// --- Public URLs ---

async function listUrls(_request: Request, env: Env, caller: Caller): Promise<Response> {
  return json({ publicUrls: await urlsOfUser(env.DB, baseDomain(env), caller.userId) });
}

async function createUrl(request: Request, env: Env, caller: Caller): Promise<Response> {
  const body = await readJson(request);
  const localDomain = readLocalDomain(body.localDomain);
  if (body.wildcard !== undefined && typeof body.wildcard !== "boolean") {
    throw invalid("wildcard must be true or false.");
  }
  const wildcard = body.wildcard === true;
  let chosenSlug: string | null = null;
  if (body.slug !== undefined && body.slug !== null) {
    chosenSlug = typeof body.slug === "string" ? body.slug.trim().toLowerCase() : "";
    if (!isValidSlug(chosenSlug)) {
      throw new ApiError(422, "invalid_slug", `${SLUG_RULES} Choose another slug.`);
    }
  }
  const protection = readProtection(body.protection ?? { mode: "password" });
  const secret = protection.mode === "password" ? await hashPassword(protection.password) : null;

  const id = newId();
  for (let attempt = 1; ; attempt++) {
    const slug =
      chosenSlug ??
      (attempt === 1 ? await freeSlug(env, caller, localDomain) : randomSlug(localDomain));
    try {
      // The quota check is part of each statement, so concurrent creates
      // cannot both pass it. The owner may take back one of their own
      // deleted slugs; for everyone else it stays reserved for 30 days.
      const [, insert] = await env.DB.batch([
        env.DB.prepare(
          `DELETE FROM public_urls
           WHERE slug = ?1 AND user_id = ?2 AND deleted_at IS NOT NULL
             AND (SELECT COUNT(*) FROM public_urls WHERE user_id = ?2 AND deleted_at IS NULL) < ?3`,
        ).bind(slug, caller.userId, MAX_URLS_PER_USER),
        env.DB.prepare(
          `INSERT INTO public_urls (id, user_id, slug, local_domain, wildcard, machine_id, protected,
                                    password_hash, password_salt, password_version, created_at)
           SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10
           WHERE (SELECT COUNT(*) FROM public_urls WHERE user_id = ?2 AND deleted_at IS NULL) < ?11`,
        ).bind(
          id,
          caller.userId,
          slug,
          localDomain,
          wildcard ? 1 : 0,
          caller.machineId,
          secret ? 1 : 0,
          secret?.hash ?? null,
          secret?.salt ?? null,
          Date.now(),
          MAX_URLS_PER_USER,
        ),
      ]);
      if (!insert || insert.meta.changes === 0) {
        throw new ApiError(
          403,
          "quota_exceeded",
          `You already have ${MAX_URLS_PER_USER} public URLs, the most an account can have. Remove one with \`localify public-url remove\`, then try again.`,
        );
      }
      forgetSite(slug);
      break;
    } catch (error) {
      if (isUniqueViolation(error, "public_urls.slug")) {
        if (chosenSlug !== null) {
          throw new ApiError(
            409,
            "slug_taken",
            `The slug ${chosenSlug} is taken. Choose another one.`,
          );
        }
        if (attempt < SLUG_ATTEMPTS) continue;
      }
      if (isUniqueViolation(error, "public_urls.local_domain")) {
        throw new ApiError(
          409,
          "local_domain_taken",
          `${localDomain} already has a public URL. Run \`localify public-url list\` to see it, or remove it first.`,
        );
      }
      throw error;
    }
  }

  // The protection this request wrote, even if another one changed it since.
  const created = {
    ...(await requireUrl(env, caller, id)),
    protection: { mode: protection.mode, version: 1 },
  };
  return json(withPassword(created, protection), 201);
}

async function deleteUrl(
  _request: Request,
  env: Env,
  caller: Caller,
  [id]: string[],
): Promise<Response> {
  // The row stays 30 days, so the slug cannot serve someone else's page behind old bookmarks.
  const deleted = await env.DB.prepare(
    `UPDATE public_urls SET deleted_at = ?3 WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL RETURNING slug`,
  )
    .bind(id, caller.userId, Date.now())
    .first<{ slug: string }>();
  if (!deleted) throw urlNotFound();
  forgetSite(deleted.slug);
  return noContent();
}

async function setProtection(
  request: Request,
  env: Env,
  caller: Caller,
  [id]: string[],
): Promise<Response> {
  const protection = readProtection((await readJson(request)).protection);
  const secret = protection.mode === "password" ? await hashPassword(protection.password) : null;
  // Every change bumps the version, which signs every browser out.
  const updated = await env.DB.prepare(
    `UPDATE public_urls
     SET protected = ?3, password_hash = ?4, password_salt = ?5, password_version = password_version + 1
     WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL
     RETURNING slug, password_version`,
  )
    .bind(id, caller.userId, secret ? 1 : 0, secret?.hash ?? null, secret?.salt ?? null)
    .first<{ slug: string; password_version: number }>();
  if (!updated) throw urlNotFound();
  forgetSite(updated.slug);
  // The version of this very change, so a client never pairs its password
  // with the version of a change made at the same time elsewhere.
  const changed = {
    ...(await requireUrl(env, caller, id)),
    protection: { mode: protection.mode, version: updated.password_version },
  };
  return json(withPassword(changed, protection));
}

async function claimUrl(
  _request: Request,
  env: Env,
  caller: Caller,
  [id]: string[],
): Promise<Response> {
  const claimed = await env.DB.prepare(
    `UPDATE public_urls SET machine_id = ?3 WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL RETURNING slug`,
  )
    .bind(id, caller.userId, caller.machineId)
    .first<{ slug: string }>();
  if (!claimed) throw urlNotFound();
  forgetSite(claimed.slug);
  return json({ publicUrl: await requireUrl(env, caller, id) });
}

async function runtime(_request: Request, env: Env, caller: Caller): Promise<Response> {
  if (!caller.tunnelId) throw new Error(`Active machine ${caller.machineId} has no tunnel`);
  const [tunnelToken, secret, publicUrls] = await Promise.all([
    new CloudflareApi(env).tunnelToken(caller.tunnelId),
    ingressSecret(env, caller.machineId),
    urlsOfMachine(env.DB, baseDomain(env), caller.machineId),
  ]);
  return json({ tunnelToken, ingressSecret: secret, publicUrls });
}

// --- Helpers ---

function readLocalDomain(value: unknown): string {
  const localDomain = typeof value === "string" ? value.trim().toLowerCase() : "";
  if (!isValidLocalDomain(localDomain)) {
    throw new ApiError(
      422,
      "invalid_local_domain",
      "localDomain must be a local domain such as journal.localify: lowercase letters, digits and hyphens, separated by dots.",
    );
  }
  return localDomain;
}

function baseDomain(env: Env): string {
  return setting(env, "PUBLIC_BASE_DOMAIN");
}

function urlNotFound(): ApiError {
  return new ApiError(
    404,
    "not_found",
    "Your account has no public URL with this id. Run `localify public-url list` to see yours.",
  );
}

async function requireUrl(env: Env, caller: Caller, id: string | undefined): Promise<PublicUrl> {
  const url = id ? await urlOfUser(env.DB, baseDomain(env), caller.userId, id) : null;
  if (!url) throw urlNotFound();
  return url;
}

type Protection = { mode: "public" } | { mode: "password"; password: string; generated: boolean };

/** `password` is in the response only when the server generated it: it is shown once. */
function withPassword(
  publicUrl: PublicUrl,
  protection: Protection,
): { publicUrl: PublicUrl; password?: string } {
  return protection.mode === "password" && protection.generated
    ? { publicUrl, password: protection.password }
    : { publicUrl };
}

function readProtection(value: unknown): Protection {
  const protection = (typeof value === "object" && value !== null ? value : {}) as {
    mode?: unknown;
    password?: unknown;
  };
  if (protection.mode === "public") return { mode: "public" };
  if (protection.mode !== "password") {
    throw invalid('protection.mode must be "password" or "public".');
  }
  if (protection.password === undefined || protection.password === null) {
    return { mode: "password", password: generatePassword(), generated: true };
  }
  if (
    typeof protection.password !== "string" ||
    protection.password.length < MIN_PASSWORD_LENGTH ||
    protection.password.length > MAX_PASSWORD_LENGTH
  ) {
    throw new ApiError(
      422,
      "invalid_password",
      `Choose a password of ${MIN_PASSWORD_LENGTH} to ${MAX_PASSWORD_LENGTH} characters, or leave it out to get a generated one.`,
    );
  }
  return { mode: "password", password: protection.password, generated: false };
}

function invalid(message: string): ApiError {
  return new ApiError(422, "invalid_request", message);
}
