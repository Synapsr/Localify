"use client";

import { useBootstrap } from "../../lib/queries";

export const SharedLogo = () => {
  const { data } = useBootstrap();
  const appName = data?.environment.appName ?? "Localify";

  return <div>{appName.charAt(0).toUpperCase()}</div>;
};
