import type { ComponentProps } from "react";
import type { Metadata } from "next";

import { getTranslations } from "next-intl/server";

import { LegalPage, LegalSection, legalLinkClass } from "@/app/_components/legal-page";
import { buildPageMetadata } from "@/app/page-metadata";
import { routing } from "@/i18n/routing";

export const generateMetadata = (): Promise<Metadata> => {
  return buildPageMetadata(routing.defaultLocale, "/terms");
};

const TermsPage = async () => {
  const t = await getTranslations("Terms");
  const sections = t.raw("sections") as Array<ComponentProps<typeof LegalSection>>;

  return (
    <LegalPage title={t("title")} updatedAt={t("updatedAt")} intro={t("intro")}>
      {sections.map((section) => (
        <LegalSection key={section.title} {...section} />
      ))}

      <LegalSection title={t("contact.title")}>
        <p>
          {t("contact.prefix")}{" "}
          <a href="mailto:support@localify.dev" className={legalLinkClass}>
            support@localify.dev
          </a>
          .
        </p>
      </LegalSection>

      <LegalSection id="legal-notice" title={t("legalNotice.title")}>
        <p>{t("legalNotice.publisher")}</p>
        <p>
          {t("legalNotice.contact")}{" "}
          <a href="mailto:support@localify.dev" className={legalLinkClass}>
            support@localify.dev
          </a>
        </p>
        <p>{t("legalNotice.host")}</p>
      </LegalSection>
    </LegalPage>
  );
};

export default TermsPage;
