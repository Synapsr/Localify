import posthog from "posthog-js";
import { z } from "zod";

import type { BootstrapDto, ProjectInfoDto } from "@/types/roxy";

export type AnalyticsEventData = Record<string, boolean | number | string | null | undefined>;
type DesktopErrorSource =
  | "react_error_boundary"
  | "react_query"
  | "react_mutation"
  | "window_error"
  | "unhandled_rejection";

interface CaptureDesktopErrorOptions {
  source: DesktopErrorSource;
  componentStack?: string | null;
}

const DEFAULT_POSTHOG_HOST = "https://us.i.posthog.com";
const INSTALL_ATTRIBUTION_PREFIX = "localify:install-attribution:v1:";
const ATTRIBUTION_CONSUMED_STORAGE_KEY = "localify.installAttribution.consumedId";
const SESSION_REPLAY_CONSENT_STORAGE_KEY = "localify.analytics.sessionReplayConsent";
const USAGE_SNAPSHOT_STORAGE_KEY = "localify.analytics.usageSnapshot";
const ERROR_DEDUPE_WINDOW_MS = 2_000;
const USAGE_SNAPSHOT_INTERVAL_MS = 24 * 60 * 60 * 1_000;
const MAX_ERROR_FIELD_LENGTH = 500;

let initialized = false;
let globalErrorReportingInstalled = false;
const recentErrorFingerprints = new Map<string, number>();

const installAttributionSchema = z.object({
  id: z.string().min(1),
  distinctId: z.string().min(1),
  source: z.string().min(1),
  createdAt: z.string().min(1),
});

const usageSnapshotStorageSchema = z.object({
  signature: z.string().min(1),
  capturedAt: z.number().finite(),
});

const isPostHogConfigured = () => {
  return Boolean(import.meta.env.VITE_PUBLIC_POSTHOG_TOKEN);
};

const isPostHogDebugEnabled = () => {
  return import.meta.env.VITE_PUBLIC_POSTHOG_DEBUG === "true";
};

export const isDesktopAnalyticsEnabled = () => {
  return (
    import.meta.env.VITE_PUBLIC_POSTHOG_DISABLED !== "true" &&
    isPostHogConfigured() &&
    (import.meta.env.PROD || isPostHogDebugEnabled())
  );
};

export const loadDesktopSessionReplayConsent = () => {
  if (typeof window === "undefined") {
    return false;
  }

  return window.localStorage.getItem(SESSION_REPLAY_CONSENT_STORAGE_KEY) === "1";
};

export const setDesktopSessionReplayConsent = (enabled: boolean) => {
  if (typeof window !== "undefined") {
    if (enabled) {
      window.localStorage.setItem(SESSION_REPLAY_CONSENT_STORAGE_KEY, "1");
    } else {
      window.localStorage.removeItem(SESSION_REPLAY_CONSENT_STORAGE_KEY);
    }
  }

  applyDesktopSessionReplayConsent(enabled);
  captureDesktopEvent("desktop_session_replay_consent_updated", { enabled });
};

export const initDesktopAnalytics = () => {
  const token = import.meta.env.VITE_PUBLIC_POSTHOG_TOKEN;
  if (!token || initialized || !isDesktopAnalyticsEnabled()) return;

  initialized = true;
  installGlobalErrorReporting();
  const sessionReplayConsent = loadDesktopSessionReplayConsent();

  posthog.init(token, {
    api_host: import.meta.env.VITE_PUBLIC_POSTHOG_HOST ?? DEFAULT_POSTHOG_HOST,
    defaults: "2026-01-30",
    capture_pageview: false,
    capture_pageleave: false,
    autocapture: false,
    disable_session_recording: !sessionReplayConsent,
    enable_recording_console_log: false,
    capture_performance: false,
    mask_all_element_attributes: sessionReplayConsent,
    mask_all_text: sessionReplayConsent,
    mask_personal_data_properties: true,
    custom_personal_data_properties: ["domain", "path", "project", "project_name", "target", "url"],
    session_recording: {
      blockSelector: "[data-analytics-block], [data-secret]",
      maskAllInputs: true,
      maskTextSelector: "[data-analytics-mask], [data-sensitive]",
    },
    loaded: (client) => {
      client.register({ surface: "desktop_app" });
      if (isPostHogDebugEnabled()) {
        client.debug();
      }
      applyDesktopSessionReplayConsent(sessionReplayConsent);
    },
  });
};

export const captureDesktopEvent = (name: string, data?: AnalyticsEventData) => {
  if (!initialized || !isDesktopAnalyticsEnabled()) return;
  posthog.capture(name, data);
};

