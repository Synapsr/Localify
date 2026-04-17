import { createFileRoute } from "@tanstack/react-router";

import { LogsScreen } from "../screens/logs";

export const Route = createFileRoute("/logs")({
  component: LogsScreen,
});
