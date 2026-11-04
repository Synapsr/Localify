"use client";

import { RiDraggable, RiMoreLine } from "@remixicon/react";
import { AnimatePresence, motion } from "motion/react";
import { useTranslations } from "next-intl";
import { Link } from "@/i18n/navigation";
import { useCallback, useState, type MouseEvent, type SyntheticEvent } from "react";

import { MACOS_APP_DOWNLOAD_URL } from "@/app/download-links";
import {
  CheckIcon,
  CogFourSolidIcon,
  CopyIcon,
  ExternalLinkIcon,
  FileTextSolidIcon,
  HexagonSolidIcon,
  PlusIcon,
  SearchIcon,
  TrashIcon,
} from "@/components/icons";
import { Button } from "@/components/ui/button";
import { buttonVariants } from "@/components/ui/button-variants";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/* ---------------------------------------------------------------------------
   Mock data (mirrors DomainInfoDto / ProjectInfoDto shape closely)
   --------------------------------------------------------------------------*/

type MockDomain = {
  pattern: string;
  port: string;
  url: string;
  reachable: boolean;
};

type MockProject = {
  id: string;
  name: string;
  enabled: boolean;
  domains: MockDomain[];
};

const MOCK_PROJECTS: MockProject[] = [
  {
    id: "acme",
    name: "Acme",
    enabled: true,
    domains: [
      {
        pattern: "acme.localify",
        port: "56431",
        url: "https://acme.localify",
        reachable: true,
      },
    ],
  },
  {
    id: "hooli",
    name: "Hooli",
    enabled: true,
    domains: [
      {
        pattern: "hooli.localify",
        port: "53489",
        url: "https://hooli.localify",
        reachable: false,
      },
    ],
  },
  {
    id: "pied-piper",
    name: "pied-piper",
    enabled: true,
    domains: [
      {
        pattern: "pied-piper.localify",
        port: "54539",
        url: "https://pied-piper.localify",
        reachable: false,
      },
      {
        pattern: "middle-out.localify",
        port: "54542",
        url: "https://middle-out.localify",
        reachable: false,
      },
    ],
  },
];

type DemoAction = (event?: SyntheticEvent) => void;

/* ---------------------------------------------------------------------------
   ProjectsMockup  (mirrors ProjectsScreen)
   --------------------------------------------------------------------------*/

export function ProjectsMockup() {
  const t = useTranslations("DemoDialog");
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);

  const trigger = useCallback<DemoAction>((event) => {
    event?.preventDefault?.();
    event?.stopPropagation?.();
    setOpen(true);
  }, []);

  return (
    <>
      <div id="projects-screen" className="relative mx-auto w-full max-w-4xl space-y-6 px-6 pb-28">
        <ProjectsMockupHeader query={query} onQueryChange={setQuery} onDemoAction={trigger} />
        <ProjectsMockupList query={query} onDemoAction={trigger} />
        <SharedNavigationMockup onDemoAction={trigger} />
      </div>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle className="text-[22px] tracking-[-0.02em]">
              {t("title")}
            </DialogTitle>
            <DialogDescription className="text-[14px] leading-[1.55]">
              {t("description")}
            </DialogDescription>
          </DialogHeader>

          <DialogFooter className="pb-6">
            <Button variant="outline" onClick={() => setOpen(false)}>
              {t("later")}
            </Button>
            <Link
              href={MACOS_APP_DOWNLOAD_URL}
              onClick={() => setOpen(false)}
              className={buttonVariants({ variant: "primary" })}
              data-umami-event="download_clicked"
              data-umami-event-source="projects_mockup_dialog"
            >
              {t("download")}
            </Link>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

/* ---------------------------------------------------------------------------
   ProjectsMockupHeader  (mirrors ProjectsHeader)
   --------------------------------------------------------------------------*/

interface ProjectsMockupHeaderProps {
  query: string;
  onQueryChange: (value: string) => void;
  onDemoAction: DemoAction;
}

function ProjectsMockupHeader({ query, onQueryChange, onDemoAction }: ProjectsMockupHeaderProps) {
  return (
    <header className="space-y-2 bg-background py-4 pt-10">
      <div className="relative z-10 flex items-end justify-between gap-4">
        <div className="relative z-10 flex items-center gap-4">
          <h1 className="text-2xl font-semibold tracking-tight text-foreground">Projects</h1>
        </div>

        <div className="flex items-center gap-2">
          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  aria-label="Create project"
                  className="border-dashed"
                  variant="outline"
                  size="icon"
                  onClick={onDemoAction}
                >
                  <PlusIcon className="size-4 text-muted-foreground" />
                </Button>
              }
            />
            <TooltipContent className="flex items-center gap-2">
              <span>Create new project</span>
              <Kbd>⌘N</Kbd>
            </TooltipContent>
          </Tooltip>

          <InputGroup className="max-w-xs border-dashed pr-5">
            <InputGroupInput
              value={query}
              onChange={(event) => onQueryChange(event.target.value)}
              placeholder="Search..."
            />
            <InputGroupAddon>
              <SearchIcon />
            </InputGroupAddon>
            <InputGroupAddon align="inline-end">
              <Kbd className="absolute right-2">⌘F</Kbd>
            </InputGroupAddon>
          </InputGroup>
        </div>
      </div>
    </header>
  );
}

