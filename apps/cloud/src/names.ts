// Slugs, labels and hosts (docs/public-urls.md, section "Names").

import { randomHex } from "./crypto";

const SLUG_PATTERN = /^[a-z][a-z0-9-]{1,30}[a-z0-9]$/;
const LABEL_PATTERN = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/;
const MAX_DNS_LABEL = 63;
const MAX_DOMAIN = 253;
const SLUG_MAX = 32;
/** Leaves room for `-` and 4 hex characters. */
const RANDOM_BASE_MAX = SLUG_MAX - 5;

export const SLUG_RULES =
  "A slug has 3 to 32 characters: lowercase letters, digits and single hyphens, starting with a letter and not ending with a hyphen.";

/** 3 to 32 of `a-z`, `0-9` and `-`, starting with a letter, not ending with `-`, never `--`. */
export function isValidSlug(slug: string): boolean {
  return SLUG_PATTERN.test(slug) && !slug.includes("--");
}

/** `a-z`, `0-9` and `-`, not starting or ending with `-`. May contain `--`. */
export function isValidLabel(label: string): boolean {
  return label.length <= MAX_DNS_LABEL && LABEL_PATTERN.test(label);
}

/** A lowercase domain of at least two valid labels, such as `journal.localify`. */
export function isValidLocalDomain(domain: string): boolean {
  if (domain.length > MAX_DOMAIN) return false;
  const labels = domain.split(".");
  return labels.length >= 2 && labels.every(isValidLabel);
}

export type PublicTarget = { slug: string; label: string | null };

/**
 * `<slug>.<base>` or `<slug>--<label>.<base>`, one DNS label deep. The first
 * `--` separates the slug from the label, so a label may contain `--`.
 * Returns null for anything else, which the Worker answers with a 404.
 */
export function parsePublicHost(host: string, baseDomain: string): PublicTarget | null {
  const suffix = `.${baseDomain}`;
  const lower = host.toLowerCase();
  if (!lower.endsWith(suffix)) return null;
  const name = lower.slice(0, -suffix.length);
  if (!name || name.includes(".") || name.length > MAX_DNS_LABEL) return null;
  const separator = name.indexOf("--");
  if (separator === -1) return isValidSlug(name) ? { slug: name, label: null } : null;
  const slug = name.slice(0, separator);
  const label = name.slice(separator + 2);
  return isValidSlug(slug) && isValidLabel(label) ? { slug, label } : null;
}

export function isPublicHost(host: string, baseDomain: string): boolean {
  return host.toLowerCase().endsWith(`.${baseDomain}`);
}

/** The host the daemon serves: the local domain, or `<label>.<local domain>`. */
export function localHostFor(localDomain: string, label: string | null): string | null {
  const host = label === null ? localDomain : `${label}.${localDomain}`;
  return host.length <= MAX_DOMAIN ? host : null;
}

/**
 * The slugs to offer for a local domain, most wanted first: its local name,
 * then the local name and the owner's login. Either may be invalid, such as
 * too short, and is then left out.
 */
export function slugCandidates(localDomain: string, login: string): string[] {
  const name = localName(localDomain);
  const owner = login
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return [name, `${name}-${owner}`].filter(isValidSlug);
}

/** The local name cut to leave room for `-` and 4 random hex characters. */
export function randomSlug(localDomain: string): string {
  const base = cutSlug(localName(localDomain), RANDOM_BASE_MAX);
  return `${base === "" ? "url" : base}-${randomHex(2)}`;
}

/**
 * The local domain as one slug, in its own order: every label but the last
 * (`localify`), joined with `-`, lowercased, every run of characters outside
 * `a-z0-9` replaced by one `-`, leading digits and `-` removed, cut to 32
 * characters without a trailing `-`. `web.journal.localify` gives
 * `web-journal`.
 */
export function localName(localDomain: string): string {
  const labels = localDomain.split(".");
  const name = (labels.length > 1 ? labels.slice(0, -1) : labels)
    .join("-")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^[0-9-]+/, "");
  return cutSlug(name, SLUG_MAX);
}

function cutSlug(name: string, max: number): string {
  return name.slice(0, max).replace(/-+$/, "");
}

/**
 * True when a browser `Origin` belongs to the public URL with this slug:
 * `https://<slug>.<base>`, or `https://<slug>--<label>.<base>` for a wildcard
 * URL. Every URL under the base domain is the same site for a browser, so
 * this, not `SameSite`, keeps another URL's pages from acting on this one.
 */
export function isOriginOfUrl(
  origin: string,
  slug: string,
  wildcard: boolean,
  baseDomain: string,
): boolean {
  let parsed: URL;
  try {
    parsed = new URL(origin);
  } catch {
    return false;
  }
  // Public URLs are served on 443 only, so an origin with a port is another site.
  if (parsed.protocol !== "https:" || parsed.port !== "" || parsed.origin !== origin) return false;
  const target = parsePublicHost(parsed.hostname, baseDomain);
  return target !== null && target.slug === slug && (target.label === null || wildcard);
}
