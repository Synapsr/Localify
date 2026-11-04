import type { Metadata } from "next";

import { getTranslations } from "next-intl/server";

import { LegalPage, LegalSection } from "@/app/_components/legal-page";

export const metadata: Metadata = {
  title: "Privacy Policy",
  description:
    "What Localify and its website collect, what they do not, and how that data is handled.",
};

const PrivacyPage = async () => {
  const t = await getTranslations("Privacy");
  const sections = t.raw("sections") as Array<{
    title: string;
    body?: string[];
    list?: Array<{ name: string; description: string }>;
  }>;

  return (
    <LegalPage title={t("title")} updatedAt={t("updatedAt")} intro={t("intro")}>
      {sections.map((section) => (
        <LegalSection key={section.title} title={section.title}>
          {section.body?.map((paragraph) => (
            <p key={paragraph}>{paragraph}</p>
          ))}
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
