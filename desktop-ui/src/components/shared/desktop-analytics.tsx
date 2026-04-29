import { useRouterState } from "@tanstack/react-router";
import { useEffect } from "react";

import { captureDesktopEvent } from "@/lib/analytics";

export const DesktopAnalytics = () => {
  const href = useRouterState({ select: (state) => state.location.href });

  useEffect(() => {
    captureDesktopEvent("desktop_pageview", { path: href });
  }, [href]);

  return null;
};
