// The JSON API on api.<MACHINE_DOMAIN> (docs/public-urls.md, "API"). The
// token gives the user and the calling machine; every read and change is
// scoped to that user, and an unknown id and someone else's id both answer
// 404.

import { CloudflareError } from "./cloudflare";
import { ConfigError, isAllowedGithubId, setting, signup } from "./config";
import { hashPassword, sha256Hex } from "./crypto";
import { isUniqueViolation, urlOfUser, urlsOfUser, type PublicUrl } from "./db";
import {
  entitlementOf,
  isEntitled,
  loadAccount,
  servingUrlLimit,
  type Entitlement,
} from "./entitlement";
import { fetchGitHubUser, pollDeviceFlow, startDeviceFlow } from "./github";
import { ApiError, errorResponse, json, noContent, readJson } from "./http";
import { newId, signInMachine, signOutMachine } from "./machines";
import {
  isValidLocalDomain,
  isValidSlug,
  randomSlug,
  slugCandidates,
  SLUG_RULES,
} from "./names";
import { generatePassword } from "./passwords";
import { forgetSite } from "./sites";
import { background, machineRuntime, reconcileOwner, releaseIfUnwanted } from "./tunnels";

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
  banned: boolean;
  machineId: string;
  machineName: string;
};

type Route = {
  method: string;
  pattern: RegExp;
  handle: (
    request: Request,
    env: Env,
    ctx: ExecutionContext,
    params: string[],
  ) => Promise<Response>;
};

const ROUTES: Route[] = [
  { method: "POST", pattern: /^\/v1\/auth\/device$/, handle: deviceStart },
  { method: "POST", pattern: /^\/v1\/auth\/device\/token$/, handle: deviceToken },
  { method: "GET", pattern: /^\/v1\/me$/, handle: authed(me) },
  // Signing out is the one route a banned account keeps.
  { method: "POST", pattern: /^\/v1\/auth\/logout$/, handle: authed(logout, { banned: true }) },
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
  { method: "GET", pattern: /^\/v2\/machines\/self\/runtime$/, handle: authed(runtimeV2) },
  { method: "GET", pattern: /^\/v1\/machines\/self\/runtime$/, handle: authed(runtimeV1) },
];

export async function handleApi(
  request: Request,
  env: Env,
  ctx: ExecutionContext,
): Promise<Response> {
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
    return await route.handle(request, env, ctx, params);
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

/** A route behind the bearer token. A banned account gets `403 banned` unless `banned` lets it in. */
function authed(
  handler: (
    request: Request,
    env: Env,
    caller: Caller,
    params: string[],
    ctx: ExecutionContext,
  ) => Promise<Response>,
  { banned = false }: { banned?: boolean } = {},
): Route["handle"] {
  return async (request, env, ctx, params) => {
    const caller = await authenticate(request, env);
    if (caller.banned && !banned) throw bannedError();
    return handler(request, env, caller, params, ctx);
  };
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
    `SELECT u.id AS user_id, u.login, u.banned_at, m.id AS machine_id, m.name AS machine_name
     FROM tokens t
     JOIN users u ON u.id = t.user_id
     JOIN machines m ON m.id = t.machine_id AND m.user_id = t.user_id
     WHERE t.hash = ?1 AND m.status = 'active'`,
  )
    .bind(await sha256Hex(match[1]))
    .first<{
      user_id: string;
      login: string;
      banned_at: number | null;
      machine_id: string;
      machine_name: string;
    }>();
  // The allowlist only gates sign-in: a signed-in account keeps its token.
  if (!row) throw signedOut;
  return {
    userId: row.user_id,
    login: row.login,
    banned: row.banned_at !== null,
    machineId: row.machine_id,
    machineName: row.machine_name,
  };
}

