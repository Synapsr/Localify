import { useEffect } from "react";
import { toast } from "sonner";

import { tauriClient } from "@/lib/tauri-client";

// Once per launch at most; the backend also keeps it to once per machine.
let started = false;

/**
 * Connects coding agents to Localify's MCP server after an update, and tells
 * the user what changed. Agents installed before 1.2 only had the old skill.
 */
export const useConnectAgentsOnce = (enabled: boolean) => {
  useEffect(() => {
    if (!enabled || started) {
      return;
    }
    started = true;

    void tauriClient
      .connectAgentsOnce()
      .then((result) => {
        if (!result) {
          return;
        }
        const labels = result.status.agents
          .filter((agent) => result.installedAgents.includes(agent.id))
          .map((agent) => agent.label);
        if (labels.length === 0) {
          return;
        }
        toast.success(`Localify is now connected to ${labels.join(", ")}`, {
          description:
            "Your agents can start services, read their logs, and clean up worktrees. Restart open agent sessions to load it. Turn this off in Settings → AI agents.",
        });
      })
      .catch((error: unknown) => {
        const message = error instanceof Error ? error.message : String(error);
        toast.error("Could not connect your coding agents", { description: message });
      });
  }, [enabled]);
};
