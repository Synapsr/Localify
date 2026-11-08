import { getLocale, getTranslations } from "next-intl/server";

import { PlanCard, PlanFeatures, PlanPrice } from "@/app/_components/plan-card";
import { ProPlanCard } from "@/app/_components/pro-plan-card";
import { publicUrlPattern } from "@/app/site-metadata";
import { FadeIn } from "@/app/site-ui";
import { buttonVariants } from "@/components/ui/button-variants";
import { Link } from "@/i18n/navigation";
import { formatMoney, getPricing } from "@/lib/pricing";

const LOGIN_COMMAND = "localify login";
const SUBSCRIBE_COMMAND = "localify subscribe";
const TEAM_MAILTO = "mailto:support@localify.dev?subject=Localify%20Cloud%20Team";

// Free, Pro and Team, then the add-ons and the billing note: the same block on the
// landing page and on the pricing page, priced live for the visitor's currency.
export const Plans = async ({ source }: { source: "landing" | "pricing" }) => {
  const t = await getTranslations("Pricing");
  const locale = await getLocale();
  const { currency, pro, addons } = await getPricing();
  const money = (amount: number) => formatMoney(amount, currency, locale);

  const yearlySaving = Math.floor((1 - pro.yearly.amount / (pro.monthly.amount * 12)) * 100) / 100;
  const percent = new Intl.NumberFormat(locale, { style: "percent" }).format(yearlySaving);
  const features = (t.raw("pro.features") as string[]).map((_, index) =>
    t(`pro.features.${index}`, {
      machines: pro.includes.servingMachines,
      urls: pro.includes.publicUrls,
    }),
  );

  return (
    <>
      <FadeIn className="mt-14 grid gap-4 md:mt-20 md:grid-cols-3 md:gap-5">
        <PlanCard name={t("free.name")} tagline={t("free.tagline")}>
          <PlanPrice note={t("free.note")} price={money(0)} />
          <PlanFeatures items={t.raw("free.features") as string[]} />
          <div className="mt-auto">
            <Link
              className={`${buttonVariants({ variant: "outline", size: "lg" })} w-full`}
              data-umami-event="download_cta_clicked"
              data-umami-event-source={source}
              href="/#download"
            >
              {t("free.cta")}
            </Link>
          </div>
        </PlanCard>

        <ProPlanCard
          badge={t("pro.badge")}
          cta={t("pro.cta")}
          features={features}
          intervalLabel={t("pro.interval")}
          monthly={{
            label: t("pro.monthly"),
            price: money(pro.monthly.amount),
            period: t("pro.perMonth"),
            note: t("pro.monthlyNote"),
            command: SUBSCRIBE_COMMAND,
          }}
          loginCommand={LOGIN_COMMAND}
          name={t("pro.name")}
          saveLabel={t("pro.save", { percent })}
          source={source}
          steps={t("pro.steps")}
          tagline={t("pro.tagline", { url: publicUrlPattern })}
          trial={t("pro.trial")}
          yearly={{
            label: t("pro.yearly"),
            price: money(pro.yearly.amount),
            period: t("pro.perYear"),
            note: t("pro.yearlyNote", { price: money(pro.yearly.amount / 12) }),
            command: `${SUBSCRIBE_COMMAND} --yearly`,
          }}
        />

        <PlanCard badge={t("team.badge")} name={t("team.name")} tagline={t("team.tagline")}>
          <p className="text-[14.5px] leading-[1.55] text-pretty text-foreground">{t("team.body")}</p>
          <div className="mt-auto">
            <a
              className={`${buttonVariants({ variant: "outline", size: "lg" })} w-full`}
              data-umami-event="cloud_team_clicked"
              data-umami-event-source={source}
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
          <p className="text-[14px] text-foreground">
            {t("addons.machine", { price: money(addons.machine.amount) })}
            <span aria-hidden="true" className="px-2 text-muted-foreground">
              ·
            </span>
            {t("addons.urls", { count: addons.urls.unit, price: money(addons.urls.amount) })}
          </p>
          <p className="text-[13.5px] leading-[1.6] text-pretty text-muted-foreground">{t("addons.how")}</p>
        </div>
        <p className="text-[13px] leading-[1.6] text-pretty text-muted-foreground">{t("billing")}</p>
      </FadeIn>
    </>
  );
};
