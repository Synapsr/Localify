import type { Metadata } from "next";

import { getTranslations } from "next-intl/server";

import { SiteFooter } from "@/app/site-footer";
import { sourceUrl } from "@/app/site-metadata";
import { SiteHeader } from "@/app/site-ui";
import { Link } from "@/i18n/navigation";

export const metadata: Metadata = {
  title: "Coming soon",
  description: "This page isn't ready yet. It's on the list and won't be long.",
  robots: {
    index: false,
    follow: true,
  },
};

const SoonPage = async () => {
  const t = await getTranslations("Soon");
  const nav = await getTranslations("Navigation");
  const navItems = [
    { href: "/#how-it-works", label: nav("howItWorks") },
    { href: "/#faq", label: nav("faq") },
    { href: sourceUrl, label: nav("source") },
  ];

  return (
    <main className="relative z-10 flex min-h-dvh flex-col overflow-x-clip rounded-b-4xl bg-background">
      <SiteHeader cta={{ href: "/#download", label: nav("cta") }} navItems={navItems} />

    <section className="site-width relative z-10 flex flex-1 items-center justify-center py-24 md:py-32">
      <div className="mx-auto max-w-xl space-y-6 text-center">
        <span
          aria-hidden="true"
          className="relative inline-flex h-2 w-2 items-center justify-center"
        >
          <span className="bg-ink/30 absolute inline-flex h-full w-full animate-ping rounded-full" />
          <span className="bg-ink relative inline-flex h-2 w-2 rounded-full" />
        </span>

        <p className="font-mono text-[11px] tracking-[0.22em] text-muted-foreground uppercase">
          {t("eyebrow")}
        </p>

        <h1 className="text-ink text-[40px] leading-[1.05] font-medium tracking-[-0.03em] text-balance md:text-[56px]">
          {t("title")}
        </h1>

        <p className="mx-auto max-w-md text-[16px] leading-[1.6] text-muted-foreground">
          {t("description")}
        </p>

        <div className="flex items-center justify-center gap-4 pt-2 text-[13.5px]">
          <Link
            href="/"
            className="text-ink inline-flex items-center gap-1.5 underline underline-offset-4 transition-opacity duration-150 hover:opacity-70"
          >
            <span aria-hidden="true">←</span>
            {t("backHome")}
          </Link>
          <span className="text-border" aria-hidden="true">
            ·
          </span>
          <a
            href="mailto:support@localify.dev"
            className="hover:text-ink text-muted-foreground transition-colors duration-150"
          >
            support@localify.dev
          </a>
        </div>
      </div>
    </section>

    <SiteFooter />
    </main>
  );
};

export default SoonPage;
