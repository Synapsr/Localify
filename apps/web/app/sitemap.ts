import type { MetadataRoute } from "next";

import { localizedPath } from "@/app/page-metadata";
import { siteUrl } from "@/app/site-metadata";
import { routing } from "@/i18n/routing";

const routes = ["", "/privacy", "/terms"] as const;

const sitemap = (): MetadataRoute.Sitemap => {
  const lastModified = new Date("2026-11-04");

  return routes.flatMap((route) => {
    const languages = Object.fromEntries(
      routing.locales.map((locale) => [locale, `${siteUrl}${localizedPath(locale, route)}`]),
    );

    return routing.locales.map((locale) => ({
      url: `${siteUrl}${localizedPath(locale, route)}`,
      lastModified,
      changeFrequency: route === "" ? ("weekly" as const) : ("monthly" as const),
      priority: route === "" ? 1 : 0.4,
      alternates: { languages },
    }));
  });
};

export default sitemap;
