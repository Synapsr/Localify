import { setRequestLocale } from "next-intl/server";

import SoonPage from "@/app/soon/page";

export { metadata } from "@/app/soon/page";

type Props = Readonly<{ params: Promise<{ locale: string }> }>;

const LocalizedSoonPage = async ({ params }: Props) => {
  // Next.js can render a page before its layout, so the page sets the locale too.
  setRequestLocale((await params).locale);

  return <SoonPage />;
};

export default LocalizedSoonPage;
