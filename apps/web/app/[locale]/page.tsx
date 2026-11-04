import { getTranslations, setRequestLocale } from "next-intl/server";

import { Faq } from "@/app/_sections/faq";
import { FinalCta } from "@/app/_sections/final-cta";
import { HeroSection } from "@/app/_sections/hero";
import { HowItWorks } from "@/app/_sections/how-it-works";
import { Problem } from "@/app/_sections/problem";
import { MACOS_APP_DOWNLOAD_URL } from "@/app/download-links";
import { SiteFooter } from "@/app/site-footer";
import { siteAuthor, siteName, siteUrl, sourceUrl } from "@/app/site-metadata";
import { SiteHeader } from "@/app/site-ui";

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

const HomePage = async ({ params }: HomePageProps) => {
  // Next.js can render a page before its layout, so the page sets the locale too.
  // app/page.tsx sets the default locale itself and passes no params.
  if (params) {
    setRequestLocale((await params).locale);
  }

  const t = await getTranslations("Home");
  const nav = await getTranslations("Navigation");
  const faq = await getTranslations("Faq");

  const navItems = [
    { href: "#how-it-works", label: nav("howItWorks") },
    { href: "#faq", label: nav("faq") },
    { href: sourceUrl, label: nav("source") },
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
      <Problem />
      <HowItWorks />
      <Faq />
      <FinalCta />
      <SiteFooter />
    </main>
  );
};

export default HomePage;
