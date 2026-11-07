import type { Metadata } from "next";

import { getTranslations } from "next-intl/server";

import { LegalPage, LegalSection } from "@/app/_components/legal-page";
import { buildPageMetadata } from "@/app/page-metadata";
import { Link } from "@/i18n/navigation";
import { routing } from "@/i18n/routing";

const linkClass = "text-ink underline underline-offset-4 hover:opacity-70";

export const generateMetadata = (): Promise<Metadata> => {
  return buildPageMetadata(routing.defaultLocale, "/terms");
};

const TermsPage = async () => {
  const t = await getTranslations("Terms");
  const sections = t.raw("sections") as Array<{
    title: string;
    body: string[];
    link?: { label: string; href: string };
  }>;

  return (
    <LegalPage title={t("title")} updatedAt={t("updatedAt")} intro={t("intro")}>
      {sections.map((section) => (
        <LegalSection key={section.title} title={section.title}>
          {section.body.map((paragraph) => (
            <p key={paragraph}>{paragraph}</p>
          ))}
          {section.link ? (
            <p>
              {section.link.href.startsWith("/") ? (
                <Link href={section.link.href} className={linkClass}>
                  {section.link.label}
                </Link>
              ) : (
                <a
                  href={section.link.href}
                  rel="noopener noreferrer"
                  target="_blank"
                  className={linkClass}
                >
                  {section.link.label}
                </a>
              )}
            </p>
          ) : null}
        </LegalSection>
      ))}

      <LegalSection title={t("contact.title")}>
        <p>
          {t("contact.prefix")}{" "}
          <a
            href="mailto:support@localify.dev"
            className={linkClass}
          >
            support@localify.dev
          </a>
          .
        </p>
      </LegalSection>

      <LegalSection id="legal-notice" title={t("legalNotice.title")}>
        <p>{t("legalNotice.publisher")}</p>
        <p>
          {t("legalNotice.contact")}{" "}
          <a
            href="mailto:support@localify.dev"
            className={linkClass}
          >
            support@localify.dev
          </a>
        </p>
        <p>{t("legalNotice.host")}</p>
      </LegalSection>
    </LegalPage>
  );
};

export default TermsPage;
