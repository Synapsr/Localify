"use client";

import type { ReactNode } from "react";

import { useLocale } from "next-intl";

import { Link, usePathname } from "@/i18n/navigation";
import { trackPlanClick } from "@/lib/track";

// The Pro card's button. The trial starts from the app or the CLI, so it leads to
// the download, and tells the team about the click on the way.
export const ProTrialLink = ({
  children,
  className,
  source,
}: {
  children: ReactNode;
  className: string;
  source: string;
}) => {
  const locale = useLocale();
  const page = usePathname();

  return (
    <Link
      className={className}
      data-umami-event="cloud_trial_clicked"
      data-umami-event-source={source}
      href="/#download"
      onClick={() => trackPlanClick({ plan: "pro", locale, page })}
    >
      {children}
    </Link>
  );
};
