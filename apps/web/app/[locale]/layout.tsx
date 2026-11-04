import type { ReactNode } from "react";

import { hasLocale } from "next-intl";
import { NextIntlClientProvider } from "next-intl";
import { getMessages, setRequestLocale } from "next-intl/server";
import { notFound } from "next/navigation";

import { DocumentLang } from "@/app/_components/document-lang";
import { pickClientMessages } from "@/i18n/client-messages";
import { routing } from "@/i18n/routing";

type LocaleLayoutProps = Readonly<{
  children: ReactNode;
  params: Promise<{ locale: string }>;
}>;

export const generateStaticParams = () => routing.locales.map((locale) => ({ locale }));

const LocaleLayout = async ({ children, params }: LocaleLayoutProps) => {
  const { locale } = await params;

  if (!hasLocale(routing.locales, locale)) {
    notFound();
  }

  setRequestLocale(locale);

  const messages = await getMessages({ locale });

  return (
    <NextIntlClientProvider locale={locale} messages={pickClientMessages(messages)}>
      <DocumentLang locale={locale} />
      {children}
    </NextIntlClientProvider>
  );
};

export default LocaleLayout;
