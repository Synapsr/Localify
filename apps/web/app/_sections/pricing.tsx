import { getTranslations } from "next-intl/server";

import { Plans } from "@/app/_components/plans";
import { FadeIn, SectionHeading, sectionClass } from "@/app/site-ui";
import { Link } from "@/i18n/navigation";

export const Pricing = async () => {
  const t = await getTranslations("Pricing.section");

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

      <Plans source="landing" />

      <FadeIn className="mt-8 text-center">
        <Link
          className="text-[14px] text-foreground underline decoration-border decoration-[1.5px] underline-offset-[5px] transition-colors hover:decoration-foreground"
          href="/pricing"
        >
          {t("more")}
        </Link>
      </FadeIn>
    </section>
  );
};
