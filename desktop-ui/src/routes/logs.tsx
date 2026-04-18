import { createFileRoute } from "@tanstack/react-router";
import { z } from "zod";

import { LogsScreen } from "@/components/app/logs/logs-screen";

export const Route = createFileRoute("/logs")({
  validateSearch: z.object({
    projectId: z.string().optional(),
    view: z.enum(["requests", "raw"]).optional(),
  }),
  component: LogsScreen,
});
