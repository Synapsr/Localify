import type { Metadata } from "next";

import { getTranslations } from "next-intl/server";

import { CommandLine } from "@/app/_components/command-line";
import { PageIntro } from "@/app/_components/page-intro";
import { PageShell } from "@/app/_components/page-shell";
import { buildPageMetadata } from "@/app/page-metadata";
import { publicUrlPattern, publicUrlsDocsUrl } from "@/app/site-metadata";
import { FadeIn, sectionClass } from "@/app/site-ui";
import { buttonVariants } from "@/components/ui/button-variants";
import { Link } from "@/i18n/navigation";
import { routing } from "@/i18n/routing";

// Polar's customer portal returns here (docs/public-urls.md, "Billing").

export const generateMetadata = (): Promise<Metadata> => {
  return buildPageMetadata(routing.defaultLocale, "/cloud");
};

const CloudPage = async () => {
  const t = await getTranslations("Cloud.overview");
  const points = t.raw("points") as Array<{ title: string; body: string }>;

  return (
    <PageShell>
      <section className={`${sectionClass} pt-16 md:pt-24`}>
        <PageIntro
          description={t("description", { url: publicUrlPattern })}
          eyebrow={t("eyebrow")}
          title={t("title")}
        />

        <FadeIn className="mx-auto mt-14 grid max-w-5xl gap-4 md:mt-20 md:grid-cols-3 md:gap-5">
          {points.map((point) => (
            <article
              key={point.title}
              className="space-y-2.5 rounded-3xl bg-muted/60 p-7 ring-1 ring-border/60 md:p-8"
            >
              <h2 className="text-ink text-[18px] font-semibold tracking-tight text-balance">
                {point.title}
              </h2>
              <p className="text-[14.5px] leading-[1.55] text-pretty text-muted-foreground">
                {point.body}
              </p>
            </article>
          ))}
        </FadeIn>

        <FadeIn className="mx-auto mt-12 max-w-2xl space-y-4 rounded-3xl bg-background p-7 shadow-lift ring-1 ring-foreground/15 md:p-8">
          <h2 className="text-ink text-[20px] font-semibold tracking-tight">{t("billing.title")}</h2>
          <p className="text-[15px] leading-[1.6] text-pretty text-muted-foreground">
            {t("billing.body")}
          </p>
          <CommandLine command="localify billing" />
          <p className="text-[13.5px] leading-[1.55] text-pretty text-muted-foreground">
            {t("billing.note")}
          </p>
        </FadeIn>

        <div className="mt-10 flex flex-wrap items-center justify-center gap-3">
          <Link className={buttonVariants({ variant: "outline", size: "lg" })} href="/pricing">
            {t("pricing")}
          </Link>
          <a
            className={buttonVariants({ variant: "outline", size: "lg" })}
            href={publicUrlsDocsUrl}
            rel="noopener noreferrer"
            target="_blank"
          >
            {t("docs")}
          </a>
        </div>
      </section>
    </PageShell>
  );
};

export default CloudPage;
