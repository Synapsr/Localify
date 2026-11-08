"use client";

import { useState } from "react";

import { useOtherMachines } from "@/components/app/remote/machine-switcher";
import { RemoteProjectsView } from "@/components/app/remote/remote-projects-view";
import { useRemote } from "@/providers/remote-provider";

import { ProjectsHeader } from "./projects-header";
import { ProjectsList } from "./projects-list";

export const ProjectsScreen = () => {
  const [query, setQuery] = useState("");
  const { selectedMachineId } = useRemote();
  const remote = useOtherMachines().find((machine) => machine.id === selectedMachineId) ?? null;

  return (
    <div id="projects-screen" className="screen-wrapper space-y-6">
      <ProjectsHeader query={query} onQueryChange={setQuery} remote={remote !== null} />
      {remote ? (
        <RemoteProjectsView key={remote.id} machine={remote} query={query} />
      ) : (
        <ProjectsList query={query} />
      )}
    </div>
  );
};
