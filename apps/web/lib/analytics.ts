import posthog from "posthog-js";

type AnalyticsEventData = Record<string, boolean | number | string | null | undefined>;

const DEFAULT_POSTHOG_HOST = "https://us.i.posthog.com";
const DATA_EVENT_SELECTOR = "[data-umami-event]";
const INSTALL_ATTRIBUTION_PREFIX = "localify:install-attribution:v1:";

let clickTrackingStarted = false;

const isPostHogConfigured = () => {
  return Boolean(process.env.NEXT_PUBLIC_POSTHOG_TOKEN);
};

const isPostHogDebugEnabled = () => {
  return process.env.NEXT_PUBLIC_POSTHOG_DEBUG === "true";
};

const isWebAnalyticsEnabled = () => {
  return (
    process.env.NEXT_PUBLIC_POSTHOG_DISABLED !== "true" &&
    isPostHogConfigured() &&
    (process.env.NODE_ENV === "production" || isPostHogDebugEnabled())
  );
};

type ConsentStatus = ReturnType<typeof posthog.get_explicit_consent_status>;

const consentListeners = new Set<() => void>();

export const subscribeToConsent = (listener: () => void) => {
  consentListeners.add(listener);
  return () => {
    consentListeners.delete(listener);
  };
};

// null when website analytics are off, so there is nothing to ask the visitor about.
export const getConsentStatus = (): ConsentStatus | null => {
  return isWebAnalyticsEnabled() ? posthog.get_explicit_consent_status() : null;
};

// opt_in_capturing() also sends the $pageview skipped while consent was pending, and PostHog
// sends it only once per page load, so a second capture here would count it twice.
export const acceptAnalytics = () => {
  posthog.opt_in_capturing();
  notifyConsentListeners();
};

export const declineAnalytics = () => {
  posthog.opt_out_capturing();
  removePostHogSessionStorage();
  notifyConsentListeners();
};

export const resetAnalyticsConsent = () => {
  posthog.clear_opt_in_out_capturing();
  removePostHogSessionStorage();
  notifyConsentListeners();
};

// Both calls above clear PostHog's cookie and local storage, but leave its window id behind.
const removePostHogSessionStorage = () => {
  try {
    for (const key of Object.keys(sessionStorage)) {
      if (key.startsWith("ph_")) sessionStorage.removeItem(key);
    }
  } catch {
    // Blocked storage has nothing to clear.
  }
};

const notifyConsentListeners = () => {
  for (const listener of consentListeners) {
    listener();
  }
};

export const initWebAnalytics = () => {
  const token = process.env.NEXT_PUBLIC_POSTHOG_TOKEN;
  if (!token || !isWebAnalyticsEnabled()) return;

  posthog.init(token, {
    api_host: process.env.NEXT_PUBLIC_POSTHOG_HOST ?? DEFAULT_POSTHOG_HOST,
    defaults: "2026-01-30",
    capture_pageview: true,
    capture_pageleave: false,
    autocapture: false,
    disable_session_recording: true,
    // Nothing is stored or sent until the visitor accepts in the cookie banner.
    opt_out_capturing_by_default: true,
    opt_out_persistence_by_default: true,
    // Without this, init fetches remote config and feature flags whatever the consent status.
    // The site uses neither.
    advanced_disable_flags: true,
    loaded: (client) => {
      client.register({ surface: "web" });
      if (isPostHogDebugEnabled()) {
        client.debug();
      }
    },
  });

  startClickTracking();
};

const startClickTracking = () => {
  if (clickTrackingStarted || typeof document === "undefined") return;
  clickTrackingStarted = true;

  document.addEventListener(
    "click",
    (event) => {
      const target = event.target;
      if (!(target instanceof Element)) return;

      const element = target.closest<HTMLElement>(DATA_EVENT_SELECTOR);
      if (!element) return;

      const name = element.dataset.umamiEvent;
      if (!name || getConsentStatus() !== "granted") return;

      const data = getEventProperties(element.dataset);
      if (name === "download_clicked") {
        const attribution = buildInstallAttribution(data);
        data.install_attribution_id = attribution.id;
        void writeInstallAttribution(attribution.token);
      }

      posthog.capture(name, data);
    },
    { capture: true },
  );
};

const buildInstallAttribution = (data: AnalyticsEventData) => {
  const id = createAttributionId();
  const payload = {
    id,
    distinctId: posthog.get_distinct_id(),
    source: typeof data.source === "string" ? data.source : "download",
    createdAt: new Date().toISOString(),
  };

  return {
    id,
    token: `${INSTALL_ATTRIBUTION_PREFIX}${btoa(JSON.stringify(payload))}`,
  };
};

const createAttributionId = () => {
  if (globalThis.crypto?.randomUUID) {
    return globalThis.crypto.randomUUID();
  }

  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
};

const writeInstallAttribution = async (token: string) => {
  if (!navigator.clipboard?.writeText) return;

  try {
    await navigator.clipboard.writeText(token);
  } catch {
    // Attribution must never block the download flow.
  }
};

const getEventProperties = (dataset: DOMStringMap): AnalyticsEventData => {
  const prefix = "umamiEvent";
  return Object.fromEntries(
    Object.entries(dataset)
      .filter(([key]) => key.startsWith(prefix) && key !== prefix)
      .map(([key, value]) => [toSnakeCase(key.slice(prefix.length)), value]),
  );
};

const toSnakeCase = (value: string) => {
  return value
    .replace(/^[A-Z]/u, (letter) => letter.toLowerCase())
    .replace(/[A-Z]/gu, (letter) => `_${letter.toLowerCase()}`);
};
