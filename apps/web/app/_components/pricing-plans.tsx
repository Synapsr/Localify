import { getLocale, getTranslations } from "next-intl/server";

import { type IntervalPrice, PricingCards } from "@/app/_components/pricing-cards";
import { FadeIn } from "@/app/site-ui";
import { CheckIcon, GlobeIcon, LockIcon } from "@/components/icons";
import { TEAM_PLAN, formatMoney, getPricing } from "@/lib/pricing";

// Free, Pro and Team with the monthly/yearly switch, then the Pro add-ons and the
// billing fine print: the same block on the landing page and on the pricing page,
// priced live in the visitor's currency (lib/pricing.ts).
export const PricingPlans = async ({ source }: { source: "landing" | "pricing" }) => {
  const t = await getTranslations("Pricing");
  const locale = await getLocale();
  const { currency, pro, addons } = await getPricing();
  const money = (amount: number) => formatMoney(amount, currency, locale);

  // Whole months a year saves against twelve monthly payments: 2 for 79 against 7.99.
  const monthsFree = (monthly: number, yearly: number) => Math.floor((monthly * 12 - yearly) / monthly);
  const prices = (monthly: number, yearly: number): { monthly: IntervalPrice; yearly: IntervalPrice } => {
    const months = monthsFree(monthly, yearly);
    const yearlyNote = t("yearlyNote", { price: money(yearly / 12) });
    return {
      monthly: { price: money(monthly), period: t("perMonth"), note: t("billedMonthly") },
      yearly: {
        price: money(yearly),
        period: t("perYear"),
        note: months >= 1 ? `${yearlyNote} · ${t("monthsFree", { months })}` : yearlyNote,
      },
    };
  };
  const proSaving = monthsFree(pro.monthly.amount, pro.yearly.amount);

  const proFeatures = (t.raw("pro.features") as string[]).map((_, index) =>
    t(`pro.features.${index}`, {
      urls: pro.includes.publicUrls,
      machines: pro.includes.servingMachines,
    }),
  );
  const teamFeatures = (t.raw("team.features") as string[]).map((_, index) =>
    t(`team.features.${index}`, { seats: TEAM_PLAN.seats }),
  );

  const addonItems = [
    { label: t("addons.machine"), price: money(addons.machine.amount) },
    { label: t("addons.urls", { count: addons.urls.unit }), price: money(addons.urls.amount) },
  ];
  const fineprint = [
    { icon: GlobeIcon, text: t("fineprint.currency") },
    { icon: LockIcon, text: t("fineprint.polar") },
    { icon: CheckIcon, text: t("fineprint.cancel") },
  ];

  return (
    <>
      <PricingCards
        free={{
          name: t("free.name"),
          price: money(0),
          period: t("free.period"),
          tagline: t("free.tagline"),
          features: t.raw("free.features") as string[],
          cta: t("free.cta"),
        }}
        intervals={{
          label: t("interval"),
          monthly: t("monthly"),
          yearly: t("yearly"),
          saving: proSaving >= 1 ? t("monthsFree", { months: proSaving }) : undefined,
        }}
        pro={{
          name: t("pro.name"),
          badge: t("pro.badge"),
          tagline: t("pro.tagline"),
          features: proFeatures,
          cta: t("pro.cta"),
          ...prices(pro.monthly.amount, pro.yearly.amount),
        }}
        source={source}
        team={{
          name: t("team.name"),
          tagline: t("team.tagline"),
          features: teamFeatures,
          cta: t("team.cta"),
          seats: TEAM_PLAN.seats,
          ...prices(TEAM_PLAN.monthly, TEAM_PLAN.yearly),
        }}
      />

      <FadeIn className="mx-auto mt-5 max-w-6xl" delay={0.25}>
        <div className="grid gap-4 sm:grid-cols-2 lg:gap-5">
          {addonItems.map((item) => (
            <div
              className="flex items-baseline justify-between gap-4 rounded-3xl bg-muted/60 px-7 py-5 ring-1 ring-border/60 md:px-8"
              key={item.label}
            >
              <p className="text-[15px] font-semibold text-foreground">{item.label}</p>
              <p className="shrink-0 text-[15px] font-semibold text-foreground tabular-nums">
                {t("addons.perMonth", { price: item.price })}
              </p>
            </div>
          ))}
        </div>
        <p className="mt-4 text-center text-[13.5px] leading-[1.6] text-pretty text-muted-foreground">
          <span className="font-medium text-foreground">{t("addons.title")}</span>
          <span aria-hidden="true" className="px-2">
            ·
          </span>
          {t.rich("addons.how", {
            cmd: (chunks) => (
              <code className="rounded-md bg-muted px-1.5 py-0.5 font-mono text-[12.5px] text-foreground">{chunks}</code>
            ),
          })}
        </p>
      </FadeIn>

      <FadeIn className="mx-auto mt-10 max-w-6xl" delay={0.3}>
        <ul className="flex flex-wrap justify-center gap-x-6 gap-y-2 text-[13px] text-muted-foreground">
          {fineprint.map(({ icon: Icon, text }) => (
            <li className="flex items-center gap-1.5" key={text}>
              <Icon aria-hidden="true" className="size-3.5 shrink-0 opacity-70" />
              {text}
            </li>
          ))}
        </ul>
      </FadeIn>
    </>
  );
};
