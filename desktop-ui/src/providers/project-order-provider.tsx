"use client";

import { useQuery } from "@tanstack/react-query";
import { createContext, type PropsWithChildren, useContext, useMemo } from "react";

import { bootstrapQueries } from "@/lib/queries";
import { useProjectOrder } from "@/lib/use-project-order";

const ProjectOrderContext = createContext<ReturnType<typeof useProjectOrder> | null>(null);

// One instance for the whole app, so the projects screen and the terminal tabs share an order.
export const ProjectOrderProvider = ({ children }: PropsWithChildren) => {
  const { data } = useQuery(bootstrapQueries.read());
  const projects = data?.projects;
  const knownIds = useMemo(() => (projects ?? []).map((project) => project.id), [projects]);
  const activities = useMemo(
    () =>
      (projects ?? []).map((project) => ({
        id: project.id,
        hasReachableDomain: project.hasReachableDomain,
        lastAccessedAt: project.lastAccessedAt,
      })),
    [projects],
  );
  const projectOrder = useProjectOrder(knownIds, activities);

  return (
    <ProjectOrderContext.Provider value={projectOrder}>{children}</ProjectOrderContext.Provider>
  );
};

export function useSharedProjectOrder() {
  const context = useContext(ProjectOrderContext);
  if (!context) {
    throw new Error("useSharedProjectOrder must be used within ProjectOrderProvider");
  }
  return context;
}
