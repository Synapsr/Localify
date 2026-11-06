"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type MouseEvent, type PointerEvent } from "react";
import { toast } from "sonner";

import type {
  DomainInfoDto,
  EnvironmentFileRequest,
  ProjectServiceInfoDto,
  TerminalTargetRequest,
} from "@/types/localify";

import { LaunchProfileBadge } from "@/components/app/launch-profiles/launch-profile-badge";
import { LaunchProfileMenu } from "@/components/app/launch-profiles/launch-profile-menu";
import { serviceLaunchProfiles } from "@/components/app/launch-profiles/util.launch-profiles";
import {
  CheckIcon,
  CopyIcon,
  DotsIcon,
  EditIcon,
  ExternalLinkIcon,
  GlobeIcon,
  PlayIcon,
  SpinnerIcon,
  TerminalIcon,
  TrashIcon,
  XIcon,
} from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { bootstrapQueries, projectMutations } from "@/lib/queries";
import { tauriClient } from "@/lib/tauri-client";
import { cn } from "@/lib/utils";
import { useTerminal } from "@/providers/terminal-provider";

import { ProjectEnvironmentFileDialog } from "./project-environment-file-dialog";
import { ProjectEnvironmentFileMenuItem } from "./project-environment-file-menu-item";
import { ProjectDeleteDomainDialog } from "./projects-list-item-delete-domain-dialog";
import { ProjectEditDomainDialog } from "./projects-list-item-edit-domain-dialog";
import { PublicUrlBadge } from "./public-url/public-url-badge";
import { PublicUrlDialog } from "./public-url/public-url-dialog";
import { usePublicUrl } from "./public-url/use-public-url";

interface ProjectDomainChipProps {
  domain: DomainInfoDto;
  service?: ProjectServiceInfoDto | null;
  terminalTarget?: TerminalTargetRequest | null;
}

function portSegment(
  scheme: "http" | "https",
  port: number | undefined,
): string {
  if (!port) return "";
  if (scheme === "http" && port === 80) return "";
  if (scheme === "https" && port === 443) return "";
  return `:${port}`;
}

function buildUrl(
  domain: DomainInfoDto,
  ports: { httpPort: number; httpsPort: number } | undefined,
): string {
  const scheme = domain.registration.httpsEnabled ? "https" : "http";
  const port = domain.registration.httpsEnabled
    ? ports?.httpsPort
    : ports?.httpPort;
  return `${scheme}://${domain.registration.domain}${portSegment(scheme, port)}`;
}

function extractPort(target: string | undefined): string | null {
  if (!target) return null;
  try {
    const parsed = new URL(target);
    if (parsed.port) return parsed.port;
    if (parsed.protocol === "https:") return "443";
    if (parsed.protocol === "http:") return "80";
    return null;
  } catch {
    const match = target.match(/:(\d+)(?:\/|$)/);
    return match ? match[1] : null;
  }
}

function getPrimaryLaunchTarget(domain: DomainInfoDto): string | null {
  const rootRoute = domain.registration.routes.find(
    (route) => route.path === "/",
  );
  const candidate = rootRoute ?? domain.registration.routes[0];
  return candidate?.target ?? null;
}

