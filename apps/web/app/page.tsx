import { NextIntlClientProvider } from "next-intl";
import { getMessages, setRequestLocale } from "next-intl/server";

import { pickClientMessages } from "@/i18n/client-messages";
import { routing } from "@/i18n/routing";

import HomePage from "./[locale]/page";

const RootPage = async () => {
  const locale = routing.defaultLocale;
  setRequestLocale(locale);
  const messages = await getMessages({ locale });

  return (
    <NextIntlClientProvider locale={locale} messages={pickClientMessages(messages)}>
      <HomePage />
    </NextIntlClientProvider>
  );
};

export default RootPage;