/* ---------------------------------------------------------------------------
   ProjectsMockupList  (mirrors ProjectsList)
   --------------------------------------------------------------------------*/

interface ProjectsMockupListProps {
  query: string;
  onDemoAction: DemoAction;
}

function ProjectsMockupList({ query, onDemoAction }: ProjectsMockupListProps) {
  const needle = query.trim().toLowerCase();
  const filtered = needle
    ? MOCK_PROJECTS.filter(
        (project) =>
          project.name.toLowerCase().includes(needle) ||
          project.domains.some((domain) => domain.pattern.toLowerCase().includes(needle)),
      )
    : MOCK_PROJECTS;

  return (
    <div className="flex flex-col gap-2">
      {filtered.map((project) => (
        <ProjectsMockupListItem key={project.id} project={project} onDemoAction={onDemoAction} />
      ))}
    </div>
  );
}

/* ---------------------------------------------------------------------------
   ProjectsMockupListItem  (mirrors ProjectsListItem)
   --------------------------------------------------------------------------*/

interface ProjectsMockupListItemProps {
  project: MockProject;
  onDemoAction: DemoAction;
}

function ProjectsMockupListItem({ project, onDemoAction }: ProjectsMockupListItemProps) {
  return (
    <div
      className={cn(
        "group/card relative flex gap-3 rounded-xl border border-border/30 bg-neutral-50/50 px-3 py-3 transition-colors dark:bg-neutral-800/50",
        "hover:border-border",
        !project.enabled && "opacity-60",
      )}
    >
      <Button
        aria-label="Drag to reorder"
        size="icon"
        variant="ghost"
        onClick={onDemoAction}
        className="cursor-grab hover:bg-muted hover:text-foreground active:cursor-grabbing"
      >
        <RiDraggable className="size-4" />
      </Button>

      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex items-center gap-2">
          <span className="truncate text-lg font-medium">{project.name}</span>
        </div>
        <ProjectsMockupDomainList project={project} onDemoAction={onDemoAction} />
      </div>

      <DropdownMenu>
        <DropdownMenuTrigger
          aria-label="Project actions"
          render={
            <Button size="icon" variant="ghost">
              <RiMoreLine className="size-4" />
            </Button>
          }
        />
        <DropdownMenuContent align="end">
          <DropdownMenuItem onClick={onDemoAction}>Open in editor</DropdownMenuItem>
          <DropdownMenuItem onClick={onDemoAction}>View logs</DropdownMenuItem>
          <DropdownMenuItem onClick={onDemoAction}>
            {project.enabled ? "Disable" : "Enable"}
          </DropdownMenuItem>
          <DropdownMenuItem variant="destructive" onClick={onDemoAction}>
            Delete project
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

/* ---------------------------------------------------------------------------
   ProjectsMockupDomainList  (mirrors ProjectDomainList)
   --------------------------------------------------------------------------*/

const chipTransition = {
  type: "spring" as const,
  stiffness: 500,
  damping: 35,
  mass: 0.6,
};

const chipVariants = {
  initial: { opacity: 0, scale: 0.6, y: -4 },
  animate: { opacity: 1, scale: 1, y: 0 },
  exit: { opacity: 0, scale: 0.6, y: -4 },
};

interface ProjectsMockupDomainListProps {
  project: MockProject;
  onDemoAction: DemoAction;
}

function ProjectsMockupDomainList({ project, onDemoAction }: ProjectsMockupDomainListProps) {
  return (
    <motion.div layout className="flex flex-wrap items-center gap-1.5">
      <AnimatePresence initial={false} mode="popLayout">
        {project.domains.map((domain) => (
          <motion.span
            key={domain.pattern}
            layout
            variants={chipVariants}
            initial="initial"
            animate="animate"
            exit="exit"
            transition={chipTransition}
            className="inline-flex"
          >
            <ProjectsMockupDomainChip domain={domain} onDemoAction={onDemoAction} />
          </motion.span>
        ))}
      </AnimatePresence>

      <Tooltip>
        <TooltipTrigger
          render={
            <button
              type="button"
              aria-label={`Add domain to ${project.name}`}
              onClick={onDemoAction}
              className="inline-flex size-8 shrink-0 items-center justify-center rounded-full border border-dashed border-border text-muted-foreground transition-colors hover:border-foreground/30 hover:bg-muted hover:text-foreground"
            />
          }
        >
          <PlusIcon className="size-3.5" />
        </TooltipTrigger>
        <TooltipContent>Add domain</TooltipContent>
      </Tooltip>
    </motion.div>
  );
}

/* ---------------------------------------------------------------------------
   ProjectsMockupDomainChip  (mirrors ProjectDomainChip)
   --------------------------------------------------------------------------*/

const actionVariants = {
  rest: { opacity: 0, scale: 0.6, x: -6 },
  hover: { opacity: 1, scale: 1, x: 0 },
};

interface ProjectsMockupDomainChipProps {
  domain: MockDomain;
  onDemoAction: DemoAction;
}

function ProjectsMockupDomainChip({ domain, onDemoAction }: ProjectsMockupDomainChipProps) {
  const [copied, setCopied] = useState(false);

  const handleCopy = (event: MouseEvent) => {
    event.stopPropagation();
    event.preventDefault();
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1400);
    if (typeof navigator !== "undefined" && navigator.clipboard) {
      void navigator.clipboard.writeText(domain.url);
    }
  };

  const handleOpen = (event: MouseEvent) => {
    event.stopPropagation();
    event.preventDefault();
    onDemoAction(event);
  };

  return (
    <motion.span
      initial="rest"
      animate="rest"
      whileHover="hover"
      whileFocus="hover"
      className={cn(
        "group/chip inline-flex h-8 items-center gap-1 rounded-full border border-border pr-1 pl-2 text-sm text-foreground transition-colors",
        "hover:border-foreground/20 hover:bg-muted/50",
        domain.reachable ? "opacity-80" : "opacity-60",
      )}
    >
      <Tooltip>
        <TooltipTrigger
          render={
            <span
              className={cn(
                "size-4 shrink-0 rounded-full",
                domain.reachable ? "bg-success" : "bg-neutral-400",
              )}
            />
          }
        />
        <TooltipContent>{domain.reachable ? "Target reachable" : "Target offline"}</TooltipContent>
      </Tooltip>

      <Tooltip>
        <TooltipTrigger
          render={
            <a
              href={domain.url}
              onClick={handleOpen}
              className="inline-flex h-full items-center rounded-full px-1 hover:text-foreground"
            />
          }
        >
          <span className="truncate">{domain.pattern}</span>
        </TooltipTrigger>
        <TooltipContent>Open {domain.url}</TooltipContent>
      </Tooltip>

      <Tooltip>
        <TooltipTrigger
          render={
            <button
              type="button"
              onClick={handleCopy}
              aria-label={copied ? "Port copied" : `Copy port ${domain.port}`}
              className="inline-flex h-5 shrink-0 items-center gap-0.5 rounded bg-muted px-1 text-xs text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground"
            />
          }
        >
          {copied ? <CheckIcon className="size-3 text-emerald-500" /> : <span>:{domain.port}</span>}
        </TooltipTrigger>
        <TooltipContent>{copied ? "Port copied" : `Copy port ${domain.port}`}</TooltipContent>
      </Tooltip>

      <motion.span
        variants={{
          rest: { width: 0, opacity: 0 },
          hover: { width: "auto", opacity: 1 },
        }}
        transition={{ type: "spring", stiffness: 420, damping: 34, mass: 0.6 }}
        className="flex items-center gap-1 overflow-hidden"
      >
        <span className="mx-0.5 h-4 w-px shrink-0 bg-border" aria-hidden />

        <motion.span
          variants={actionVariants}
          transition={{ type: "spring", stiffness: 500, damping: 30 }}
          className="inline-flex shrink-0"
        >
          <Tooltip>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  onClick={handleCopy}
                  aria-label={copied ? "URL copied" : `Copy ${domain.url}`}
                  className="inline-flex size-6 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground"
                />
              }
            >
              {copied ? (
                <CheckIcon className="size-3.5 text-emerald-500" />
              ) : (
                <CopyIcon className="size-3.5" />
              )}
            </TooltipTrigger>
            <TooltipContent>{copied ? "URL copied" : "Copy URL"}</TooltipContent>
          </Tooltip>
        </motion.span>

        <motion.span
          variants={actionVariants}
          transition={{ type: "spring", stiffness: 500, damping: 30, delay: 0.04 }}
          className="inline-flex shrink-0"
        >
          <Tooltip>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  onClick={handleOpen}
                  aria-label={`Open ${domain.url}`}
                  className="inline-flex size-6 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground"
                />
              }
            >
              <ExternalLinkIcon className="size-3.5" />
            </TooltipTrigger>
            <TooltipContent>Open in browser</TooltipContent>
          </Tooltip>
        </motion.span>

        <motion.span
          variants={actionVariants}
          transition={{ type: "spring", stiffness: 500, damping: 30, delay: 0.08 }}
          className="inline-flex shrink-0"
        >
          <Tooltip>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  aria-label={`Remove ${domain.pattern}`}
                  onClick={(event) => {
                    event.stopPropagation();
                    onDemoAction(event);
                  }}
                  className="inline-flex size-6 shrink-0 items-center justify-center rounded-full text-muted-foreground/70 transition-colors hover:bg-destructive/10 hover:text-destructive focus-visible:bg-destructive/10 focus-visible:text-destructive"
                />
              }
            >
              <TrashIcon className="size-3.5" />
            </TooltipTrigger>
            <TooltipContent>Remove domain</TooltipContent>
          </Tooltip>
        </motion.span>
      </motion.span>
    </motion.span>
  );
}

