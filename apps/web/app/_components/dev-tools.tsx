"use client";

import dynamic from "next/dynamic";

// The import sits inside this condition so production builds drop the package.
const Agentation =
  process.env.NODE_ENV === "development"
    ? dynamic(() => import("agentation").then((m) => m.Agentation), { ssr: false })
    : null;

export const DevTools = () => {
  return Agentation ? <Agentation /> : null;
};
