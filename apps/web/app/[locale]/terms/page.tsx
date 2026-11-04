import { setRequestLocale } from "next-intl/server";

import TermsPage from "@/app/terms/page";

export { metadata } from "@/app/terms/page";

type Props = Readonly<{ params: Promise<{ locale: string }> }>;

const LocalizedTermsPage = async ({ params }: Props) => {
  // Next.js can render a page before its layout, so the page sets the locale too.
  setRequestLocale((await params).locale);

  return <TermsPage />;
};

export default LocalizedTermsPage;