function bannedError(): ApiError {
  return new ApiError(
    403,
    "banned",
    "This Localify Cloud account was suspended for breaking the terms. Write to abuse@localify.dev if you think this is a mistake.",
  );
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
  const known = await env.DB.prepare(`SELECT banned_at FROM users WHERE github_id = ?1`)
    .bind(github.id)
    .first<{ banned_at: number | null }>();
  if (known && known.banned_at !== null) throw bannedError();
  // SIGNUP decides who may finish a sign-in, nothing else.
  if (signup(env) === "allowlist" && !isAllowedGithubId(env, github.id)) {
    throw notAllowed(github.login);
  }

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

async function me(_request: Request, env: Env, caller: Caller): Promise<Response> {
  const account = await loadAccount(env.DB, caller.userId);
  if (!account) throw new Error(`User ${caller.userId} has no row`);
  // A slot is held by any machine with a tunnel, a revoked one still releasing included.
  const usage = await env.DB.prepare(
    `SELECT (SELECT COUNT(*) FROM machines WHERE user_id = ?1 AND tunnel_state != 'none') AS machines,
            (SELECT COUNT(*) FROM public_urls
             WHERE user_id = ?1 AND deleted_at IS NULL AND taken_down_at IS NULL) AS urls`,
  )
    .bind(caller.userId)
    .first<{ machines: number; urls: number }>();
  const { addons, ...plan } = account.plan;
  return json({
    user: { login: caller.login },
    machine: { id: caller.machineId, name: caller.machineName },
    plan: {
      ...plan,
      usage: { servingMachines: usage?.machines ?? 0, publicUrls: usage?.urls ?? 0 },
      addons,
    },
  });
}

async function logout(_request: Request, env: Env, caller: Caller): Promise<Response> {
  await signOutMachine(env, caller.machineId);
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
 * Slugs held by a live URL, by someone else's deleted one for 30 days, or
 * by a taken-down one forever. The caller's own deleted slugs are free: a
 * create takes them back.
 */
async function takenSlugs(env: Env, caller: Caller, slugs: string[]): Promise<Set<string>> {
  const placeholders = slugs.map((_, index) => `?${index + 2}`).join(", ");
  const { results } = await env.DB.prepare(
    `SELECT slug FROM public_urls
     WHERE slug IN (${placeholders})
       AND (deleted_at IS NULL OR user_id <> ?1 OR taken_down_at IS NOT NULL)`,
  )
    .bind(caller.userId, ...slugs)
    .all<{ slug: string }>();
  return new Set(results.map((row) => row.slug));
}

// --- Public URLs ---

async function listUrls(_request: Request, env: Env, caller: Caller): Promise<Response> {
  return json({
    publicUrls: await urlsOfUser(
      env.DB,
      baseDomain(env),
      caller.userId,
      servingUrlLimit(await entitlementOf(env, caller.userId)),
    ),
  });
}

async function createUrl(
  request: Request,
  env: Env,
  caller: Caller,
  _params: string[],
  ctx: ExecutionContext,
): Promise<Response> {
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
  const entitlement = await requireEntitlement(env, caller);
  const secret = protection.mode === "password" ? await hashPassword(protection.password) : null;

  // A live URL is neither deleted nor taken down; suspended ones count.
  const live = `(SELECT COUNT(*) FROM public_urls
                 WHERE user_id = ?2 AND deleted_at IS NULL AND taken_down_at IS NULL)`;
  const id = newId();
  for (let attempt = 1; ; attempt++) {
    const slug =
      chosenSlug ??
      (attempt === 1 ? await freeSlug(env, caller, localDomain) : randomSlug(localDomain));
    try {
      // The quota check is part of each statement, so concurrent creates
      // cannot both pass it. The owner may take back one of their own
      // deleted slugs; for everyone else it stays reserved for 30 days, and
      // a taken-down one stays held forever.
      const [, insert] = await env.DB.batch([
        env.DB.prepare(
          `DELETE FROM public_urls
           WHERE slug = ?1 AND user_id = ?2 AND deleted_at IS NOT NULL AND taken_down_at IS NULL
             AND ${live} < ?3`,
        ).bind(slug, caller.userId, entitlement.publicUrls),
        env.DB.prepare(
          `INSERT INTO public_urls (id, user_id, slug, local_domain, wildcard, machine_id, protected,
                                    password_hash, password_salt, password_version, created_at)
           SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10
           WHERE ${live} < ?11`,
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
          entitlement.publicUrls,
        ),
      ]);
      if (!insert || insert.meta.changes === 0) {
        throw new ApiError(
          403,
          "quota_exceeded",
          `You already have ${entitlement.publicUrls} public URLs, the most your plan allows. Remove one with \`localify public-url remove\`, then try again.`,
        );
      }
      forgetSite(slug);
      // The new URL serves: it can only end a suspension recorded earlier.
      background(ctx, reconcileOwner(env, caller.userId));
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
  ctx: ExecutionContext,
): Promise<Response> {
  // The row stays 30 days, so the slug cannot serve someone else's page behind old bookmarks.
  const deleted = await env.DB.prepare(
    `UPDATE public_urls SET deleted_at = ?3 WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL
     RETURNING slug, machine_id`,
  )
    .bind(id, caller.userId, Date.now())
    .first<{ slug: string; machine_id: string | null }>();
  if (!deleted) throw urlNotFound();
  forgetSite(deleted.slug);
  if (deleted.machine_id) background(ctx, releaseIfUnwanted(env, deleted.machine_id));
  // The next URL in line may serve again.
  background(ctx, reconcileOwner(env, caller.userId));
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
  ctx: ExecutionContext,
): Promise<Response> {
  const before = await env.DB.prepare(
    `SELECT machine_id, taken_down_at FROM public_urls
     WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL`,
  )
    .bind(id, caller.userId)
    .first<{ machine_id: string | null; taken_down_at: number | null }>();
  if (!before) throw urlNotFound();
  if (before.taken_down_at !== null) throw takenDown();
  const entitlement = await requireEntitlement(env, caller);
  // Only a URL that serves moves: live, among the owner's `publicUrls`
  // oldest, and not marked suspended. A URL whose mark a reconcile has yet
  // to clear waits for it, within 5 minutes. A takedown or a deletion
  // between the read and this update leaves it where it was.
  const claimed = await env.DB.prepare(
    `UPDATE public_urls SET machine_id = ?3
     WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL AND taken_down_at IS NULL
       AND suspended_at IS NULL
       AND (SELECT COUNT(*) FROM public_urls q
            WHERE q.user_id = ?2 AND q.deleted_at IS NULL AND q.taken_down_at IS NULL
              AND (q.created_at < public_urls.created_at
                   OR (q.created_at = public_urls.created_at AND q.id <= public_urls.id))) <= ?4
     RETURNING slug`,
  )
    .bind(id, caller.userId, caller.machineId, entitlement.publicUrls)
    .first<{ slug: string }>();
  if (!claimed) {
    const current = await env.DB.prepare(
      `SELECT taken_down_at FROM public_urls WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL`,
    )
      .bind(id, caller.userId)
      .first<{ taken_down_at: number | null }>();
    if (!current) throw urlNotFound();
    if (current.taken_down_at !== null) throw takenDown();
    throw new ApiError(
      403,
      "quota_exceeded",
      `This public URL is not serving: your plan serves your ${entitlement.publicUrls} oldest public URLs, and this one comes after them, or it is waiting to serve again, which takes up to 5 minutes. Remove an older one with \`localify public-url remove\`, or try again in a few minutes.`,
    );
  }
  forgetSite(claimed.slug);
  const previous = before.machine_id;
  if (previous && previous !== caller.machineId) {
    background(ctx, releaseIfUnwanted(env, previous));
  }
  return json({ publicUrl: await requireUrl(env, caller, id) });
}

// --- Machine runtime ---

/**
 * `GET /v2/machines/self/runtime?generation=<n>`: `n` is the generation the
 * daemon runs, 0 for none. The token comes only for another generation.
 */
async function runtimeV2(
  request: Request,
  env: Env,
  caller: Caller,
  _params: string[],
  ctx: ExecutionContext,
): Promise<Response> {
  const value = new URL(request.url).searchParams.get("generation") ?? "";
  const generation = /^\d{1,9}$/.test(value) ? Number(value) : null;
  if (generation === null) {
    throw invalid("generation must be the tunnel generation the daemon runs, or 0 for none.");
  }
  return json(await machineRuntime(env, ctx, caller, generation));
}

/**
 * `GET /v1/machines/self/runtime`, for daemons before generations: the same
 * reservation and provisioning, with the current token every time once the
 * tunnel is ready.
 */
async function runtimeV1(
  _request: Request,
  env: Env,
  caller: Caller,
  _params: string[],
  ctx: ExecutionContext,
): Promise<Response> {
  const runtime = await machineRuntime(env, ctx, caller, null);
  if (runtime.state !== "ready" || !runtime.tunnel?.token || !runtime.ingressSecret) {
    return json(
      {
        error: "runtime_not_ready",
        message: `This machine's tunnel is not ready (${runtime.state}). Update Localify to see why.`,
        state: runtime.state,
      },
      409,
    );
  }
  return json({
    tunnelToken: runtime.tunnel.token,
    ingressSecret: runtime.ingressSecret,
    publicUrls: runtime.publicUrls.filter((url) => url.state === "serving"),
  });
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

/** The caller's entitlement, or `402 subscription_required` when it is zero. */
async function requireEntitlement(env: Env, caller: Caller): Promise<Entitlement> {
  const entitlement = await entitlementOf(env, caller.userId);
  if (!isEntitled(entitlement)) {
    throw new ApiError(
      402,
      "subscription_required",
      "Public URLs need Localify Cloud Pro. Start a 7-day trial with `localify subscribe`.",
    );
  }
  return entitlement;
}

function takenDown(): ApiError {
  return new ApiError(
    403,
    "taken_down",
    "This public URL was removed for breaking the terms. Write to abuse@localify.dev if you think this is a mistake.",
  );
}

function urlNotFound(): ApiError {
  return new ApiError(
    404,
    "not_found",
    "Your account has no public URL with this id. Run `localify public-url list` to see yours.",
  );
}

async function requireUrl(env: Env, caller: Caller, id: string | undefined): Promise<PublicUrl> {
  const url = id
    ? await urlOfUser(
        env.DB,
        baseDomain(env),
        caller.userId,
        id,
        servingUrlLimit(await entitlementOf(env, caller.userId)),
      )
    : null;
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
