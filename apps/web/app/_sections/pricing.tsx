import { getTranslations } from "next-intl/server";

import { AppleGlyph } from "@/app/_components/apple-glyph";
import { MACOS_APP_DOWNLOAD_URL } from "@/app/download-links";
import { FadeIn, SectionHeading, sectionClass } from "@/app/site-ui";
import { CheckIcon } from "@/components/icons";
import { buttonVariants } from "@/components/ui/button-variants";
import { Link } from "@/i18n/navigation";
import { cn } from "@/lib/utils";

const INVITE_URL = "mailto:support@localify.dev?subject=Localify%20Cloud%20invite";
const TEAM_URL = "mailto:support@localify.dev?subject=Localify%20Cloud%20Team";

const Features = ({ items }: { items: string[] }) => (
  <ul className="space-y-3">
    {items.map((item) => (
      <li className="flex items-start gap-3 text-[14.5px] leading-[1.45] text-pretty" key={item}>
        <CheckIcon className="mt-0.5 size-4 shrink-0 opacity-70" strokeWidth={2.5} />
        {item}
      </li>
    ))}
  </ul>
);

const Price = ({ price, period, extra, muted }: { price: string; period: string; extra?: string; muted: string }) => (
  <p className="flex flex-wrap items-baseline gap-x-2">
    <span className="text-[44px] leading-none font-semibold tracking-[-0.03em]">{price}</span>
    <span className={cn("text-[14px]", muted)}>{period}</span>
    {extra ? <span className={cn("text-[14px]", muted)}>· {extra}</span> : null}
  </p>
);

export const Pricing = async () => {
  const t = await getTranslations("Pricing");
  const card = "flex h-full flex-col gap-7 rounded-3xl p-7 md:p-8";

  return (
    <section className={sectionClass} id="pricing">
      <FadeIn>
        <SectionHeading
          align="center"
          title={
            <>
              {t("titleLine1")}
              <br />
              {t("titleLine2")}
            </>
          }
          description={t("description")}
        />
      </FadeIn>

      <div className="mx-auto mt-16 grid max-w-6xl gap-4 md:mt-20 lg:grid-cols-3 lg:gap-5">
        <FadeIn delay={0.05}>
          <article className={cn(card, "bg-muted/60 text-foreground ring-1 ring-border/60")}>
            <header className="space-y-2">
              <h3 className="text-[15px] font-semibold">{t("free.name")}</h3>
              <Price muted="text-muted-foreground" period={t("free.period")} price={t("free.price")} />
              <p className="text-[14.5px] text-muted-foreground">{t("free.tagline")}</p>
            </header>
            <div className="flex-1">
              <Features items={t.raw("free.features") as string[]} />
            </div>
            <Link
              className={cn(buttonVariants({ variant: "outline", size: "lg" }), "w-full")}
              data-umami-event="download_clicked"
              data-umami-event-source="pricing"
              href={MACOS_APP_DOWNLOAD_URL}
            >
              <AppleGlyph className="h-4 w-4" />
              {t("free.cta")}
            </Link>
          </article>
        </FadeIn>

        <FadeIn delay={0.12}>
          <article className={cn(card, "relative bg-foreground text-background shadow-[0_40px_80px_-40px_rgba(13,13,13,0.6)]")}>
            <span className="absolute top-7 right-7 rounded-full bg-background/15 px-2.5 py-1 text-[12px] font-medium md:top-8 md:right-8">
              {t("pro.badge")}
            </span>
            <header className="space-y-2">
              <h3 className="text-[15px] font-semibold">{t("pro.name")}</h3>
              <Price extra={t("pro.yearly")} muted="text-background/65" period={t("pro.period")} price={t("pro.price")} />
              <p className="text-[14.5px] text-background/65">{t("pro.tagline")}</p>
            </header>
            <div className="flex-1 space-y-6">
              <Features items={t.raw("pro.features") as string[]} />
              <div className="rounded-2xl bg-background/10 p-4">
                <p className="flex items-center gap-2 text-[13px] font-medium">
                  {t("addons.title")}
                  <span className="rounded-full bg-background/15 px-2 py-0.5 text-[11px]">{t("addons.soon")}</span>
                </p>
                <ul className="mt-2 space-y-1 text-[13px] text-background/70">
                  <li>{t("addons.machine")}</li>
                  <li>{t("addons.urls")}</li>
                </ul>
              </div>
            </div>
            <div className="space-y-3">
              <a
                className={cn(
                  buttonVariants({ variant: "outline", size: "lg" }),
                  "w-full border-background/20 bg-background text-foreground hover:bg-background/90",
                )}
                data-umami-event="cloud_invite_clicked"
                data-umami-event-source="pricing"
                href={INVITE_URL}
              >
                {t("pro.cta")}
              </a>
              <p className="text-center text-[12.5px] text-background/60">{t("pro.note")}</p>
            </div>
          </article>
        </FadeIn>

        <FadeIn delay={0.19}>
          <article className={cn(card, "bg-background text-foreground ring-1 ring-border")}>
            <header className="space-y-2">
              <h3 className="text-[15px] font-semibold">{t("team.name")}</h3>
              <Price muted="text-muted-foreground" period={t("team.period")} price={t("team.price")} />
              <p className="text-[14.5px] text-muted-foreground">{t("team.tagline")}</p>
            </header>
            <div className="flex-1">
              <Features items={t.raw("team.features") as string[]} />
            </div>
            <a
              className={cn(buttonVariants({ variant: "outline", size: "lg" }), "w-full")}
              data-umami-event="cloud_team_clicked"
              data-umami-event-source="pricing"
              href={TEAM_URL}
            >
              {t("team.cta")}
            </a>
          </article>
        </FadeIn>
      </div>
    </section>
  );
};
