import type {
  CloudAccountDto,
  CloudPlanDto,
  CloudPlanIntervalDto,
  CloudRuntimeBlockDto,
  PublicUrlDto,
  TunnelStatusDto,
} from "@/types/localify";

import { normalizeSuffix } from "@/lib/validators/validator.create-project";

/** The Localify Cloud card's id, for links to `/settings#localify-cloud`. */
export const SETTINGS_CLOUD_ANCHOR = "localify-cloud";

export type CloudStatusTone = "ok" | "pending" | "error" | "off";

export interface CloudStatus {
  /** No tone: the app makes no claim about the state. */
  tone: CloudStatusTone | null;
  label: string;
  detail: string | null;
}

/** Pro's prices (`docs/public-urls.md`, "Plans"). */
export const PRO_PRICE: Record<CloudPlanIntervalDto, string> = {
  month: "$7.99 a month",
  year: "$79 a year",
};

export const ABUSE_CONTACT = "abuse@localify.dev";

/** `14 Nov 2026` from an RFC 3339 time. */
export function formatPlanDate(value: string | null): string {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

/** The plan in words, as the plan card shows it. */
export function describePlan(plan: CloudPlanDto): { label: string; detail: string | null } {
  const interval = plan.interval === "year" ? "yearly" : "monthly";
  switch (plan.status) {
    case "none":
      return {
        label: "No plan",
        detail: plan.trialAvailable
          ? "Public URLs need Localify Cloud Pro. It starts with a 7-day free trial."
          : "Public URLs need Localify Cloud Pro.",
      };
    case "trialing":
      return { label: `Trial, ends ${formatPlanDate(plan.trialEndsAt)}`, detail: null };
    case "active":
      return {
        label: plan.renewsAt
          ? `Pro ${interval}, renews ${formatPlanDate(plan.renewsAt)}`
          : `Pro ${interval}`,
        detail: null,
      };
    case "canceling":
      return {
        label: `Ending ${formatPlanDate(plan.endsAt)}`,
        detail: "Public URLs stop serving then. Renew it in Manage billing.",
      };
    case "past_due":
      return {
        label: "Payment failed, update the card",
        detail: plan.entitled
          ? "Public URLs keep serving for a few days while the card is retried."
          : "Public URLs are paused until the card is updated.",
      };
    case "override":
      return {
        label: plan.endsAt ? `Team access, until ${formatPlanDate(plan.endsAt)}` : "Team access",
        detail: null,
      };
  }
}

/**
 * The other machines serving the account's URLs: the ones holding its serving slots when
 * this Mac finds none left.
 */
export function slotHolders(publicUrls: PublicUrlDto[] | undefined): string[] {
  const names = (publicUrls ?? [])
    .filter((url) => !url.servedHere && url.state !== "suspended" && url.state !== "taken_down")
    .map((url) => url.machineName)
    .filter((name): name is string => name !== null);
  return [...new Set(names)].sort();
}

/** Why this Mac serves nothing although it has URLs, when Localify Cloud blocks it. */
function describeBlock(block: CloudRuntimeBlockDto): CloudStatus {
  switch (block.reason) {
    case "subscription_required":
      return { tone: "error", label: "Needs a plan", detail: block.message };
    case "serving_slots_full":
      return {
        tone: "error",
        label: "No serving slot left",
        detail: block.upgrade
          ? `${block.message} Or raise the limit with: ${block.upgrade.command}`
          : block.message,
      };
    case "capacity":
      return {
        tone: "pending",
        label: "Waiting for room",
        detail: `${block.message} Serving resumes by itself.`,
      };
    default:
      return { tone: "error", label: "Blocked", detail: block.message };
  }
}

/** This Mac's tunnel, which carries only the URLs this Mac serves. */
export function describeTunnel(tunnel: TunnelStatusDto): CloudStatus {
  if (tunnel.runtime === "blocked" && tunnel.blocked) return describeBlock(tunnel.blocked);
  if (tunnel.runtime === "provisioning" && tunnel.state === "stopped") {
    return {
      tone: "pending",
      label: "Starting",
      detail: "Localify Cloud is preparing this Mac's tunnel.",
    };
  }
  switch (tunnel.state) {
    case "connected":
      return {
        tone: "ok",
        label: "Connected",
        detail: `${tunnel.connections} ${tunnel.connections === 1 ? "connection" : "connections"} to Cloudflare.`,
      };
    case "starting":
      return { tone: "pending", label: "Starting", detail: "The tunnel is connecting." };
    case "stopped":
      return { tone: "off", label: "Stopped", detail: "This Mac serves no public URL." };
    case "error":
      return { tone: "error", label: "Error", detail: tunnel.error ?? "The tunnel stopped." };
    case "unknown":
      return {
        tone: "off",
        label: "Unknown",
        detail: "Localify is off, so the tunnel cannot run.",
      };
  }
}

/**
 * Only this Mac's tunnel is known here: a URL served by another machine gets no online
 * or offline claim, and a connected tunnel says nothing about the dev server behind it.
 */
export function describePublicUrlState(
  publicUrl: PublicUrlDto,
  account: CloudAccountDto | undefined,
): CloudStatus {
  if (publicUrl.state === "suspended") {
    return {
      tone: "off",
      label: "Suspended",
      detail:
        "The plan does not cover it right now, so it does not serve. Its name and password are kept.",
    };
  }
  if (publicUrl.state === "taken_down") {
    return {
      tone: "error",
      label: "Taken down",
      detail: `It was removed for abuse and cannot serve again. To appeal, write to ${ABUSE_CONTACT}.`,
    };
  }
  if (!publicUrl.servedHere) {
    return publicUrl.machineName
      ? {
          tone: null,
          label: `Served by ${publicUrl.machineName}`,
          detail: "Serving it from this Mac stops it there.",
        }
      : {
          tone: "off",
          label: "No machine serves it",
          detail: "Nobody can open it until a Mac serves it.",
        };
  }

  if (account?.tunnel.runtime === "blocked" && account.tunnel.blocked) {
    return describeBlock(account.tunnel.blocked);
  }
  switch (account?.tunnel.state) {
    case "connected":
      return {
        tone: "ok",
        label: "Reachable through this Mac's tunnel",
        detail: "Your dev server still has to be running to answer.",
      };
    case "starting":
      return { tone: "pending", label: "This Mac's tunnel is starting", detail: null };
    case "error":
      return {
        tone: "error",
        label: "This Mac's tunnel failed",
        detail: account.tunnel.error ?? "The tunnel stopped.",
      };
    case "stopped":
      return { tone: "off", label: "This Mac's tunnel is stopped", detail: null };
    case "unknown":
      return {
        tone: "off",
        label: "Localify is off on this Mac",
        detail: "Turn it on to serve this URL.",
      };
    case undefined:
      return { tone: "pending", label: "Checking this Mac's tunnel", detail: null };
  }
}

export const cloudStatusDotClassName: Record<CloudStatusTone, string> = {
  ok: "bg-success",
  pending: "bg-amber-500",
  error: "bg-destructive",
  off: "bg-neutral-400",
};

/** The longest a DNS label may be. */
const MAX_DNS_LABEL = 63;

/** The public URL that serves a local domain, and what it opens for it. */
export interface PublicUrlMatch {
  publicUrl: PublicUrlDto;
  /** The local domain's label under the URL's own domain; null when it is that domain. */
  label: string | null;
  /** The local domain's own public URL; null when this URL does not reach it. */
  url: string | null;
}

/**
 * A URL created for the local domain itself wins. Otherwise the closest parent that is a
 * wildcard URL serves it at `<slug>--<label>`, and the project's own URL is shown even when
 * it is not wildcard, so the dialog can say it does not reach this domain.
 */
export function findPublicUrl(
  publicUrls: PublicUrlDto[] | undefined,
  localDomain: string,
  projectNamespace: string | null,
): PublicUrlMatch | null {
  if (!publicUrls) return null;
  const domain = localDomain.toLowerCase();
  const own = publicUrls.find((url) => url.localDomain.toLowerCase() === domain);
  if (own) return { publicUrl: own, label: null, url: own.url };

  let closest: PublicUrlMatch | null = null;
  for (const publicUrl of publicUrls) {
    const parent = publicUrl.localDomain.toLowerCase();
    const label = labelUnder(domain, parent);
    if (label === null || !(publicUrl.wildcard || parent === projectNamespace)) continue;
    if (closest?.label && closest.label.length <= label.length) continue;
    closest = {
      publicUrl,
      label,
      url: publicUrl.wildcard ? publicUrlOf(publicUrl.url, publicUrl.slug, label) : null,
    };
  }
  return closest;
}

/** What creating a URL for a local domain makes: one URL per project, as locally. */
export interface PublicUrlTarget {
  /** The local domain the URL is created for. */
  localDomain: string;
  wildcard: boolean;
  /** The opened domain's label under it; null when it is that domain. */
  label: string | null;
}

/**
 * The project's namespace, wildcard, for the namespace and every domain under it; any other
 * domain gets a URL of its own.
 */
export function publicUrlTarget(
  localDomain: string,
  projectNamespace: string | null,
): PublicUrlTarget {
  const domain = localDomain.toLowerCase();
  if (projectNamespace !== null) {
    if (domain === projectNamespace) {
      return { localDomain: projectNamespace, wildcard: true, label: null };
    }
    const label = labelUnder(domain, projectNamespace);
    if (label !== null) return { localDomain: projectNamespace, wildcard: true, label };
  }
  return { localDomain: domain, wildcard: false, label: null };
}

/** `web` for `web.journal.localify` under `journal.localify`; null when it is not under it. */
function labelUnder(domain: string, parent: string): string | null {
  const suffix = `.${parent.toLowerCase()}`;
  const name = domain.toLowerCase();
  if (!name.endsWith(suffix) || name.length === suffix.length) return null;
  return name.slice(0, -suffix.length);
}

/** One DNS label: a-z, 0-9 and hyphens, not at either end. */
export function isDnsLabel(label: string): boolean {
  return /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label);
}

