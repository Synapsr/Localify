import { useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import { useEffect } from "react";

import { captureDesktopEvent, captureDesktopUsageSnapshot } from "@/lib/analytics";
import { bootstrapQueries } from "@/lib/queries";

export const DesktopAnalytics = () => {
  const href = useRouterState({ select: (state) => state.location.href });
  const bootstrapQuery = useQuery(bootstrapQueries.read());

  useEffect(() => {
    captureDesktopEvent("desktop_pageview", { path: href });
  }, [href]);

  useEffect(() => {
    if (!bootstrapQuery.data) return;
    captureDesktopUsageSnapshot(bootstrapQuery.data);
  }, [bootstrapQuery.data]);

  return null;
};
