"use client";

import { formatForDisplay } from "@tanstack/react-hotkeys";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef } from "react";

import { PlusIcon, SearchIcon, XIcon } from "@/components/icons";
import { ProgressiveBlur } from "@/components/progresive-blur";
import { Button } from "@/components/ui/button";
import { DialogTrigger } from "@/components/ui/dialog";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group";
import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useScroll } from "@/hooks/use-scroll";
import { bootstrapQueries } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { useAppHotkeys } from "@/providers/hotkeys-provider";

import { ProjectsCreateDialog } from "./projects-create-dialog";
import {
  consumeQueuedProjectsSearchFocus,
  subscribeToProjectsSearchFocusRequest,
} from "./util.project-search-focus";

interface ProjectsHeaderProps {
  query: string;
  onQueryChange: (value: string) => void;
}

export const ProjectsHeader = ({ query, onQueryChange }: ProjectsHeaderProps) => {
  const { hotkeys } = useAppHotkeys();
  const { scrolled } = useScroll(4);
  const inputRef = useRef<HTMLInputElement>(null);
  const { data } = useQuery(bootstrapQueries.read());

  const maxProjects = data?.licensing.limits.maxProjects ?? null;
  const projectLimitReached =
    maxProjects !== null && (data?.licensing.projectCount ?? 0) >= maxProjects;

  useEffect(() => {
    const focusSearch = () => {
      window.requestAnimationFrame(() => {
        inputRef.current?.focus();
        inputRef.current?.select();
      });
    };

    if (consumeQueuedProjectsSearchFocus()) {
      focusSearch();
    }

    return subscribeToProjectsSearchFocusRequest(focusSearch);
  }, []);

  return (
    <header className="sticky top-0 z-10 space-y-2 bg-background py-4 pt-12">
      <ProgressiveBlur
        position="top"
        height="100px"
        className={`top-0 transition-opacity duration-200 ${scrolled ? "opacity-100" : "opacity-0"}`}
      />

      <div className="relative z-10 flex items-end justify-between gap-4 ">
        <div className="relative z-10 flex items-center gap-4">
          <h1 className="text-2xl font-semibold tracking-tight">Projects</h1>
        </div>

        <div className="flex  items-center gap-2">
          <ProjectsCreateDialog>
            <Tooltip>
              <TooltipTrigger
                render={
                  <DialogTrigger
                    render={
                      <Button
                        aria-label="Create project"
                        className={cn("border-dashed")}
                        variant="outline"
                        size="icon"
                        disabled={projectLimitReached}
                        title={
                          projectLimitReached
                            ? "Project limit reached. Activate a Keygen license in Settings."
                            : "Create project"
                        }
                      >
                        <PlusIcon className="size-4 text-muted-foreground" />
                      </Button>
                    }
                  />
                }
              />
              <TooltipContent className="flex items-center gap-2">
                <span>Create new project</span>
                <Kbd>{formatForDisplay(hotkeys.createProject)}</Kbd>
              </TooltipContent>
            </Tooltip>
          </ProjectsCreateDialog>
          <InputGroup className="max-w-xs border-dashed pr-5">
            <InputGroupInput
              ref={inputRef}
              value={query}
              className=""
              onChange={(event) => onQueryChange(event.target.value)}
              placeholder="Search..."
            />
            <InputGroupAddon>
              <SearchIcon />
            </InputGroupAddon>

            <InputGroupAddon align="inline-end">
              {query ? (
                <InputGroupButton
                  aria-label="Clear search"
                  title="Clear search"
                  className=" absolute right-1"
                  size="icon-xs"
                  onClick={() => onQueryChange("")}
                >
                  <XIcon />
                </InputGroupButton>
              ) : (
                <Kbd className="absolute right-2 ">{formatForDisplay(hotkeys.searchProjects)}</Kbd>
              )}
            </InputGroupAddon>
          </InputGroup>
        </div>
      </div>
    </header>
  );
};
