/// <reference types="node" />
// `pnpm cloud:admin [--local] <command>`: the operator's moderation actions
// (docs/public-urls.md, "Moderation"). Each command writes D1 only, in one
// wrangler call with the deploy identity (infisical.mjs), and prints what it
// changed. The public path feels it within its 10-second cache; the next
// sweep releases or rotates the affected tunnels, which closes open streams
// (ADR 0027). `--local` works on the database of `pnpm cloud:dev` instead.
//
// Node runs this file as it is, so it imports only files without build
// steps: the entitlement rules come from the Worker's own module.

import { pathToFileURL } from "node:url";

import {
  ACCOUNT_SQL,
  OWNED_URLS_SQL,
  accountOf,
  isRotation,
  reconcileStatements,
  servingUrlLimit,
  STAMP_STOP,
  stopStatements,
  type AccountRow,
  type Entitlement,
  type OwnedUrl,
  type Statement,
} from "../src/entitlement.ts";
import { readCloudValues, wranglerOutput } from "./infisical.mjs";

export const USAGE = `Usage: pnpm cloud:admin [--local] <command>

  show <github-login>                     the account, its plan and its URLs
  ban <github-login> <reason>             refuses its API calls and takes its URLs offline
  unban <github-login>
  takedown <slug> <reason>                shows a removal page; the slug is never given out again
  restore <slug>                          undoes a takedown
  override <github-login> <serving-machines> <urls> <reason> [--expires <date>]
  override --remove <github-login>
  refresh <github-login>                  queues a billing refresh for the next 5-minute run

<github-login> may also be id:<numeric GitHub id>, for a login two accounts held.
<date> is an ISO 8601 date or time, such as 2027-01-31.`;

type Param = Statement["params"][number];
type Row = Record<string, unknown>;
/** Runs the statements in order and returns the rows of each one. */
export type AdminDb = (statements: Statement[]) => Promise<Row[][]>;

/** A refusal for the operator: printed alone, nothing written. */
export class AdminError extends Error {}
/** A mistake in the command: printed with the usage, nothing written. */
export class UsageError extends AdminError {}

const MAX_REASON_LENGTH = 500;

/**
 * Writes `sql` with its `?N` placeholders replaced by SQL literals, since
 * `wrangler d1 execute` takes no parameters. Strings are quoted with every
 * `'` doubled, numbers must be safe integers: a value never becomes SQL.
 */
export function bindParams(sql: string, params: Param[]): string {
  return sql.replace(/\?(\d+)/g, (_, index: string) => {
    const position = Number(index) - 1;
    if (position < 0 || position >= params.length) {
      throw new Error(`No value for ?${index}`);
    }
    return literal(params[position]!);
  });
}

function literal(value: Param): string {
  if (value === null) return "NULL";
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value)) throw new Error(`Not an integer: ${value}`);
    return String(value);
  }
  if (value.includes("\0")) throw new Error("A value holds a NUL character");
  return `'${value.replaceAll("'", "''")}'`;
}

/** D1 through `wrangler d1 execute`: one call for all the statements, in order. */
export function wranglerDb(local: boolean): AdminDb {
  const values = local ? {} : readCloudValues();
  return async (statements) => {
    const command = statements.map((s) => bindParams(s.sql.trim(), s.params)).join(";\n");
    const output = wranglerOutput(values, [
      "d1",
      "execute",
      "localify-cloud",
      local ? "--local" : "--remote",
      "--yes",
      "--json",
      "--command",
      command,
    ]);
    const results = JSON.parse(output) as { results?: Row[] }[];
    if (!Array.isArray(results) || results.length !== statements.length) {
      throw new Error(
        `wrangler returned ${results.length} results for ${statements.length} statements`,
      );
    }
    return results.map((result) => result.results ?? []);
  };
}

/** Runs one command and returns the lines to print. */
export async function runCommand(db: AdminDb, args: string[], now = Date.now()): Promise<string[]> {
  const [command, ...rest] = args;
  switch (command) {
    case "show":
      return show(db, one(rest, "show <github-login>"), now);
    case "ban":
      return ban(db, rest, now);
    case "unban":
      return unban(db, one(rest, "unban <github-login>"), now);
    case "takedown":
      return takedown(db, rest, now);
    case "restore":
      return restore(db, one(rest, "restore <slug>"), now);
    case "override":
      return rest[0] === "--remove"
        ? removeOverride(db, one(rest.slice(1), "override --remove <github-login>"), now)
        : setOverride(db, rest, now);
    case "refresh":
      return refresh(db, one(rest, "refresh <github-login>"), now);
    default:
      throw new UsageError(command ? `Unknown command: ${command}` : "No command given.");
  }
}

