/// <reference types="node" />
// The dashboard's only way into D1: plain SELECT statements, refused before
// they leave the process when they are anything else, sent in one
// `wrangler d1 execute` call. Its URL is guarded by a password alone, so a
// leak may show data but never change it.

import { readCloudValues, wranglerOutputAsync } from "../infisical.mjs";

export type Row = Record<string, unknown>;
/** Runs SELECT statements, in order, and returns the rows of each one. */
export type ReadDb = (statements: string[]) => Promise<Row[][]>;

/** How long one read of D1 may take; a production read takes a few seconds. */
export const READ_TIMEOUT_MS = 60_000;

/** A statement the dashboard refuses to send. */
export class NotReadOnlyError extends Error {}

/**
 * Words that change data, the schema or the connection, anywhere outside a
 * string literal or a quoted identifier. Column names such as `deleted_at`
 * are not words of their own, so they pass.
 */
const FORBIDDEN =
  /\b(INSERT|UPDATE|DELETE|REPLACE|UPSERT|MERGE|CREATE|DROP|ALTER|TRUNCATE|ATTACH|DETACH|PRAGMA|VACUUM|REINDEX|ANALYZE|BEGIN|COMMIT|ROLLBACK|SAVEPOINT|RELEASE|RETURNING|TRANSACTION|LOAD_EXTENSION)\b/i;

/** String literals, quoted identifiers and comments, in the order SQLite reads them. */
const LEXEMES = /'(?:[^']|'')*'|"(?:[^"]|"")*"|`[^`]*`|\[[^\]]*\]|--[^\n]*|\/\*[\s\S]*?\*\//g;

/**
 * Returns `sql` trimmed when it is one SELECT statement, and throws
 * `NotReadOnlyError` otherwise: a statement that starts with anything but
 * SELECT (a CTE may hold a write, so WITH is refused too), holds a `;`, an
 * unterminated quote or comment, or a forbidden word.
 */
export function assertSelect(sql: string): string {
  const trimmed = sql.trim();
  const code = trimmed.replace(LEXEMES, (lexeme) =>
    lexeme.startsWith("--") || lexeme.startsWith("/*") ? " " : "''",
  );
  const refuse = (why: string): never => {
    throw new NotReadOnlyError(`The dashboard sends SELECT statements only: ${why}`);
  };
  if (!/^SELECT\b/i.test(code)) refuse("this one does not start with SELECT.");
  if (code.includes(";")) refuse("one statement at a time, without `;`.");
  // What is left of a quote or a comment after the literals are gone is
  // an unterminated one, which could hide the rest of the statement.
  if (/['"`[]|\/\*|\*\//.test(code.replaceAll("''", ""))) {
    refuse("an unterminated quote or comment.");
  }
  const word = FORBIDDEN.exec(code);
  if (word) refuse(`it holds ${word[1]!.toUpperCase()}.`);
  return trimmed;
}

/** `run` behind the guard: every statement is checked before any is sent. */
export function readOnly(run: ReadDb): ReadDb {
  return async (statements) => run(statements.map(assertSelect));
}

/**
 * Production D1, or with `local` the D1 of `pnpm cloud:dev`, through
 * `wrangler d1 execute`: one call for all the statements. Only wrangler's
 * identity is kept from Infisical, and it never leaves this process but in
 * wrangler's environment. A read that runs past `READ_TIMEOUT_MS`, or that
 * `signal` aborts, kills wrangler and fails, without blocking the server.
 */
export function wranglerReader(local: boolean, signal?: AbortSignal): ReadDb {
  const identity: Record<string, string | undefined> = {};
  if (!local) {
    const { CLOUDFLARE_DEPLOY_TOKEN, CLOUDFLARE_ACCOUNT_ID } = readCloudValues();
    identity.CLOUDFLARE_DEPLOY_TOKEN = CLOUDFLARE_DEPLOY_TOKEN;
    identity.CLOUDFLARE_ACCOUNT_ID = CLOUDFLARE_ACCOUNT_ID;
  }
  return readOnly(async (statements) => {
    const output = await wranglerOutputAsync(
      identity,
      [
        "d1",
        "execute",
        "localify-cloud",
        local ? "--local" : "--remote",
        "--yes",
        "--json",
        "--command",
        statements.join(";\n"),
      ],
      { timeoutMs: READ_TIMEOUT_MS, signal },
    );
    const results = JSON.parse(output) as { results?: Row[] }[];
    if (!Array.isArray(results) || results.length !== statements.length) {
      throw new Error(`wrangler returned ${results.length} results for ${statements.length} reads`);
    }
    return results.map((result) => result.results ?? []);
  });
}
