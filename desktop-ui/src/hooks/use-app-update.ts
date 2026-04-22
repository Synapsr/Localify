import { useContext } from "react";

import { AppUpdateContext } from "@/providers/app-update-provider";

export function useAppUpdate() {
  const context = useContext(AppUpdateContext);

  if (!context) {
    throw new Error("useAppUpdate must be used within AppUpdateProvider");
  }

  return context;
}
