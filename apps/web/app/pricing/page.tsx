import type { Metadata } from "next";

import { getFormatter, getTranslations } from "next-intl/server";

import { PlanCard, PlanFeatures, PlanPrice } from "@/app/_components/plan-card";
import { ProPlanCard } from "@/app/_components/pro-plan-card";
import { type FaqEntry, FaqItem } from "@/app/_sections/faq";
import { buildPageMetadata } from "@/app/page-metadata";
import { SiteFooter } from "@/app/site-footer";
import { sourceUrl } from "@/app/site-metadata";
import { FadeIn, SiteHeader, sectionClass } from "@/app/site-ui";
import { buttonVariants } from "@/components/ui/button-variants";
import { Link } from "@/i18n/navigation";
import { routing } from "@/i18n/routing";

// Prices in US dollars, as Polar charges them.
const PRO_MONTHLY = 7.99;
const PRO_YEARLY = 79;
const ADDON_MACHINE = 3;
const ADDON_URLS = 2;

const PUBLIC_URL_EXAMPLE = "https://<slug>.localify.fyi";
const SUBSCRIBE_COMMAND = "localify subscribe";
const TEAM_MAILTO = "mailto:support@localify.dev?subject=Localify%20Cloud%20Team";

export const generateMetadata = (): Promise<Metadata> => {
  return buildPageMetadata(routing.defaultLocale, "/pricing");
};

const PricingPage = async () => {
  const t = await getTranslations("Pricing");
  const nav = await getTranslations("Navigation");
  const format = await getFormatter();

  const usd = (amount: number) => {
    const digits = Number.isInteger(amount) ? 0 : 2;
    return format.number(amount, {
      style: "currency",
      currency: "USD",
      minimumFractionDigits: digits,
      maximumFractionDigits: digits,
    });
  };
  const yearlySaving = Math.floor((1 - PRO_YEARLY / (PRO_MONTHLY * 12)) * 100) / 100;

  const navItems = [
    { href: "/#how-it-works", label: nav("howItWorks") },
    { href: "/#faq", label: nav("faq") },
    { href: "/pricing", label: nav("pricing") },
    { href: sourceUrl, label: nav("source") },
  ];
  const faqItems = t.raw("faq.items") as FaqEntry[];

  return (
    <main className="relative z-10 overflow-x-clip rounded-b-4xl bg-background">
      <SiteHeader cta={{ href: "/#download", label: nav("cta") }} navItems={navItems} />

      <section className={`${sectionClass} pt-16 md:pt-24`}>
        <div className="mx-auto max-w-4xl space-y-6 text-center">
          <h1 className="text-heading-gradient text-[44px] leading-none font-semibold tracking-[-0.035em] text-balance md:text-[64px] lg:text-[72px]">
            {t("titleLine1")}
            <br />
            {t("titleLine2")}
          </h1>
          <p className="mx-auto max-w-xl text-[17px] leading-[1.55] text-pretty text-muted-foreground md:text-[18px]">
            {t("description")}
          </p>
        </div>

        <FadeIn className="mt-14 grid gap-4 md:mt-20 md:grid-cols-3 md:gap-5">
          <PlanCard name={t("free.name")} tagline={t("free.tagline")}>
            <PlanPrice note={t("free.note")} price={usd(0)} />
            <PlanFeatures items={t.raw("free.features") as string[]} />
            <div className="mt-auto">
              <Link
                className={`${buttonVariants({ variant: "outline", size: "lg" })} w-full`}
                data-umami-event="download_cta_clicked"
                data-umami-event-source="pricing"
                href="/#download"
              >
                {t("free.cta")}
              </Link>
            </div>
          </PlanCard>

          <ProPlanCard
            badge={t("pro.badge")}
            features={t.raw("pro.features") as string[]}
            intervalLabel={t("pro.interval")}
            monthly={{
              label: t("pro.monthly"),
              price: usd(PRO_MONTHLY),
              period: t("pro.perMonth"),
              note: t("pro.monthlyNote"),
              command: SUBSCRIBE_COMMAND,
            }}
            name={t("pro.name")}
            saveLabel={t("pro.save", { percent: format.number(yearlySaving, { style: "percent" }) })}
            start={t("pro.start")}
            tagline={t("pro.tagline", { url: PUBLIC_URL_EXAMPLE })}
            trial={t("pro.trial")}
            yearly={{
              label: t("pro.yearly"),
              price: usd(PRO_YEARLY),
              period: t("pro.perYear"),
              note: t("pro.yearlyNote", {
                price: format.number(PRO_YEARLY / 12, {
                  style: "currency",
                  currency: "USD",
                  maximumFractionDigits: 2,
                }),
              }),
              command: `${SUBSCRIBE_COMMAND} --yearly`,
            }}
          />

          <PlanCard badge={t("team.badge")} name={t("team.name")} tagline={t("team.tagline")}>
            <p className="text-[14.5px] leading-[1.55] text-pretty text-foreground">{t("team.body")}</p>
            <div className="mt-auto">
              <a
                className={`${buttonVariants({ variant: "outline", size: "lg" })} w-full`}
                href={TEAM_MAILTO}
              >
                {t("team.cta")}
              </a>
            </div>
          </PlanCard>
        </FadeIn>

        <FadeIn className="mx-auto mt-10 max-w-3xl space-y-6 text-center">
          <div className="space-y-2">
            <p className="text-ink text-[14px] font-medium">{t("addons.title")}</p>
            <p className="text-[14px] text-muted-foreground">
              {t("addons.machine", { price: usd(ADDON_MACHINE) })}
              <span aria-hidden="true" className="px-2">
                ·
              </span>
              {t("addons.urls", { price: usd(ADDON_URLS) })}
            </p>
          </div>
          <p className="text-[13px] leading-[1.6] text-pretty text-muted-foreground">
            {t("billing")}
          </p>
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

      <SiteFooter />
    </main>
  );
};

export default PricingPage;
