import "./styles.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { RouterProvider } from "@tanstack/react-router";
import { Agentation } from "agentation";

import { router } from "./router";
import { AppShell } from "./components/app-shell";
import { SharedProvider } from "./components/shared/shared-provider";

const container = document.getElementById("app");
if (!container) {
  throw new Error("Missing #app root element");
}

createRoot(container).render(
  <StrictMode>
    <SharedProvider>
      <AppShell>
        <RouterProvider router={router} />
      </AppShell>
      {import.meta.env.DEV && <Agentation />}
    </SharedProvider>
  </StrictMode>,
);
