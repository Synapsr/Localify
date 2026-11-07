import type { Metadata } from "next";

import { getTranslations } from "next-intl/server";

import { type LegalLink, LegalPage, LegalSection } from "@/app/_components/legal-page";
import { buildPageMetadata } from "@/app/page-metadata";
import { routing } from "@/i18n/routing";

export const generateMetadata = (): Promise<Metadata> => {
  return buildPageMetadata(routing.defaultLocale, "/privacy");
};

const PrivacyPage = async () => {
  const t = await getTranslations("Privacy");
  const sections = t.raw("sections") as Array<{
    title: string;
    body?: string[];
    list?: Array<{ name: string; description: string }>;
    links?: LegalLink[];
  }>;

  return (
    <LegalPage title={t("title")} updatedAt={t("updatedAt")} intro={t("intro")}>
      {sections.map((section) => (
        <LegalSection
          key={section.title}
          body={section.body}
          links={section.links}
          title={section.title}
        >
          {section.list ? (
            <ul className="list-disc space-y-1.5 pl-5">
              {section.list.map((item) => (
                <li key={item.name}>
                  <strong className="text-ink font-medium">{item.name}</strong> —{" "}
                  {item.description}
                </li>
              ))}
            </ul>
          ) : null}
        </LegalSection>
      ))}
    </LegalPage>
  );
};

export default PrivacyPage;
