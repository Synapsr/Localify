"use client";

import { ProgressiveBlur } from "@/components/progresive-blur";
import { useScroll } from "@/hooks/use-scroll";

export const SettingsHeader = () => {
  const { scrolled } = useScroll(0);

  return (
    <header className="sticky top-0 z-10 space-y-2 bg-white py-4 pt-12">
      <ProgressiveBlur
        position="top"
        backgroundColor="#fff"
        className={`top-0 transition-opacity duration-200 ${scrolled ? "opacity-100" : "opacity-0"}`}
      />

      <div className="relative z-10 flex items-end justify-between gap-4 ">
        <div className="relative z-10 flex items-center gap-4">
          <h1 className="text-2xl font-semibold tracking-tight">Settings</h1>
        </div>
      </div>
    </header>
  );
};