// --- Commands ---

async function show(db: AdminDb, who: string, now: number): Promise<string[]> {
  const user = await findUser(db, who);
  const [[override], [billing], urls] = await query(
    db,
    statement(
      `SELECT serving_machines, public_urls, reason, expires_at FROM plan_overrides WHERE user_id = ?1`,
      user.id,
    ),
    statement(
      `SELECT pro_status, pro_interval, cancel_at_period_end, current_period_end, ends_at,
              past_due_at, addon_machines, addon_url_packs, refreshed_at
       FROM billing WHERE user_id = ?1`,
      user.id,
    ),
    statement(
      `SELECT p.slug, p.machine_id, m.name AS machine_name, p.deleted_at, p.taken_down_at,
              p.taken_down_reason, p.suspended_at
       FROM public_urls p LEFT JOIN machines m ON m.id = p.machine_id
       WHERE p.user_id = ?1 AND (p.deleted_at IS NULL OR p.taken_down_at IS NOT NULL)
       ORDER BY p.created_at, p.id`,
      user.id,
    ),
  );
  const account = accountOf((await snapshot(db, user.id)).row, now);
  const lines = [describeUser(user)];
  lines.push(
    user.banned_at === null
      ? "Not banned."
      : `Banned on ${date(user.banned_at)}: ${user.banned_reason ?? "(no reason)"}.`,
  );
  lines.push(
    override
      ? `Override: ${limits(override.serving_machines, override.public_urls)}, reason "${override.reason}", ${
          override.expires_at === null
            ? "no expiry"
            : `expires ${date(Number(override.expires_at))}`
        }.`
      : "No override.",
  );
  lines.push(
    billing
      ? `Billing snapshot of ${date(Number(billing.refreshed_at))}: Pro ${billing.pro_status ?? "none"}${
          billing.pro_interval ? ` (${billing.pro_interval})` : ""
        }, add-ons: ${billing.addon_machines} machines, ${billing.addon_url_packs} URL packs.`
      : "No billing snapshot.",
  );
  lines.push(
    `Plan: ${account.plan.status}. Entitlement: ${describeEntitlement(account.entitlement)}.`,
  );
  const limit = servingUrlLimit(account.entitlement);
  let rank = 0;
  for (const url of urls) {
    const live = url.deleted_at === null && url.taken_down_at === null;
    if (live) rank++;
    const state =
      url.taken_down_at !== null
        ? `taken down on ${date(Number(url.taken_down_at))}: ${url.taken_down_reason ?? "(no reason)"}`
        : rank <= limit && url.suspended_at === null
          ? "serving"
          : rank <= limit
            ? "suspended until the next reconcile"
            : "suspended";
    const machine = url.machine_name ? ` on ${url.machine_name}` : "";
    lines.push(`  ${url.slug}: ${state}${url.deleted_at !== null ? ", deleted" : ""}${machine}`);
  }
  if (urls.length === 0) lines.push("No public URLs.");
  return lines;
}

async function ban(db: AdminDb, args: string[], now: number): Promise<string[]> {
  const [who, ...reasonWords] = args;
  if (!who) throw new UsageError("ban <github-login> <reason>");
  const reason = readReason(reasonWords, "ban <github-login> <reason>");
  const user = await findUser(db, who);
  if (user.banned_at !== null) {
    return applyChange(db, user, now, {
      lines: [`${describeUser(user)} was already banned on ${date(user.banned_at)}.`],
    });
  }
  return applyChange(db, user, now, {
    lines: [describeUser(user), `Banned: ${reason}.`],
    change: [
      statement(
        `UPDATE users SET banned_at = ?2, banned_reason = ?3, ${STAMP_STOP}
         WHERE id = ?1 AND banned_at IS NULL`,
        user.id,
        now,
        reason,
      ),
    ],
    predict: ({ row, urls }) => ({ row: { ...row, banned_at: now }, urls }),
  });
}

async function unban(db: AdminDb, who: string, now: number): Promise<string[]> {
  const user = await findUser(db, who);
  if (user.banned_at === null) {
    return applyChange(db, user, now, { lines: [`${describeUser(user)} is not banned.`] });
  }
  return applyChange(db, user, now, {
    lines: [
      describeUser(user),
      `Unbanned; the ban of ${date(user.banned_at)} was: ${user.banned_reason ?? "(no reason)"}.`,
    ],
    change: [
      statement(`UPDATE users SET banned_at = NULL, banned_reason = NULL WHERE id = ?1`, user.id),
    ],
    predict: ({ row, urls }) => ({ row: { ...row, banned_at: null }, urls }),
  });
}

