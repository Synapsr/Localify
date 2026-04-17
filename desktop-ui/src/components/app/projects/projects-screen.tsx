"use client";

import { useState } from "react";

import { ProjectsHeader } from "./projects-header";
import { ProjectsList } from "./projects-list";

export const ProjectsScreen = () => {
  const [query, setQuery] = useState("");

  return (
    <div id="projects-screen" className="screen-wrapper space-y-6">
      <ProjectsHeader query={query} onQueryChange={setQuery} />
      <ProjectsList query={query} />
    </div>
  );
};
