// The cookies the Worker sets on a public host, both host-only and signed with
// COOKIE_KEY: the access cookie a correct password sets, tied to the password
// version so that changing or removing the password signs every browser out,
// and the notice cookie that acknowledges the warning page.

import { constantTimeEqual, hmacHex } from "./crypto";

export const ACCESS_COOKIE = "__Host-localify-access";
export const ACCESS_TTL_SECONDS = 7 * 24 * 60 * 60;

export type AccessScope = { host: string; urlId: string; version: number };

/** `<expiry>.<password version>.<hex HMAC-SHA-256(key, host|url id|version|expiry)>`. */
export async function signAccess(
  key: string,
  scope: AccessScope,
  now = Date.now(),
): Promise<string> {
  const expiry = Math.floor(now / 1000) + ACCESS_TTL_SECONDS;
  const signature = await hmacHex(key, message(scope, scope.version, expiry));
  return `${expiry}.${scope.version}.${signature}`;
}

export async function verifyAccess(
  key: string,
  value: string,
  scope: AccessScope,
  now = Date.now(),
): Promise<boolean> {
  const match = /^(\d{1,12})\.(\d{1,12})\.([0-9a-f]{64})$/.exec(value);
  if (!match) return false;
  const [, expiryText, versionText, signature] = match as unknown as [
    string,
    string,
    string,
    string,
  ];
  const expiry = Number(expiryText);
  const version = Number(versionText);
  if (version !== scope.version || expiry * 1000 <= now) return false;
  return constantTimeEqual(await hmacHex(key, message(scope, version, expiry)), signature);
}

function message(scope: AccessScope, version: number, expiry: number): string {
  return `${scope.host}|${scope.urlId}|${version}|${expiry}`;
}

/** No `Domain`, so the cookie belongs to this exact host only. */
export function accessCookieHeader(value: string): string {
  return `${ACCESS_COOKIE}=${value}; Max-Age=${ACCESS_TTL_SECONDS}; Path=/; Secure; HttpOnly; SameSite=Lax`;
}

export const NOTICE_COOKIE = "__Host-localify-notice";
export const NOTICE_TTL_SECONDS = 7 * 24 * 60 * 60;

export type NoticeScope = { host: string; urlId: string };

/** `<expiry>.<hex HMAC-SHA-256(key, "notice"|host|url id|expiry)>`. */
export async function signNotice(
  key: string,
  scope: NoticeScope,
  now = Date.now(),
): Promise<string> {
  const expiry = Math.floor(now / 1000) + NOTICE_TTL_SECONDS;
  return `${expiry}.${await hmacHex(key, noticeMessage(scope, expiry))}`;
}

export async function verifyNotice(
  key: string,
  value: string,
  scope: NoticeScope,
  now = Date.now(),
): Promise<boolean> {
  const match = /^(\d{1,12})\.([0-9a-f]{64})$/.exec(value);
  if (!match) return false;
  const [, expiryText, signature] = match as unknown as [string, string, string];
  const expiry = Number(expiryText);
  if (expiry * 1000 <= now) return false;
  return constantTimeEqual(await hmacHex(key, noticeMessage(scope, expiry)), signature);
}

function noticeMessage(scope: NoticeScope, expiry: number): string {
  return `notice|${scope.host}|${scope.urlId}|${expiry}`;
}

/** How long a warning page's form can be posted after it was served. */
export const NOTICE_FORM_TTL_SECONDS = 24 * 60 * 60;

/**
 * The token a warning form posts, `<expiry>.<hex HMAC-SHA-256(key,
 * "notice-form"|host|expiry)>`: only a page this Worker served for this exact
 * host can acknowledge the warning, never a sibling label's page.
 */
export async function signNoticeForm(key: string, host: string, now = Date.now()): Promise<string> {
  const expiry = Math.floor(now / 1000) + NOTICE_FORM_TTL_SECONDS;
  return `${expiry}.${await hmacHex(key, `notice-form|${host}|${expiry}`)}`;
}

export async function verifyNoticeForm(
  key: string,
  value: string,
  host: string,
  now = Date.now(),
): Promise<boolean> {
  const match = /^(\d{1,12})\.([0-9a-f]{64})$/.exec(value);
  if (!match) return false;
  const [, expiryText, signature] = match as unknown as [string, string, string];
  const expiry = Number(expiryText);
  if (expiry * 1000 <= now) return false;
  return constantTimeEqual(await hmacHex(key, `notice-form|${host}|${expiry}`), signature);
}

/** No `Domain`, so the acknowledgement belongs to this exact host only. */
export function noticeCookieHeader(value: string): string {
  return `${NOTICE_COOKIE}=${value}; Max-Age=${NOTICE_TTL_SECONDS}; Path=/; Secure; HttpOnly; SameSite=Lax`;
}

/** Every value of a cookie in a `Cookie` header; a browser may send several. */
export function readCookie(header: string | null, name: string): string[] {
  return cookieParts(header)
    .filter((part) => cookieName(part) === name)
    .map((part) => part.slice(part.indexOf("=") + 1).trim());
}

/** The `Cookie` header without some cookies, or null when nothing is left. */
export function withoutCookies(header: string | null, names: readonly string[]): string | null {
  const kept = cookieParts(header).filter((part) => !names.includes(cookieName(part)));
  return kept.length > 0 ? kept.join("; ") : null;
}

function cookieParts(header: string | null): string[] {
  if (!header) return [];
  return header
    .split(";")
    .map((part) => part.trim())
    .filter(Boolean);
}

function cookieName(part: string): string {
  const index = part.indexOf("=");
  return (index === -1 ? part : part.slice(0, index)).trim();
}
