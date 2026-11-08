import type { Metadata } from "next";

import { getTranslations } from "next-intl/server";

import { CommandLine } from "@/app/_components/command-line";
import { PageIntro } from "@/app/_components/page-intro";
import { PageShell } from "@/app/_components/page-shell";
import { buildPageMetadata } from "@/app/page-metadata";
import { publicUrlsDocsUrl } from "@/app/site-metadata";
import { FadeIn, sectionClass } from "@/app/site-ui";
import { buttonVariants } from "@/components/ui/button-variants";
import { Link } from "@/i18n/navigation";
import { routing } from "@/i18n/routing";

// Polar's checkout returns here (docs/public-urls.md, "Billing"). Reaching this page grants
// nothing: the plan starts when Localify Cloud sees the subscription, so the copy never says
// it is active. The checkout_id in the query is not read.

const steps = [
  { key: "check", command: "localify whoami" },
  { key: "create", command: "localify public-url create <project-domain>" },
] as const;

export const generateMetadata = (): Promise<Metadata> => {
  return buildPageMetadata(routing.defaultLocale, "/cloud/subscribed");
};

const SubscribedPage = async () => {
  const t = await getTranslations("Cloud.subscribed");

  return (
    <PageShell>
      <section className={`${sectionClass} pt-16 md:pt-24`}>
        <PageIntro description={t("description")} eyebrow={t("eyebrow")} title={t("title")} />

        <FadeIn className="mx-auto mt-14 max-w-2xl space-y-4 md:mt-20">
          {steps.map((step, index) => (
            <article
              key={step.key}
              className="space-y-3 rounded-3xl bg-muted/60 p-7 ring-1 ring-border/60 md:p-8"
            >
              <h2 className="text-ink flex items-baseline gap-3 text-[18px] font-semibold tracking-tight">
                <span className="font-mono text-[13px] text-muted-foreground tabular-nums">
                  {index + 1}
                </span>
                {t(`steps.${step.key}.title`)}
              </h2>
              <p className="text-[15px] leading-[1.6] text-pretty text-muted-foreground">
                {t(`steps.${step.key}.body`)}
              </p>
              <CommandLine command={step.command} />
            </article>
          ))}

          <p className="pt-4 text-center text-[14px] leading-[1.6] text-pretty text-muted-foreground">
            {t("trialNote")}
          </p>
          <p className="text-center text-[14px] leading-[1.6] text-pretty text-muted-foreground">
            {t("helpPrefix")}{" "}
            <a
              className="text-foreground underline decoration-border decoration-[1.5px] underline-offset-[5px] transition-colors hover:decoration-foreground"
              href="mailto:support@localify.dev"
            >
              support@localify.dev
            </a>
          </p>
        </FadeIn>

        <div className="mt-10 flex flex-wrap items-center justify-center gap-3">
          <Link className={buttonVariants({ variant: "outline", size: "lg" })} href="/cloud">
            {t("overview")}
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

export default SubscribedPage;