async function takedown(db: AdminDb, args: string[], now: number): Promise<string[]> {
  const [slug, ...reasonWords] = args;
  if (!slug) throw new UsageError("takedown <slug> <reason>");
  const reason = readReason(reasonWords, "takedown <slug> <reason>");
  const url = await findUrl(db, slug);
  const user = await userById(db, url.user_id);
  // The URL stops serving, deleted or not: every serving machine of its
  // owner rotates, unless `suspended_at` already records the stop. That
  // guard is checked at write time, so a retry of a takedown written
  // without its rotation still rotates, and a repeated one does nothing.
  const stop = stopStatements(user.id, [url.id], now);
  if (url.taken_down_at !== null) {
    return applyChange(db, user, now, {
      lines: [`${url.slug} was already taken down on ${date(url.taken_down_at)}.`],
      change: url.suspended_at === null ? stop : [],
    });
  }
  return applyChange(db, user, now, {
    lines: [`${url.slug}, owned by ${describeUser(user)}: taken down: ${reason}.`],
    change: [
      statement(
        `UPDATE public_urls SET taken_down_at = ?2, taken_down_reason = ?3
         WHERE id = ?1 AND taken_down_at IS NULL`,
        url.id,
        now,
        reason,
      ),
      ...stop,
    ],
    predict: ({ row, urls }) => ({
      row,
      urls: urls.map((owned) => (owned.id === url.id ? { ...owned, taken_down_at: now } : owned)),
    }),
  });
}

async function restore(db: AdminDb, slug: string, now: number): Promise<string[]> {
  const url = await findUrl(db, slug);
  const user = await userById(db, url.user_id);
  if (url.taken_down_at === null) {
    return applyChange(db, user, now, { lines: [`${url.slug} is not taken down.`] });
  }
  return applyChange(db, user, now, {
    lines: [
      `${url.slug}, owned by ${describeUser(user)}: restored; the takedown of ${date(url.taken_down_at)} was: ${url.taken_down_reason ?? "(no reason)"}.`,
      ...(url.deleted_at !== null ? ["Its owner had deleted it, so it stays deleted."] : []),
    ],
    change: [
      statement(
        `UPDATE public_urls SET taken_down_at = NULL, taken_down_reason = NULL WHERE id = ?1`,
        url.id,
      ),
    ],
    predict: ({ row, urls }) => ({
      row,
      urls: urls.map((owned) => (owned.id === url.id ? { ...owned, taken_down_at: null } : owned)),
    }),
  });
}

async function setOverride(db: AdminDb, args: string[], now: number): Promise<string[]> {
  const usage = "override <github-login> <serving-machines> <urls> <reason> [--expires <date>]";
  const words = [...args];
  let expiresAt: number | null = null;
  const flag = words.indexOf("--expires");
  if (flag !== -1) {
    const value = words[flag + 1];
    const parsed = value ? Date.parse(value) : Number.NaN;
    if (!Number.isFinite(parsed)) throw new UsageError(`--expires needs a date. ${usage}`);
    if (parsed <= now) throw new UsageError(`--expires ${value} is already past.`);
    expiresAt = parsed;
    words.splice(flag, 2);
  }
  const [who, machines, urls, ...reasonWords] = words;
  if (!who || machines === undefined || urls === undefined) throw new UsageError(usage);
  const servingMachines = readCount(machines, "serving-machines");
  const publicUrls = readCount(urls, "urls");
  const reason = readReason(reasonWords, usage);
  const user = await findUser(db, who);
  const [[previous]] = await query(
    db,
    statement(
      `SELECT serving_machines, public_urls, reason, expires_at FROM plan_overrides WHERE user_id = ?1`,
      user.id,
    ),
  );
  return applyChange(db, user, now, {
    lines: [
      describeUser(user),
      `Override: ${previous ? `${limits(previous.serving_machines, previous.public_urls)} ("${previous.reason}")` : "none"} -> ${limits(servingMachines, publicUrls)} ("${reason}"), ${
        expiresAt === null ? "no expiry" : `expires ${date(expiresAt)}`
      }.`,
    ],
    change: [
      statement(
        `INSERT INTO plan_overrides (user_id, serving_machines, public_urls, reason, expires_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (user_id) DO UPDATE SET serving_machines = excluded.serving_machines,
           public_urls = excluded.public_urls, reason = excluded.reason,
           expires_at = excluded.expires_at, created_at = excluded.created_at`,
        user.id,
        servingMachines,
        publicUrls,
        reason,
        expiresAt,
        now,
      ),
    ],
    predict: ({ row, urls }) => ({
      row: {
        ...row,
        override_machines: servingMachines,
        override_urls: publicUrls,
        override_expires_at: expiresAt,
      },
      urls,
    }),
  });
}

