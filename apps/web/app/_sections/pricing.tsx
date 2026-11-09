import { getTranslations } from "next-intl/server";

import { PricingPlans } from "@/app/_components/pricing-plans";
import { FadeIn, SectionHeading, sectionClass } from "@/app/site-ui";

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

      <PricingPlans source="landing" />
    </section>
  );
};