/** The longest environment label a slug leaves room for in `<slug>--<label>`. */
export function maxEnvironmentLabel(slug: string): number {
  return MAX_DNS_LABEL - slug.length - 2;
}

/**
 * `https://<slug>.<public domain>`, or `https://<slug>--<label>.<public domain>` for an
 * environment, on the domain of `url`, a URL Localify Cloud gave: the clients never write
 * the public domain themselves. Null when the label cannot be served under this slug.
 * Mirrors `PublicSlug::environment_label`.
 */
export function publicUrlOf(url: string, slug: string, label: string | null): string | null {
  if (label === null) return urlNamed(url, slug);
  if (!isDnsLabel(label) || label.length > maxEnvironmentLabel(slug)) return null;
  return urlNamed(url, `${slug}--${label}`);
}

/** `url` with its first host label replaced by `name`, kept as typed for a preview. */
export function urlNamed(url: string, name: string): string {
  return url.replace(/^https:\/\/[^./]+/, `https://${name}`);
}

/** `localify.fyi` for `https://journal.localify.fyi`. */
export function publicDomainOf(url: string): string {
  return url.replace(/^https:\/\/[^./]+\./, "").replace(/\/.*$/, "");
}

/** The same link on another host of the URL: each host asks for the password separately. */
export function linkOnHost(link: string, url: string): string {
  try {
    const moved = new URL(link);
    moved.host = new URL(url).host;
    return moved.toString();
  } catch {
    return link;
  }
}

/**
 * A project's namespace (`journal.localify`): its environments live one label below it,
 * so a wildcard public URL on it reaches every environment. A project can expose it even
 * when no service registers it, as in a monorepo.
 */
export function projectNamespaceDomain(projectId: string, domainSuffix: string): string {
  return `${projectId}${normalizeSuffix(domainSuffix)}`.toLowerCase();
}

/** `https://journal-a8f3.localify.fyi` becomes `journal-a8f3--<label>.localify.fyi`. */
export function wildcardHostPattern(publicUrl: PublicUrlDto): string {
  let host = publicUrl.url;
  try {
    host = new URL(publicUrl.url).host;
  } catch {
    // Keep the raw value; the cloud always sends a full URL.
  }
  return host.replace(/^[^.]+/, `${publicUrl.slug}--<label>`);
}
