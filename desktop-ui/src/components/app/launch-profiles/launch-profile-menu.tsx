"use client";

import { useRef } from "react";

import type { LaunchProfileInfoDto } from "@/types/localify";

import { ChevronDownIcon, DangerTriangleIcon, PlayIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";

interface LaunchProfileMenuProps {
  profiles: LaunchProfileInfoDto[];
  error: string | null;
  serviceName: string;
  onLaunch: (profile: string) => void;
  disabled?: boolean;
  className?: string;
}

export const LaunchProfileMenu = ({
  profiles,
  error,
  serviceName,
  onLaunch,
  disabled = false,
  className,
}: LaunchProfileMenuProps) => {
  // After a launch, focus belongs to the terminal that runs the service, so the
  // menu must not hand it back to its trigger.
  const launchedRef = useRef(false);

  if (profiles.length === 0 && !error) {
    return null;
  }

  const safeProfiles = profiles.filter((profile) => !profile.danger);
  const dangerousProfiles = profiles.filter((profile) => profile.danger);
  const errorOnly = profiles.length === 0;

  const renderProfile = (profile: LaunchProfileInfoDto) => (
    <DropdownMenuItem
      key={profile.name}
      onClick={(event) => {
        event.stopPropagation();
        launchedRef.current = true;
        onLaunch(profile.name);
      }}
      className={cn(
        profile.danger && "text-destructive-foreground! **:text-destructive-foreground!",
      )}
    >
      {profile.danger ? <DangerTriangleIcon /> : <PlayIcon />}
      <span className="min-w-0 truncate">Launch with {profile.name}</span>
      {profile.maxDuration ? (
        <span className="ml-auto shrink-0 pl-2 text-xs opacity-70">
          stops after {profile.maxDuration}
        </span>
      ) : null}
    </DropdownMenuItem>
  );

  return (
    <DropdownMenu
      onOpenChange={(open) => {
        if (open) {
          launchedRef.current = false;
        }
      }}
    >
      <DropdownMenuTrigger
        disabled={disabled}
        render={
          <Button
            type="button"
            size="icon-xs"
            variant="ghost"
            aria-label={
              errorOnly
                ? `Launch profiles unavailable for ${serviceName}`
                : `Launch ${serviceName} with a profile`
            }
            className={className}
          >
            {errorOnly ? (
              <DangerTriangleIcon className="size-3.5 text-amber-500" />
            ) : (
              <ChevronDownIcon className="size-3.5" />
            )}
          </Button>
        }
      />
      <DropdownMenuContent
        align="end"
        sideOffset={6}
        className="w-auto max-w-80 min-w-56"
        finalFocus={() => !launchedRef.current}
      >
        {safeProfiles.map(renderProfile)}
        {safeProfiles.length > 0 && dangerousProfiles.length > 0 ? <DropdownMenuSeparator /> : null}
        {dangerousProfiles.map(renderProfile)}
        {error ? (
          <>
            {profiles.length > 0 ? <DropdownMenuSeparator /> : null}
            <DropdownMenuItem disabled className="items-start data-disabled:opacity-100">
              <DangerTriangleIcon className="mt-0.5 text-amber-500" />
              <span className="flex min-w-0 flex-col gap-1">
                <span>Could not read launch profiles</span>
                <span className="font-mono text-[11px] break-words whitespace-pre-wrap text-muted-foreground">
                  {error}
                </span>
              </span>
            </DropdownMenuItem>
          </>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );
};
