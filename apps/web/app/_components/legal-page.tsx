import type { ReactNode } from "react";

import { useTranslations } from "next-intl";

import { SiteFooter } from "@/app/site-footer";
import { sourceUrl } from "@/app/site-metadata";
import { SiteHeader } from "@/app/site-ui";

type LegalPageProps = {
  title: string;
  updatedAt: string;
  intro: ReactNode;
  children: ReactNode;
};

export const LegalPage = ({ title, updatedAt, intro, children }: LegalPageProps) => {
  const nav = useTranslations("Navigation");
  const legal = useTranslations("Legal");
  const navItems = [
    { href: "/#how-it-works", label: nav("howItWorks") },
    { href: "/#faq", label: nav("faq") },
    { href: "/pricing", label: nav("pricing") },
    { href: sourceUrl, label: nav("source") },
  ];

  return (
    <main className="relative z-10 overflow-x-clip rounded-b-4xl bg-background">
      <SiteHeader cta={{ href: "/#download", label: nav("cta") }} navItems={navItems} />

      <section className="site-width relative z-10 pt-16 pb-20 md:pt-24 md:pb-28">
        <div className="mx-auto max-w-3xl space-y-10">
          <header className="space-y-5">
            <h1 className="text-ink text-[40px] leading-[1.05] font-medium tracking-[-0.03em] text-balance md:text-[56px]">
              {title}
            </h1>
            <p className="font-mono text-[11px] tracking-[0.18em] text-muted-foreground uppercase">
              {legal("lastUpdated")} · {updatedAt}
            </p>
            {intro ? (
              <p className="text-[17px] leading-[1.65] text-muted-foreground">{intro}</p>
            ) : null}
          </header>

          <article className="legal-prose space-y-10 text-[15.5px] leading-[1.7] text-muted-foreground">
            {children}
          </article>
        </div>
      </section>

      <SiteFooter />
    </main>
  );
};

type SectionProps = {
  id?: string;
  title: string;
  children: ReactNode;
};

export const LegalSection = ({ id, title, children }: SectionProps) => {
  return (
    <section id={id} className="scroll-mt-8 space-y-3">
      <h2 className="text-ink text-[20px] font-medium tracking-[-0.015em]">{title}</h2>
      <div className="space-y-3">{children}</div>
    </section>
  );
};
