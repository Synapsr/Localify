import type { Metadata } from "next";

import { getTranslations } from "next-intl/server";

import { PageIntro } from "@/app/_components/page-intro";
import { PageShell } from "@/app/_components/page-shell";
import { PricingPlans } from "@/app/_components/pricing-plans";
import { type FaqEntry, FaqItem } from "@/app/_sections/faq";
import { buildPageMetadata } from "@/app/page-metadata";
import { FadeIn, sectionClass } from "@/app/site-ui";
import { routing } from "@/i18n/routing";

export const generateMetadata = (): Promise<Metadata> => {
  return buildPageMetadata(routing.defaultLocale, "/pricing");
};

const PricingPage = async () => {
  const t = await getTranslations("Pricing");
  const faqItems = t.raw("faq.items") as FaqEntry[];

  return (
    <PageShell>
      <section className={`${sectionClass} pt-16 md:pt-24`}>
        <PageIntro
          description={t("description")}
          title={
            <>
              {t("titleLine1")}
              <br />
              {t("titleLine2")}
            </>
          }
        />
        <PricingPlans source="pricing" />
        <FadeIn className="mx-auto mt-10 max-w-3xl space-y-4 text-center">
          <p className="text-[13.5px] leading-[1.6] text-pretty text-muted-foreground">{t("addons.how")}</p>
          <p className="text-[13px] leading-[1.6] text-pretty text-muted-foreground">{t("billing")}</p>
        </FadeIn>
      </section>

      <section className={sectionClass} id="pricing-faq">
        <h2 className="text-heading-gradient mx-auto max-w-3xl text-center text-[36px] leading-none font-semibold tracking-[-0.035em] text-balance md:text-[48px]">
          {t("faq.title")}
        </h2>
        <FadeIn className="mx-auto mt-12 flex max-w-3xl flex-col gap-2.5 md:mt-16">
          {faqItems.map((item) => (
            <FaqItem key={item.question} {...item} />
          ))}
        </FadeIn>
      </section>
    </PageShell>
  );
};

export default PricingPage;
