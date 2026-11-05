// Slugs, labels and hosts (docs/public-urls.md, section "Names").

import { randomHex } from "./crypto";

const SLUG_PATTERN = /^[a-z][a-z0-9-]{1,30}[a-z0-9]$/;
const LABEL_PATTERN = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/;
const MAX_DNS_LABEL = 63;
const MAX_DOMAIN = 253;
const SLUG_BASE_MAX = 20;

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

/** A slug for a local domain: its normalized first label, `-`, and 4 random hex characters. */
export function generateSlug(localDomain: string): string {
  return `${slugBase(localDomain)}-${randomHex(2)}`;
}

/**
 * The first label of the local domain, lowercased, every run of characters
 * outside `a-z0-9` replaced by one `-`, leading digits and `-` removed (`url`
 * when nothing is left), cut to 20 characters without a trailing `-`.
 */
export function slugBase(localDomain: string): string {
  const cleaned = (localDomain.split(".")[0] ?? "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^[0-9-]+/, "");
  return cleaned === "" ? "url" : cleaned.slice(0, SLUG_BASE_MAX).replace(/-+$/, "");
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
