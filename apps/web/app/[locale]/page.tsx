import type { Metadata } from "next";

import { hasLocale } from "next-intl";
import { getTranslations, setRequestLocale } from "next-intl/server";
import { notFound } from "next/navigation";

import { Faq } from "@/app/_sections/faq";
import { Features } from "@/app/_sections/features";
import { FinalCta } from "@/app/_sections/final-cta";
import { HeroSection } from "@/app/_sections/hero";
import { HowItWorks } from "@/app/_sections/how-it-works";
import { Pricing } from "@/app/_sections/pricing";
import { UseCases } from "@/app/_sections/use-cases";
import { MACOS_APP_DOWNLOAD_URL } from "@/app/download-links";
import { buildPageMetadata } from "@/app/page-metadata";
import { SiteFooter } from "@/app/site-footer";
import { siteAuthor, siteName, siteUrl } from "@/app/site-metadata";
import { SiteHeader } from "@/app/site-ui";
import { routing } from "@/i18n/routing";

const buildSoftwareJsonLd = (description: string) => ({
  "@context": "https://schema.org",
  "@type": "SoftwareApplication",
  name: siteName,
  applicationCategory: "DeveloperApplication",
  operatingSystem: "macOS",
  url: siteUrl,
  downloadUrl: MACOS_APP_DOWNLOAD_URL,
  description,
  isAccessibleForFree: true,
  license: "https://www.gnu.org/licenses/agpl-3.0.html",
  author: {
    "@type": "Person",
    name: siteAuthor,
  },
});

const buildFaqJsonLd = (
  items: Array<{ question: string; answer: string }>,
) => ({
  "@context": "https://schema.org",
  "@type": "FAQPage",
  mainEntity: items.map((item) => ({
    "@type": "Question",
    name: item.question,
    acceptedAnswer: {
      "@type": "Answer",
      text: item.answer,
    },
  })),
});

type HomePageProps = Readonly<{ params?: Promise<{ locale: string }> }>;

export const generateMetadata = async ({
  params,
}: {
  params: Promise<{ locale: string }>;
}): Promise<Metadata> => {
  const { locale } = await params;
  if (!hasLocale(routing.locales, locale)) notFound();

  return buildPageMetadata(locale, "");
};

const HomePage = async ({ params }: HomePageProps) => {
  // Next.js can render a page before its layout, so the page sets the locale too.
  // app/page.tsx sets the default locale itself and passes no params.
  if (params) {
    setRequestLocale((await params).locale);
  }

  const t = await getTranslations("Metadata");
  const nav = await getTranslations("Navigation");
  const faq = await getTranslations("Faq");

  const navItems = [
    { href: "#use-cases", label: nav("useCases") },
    { href: "#how-it-works", label: nav("howItWorks") },
    { href: "#pricing", label: nav("pricing") },
    { href: "#faq", label: nav("faq") },
  ];

  return (
    <main className="relative z-10 overflow-x-clip rounded-b-4xl bg-background">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{
          __html: JSON.stringify([
            buildSoftwareJsonLd(t("description")),
            buildFaqJsonLd(faq.raw("items").slice(0, 3)),
          ]),
        }}
      />
      <SiteHeader cta={{ href: "#download", label: nav("cta") }} navItems={navItems} />

      <HeroSection />
      <UseCases />
      <Features />
      <HowItWorks />
      <Pricing />
      <Faq />
      <FinalCta />
      <SiteFooter />
    </main>
  );
};

export default HomePage;