async function removeOverride(db: AdminDb, who: string, now: number): Promise<string[]> {
  const user = await findUser(db, who);
  const [[previous]] = await query(
    db,
    statement(
      `SELECT serving_machines, public_urls, reason FROM plan_overrides WHERE user_id = ?1`,
      user.id,
    ),
  );
  if (!previous) {
    return applyChange(db, user, now, { lines: [`${describeUser(user)} has no override.`] });
  }
  return applyChange(db, user, now, {
    lines: [
      describeUser(user),
      `Override removed: ${limits(previous.serving_machines, previous.public_urls)} ("${previous.reason}"). The plan applies again.`,
    ],
    change: [statement(`DELETE FROM plan_overrides WHERE user_id = ?1`, user.id)],
    predict: ({ row, urls }) => ({
      row: { ...row, override_machines: null, override_urls: null, override_expires_at: null },
      urls,
    }),
  });
}

/**
 * Queues the user for the 5-minute job's billing refresh, which reads Polar.
 * `last` starts at 0, so the first refresh takes revision 1.
 */
async function refresh(db: AdminDb, who: string, now: number): Promise<string[]> {
  const user = await findUser(db, who);
  await query(
    db,
    statement(
      `INSERT INTO billing_revisions (user_id, last, check_due_at) VALUES (?1, 0, ?2)
       ON CONFLICT (user_id) DO UPDATE SET check_due_at = ?2`,
      user.id,
      now,
    ),
  );
  return [describeUser(user), "Queued for a billing refresh at the next 5-minute run."];
}

// --- Applying a change ---

/** What decides which of an owner's URLs serve. */
type Snapshot = { row: AccountRow; urls: OwnedUrl[] };

async function snapshot(db: AdminDb, userId: string): Promise<Snapshot> {
  const [[row], urls] = await query(
    db,
    statement(ACCOUNT_SQL, userId),
    statement(OWNED_URLS_SQL, JSON.stringify([userId])),
  );
  if (!row) throw new Error(`User ${userId} has no row`);
  return { row: row as AccountRow, urls: urls as OwnedUrl[] };
}

type Change = {
  lines: string[];
  /** The moderation statements; none when the command finds nothing to change. */
  change?: Statement[];
  /** The owner's state once `change` applied. */
  predict?: (before: Snapshot) => Snapshot;
};

/**
 * Writes the change together with the reconcile it causes, in one call
 * that applies all or nothing: each URL that served at the last reconcile
 * and no longer does gets `suspended_at`, and every serving machine of the
 * owner a rotation, resolved at write time (ADR 0027). The reconcile runs
 * without a change too, so a retry repairs what an earlier attempt missed.
 * The prediction comes from a read, so a change made meanwhile costs at
 * most an extra rotation or a clear, which the next sweep corrects. Prints
 * the entitlement before and after, and the machines marked.
 */
async function applyChange(
  db: AdminDb,
  user: User,
  now: number,
  { lines, change = [], predict = (before) => before }: Change,
): Promise<string[]> {
  const before = await snapshot(db, user.id);
  const after = predict(before);
  const was = accountOf(before.row, now).entitlement;
  const is = accountOf(after.row, now).entitlement;
  const reconcile = reconcileStatements(user.id, is, after.urls, now);
  const out = [...lines];
  if (change.length === 0 && reconcile.length === 0) {
    out.push("Nothing changed.");
    return out;
  }
  const statements = [...change, ...reconcile];
  const results = await db(statements);
  const rotated = statements.flatMap((written, index) =>
    isRotation(written) ? (results[index] ?? []) : [],
  );
  const wasText = describeEntitlement(was);
  const isText = describeEntitlement(is);
  out.push(
    wasText === isText
      ? `Entitlement unchanged: ${isText}.`
      : `Entitlement: ${wasText} -> ${isText}.`,
  );
  if (rotated.length > 0) {
    out.push(
      `Rotating ${rotated.map((machine) => `${machine.name} (${machine.id})`).join(", ")}: the next sweep closes their open streams.`,
    );
  }
  out.push("The public path applies this within 10 seconds.");
  return out;
}

// --- Lookups ---

type User = {
  id: string;
  github_id: number;
  login: string;
  banned_at: number | null;
  banned_reason: string | null;
};

const SELECT_USER = `SELECT id, github_id, login, banned_at, banned_reason FROM users`;

/**
 * By GitHub login, case-insensitive as GitHub's, or `id:<github id>`. A
 * login is the one last seen at sign-in, so two accounts may share one.
 */
