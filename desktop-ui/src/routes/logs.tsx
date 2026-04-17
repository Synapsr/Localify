import { createFileRoute } from "@tanstack/react-router";

import { LogsScreen } from "@/components/app/logs/logs-screen";

export const Route = createFileRoute("/logs")({
  component: LogsScreen,
});
