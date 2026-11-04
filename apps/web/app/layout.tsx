import type { Metadata } from "next";
import type { ReactNode } from "react";

import { Agentation } from "agentation";
import Script from "next/script";
import { NextIntlClientProvider } from "next-intl";
import { getMessages } from "next-intl/server";

import { pickClientMessages } from "@/i18n/client-messages";
import { routing } from "@/i18n/routing";

import "./globals.css";
import { PageLogoReveal } from "./_components/page-logo-reveal";
import { siteAuthor, siteDescription, siteName, siteTitle, siteUrl } from "./site-metadata";

export const metadata: Metadata = {
  metadataBase: new URL(siteUrl),
  applicationName: siteName,
  title: {
    default: siteTitle,
    template: `%s - ${siteName}`,
  },
  description: siteDescription,
  alternates: {
    canonical: "/",
    languages: {
      en: "/",
      fr: "/fr",
      es: "/es",
      de: "/de",
      "pt-BR": "/pt-BR",
      ja: "/ja",
      ko: "/ko",
      "zh-CN": "/zh-CN",
      ru: "/ru",
    },
  },
  keywords: [
    "local development domains",
    "trusted localhost HTTPS",
    "local HTTPS proxy",
    "mkcert alternative",
    "open source local proxy",
    "macOS developer tool",
  ],
  authors: [{ name: siteAuthor }],
  creator: siteAuthor,
  publisher: siteAuthor,
  category: "developer tools",
  openGraph: {
    type: "website",
    locale: "en_US",
    url: "/",
    siteName,
    title: siteTitle,
    description: siteDescription,
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
    title: siteTitle,
    description: siteDescription,
    images: ["/og-image.png"],
    creator: "@Teo_Goulois",
  },
  icons: {
    icon: [
      { url: "/favicon.ico" },
      { url: "/icon0.svg", type: "image/svg+xml" },
      { url: "/icon1.png", type: "image/png", sizes: "96x96" },
    ],
    apple: [{ url: "/apple-icon.png", sizes: "180x180", type: "image/png" }],
  },
  manifest: "/manifest.json",
  robots: {
    index: true,
    follow: true,
    googleBot: {
      index: true,
      follow: true,
      "max-image-preview": "large",
      "max-snippet": -1,
      "max-video-preview": -1,
    },
  },
};

type RootLayoutProps = Readonly<{
  children: ReactNode;
}>;

const RootLayout = async ({ children }: RootLayoutProps) => {
  // Pass the locale explicitly. With no middleware, a bare getLocale() or getMessages() here
  // runs before [locale]/layout.tsx sets the locale and caches the default for the whole
  // request, so server components on /fr would render English.
  const locale = routing.defaultLocale;
  const messages = await getMessages({ locale });

  return (
    <html lang={locale} className=" bg-muted">
      <body className=" font-sans scheme-light">
        <NextIntlClientProvider messages={pickClientMessages(messages)}>
          <PageLogoReveal>{children}</PageLogoReveal>
        </NextIntlClientProvider>
        {process.env.NODE_ENV === "development" && <Agentation />}
        {process.env.NODE_ENV === "production" && (
          <Script
            defer
            src="https://analytics.teogoulois.com/script.js"
            data-website-id="0b09d609-3961-4e40-8eab-e87ab51201c9"
            data-domains="localify.dev,www.localify.dev"
            strategy="afterInteractive"
          />
        )}
      </body>
    </html>
  );
};

export default RootLayout;
