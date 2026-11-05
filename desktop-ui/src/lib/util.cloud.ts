import type {
  CloudAccountDto,
  DomainInfoDto,
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

/** This Mac's tunnel, which carries only the URLs this Mac serves. */
export function describeTunnel(tunnel: TunnelStatusDto): CloudStatus {
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

export function findPublicUrlForDomain(
  publicUrls: PublicUrlDto[] | undefined,
  domain: DomainInfoDto,
): PublicUrlDto | null {
  const localDomain = domain.registration.domain.toLowerCase();
  return publicUrls?.find((url) => url.localDomain.toLowerCase() === localDomain) ?? null;
}

/**
 * A project's own domain (`journal.localify`): its environments live one label below
 * it, so a wildcard public URL on it reaches every environment.
 */
export function isProjectNamespaceDomain(domain: DomainInfoDto, domainSuffix: string): boolean {
  if (!domain.projectId) return false;
  const expected = `${domain.projectId}${normalizeSuffix(domainSuffix)}`;
  return domain.registration.domain.toLowerCase() === expected.toLowerCase();
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

/** What a generated slug looks like for this domain (docs/public-urls.md, "Names"). */
export function slugExample(localDomain: string): string {
  const base =
    (localDomain.split(".")[0] ?? "")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^[0-9-]+/, "")
      .slice(0, 20)
      .replace(/-+$/, "") || "url";
  return `${base}-a8f3`;
}
