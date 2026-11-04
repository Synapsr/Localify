"use client";

import { useSyncExternalStore } from "react";
import { useTranslations } from "next-intl";

import { buttonVariants } from "@/components/ui/button-variants";
import { Link } from "@/i18n/navigation";
import {
  acceptAnalytics,
  declineAnalytics,
  getConsentStatus,
  resetAnalyticsConsent,
  subscribeToConsent,
} from "@/lib/analytics";

// The server snapshot is null, so nothing renders until hydration has finished.
const useConsentStatus = () => {
  return useSyncExternalStore(subscribeToConsent, getConsentStatus, () => null);
};

export const CookieBanner = () => {
  const t = useTranslations("CookieBanner");
  const footer = useTranslations("Footer");
  const status = useConsentStatus();

  if (status !== "pending") return null;

  const buttonClass = buttonVariants({ variant: "secondary" });

  return (
    <section
      aria-label={t("label")}
      className="fixed inset-x-4 bottom-4 z-50 flex flex-col gap-3 rounded-xl border border-border bg-background p-4 shadow-lg sm:left-auto sm:max-w-sm"
    >
      <p className="text-[13.5px] leading-[1.5] text-pretty text-muted-foreground">
        {t("message")}{" "}
        <Link href="/privacy" className="text-ink underline underline-offset-4 hover:opacity-70">
          {footer("privacy")}
        </Link>
      </p>
      <div className="grid grid-cols-2 gap-2">
        <button type="button" className={buttonClass} onClick={acceptAnalytics}>
          {t("accept")}
        </button>
        <button type="button" className={buttonClass} onClick={declineAnalytics}>
          {t("decline")}
        </button>
      </div>
    </section>
  );
};

export const CookieSettingsItem = () => {
  const t = useTranslations("Footer");
  const status = useConsentStatus();

  if (status === null) return null;

  return (
    <li>
      <button
        type="button"
        className="inline-flex cursor-pointer py-1 transition-colors duration-150 hover:text-primary"
        onClick={resetAnalyticsConsent}
      >
        {t("cookies")}
      </button>
    </li>
  );
};
