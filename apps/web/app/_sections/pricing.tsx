import { getTranslations } from "next-intl/server";

import { AppleGlyph } from "@/app/_components/apple-glyph";
import { MACOS_APP_DOWNLOAD_URL } from "@/app/download-links";
import { FadeIn, SectionHeading, sectionClass } from "@/app/site-ui";
import { CheckIcon } from "@/components/icons";
import { buttonVariants } from "@/components/ui/button-variants";
import { Link } from "@/i18n/navigation";

const INVITE_URL = "mailto:support@localify.dev?subject=Localify%20Cloud%20invite";

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

export const Pricing = async () => {
  const t = await getTranslations("Pricing");

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

      <div className="mx-auto mt-16 grid max-w-4xl gap-4 md:mt-20 md:grid-cols-2 md:gap-5">
        <FadeIn delay={0.05}>
          <article className="flex h-full flex-col gap-7 rounded-3xl bg-muted/60 p-7 ring-1 ring-border/60 md:p-9">
            <header className="space-y-2">
              <h3 className="text-[15px] font-semibold text-foreground">{t("free.name")}</h3>
              <p className="flex items-baseline gap-2">
                <span className="text-[44px] leading-none font-semibold tracking-[-0.03em] text-foreground">
                  {t("free.price")}
                </span>
                <span className="text-[14px] text-muted-foreground">{t("free.period")}</span>
              </p>
              <p className="text-[14.5px] text-muted-foreground">{t("free.tagline")}</p>
            </header>
            <div className="flex-1 text-foreground">
              <Features items={t.raw("free.features") as string[]} />
            </div>
            <Link
              className={`${buttonVariants({ variant: "outline", size: "lg" })} w-full`}
              data-umami-event="download_clicked"
              data-umami-event-source="pricing"
              href={MACOS_APP_DOWNLOAD_URL}
            >
              <AppleGlyph className="h-4 w-4" />
              {t("free.cta")}
            </Link>
          </article>
        </FadeIn>

        <FadeIn delay={0.13}>
          <article className="flex h-full flex-col gap-7 rounded-3xl bg-foreground p-7 text-background md:p-9">
            <header className="space-y-2">
              <h3 className="text-[15px] font-semibold">{t("pro.name")}</h3>
              <p className="flex flex-wrap items-baseline gap-x-2">
                <span className="text-[44px] leading-none font-semibold tracking-[-0.03em]">{t("pro.price")}</span>
                <span className="text-[14px] text-background/65">{t("pro.period")}</span>
                <span className="text-[14px] text-background/65">· {t("pro.yearly")}</span>
              </p>
              <p className="text-[14.5px] text-background/65">{t("pro.tagline")}</p>
            </header>
            <div className="flex-1">
              <Features items={t.raw("pro.features") as string[]} />
            </div>
            <div className="space-y-3">
              <a
                className={`${buttonVariants({ variant: "outline", size: "lg" })} w-full border-background/20 bg-background text-foreground hover:bg-background/90`}
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
      </div>
    </section>
  );
};
