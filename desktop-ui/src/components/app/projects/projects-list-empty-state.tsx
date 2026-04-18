"use client";

import { motion } from "motion/react";

import { PlusIcon, SearchIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";

import { ProjectsListEmptyStateIllustration } from "./projects-list-empty-state-illustration";
import { requestProjectCreateDialogOpen } from "./util.project-create-dialog";

interface ProjectsListEmptyStateProps {
  query: string;
}

export const ProjectsListEmptyState = ({ query }: ProjectsListEmptyStateProps) => {
  const trimmedQuery = query.trim();

  /* return (
    <div className="">
      {Array.from({ length: 10 }).map((_, index) => (
        <div key={index} className="h-[500px] w-full bg-muted">
          <p className="">
            Lorem ipsum dolor sit amet, consectetur adipisicing elit. Doloremque earum id fugiat
            temporibus, quidem labore saepe eveniet quia illum asperiores. Iste doloremque rem ipsam
            blanditiis temporibus quam error. Eaque, doloribus.
          </p>
        </div>
      ))}
    </div>
  ); */

  if (trimmedQuery.length > 0) {
    return (
      <div className="flex flex-col items-center justify-center gap-3 rounded-xl border border-dashed border-border  px-6 py-12 text-center">
        <SearchIcon className="size-5 text-muted-foreground/70" />
        <p className="text-sm text-muted-foreground">
          No projects match &ldquo;{trimmedQuery}&rdquo;.
        </p>
      </div>
    );
  }

  return (
    <motion.div
      animate="rest"
      whileHover="hover"
      className="flex flex-col items-center justify-center gap-5 rounded-xl border border-dashed border-border bg-neutral-50 px-6 py-14 text-center dark:bg-neutral-950/40"
    >
      <ProjectsListEmptyStateIllustration />
      <div className="space-y-1.5">
        <h3 className="text-base font-semibold text-foreground">No projects yet</h3>
        <p className="max-w-xs text-sm text-muted-foreground">
          Create a project to route custom domains to your local services.
        </p>
      </div>
      <Button size="lg" onClick={requestProjectCreateDialogOpen}>
        <PlusIcon />
        Create project
      </Button>
    </motion.div>
  );
};
