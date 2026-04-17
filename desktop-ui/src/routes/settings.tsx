import { createFileRoute } from "@tanstack/react-router";

import { SettingsScreen } from "../screens/settings";

export const Route = createFileRoute("/settings")({
  component: SettingsScreen,
});
