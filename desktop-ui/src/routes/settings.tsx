import { createFileRoute } from "@tanstack/react-router";

import { SettingsScreen } from "@/components/app/settings/settings-screen";

export const Route = createFileRoute("/settings")({
  component: SettingsScreen,
});
