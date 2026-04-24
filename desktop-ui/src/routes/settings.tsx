import { createFileRoute } from "@tanstack/react-router";
import { z } from "zod";

import { SettingsScreen } from "@/components/app/settings/settings-screen";

export const Route = createFileRoute("/settings")({
  validateSearch: z.object({
    section: z.enum(["licensing"]).optional(),
    action: z.enum(["activateLicense"]).optional(),
  }),
  component: SettingsScreen,
});
