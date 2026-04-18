"use client";

import { RiAddLine } from "@remixicon/react";
import { AnimatePresence, motion } from "motion/react";

import { DialogTrigger } from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { DomainInfoDto, ProjectInfoDto } from "@/types/roxy";

import { ProjectAddDomainDialog } from "./projects-list-item-add-domain-dialog";
import { ProjectDomainChip } from "./projects-list-item-domain-chip";

interface ProjectDomainListProps {
  project: ProjectInfoDto;
  domains: DomainInfoDto[];
}

const chipTransition = { type: "spring", stiffness: 500, damping: 35, mass: 0.6 } as const;

const chipVariants = {
  initial: { opacity: 0, scale: 0.6, y: -4 },
  animate: { opacity: 1, scale: 1, y: 0 },
  exit: { opacity: 0, scale: 0.6, y: -4 },
};

export const ProjectDomainList = ({ project, domains }: ProjectDomainListProps) => {
  return (
    <motion.div layout className="flex flex-wrap items-center gap-1.5">
      <AnimatePresence initial={false} mode="popLayout">
        {domains.length === 0 ? (
          <motion.span
            key="empty"
            layout
            variants={chipVariants}
            initial="initial"
            animate="animate"
            exit="exit"
            transition={chipTransition}
            className="text-sm text-muted-foreground"
          >
            No domains yet
          </motion.span>
        ) : (
          domains.map((domain) => (
            <motion.span
              key={domain.registration.pattern}
              layout
              variants={chipVariants}
              initial="initial"
              animate="animate"
              exit="exit"
              transition={chipTransition}
              className="inline-flex"
            >
              <ProjectDomainChip domain={domain} />
            </motion.span>
          ))
        )}
      </AnimatePresence>

      <ProjectAddDomainDialog project={project}>
        <Tooltip>
          <TooltipTrigger
            render={
              <DialogTrigger
                aria-label={`Add domain to ${project.name}`}
                className="inline-flex size-8 shrink-0 items-center justify-center rounded-full border border-dashed border-border text-muted-foreground transition-colors hover:border-foreground/30 hover:bg-muted hover:text-foreground"
                onPointerDown={(event) => event.stopPropagation()}
              />
            }
          >
            <RiAddLine className="size-3.5" />
          </TooltipTrigger>
          <TooltipContent>Add domain</TooltipContent>
        </Tooltip>
      </ProjectAddDomainDialog>
    </motion.div>
  );
};
