"use client";

import { useQuery } from "@tanstack/react-query";

import { bootstrapQueries } from "../../lib/queries";

export const SharedLogo = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const appName = data?.environment.appName ?? "Localify";

  return <div>{appName.charAt(0).toUpperCase()}</div>;
};
