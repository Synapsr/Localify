import type { Metadata } from "next";

import { getTranslations } from "next-intl/server";

import { routing, type Locale } from "@/i18n/routing";

import { publicUrlPattern, siteName } from "./site-metadata";

export type SiteRoute = "" | "/cloud" | "/cloud/subscribed" | "/pricing" | "/privacy" | "/terms";

// Pages people reach from Polar's checkout and customer portal: kept out of search results.
const noindexRoutes: ReadonlySet<SiteRoute> = new Set(["/cloud", "/cloud/subscribed"]);

export const localizedPath = (locale: Locale, route: SiteRoute) => {
  return locale === routing.defaultLocale ? route || "/" : `/${locale}${route}`;
};

const openGraphLocales: Record<Locale, string> = {
  en: "en_US",
  fr: "fr_FR",
  es: "es_ES",
  de: "de_DE",
  "pt-BR": "pt_BR",
  ja: "ja_JP",
  ko: "ko_KR",
  "zh-CN": "zh_CN",
  ru: "ru_RU",
};

export const buildPageMetadata = async (locale: Locale, route: SiteRoute): Promise<Metadata> => {
  const t = await getTranslations({ locale });
  const { title, description } = {
    "": { title: t("Metadata.title"), description: t("Metadata.description") },
    "/cloud": {
      title: t("Cloud.overview.metaTitle"),
      description: t("Cloud.overview.description", { url: publicUrlPattern }),
    },
    "/cloud/subscribed": {
      title: t("Cloud.subscribed.metaTitle"),
      description: t("Cloud.subscribed.description"),
    },
    "/pricing": { title: t("Pricing.metaTitle"), description: t("Pricing.metaDescription") },
    "/privacy": { title: t("Privacy.title"), description: t("Privacy.intro") },
    "/terms": { title: t("Terms.title"), description: t("Terms.intro") },
  }[route];
  const path = localizedPath(locale, route);

  return {
    // The home page title already names the site, so it skips the layout's title template.
    title: route === "" ? { absolute: title } : title,
    description,
    ...(noindexRoutes.has(route) && { robots: { index: false, follow: true } }),
    alternates: {
      canonical: path,
      languages: Object.fromEntries(
        routing.locales.map((alternate) => [alternate, localizedPath(alternate, route)]),
      ),
    },
    openGraph: {
      type: "website",
      locale: openGraphLocales[locale],
      url: path,
      siteName,
      title,
      description,
      images: [
        {
          url: "/og-image.png",
          width: 1200,
          height: 630,
          alt: "Localify macOS app showing local projects with .localify domains",
        },
      ],
    },
    twitter: {
      card: "summary_large_image",
      title,
      description,
      images: ["/og-image.png"],
      creator: "@Teo_Goulois",
    },
  };
};