/* ---------------------------------------------------------------------------
   SharedNavigationMockup  (mirrors SharedNavigation)
   --------------------------------------------------------------------------*/

const sharedNavigationTabs = [
  { id: "projects", label: "Projects", icon: HexagonSolidIcon, shortcut: "⌘1", active: true },
  { id: "logs", label: "Logs", icon: FileTextSolidIcon, shortcut: "⌘2", active: false },
  { id: "settings", label: "Settings", icon: CogFourSolidIcon, shortcut: "⌘3", active: false },
] as const;

function SharedNavigationMockup({ onDemoAction }: { onDemoAction: DemoAction }) {
  return (
    <div className="absolute bottom-4 left-1/2 z-50 w-fit -translate-x-1/2 rounded-full bg-neutral-900/90 p-2 text-white backdrop-blur-3xl">
      <div className="flex items-center gap-1">
        {sharedNavigationTabs.map((tab) => (
          <Tooltip key={tab.id}>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  onClick={onDemoAction}
                  aria-label={tab.label}
                  className={cn(
                    "rounded-full p-2 text-sm font-medium transition-all duration-150 hover:bg-white/10 hover:text-white/90",
                    tab.active ? "bg-white/15 text-white" : "text-white/75",
                  )}
                >
                  <tab.icon className="size-6 stroke-2" />
                </button>
              }
            />
            <TooltipContent side="top" className="flex items-center gap-2">
              <span>{tab.label}</span>
              <Kbd>{tab.shortcut}</Kbd>
            </TooltipContent>
          </Tooltip>
        ))}
      </div>
    </div>
  );
}
