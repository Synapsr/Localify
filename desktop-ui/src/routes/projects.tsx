import { createFileRoute } from "@tanstack/react-router";

import { ProjectsScreen } from "@/components/app/projects/projects-screen";

export const Route = createFileRoute("/projects")({
  component: ProjectsScreen,
});
