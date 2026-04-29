import posthog from "posthog-js";
import { z } from "zod";

export type AnalyticsEventData = Record<string, boolean | number | string | null | undefined>;

const DEFAULT_POSTHOG_HOST = "https://us.i.posthog.com";
const INSTALL_ATTRIBUTION_PREFIX = "localify:install-attribution:v1:";
const ATTRIBUTION_CONSUMED_STORAGE_KEY = "localify.installAttribution.consumedId";
const SESSION_REPLAY_CONSENT_STORAGE_KEY = "localify.analytics.sessionReplayConsent";

let initialized = false;

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

const parseInstallAttribution = (token: string) => {
  try {
    const encoded = token.slice(INSTALL_ATTRIBUTION_PREFIX.length);
    return installAttributionSchema.parse(JSON.parse(atob(encoded)));
  } catch {
    return null;
  }
};
