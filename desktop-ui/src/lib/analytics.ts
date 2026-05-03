import posthog from "posthog-js";
import { z } from "zod";

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
const ERROR_DEDUPE_WINDOW_MS = 2_000;
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
