import type { ReactNode } from "react";

import { useTranslations } from "next-intl";

import { Link } from "@/i18n/navigation";

import { PageShell } from "./page-shell";

type LegalPageProps = {
  title: string;
  updatedAt: string;
  intro: ReactNode;
  children: ReactNode;
};

export const LegalPage = ({ title, updatedAt, intro, children }: LegalPageProps) => {
  const legal = useTranslations("Legal");

  return (
    <PageShell>
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
    </PageShell>
  );
};

export const legalLinkClass = "text-ink underline underline-offset-4 hover:opacity-70";

export type LegalLink = { label: string; href: string };

type SectionProps = {
  id?: string;
  title: string;
  body?: string[];
  quote?: string[];
  links?: LegalLink[];
  children?: ReactNode;
};

// One section of the Terms or the Privacy Policy, from its messages: paragraphs, an optional
// quoted block (a form to copy), whatever the page adds, then its links. Paths starting with
// "/" stay on the site in the reader's locale; other links open in a new tab.
export const LegalSection = ({ id, title, body, quote, links, children }: SectionProps) => {
  return (
    <section id={id} className="scroll-mt-8 space-y-3">
      <h2 className="text-ink text-[20px] font-medium tracking-[-0.015em]">{title}</h2>
      <div className="space-y-3">
        {body?.map((paragraph) => (
          <p key={paragraph}>{paragraph}</p>
        ))}
        {quote ? (
          <blockquote className="space-y-1 border-l-2 border-border pl-4">
            {quote.map((line) => (
              <p key={line}>{line}</p>
            ))}
          </blockquote>
        ) : null}
        {children}
        {links ? (
          <p className="flex flex-wrap gap-x-6 gap-y-1">
            {links.map((link) =>
              link.href.startsWith("/") ? (
                <Link key={link.href} href={link.href} className={legalLinkClass}>
                  {link.label}
                </Link>
              ) : (
                <a
                  key={link.href}
                  href={link.href}
                  rel="noopener noreferrer"
                  target="_blank"
                  className={legalLinkClass}
                >
                  {link.label}
                </a>
              ),
            )}
          </p>
        ) : null}
      </div>
    </section>
  );
};