export const ProjectDomainChip = ({
  domain,
  service = null,
  terminalTarget = null,
}: ProjectDomainChipProps) => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const {
    findSessionForTarget,
    hasOwnedServiceSession,
    interruptSession,
    launchService,
    openTerminal,
  } = useTerminal();
  const [editDialogOpen, setEditDialogOpen] = useState(false);
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const [publicUrlDialogOpen, setPublicUrlDialogOpen] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [environmentFileDialogOpen, setEnvironmentFileDialogOpen] =
    useState(false);
  const [selectedEnvironmentFileTarget, setSelectedEnvironmentFileTarget] =
    useState<EnvironmentFileRequest | null>(null);
  const [serviceActionInFlight, setServiceActionInFlight] = useState(false);
  const { copy: copyUrl, copied: urlCopied } = useCopyToClipboard();
  const { copy: copyPort, copied: portCopied } = useCopyToClipboard();
  const stopLocalDevServer = useMutation(
    projectMutations.stopLocalDevServer(queryClient),
  );
  const publicUrl = usePublicUrl(domain.registration.domain, domain.projectId);
  const { pattern, routes } = domain.registration;
  const url = buildUrl(domain, data?.config.daemon);
  const reachable = domain.targetReachable;
  const port = extractPort(routes[0]?.target);
  const launchTarget = getPrimaryLaunchTarget(domain);
  const hasTerminalTarget = Boolean(terminalTarget);
  const serviceTerminalTarget =
    terminalTarget?.kind === "service" ? terminalTarget : null;
  const environmentFileTarget = serviceTerminalTarget?.serviceId
    ? {
        projectId: serviceTerminalTarget.projectId,
        environmentSlug: serviceTerminalTarget.environmentSlug,
        serviceId: serviceTerminalTarget.serviceId,
      }
    : terminalTarget
      ? {
          projectId: terminalTarget.projectId,
          environmentSlug: terminalTarget.environmentSlug,
          serviceId: null,
        }
      : null;
  const ownedServiceSession = serviceTerminalTarget
    ? hasOwnedServiceSession(serviceTerminalTarget)
    : false;
  const launchProfiles = serviceLaunchProfiles(data, serviceTerminalTarget);
  const serviceTarget = service?.target ?? launchTarget;
  const serviceActionPending =
    stopLocalDevServer.isPending || serviceActionInFlight;
  const serviceActionUnavailable = reachable
    ? !serviceTarget && !ownedServiceSession
    : !serviceTerminalTarget;
  const serviceActionDisabled =
    serviceActionPending || serviceActionUnavailable;
  const serviceActionLabel = reachable ? "Stop service" : "Launch service";
  const serviceActionStatusLabel = serviceActionPending
    ? reachable
      ? "Stopping service"
      : "Launching service"
    : serviceActionUnavailable
      ? reachable
        ? "No linked service target"
        : "No terminal target"
      : serviceActionLabel;

  const stopPointerPropagation = (event: PointerEvent) => {
    event.stopPropagation();
  };

  const handleCopyUrl = async () => {
    await copyUrl(url);
  };

  const handleCopyPort = async (event: MouseEvent) => {
    event.stopPropagation();
    event.preventDefault();
    if (!port) return;
    await copyPort(port);
  };

  const handleOpen = async () => {
    try {
      await tauriClient.openProjectUrl({
        url,
        browser: data?.launcherPreferences.browser ?? "default",
      });
    } catch (error) {
      const description =
        error instanceof Error ? error.message : "Unable to open this URL.";
      toast.error("Open in browser failed", { description });
    }
  };

  const handleOpenLinkClick = (event: MouseEvent) => {
    event.stopPropagation();
    event.preventDefault();
    void handleOpen();
  };

  const handleOpenTerminal = async () => {
    if (!terminalTarget) {
      return;
    }

    try {
      await openTerminal(terminalTarget);
    } catch (error) {
      toast.error("Open terminal failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const handleToggleService = async () => {
    if (!serviceTerminalTarget) {
      return;
    }

    setServiceActionInFlight(true);
    try {
      if (reachable) {
        const ownedSession = findSessionForTarget(serviceTerminalTarget);
        if (ownedSession) {
          await interruptSession(ownedSession.id);
          return;
        }

        if (!serviceTarget) {
          return;
        }

        if (
          !window.confirm(
            "Stop the process currently listening on this service port?",
          )
        ) {
          return;
        }

        await stopLocalDevServer.mutateAsync({
          target: serviceTarget,
        });
        return;
      }

      await launchService(serviceTerminalTarget);
    } catch (error) {
      const description =
        error instanceof Error
          ? error.message
          : "Unable to update this service.";
      toast.error(`${serviceActionLabel} failed`, { description });
    } finally {
      setServiceActionInFlight(false);
    }
  };

  const handleLaunchWithProfile = async (profile: string) => {
    if (!serviceTerminalTarget) {
      return;
    }

    setServiceActionInFlight(true);
    try {
      await launchService(serviceTerminalTarget, profile);
    } catch (error) {
      const description =
        error instanceof Error
          ? error.message
          : "Unable to launch this service.";
      toast.error("Launch service failed", { description });
    } finally {
      setServiceActionInFlight(false);
    }
  };

  return (
    <>
      <span
        onPointerDown={stopPointerPropagation}
        className={cn(
          "group/chip inline-flex h-8 items-center gap-1 rounded-full border border-border pr-1 pl-2 text-sm text-foreground transition-colors",
          "hover:border-foreground/20 hover:bg-muted/50",
          !reachable ? "opacity-60" : "opacity-80",
        )}
      >
        <Tooltip>
          <TooltipTrigger
            render={
              <span
                className={cn(
                  "size-4 shrink-0 rounded-full",
                  reachable ? "bg-success" : "bg-neutral-400",
                )}
              />
            }
          />
          <TooltipContent>
            {reachable ? "Target reachable" : "Target offline"}
          </TooltipContent>
        </Tooltip>

        <Tooltip>
          <TooltipTrigger
            render={
              <a
                href={url}
                target="_blank"
                rel="noreferrer"
                onClick={handleOpenLinkClick}
                className="inline-flex h-full items-center rounded-full px-1 hover:text-foreground"
              />
            }
          >
            <span className="truncate">{pattern}</span>
          </TooltipTrigger>
          <TooltipContent>Open {url}</TooltipContent>
        </Tooltip>

        {port && (
          <Tooltip>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  onClick={handleCopyPort}
                  aria-label={portCopied ? "Port copied" : `Copy port ${port}`}
                  className="inline-flex h-5 shrink-0 items-center gap-0.5 rounded bg-muted px-1 text-xs text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground"
                />
              }
            >
              {portCopied ? (
                <CheckIcon className="size-3 text-emerald-500" />
              ) : (
                <span>:{port}</span>
              )}
            </TooltipTrigger>
            <TooltipContent>
              {portCopied ? "Port copied" : `Copy port ${port}`}
            </TooltipContent>
          </Tooltip>
        )}

        {publicUrl?.url ? (
          <PublicUrlBadge
            url={publicUrl.url}
            onOpen={() => setPublicUrlDialogOpen(true)}
          />
        ) : null}

        {launchProfiles.session ? (
          <LaunchProfileBadge session={launchProfiles.session} />
        ) : null}

        {serviceTerminalTarget ? (
          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  type="button"
                  size="icon-xs"
                  variant="ghost"
                  onClick={(event) => {
                    event.stopPropagation();
                    void handleToggleService();
                  }}
                  disabled={serviceActionDisabled}
                  aria-label={serviceActionStatusLabel}
                  className="ml-0.5 size-6 rounded-full text-foreground hover:bg-foreground/10 hover:text-foreground"
                >
                  {serviceActionPending ? (
                    <SpinnerIcon className="animate-spin" />
                  ) : reachable ? (
                    <XIcon />
                  ) : (
                    <PlayIcon className="translate-x-px" />
                  )}
                </Button>
              }
            />
            <TooltipContent>{serviceActionStatusLabel}</TooltipContent>
          </Tooltip>
        ) : null}

        {serviceTerminalTarget && !reachable ? (
          <LaunchProfileMenu
            profiles={launchProfiles.profiles}
            error={launchProfiles.profilesError}
            serviceName={service?.name ?? pattern}
            onLaunch={(profile) => void handleLaunchWithProfile(profile)}
            disabled={serviceActionDisabled}
            className="-ml-1 h-6 w-4 rounded-full text-muted-foreground hover:bg-foreground/10 hover:text-foreground aria-expanded:bg-foreground/10 aria-expanded:text-foreground"
          />
        ) : null}

        <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
          <DropdownMenuTrigger
            render={
              <Button
                type="button"
                size="icon-xs"
                variant="ghost"
                aria-label={`Domain actions for ${pattern}`}
                className="ml-0.5 size-6 rounded-full text-muted-foreground hover:bg-foreground/10 hover:text-foreground aria-expanded:bg-foreground/10 aria-expanded:text-foreground"
              >
                <DotsIcon className="size-5 stroke-2" />
              </Button>
            }
          />
          <DropdownMenuContent align="end" sideOffset={6} className="w-56">
            <DropdownMenuItem
              onClick={(event) => {
                event.stopPropagation();
                void handleOpen();
              }}
            >
              <ExternalLinkIcon />
              Open in browser
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={(event) => {
                event.stopPropagation();
                void handleOpenTerminal();
              }}
              disabled={!hasTerminalTarget}
            >
              <TerminalIcon />
              Open terminal
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={(event) => {
                event.stopPropagation();
                void handleCopyUrl();
              }}
            >
              {urlCopied ? (
                <CheckIcon className="text-emerald-500" />
              ) : (
                <CopyIcon />
              )}
              {urlCopied ? "URL copied" : "Copy URL"}
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={(event) => {
                event.stopPropagation();
                setPublicUrlDialogOpen(true);
              }}
            >
              <GlobeIcon />
              Public URL
            </DropdownMenuItem>
            <ProjectEnvironmentFileMenuItem
              active={menuOpen}
              label={serviceTerminalTarget ? "Open .env" : "Open .env"}
              target={environmentFileTarget}
              disabled={!environmentFileTarget}
              onOpenTarget={(nextTarget) => {
                setSelectedEnvironmentFileTarget(nextTarget);
                setEnvironmentFileDialogOpen(true);
              }}
            />
            <DropdownMenuSeparator />
            <DropdownMenuItem
              onClick={(event) => {
                event.stopPropagation();
                setEditDialogOpen(true);
              }}
            >
              <EditIcon />
              Edit domain
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              variant="destructive"
              onClick={(event) => {
                event.stopPropagation();
                setDeleteDialogOpen(true);
              }}
            >
              <TrashIcon />
              Remove domain
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </span>

      <ProjectEditDomainDialog
        domain={domain}
        open={editDialogOpen}
        onOpenChange={setEditDialogOpen}
      />
      <ProjectDeleteDomainDialog
        domain={domain}
        open={deleteDialogOpen}
        onOpenChange={setDeleteDialogOpen}
      />
      <PublicUrlDialog
        localDomain={domain.registration.domain}
        projectId={domain.projectId}
        open={publicUrlDialogOpen}
        onOpenChange={setPublicUrlDialogOpen}
      />
      <ProjectEnvironmentFileDialog
        open={environmentFileDialogOpen}
        target={selectedEnvironmentFileTarget}
        onOpenChange={setEnvironmentFileDialogOpen}
      />
    </>
  );
};
