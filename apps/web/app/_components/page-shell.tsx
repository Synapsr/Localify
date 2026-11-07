import type { ReactNode } from "react";

import { useTranslations } from "next-intl";

import { SiteFooter } from "@/app/site-footer";
import { sourceUrl } from "@/app/site-metadata";
import { SiteHeader } from "@/app/site-ui";

// The header, footer and page surface every page but the landing page shares.
export const PageShell = ({ children }: { children: ReactNode }) => {
  const nav = useTranslations("Navigation");
  const navItems = [
    { href: "/#how-it-works", label: nav("howItWorks") },
    { href: "/#faq", label: nav("faq") },
    { href: "/pricing", label: nav("pricing") },
    { href: sourceUrl, label: nav("source") },
  ];

  return (
    <main className="relative z-10 overflow-x-clip rounded-b-4xl bg-background">
      <SiteHeader cta={{ href: "/#download", label: nav("cta") }} navItems={navItems} />
      {children}
      <SiteFooter />
    </main>
  );
};
