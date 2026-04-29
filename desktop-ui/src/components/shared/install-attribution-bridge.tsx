import { useEffect } from "react";

import { consumeInstallAttribution } from "@/lib/analytics";
import { tauriClient } from "@/lib/tauri-client";

export const InstallAttributionBridge = () => {
  useEffect(() => {
    let cancelled = false;

    tauriClient
      .readInstallAttributionClipboard()
      .then((token) => {
        if (!cancelled) {
          consumeInstallAttribution(token);
        }
      })
      .catch(() => {
        // Attribution is best-effort and must never affect app startup.
      });

    return () => {
      cancelled = true;
    };
  }, []);

  return null;
};
