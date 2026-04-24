"use client";

import { useQuery } from "@tanstack/react-query";
import { Reorder } from "motion/react";
import { useMemo } from "react";

import { bootstrapQueries, domainQueries, projectQueries } from "@/lib/queries";
import { buildProjectGroups } from "@/lib/util.project-groups";
import { useProjectOrder } from "@/lib/use-project-order";

import { ProjectsListEmptyState } from "./projects-list-empty-state";
import { ProjectListItemSkeleton, ProjectsListItem } from "./projects-list-item";
import { ProjectsListUpsell } from "./projects-list-upsell";
import { filterProjectGroups } from "./util.project-search";

interface ProjectsListProps {
  query: string;
}

export const ProjectsList = ({ query }: ProjectsListProps) => {
  const { data: projects, isLoading: loadingProjects } = useQuery({
    ...projectQueries.list(),
    refetchInterval: 5000,
  });
  const { data: domains, isLoading: loadingDomains } = useQuery({
    ...domainQueries.list(),
    refetchInterval: 5000,
  });
  const { data: bootstrap } = useQuery(bootstrapQueries.read());

  const isLoading = loadingProjects || loadingDomains;

  const licensing = bootstrap?.licensing;
  const atProjectLimit =
    licensing !== undefined &&
    licensing.status !== "licensed" &&
    licensing.status !== "licensedOffline" &&
    licensing.limits.maxProjects !== null &&
    licensing.projectCount >= licensing.limits.maxProjects;

  const groups = useMemo(
    () => buildProjectGroups(projects ?? [], domains ?? []),
    [projects, domains],
  );
  const groupById = useMemo(
    () => new Map(groups.map((group) => [group.project.id, group])),
    [groups],
  );
  const knownIds = useMemo(() => groups.map((group) => group.project.id), [groups]);
  const { order, setOrder } = useProjectOrder(knownIds);
  const filteredGroups = useMemo(() => filterProjectGroups(groups, query), [groups, query]);
  const filteredIds = useMemo(
    () => new Set(filteredGroups.map((group) => group.project.id)),
    [filteredGroups],
  );
  const orderedFilteredIds = useMemo(
    () => order.filter((id) => filteredIds.has(id)),
    [filteredIds, order],
  );
  const hasSearchQuery = query.trim().length > 0;

  if (isLoading) {
    return <ProjectListSkeleton />;
  }

  if (groups.length === 0) {
    return <ProjectsListEmptyState query={query} />;
  }

  if (filteredGroups.length === 0) {
    return <ProjectsListEmptyState query={query} />;
  }

  if (hasSearchQuery) {
    return (
      <div className="flex flex-col gap-2">
        {orderedFilteredIds.map((id) => {
          const group = groupById.get(id);
          if (!group) return null;

          return (
            <ProjectsListItem
              key={id}
              project={group.project}
              domains={group.domains}
              reorderEnabled={false}
            />
          );
        })}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2">
      <Reorder.Group
        axis="y"
        values={order}
        onReorder={setOrder}
        className="flex flex-col gap-2"
      >
        {order.map((id) => {
          const group = groupById.get(id);
          if (!group) return null;
          return (
            <ProjectsListItem
              key={id}
              project={group.project}
              domains={group.domains}
            />
          );
        })}
      </Reorder.Group>
      {atProjectLimit ? <ProjectsListUpsell /> : null}
    </div>
  );
};

const ProjectListSkeleton = () => {
  return (
    <div className="flex flex-col gap-2">
      {Array.from({ length: 4 }).map((_, index) => (
        <ProjectListItemSkeleton key={index} />
      ))}
    </div>
  );
};
