import "./styles.css";
import { RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { AppShell } from "./components/app-shell";
import { DesktopErrorBoundary } from "./components/shared/desktop-error-boundary";
import { InstallAttributionBridge } from "./components/shared/install-attribution-bridge";
import { SharedProvider } from "./components/shared/shared-provider";
import { initDesktopAnalytics } from "./lib/analytics";
import { startDeepLinkBridge } from "./lib/deep-link";
import { router } from "./router";

const container = document.getElementById("app");
if (!container) {
  throw new Error("Missing #app root element");
}

initDesktopAnalytics();

createRoot(container).render(
  <StrictMode>
    <SharedProvider>
      <DesktopErrorBoundary>
        <InstallAttributionBridge />
        <AppShell>
          <RouterProvider router={router} />
        </AppShell>
      </DesktopErrorBoundary>
    </SharedProvider>
  </StrictMode>,
);

void startDeepLinkBridge();
