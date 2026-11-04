"use client";

import { useEffect } from "react";

import { routing } from "@/i18n/routing";

// The root layout renders <html lang="en"> for every page, because it has no locale to read
// without middleware. This corrects the attribute in the browser once the page has loaded.
export const DocumentLang = ({ locale }: { locale: string }) => {
  useEffect(() => {
    document.documentElement.lang = locale;

    return () => {
      document.documentElement.lang = routing.defaultLocale;
    };
  }, [locale]);

  return null;
};