async function findUser(db: AdminDb, who: string): Promise<User> {
  let lookup: Statement;
  if (who.startsWith("id:")) {
    const githubId = /^\d{1,15}$/.test(who.slice(3)) ? Number(who.slice(3)) : Number.NaN;
    if (!Number.isSafeInteger(githubId)) throw new UsageError(`${who} is not a numeric GitHub id.`);
    lookup = statement(`${SELECT_USER} WHERE github_id = ?1`, githubId);
  } else {
    lookup = statement(`${SELECT_USER} WHERE login = ?1 COLLATE NOCASE ORDER BY created_at`, who);
  }
  const [rows] = await query(db, lookup);
  const users = (rows ?? []) as User[];
  if (users.length === 0) throw new AdminError(`No Localify Cloud account is known as ${who}.`);
  if (users.length > 1) {
    throw new AdminError(
      `${users.length} accounts were last seen as ${who}: ${users
        .map((user) => `id:${user.github_id}`)
        .join(", ")}. Pass the one you mean.`,
    );
  }
  return users[0]!;
}

async function userById(db: AdminDb, id: string): Promise<User> {
  const [rows] = await query(db, statement(`${SELECT_USER} WHERE id = ?1`, id));
  const user = rows?.[0] as User | undefined;
  if (!user) throw new Error(`User ${id} has no row`);
  return user;
}

type Url = {
  id: string;
  user_id: string;
  slug: string;
  deleted_at: number | null;
  taken_down_at: number | null;
  taken_down_reason: string | null;
  suspended_at: number | null;
};

async function findUrl(db: AdminDb, slug: string): Promise<Url> {
  const [rows] = await query(
    db,
    statement(
      `SELECT id, user_id, slug, deleted_at, taken_down_at, taken_down_reason,
              suspended_at
       FROM public_urls WHERE slug = ?1`,
      slug.trim().toLowerCase(),
    ),
  );
  const url = rows?.[0] as Url | undefined;
  if (!url) throw new AdminError(`No public URL has the slug ${slug}.`);
  return url;
}

// --- Helpers ---

/** Runs statements and gives the rows of each, in order. */
async function query<T extends Statement[]>(
  db: AdminDb,
  ...statements: T
): Promise<{ [K in keyof T]: Row[] }> {
  const results = await db(statements);
  return statements.map((_, index) => results[index] ?? []) as { [K in keyof T]: Row[] };
}

function statement(sql: string, ...params: Param[]): Statement {
  return { sql, params };
}

function one(args: string[], usage: string): string {
  if (args.length !== 1 || !args[0]) throw new UsageError(usage);
  return args[0];
}

function readReason(words: string[], usage: string): string {
  const reason = words.join(" ").trim();
  if (reason === "") throw new UsageError(`A reason is required. ${usage}`);
  if (reason.length > MAX_REASON_LENGTH) {
    throw new UsageError(`Keep the reason under ${MAX_REASON_LENGTH} characters.`);
  }
  return reason;
}

function readCount(value: string, name: string): number {
  const count = /^\d{1,6}$/.test(value) ? Number(value) : Number.NaN;
  if (!Number.isSafeInteger(count))
    throw new UsageError(`<${name}> must be a whole number, 0 or more.`);
  return count;
}

function describeUser(user: User): string {
  return `${user.login} (GitHub id ${user.github_id}, user ${user.id})`;
}

function describeEntitlement(entitlement: Entitlement): string {
  return entitlement.servingMachines === 0 && entitlement.publicUrls === 0
    ? "none"
    : limits(entitlement.servingMachines, entitlement.publicUrls);
}

function limits(servingMachines: unknown, publicUrls: unknown): string {
  const machines = Number(servingMachines);
  const urls = Number(publicUrls);
  return `${machines} serving ${machines === 1 ? "machine" : "machines"}, ${urls} ${urls === 1 ? "URL" : "URLs"}`;
}

function date(ms: number): string {
  return new Date(ms).toISOString();
}

// `node scripts/admin.ts [--local] <command>`
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = process.argv.slice(2);
  const local = args[0] === "--local";
  try {
    if (args.length === 0 || args.includes("--help")) {
      console.log(USAGE);
    } else {
      const lines = await runCommand(wranglerDb(local), local ? args.slice(1) : args);
      console.log([local ? "Local D1:" : "Production D1:", ...lines].join("\n"));
    }
  } catch (error) {
    // An unexpected error keeps its stack; a refusal is one sentence.
    console.error(error instanceof AdminError ? error.message : error);
    if (error instanceof UsageError) console.error(`\n${USAGE}`);
    process.exitCode = 1;
  }
}
