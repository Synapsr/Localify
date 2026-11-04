import { setRequestLocale } from "next-intl/server";

import PrivacyPage from "@/app/privacy/page";

export { metadata } from "@/app/privacy/page";

type Props = Readonly<{ params: Promise<{ locale: string }> }>;

const LocalizedPrivacyPage = async ({ params }: Props) => {
  // Next.js can render a page before its layout, so the page sets the locale too.
  setRequestLocale((await params).locale);

  return <PrivacyPage />;
};

export default LocalizedPrivacyPage;