export const captureDesktopUsageSnapshot = (bootstrap: BootstrapDto) => {
  if (!initialized || !isDesktopAnalyticsEnabled()) return;

  const properties = buildDesktopUsageProperties(bootstrap);
  posthog.register(properties);

  const signature = JSON.stringify(properties);
  const previousSnapshot = readUsageSnapshotStorage();
  const now = Date.now();

  if (
    previousSnapshot?.signature === signature &&
    now - previousSnapshot.capturedAt < USAGE_SNAPSHOT_INTERVAL_MS
  ) {
    return;
  }

  posthog.setPersonProperties(properties);
  captureDesktopEvent("desktop_usage_snapshot", {
    ...properties,
    usage_snapshot_changed: previousSnapshot?.signature !== signature,
  });
  writeUsageSnapshotStorage(signature, now);
};

export const captureDesktopError = (error: unknown, options: CaptureDesktopErrorOptions) => {
  if (!initialized || !isDesktopAnalyticsEnabled()) return;

  const normalized = normalizeError(error);
  const componentStack = options.componentStack ? sanitizeErrorField(options.componentStack) : null;
  const fingerprint = hashString(
    [options.source, normalized.name, normalized.message, normalized.stackHash, componentStack].join(
      "|",
    ),
  );

  if (isDuplicateError(fingerprint)) return;

  posthog.capture("desktop_error_reported", {
    error_source: options.source,
    error_name: normalized.name,
    error_message: normalized.message,
    error_stack_hash: normalized.stackHash,
    error_component_stack_hash: componentStack ? hashString(componentStack) : null,
    error_fingerprint: fingerprint,
  });
};

export const consumeInstallAttribution = (token: string | null) => {
  if (
    !initialized ||
    !isDesktopAnalyticsEnabled() ||
    !token?.startsWith(INSTALL_ATTRIBUTION_PREFIX)
  ) {
    return;
  }

  const attribution = parseInstallAttribution(token);
  if (!attribution) return;

  try {
    if (window.localStorage.getItem(ATTRIBUTION_CONSUMED_STORAGE_KEY) === attribution.id) {
      return;
    }
    window.localStorage.setItem(ATTRIBUTION_CONSUMED_STORAGE_KEY, attribution.id);
  } catch {
    // Storage failures should not prevent analytics attribution.
  }

  posthog.alias(attribution.distinctId);
  captureDesktopEvent("desktop_install_attribution_detected", {
    install_attribution_id: attribution.id,
    source: attribution.source,
    created_at: attribution.createdAt,
  });
};

const applyDesktopSessionReplayConsent = (enabled: boolean) => {
  if (!initialized || !isDesktopAnalyticsEnabled()) return;

  if (enabled) {
    posthog.startSessionRecording({ linked_flag: true, sampling: true });
    return;
  }

  posthog.stopSessionRecording();
};

const installGlobalErrorReporting = () => {
  if (typeof window === "undefined" || globalErrorReportingInstalled) return;

  globalErrorReportingInstalled = true;

  window.addEventListener("error", (event) => {
    captureDesktopError(event.error ?? event.message, { source: "window_error" });
  });

  window.addEventListener("unhandledrejection", (event) => {
    captureDesktopError(event.reason, { source: "unhandled_rejection" });
  });
};

const parseInstallAttribution = (token: string) => {
  try {
    const encoded = token.slice(INSTALL_ATTRIBUTION_PREFIX.length);
    return installAttributionSchema.parse(JSON.parse(atob(encoded)));
  } catch {
    return null;
  }
};

const buildDesktopUsageProperties = (bootstrap: BootstrapDto): AnalyticsEventData => {
  const projects = bootstrap.projects;
  const domains = bootstrap.domains;
  const assignedDomainCount = domains.filter((domain) => domain.projectId !== null).length;
  const standaloneDomainCount = domains.length - assignedDomainCount;
  const projectsWithDomainsCount = projects.filter((project) => project.domainCount > 0).length;
  const maxDomainsPerProject = getMaxDomainsPerProject(projects);

  return {
    usage_project_count: projects.length,
    usage_enabled_project_count: projects.filter((project) => project.enabled).length,
    usage_projects_with_domains_count: projectsWithDomainsCount,
    usage_projects_with_reachable_domain_count: projects.filter(
      (project) => project.hasReachableDomain,
    ).length,
    usage_project_root_path_count: projects.filter((project) => project.rootPath !== null).length,
    usage_domain_count: domains.length,
    usage_assigned_domain_count: assignedDomainCount,
    usage_standalone_domain_count: standaloneDomainCount,
    usage_wildcard_domain_count: domains.filter((domain) => domain.registration.wildcard).length,
    usage_https_domain_count: domains.filter((domain) => domain.registration.httpsEnabled).length,
    usage_active_domain_count: domains.filter((domain) => domain.active).length,
    usage_reachable_domain_count: domains.filter((domain) => domain.targetReachable).length,
    usage_cert_present_domain_count: domains.filter((domain) => domain.hasCert).length,
    usage_cert_trusted_domain_count: domains.filter((domain) => domain.certTrusted === true).length,
    usage_cert_untrusted_domain_count: domains.filter((domain) => domain.certTrusted === false)
      .length,
    usage_domains_per_project_max: maxDomainsPerProject,
    usage_domains_per_project_avg:
      projects.length === 0 ? 0 : roundToTwoDecimals(domains.length / projects.length),
    usage_has_projects: projects.length > 0,
    usage_has_domains: domains.length > 0,
    usage_has_standalone_domains: standaloneDomainCount > 0,
    usage_daemon_running: bootstrap.daemon.running,
    usage_ca_installed: bootstrap.daemon.caInstalled,
    usage_cli_helper_ready: bootstrap.environment.cliHelperReady,
    usage_cli_access_ready: bootstrap.environment.cliAccessReady,
    usage_node_trust_ready: bootstrap.environment.nodeTrust.ready,
    usage_license_status: bootstrap.licensing.status,
    usage_license_plan: bootstrap.licensing.limits.plan,
    usage_license_configured: bootstrap.licensing.configured,
    usage_license_offline_cache_present: bootstrap.licensing.offlineCachePresent,
  };
};

