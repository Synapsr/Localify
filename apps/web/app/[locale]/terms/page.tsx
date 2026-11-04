import type { Metadata } from "next";

import { hasLocale } from "next-intl";
import { setRequestLocale } from "next-intl/server";
import { notFound } from "next/navigation";

import { buildPageMetadata } from "@/app/page-metadata";
import TermsPage from "@/app/terms/page";
import { routing } from "@/i18n/routing";

type Props = Readonly<{ params: Promise<{ locale: string }> }>;

export const generateMetadata = async ({ params }: Props): Promise<Metadata> => {
  const { locale } = await params;
  if (!hasLocale(routing.locales, locale)) notFound();

  return buildPageMetadata(locale, "/terms");
};

const LocalizedTermsPage = async ({ params }: Props) => {
  // Next.js can render a page before its layout, so the page sets the locale too.
  setRequestLocale((await params).locale);

  return <TermsPage />;
};

export default LocalizedTermsPage;
