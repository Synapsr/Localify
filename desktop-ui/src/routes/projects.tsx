import { createFileRoute } from "@tanstack/react-router";

import { ProjectsScreen } from "../screens/projects";

export const Route = createFileRoute("/projects")({
  component: ProjectsScreen,
});
