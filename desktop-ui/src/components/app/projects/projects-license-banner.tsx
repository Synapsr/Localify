"use client";

import { Link } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";

import { BookmarkIcon } from "@/components/icons";
import { Badge } from "@/components/ui/badge";
import { buttonVariants } from "@/components/ui/button";
import { bootstrapQueries } from "@/lib/queries";
import { cn } from "@/lib/utils";

const formatLimit = (value: number | null) => (value === null ? "Unlimited" : String(value));

export const ProjectsLicenseBanner = () => {
  const { data } = useQuery(bootstrapQueries.read());

  if (!data) return null;

  const { licensing } = data;
  const showUpgradeCopy =
    licensing.status !== "licensed" &&
    licensing.limits.maxProjects !== null &&
    licensing.projectCount >= licensing.limits.maxProjects;

  return (
    <div className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-border/60 bg-neutral-50/70 px-4 py-3 dark:bg-neutral-900/40">
      <div className="flex min-w-0 items-start gap-3">
        <div className="mt-0.5 text-muted-foreground">
          <BookmarkIcon />
        </div>
        <div className="min-w-0 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm font-medium text-foreground">{licensing.limits.plan}</span>
            <Badge tone={licensing.status === "licensed" ? "accent" : "muted"}>
              {formatLimit(licensing.limits.maxProjects)} projects /{" "}
              {formatLimit(licensing.limits.maxDomainsPerProject)} domains
            </Badge>
          </div>
          <p className="text-xs text-muted-foreground">
            {licensing.projectCount} project{licensing.projectCount === 1 ? "" : "s"} configured.
            {showUpgradeCopy
              ? " Activate a Keygen license in Settings to add more."
              : " Keygen limits are enforced in the Rust core for desktop and CLI flows."}
          </p>
        </div>
      </div>

      <Link
        to="/settings"
        className={cn(buttonVariants({ size: "sm", variant: "outline" }))}
      >
        Manage license
      </Link>
    </div>
  );
};
