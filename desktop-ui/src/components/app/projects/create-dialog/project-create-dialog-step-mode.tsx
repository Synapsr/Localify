"use client";

import type { ComponentType, SVGProps } from "react";

import { FolderIcon, InfoCircleIcon, PenIcon } from "@/components/icons";
import { DialogBody, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

export type ProjectCreationMode = "manual" | "folder";

interface ProjectCreateDialogStepModeProps {
  onSelect: (mode: ProjectCreationMode) => void;
}

export const ProjectCreateDialogStepMode = ({ onSelect }: ProjectCreateDialogStepModeProps) => {
  return (
    <>
      <DialogHeader>
        <DialogTitle className="flex items-center gap-1.5">
          <span>New project</span>
          <Tooltip>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  aria-label="What is a project?"
                  className="inline-flex size-4 items-center justify-center rounded-full text-muted-foreground/70 transition-colors outline-none hover:text-foreground focus-visible:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
                />
              }
            >
              <InfoCircleIcon className="size-4" />
            </TooltipTrigger>
            <TooltipContent
              side="right"
              className="max-w-xs flex-col items-start gap-1 px-3 py-2 text-left leading-relaxed whitespace-normal"
            >
              <span className="font-medium">What's a project?</span>
              <span className="text-background/80">
                A named group of domains — one per app or repo. A monorepo like
                <span className="font-medium text-background"> acme</span> can hold a web app, a
                dashboard and an API, all under one project.
              </span>
            </TooltipContent>
          </Tooltip>
        </DialogTitle>
        <DialogDescription>How would you like to start?</DialogDescription>
      </DialogHeader>

      <DialogBody className="grid gap-3 pb-6 sm:grid-cols-2">
        <ModeCard
          icon={PenIcon}
          accent="clay"
          title="Empty project"
          description="Configure domains and ports manually."
          onClick={() => onSelect("manual")}
        />
        <ModeCard
          icon={FolderIcon}
          accent="sage"
          title="From folder"
          description="Detect apps in a local folder and pre-fill domains."
          onClick={() => onSelect("folder")}
        />
      </DialogBody>
    </>
  );
};

type Accent = "clay" | "sage";

const accentStyles: Record<Accent, string> = {
  clay: cn("bg-muted text-muted-foreground"),
  sage: cn("bg-muted text-muted-foreground"),
};

interface ModeCardProps {
  icon: ComponentType<SVGProps<SVGSVGElement>>;
  accent: Accent;
  title: string;
  description: string;
  onClick: () => void;
}

const ModeCard = ({ icon: Icon, accent, title, description, onClick }: ModeCardProps) => (
  <button
    type="button"
    onClick={onClick}
    className={cn(
      "group/mode-card flex flex-col items-start gap-4 rounded-xl border border-border bg-background p-4 text-left transition-colors outline-none",
      "hover:border-foreground/20 hover:bg-muted/40",
      "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50",
      "active:translate-y-px",
    )}
  >
    <span
      aria-hidden
      className={cn(
        "flex size-9 items-center justify-center rounded-lg transition-colors",
        accentStyles[accent],
      )}
    >
      <Icon className="size-4 stroke-[2px]" />
    </span>

    <span className="space-y-1">
      <span className="block text-sm font-medium text-foreground">{title}</span>
      <span className="block text-xs leading-relaxed text-muted-foreground">{description}</span>
    </span>
  </button>
);