const getMaxDomainsPerProject = (projects: ProjectInfoDto[]) => {
  return projects.reduce((max, project) => Math.max(max, project.domainCount), 0);
};

const roundToTwoDecimals = (value: number) => {
  return Math.round(value * 100) / 100;
};

const readUsageSnapshotStorage = () => {
  if (typeof window === "undefined") return null;

  try {
    const rawValue = window.localStorage.getItem(USAGE_SNAPSHOT_STORAGE_KEY);
    if (!rawValue) return null;
    const parsed = usageSnapshotStorageSchema.safeParse(JSON.parse(rawValue));
    return parsed.success ? parsed.data : null;
  } catch {
    return null;
  }
};

const writeUsageSnapshotStorage = (signature: string, capturedAt: number) => {
  if (typeof window === "undefined") return;

  try {
    window.localStorage.setItem(
      USAGE_SNAPSHOT_STORAGE_KEY,
      JSON.stringify({ signature, capturedAt }),
    );
  } catch {
    // Storage failures should not prevent analytics.
  }
};

const normalizeError = (error: unknown) => {
  if (error instanceof Error) {
    return {
      name: sanitizeErrorField(error.name || "Error"),
      message: sanitizeErrorField(error.message || "Unknown error"),
      stackHash: error.stack ? hashString(sanitizeErrorField(error.stack)) : null,
    };
  }

  if (typeof error === "string") {
    return {
      name: "Error",
      message: sanitizeErrorField(error),
      stackHash: null,
    };
  }

  if (isErrorLike(error)) {
    return {
      name: sanitizeErrorField(error.name ?? "Error"),
      message: sanitizeErrorField(error.message ?? "Unknown error"),
      stackHash: null,
    };
  }

  return {
    name: "Error",
    message: "Unknown error",
    stackHash: null,
  };
};

const isErrorLike = (error: unknown): error is Record<"name" | "message", string | undefined> => {
  if (typeof error !== "object" || error === null) {
    return false;
  }

  const record = error as Record<string, unknown>;
  return (
    ("name" in record || "message" in record) &&
    (typeof record.name === "string" || typeof record.name === "undefined") &&
    (typeof record.message === "string" || typeof record.message === "undefined")
  );
};

const sanitizeErrorField = (value: string) => {
  return value
    .replace(/https?:\/\/[^\s)]+/g, "[url]")
    .replace(/file:\/\/[^\s)]+/g, "[path]")
    .replace(/(?:\/(?:Users|Volumes|private|tmp|var|etc)\/[^\s)]+)/g, "[path]")
    .replace(/[A-Za-z]:\\[^\s)]+/g, "[path]")
    .slice(0, MAX_ERROR_FIELD_LENGTH);
};

const hashString = (value: string) => {
  let hash = 5381;

  for (let index = 0; index < value.length; index += 1) {
    hash = (hash * 33) ^ value.charCodeAt(index);
  }

  return (hash >>> 0).toString(36);
};

const isDuplicateError = (fingerprint: string) => {
  const now = Date.now();
  const lastSeenAt = recentErrorFingerprints.get(fingerprint);

  for (const [key, seenAt] of recentErrorFingerprints) {
    if (now - seenAt > ERROR_DEDUPE_WINDOW_MS) {
      recentErrorFingerprints.delete(key);
    }
  }

  if (lastSeenAt && now - lastSeenAt <= ERROR_DEDUPE_WINDOW_MS) {
    return true;
  }

  recentErrorFingerprints.set(fingerprint, now);
  return false;
};
