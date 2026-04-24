"use client";

import { openUrl } from "@tauri-apps/plugin-opener";
import { motion } from "motion/react";

import { ArrowRightIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";

const BUY_URL = "https://localify.dev/#pricing";

export const ProjectsListUpsell = () => {
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: "spring", duration: 0.5, bounce: 0, delay: 0.1 }}
      className="relative isolate mt-2  overflow-hidden rounded-2xl border border-border/40 bg-neutral-50/80 px-6 py-6 shadow-[0_1px_0_0_rgba(255,255,255,0.6)_inset] transition-colors duration-200 hover:border-border dark:bg-neutral-900/50 dark:shadow-[0_1px_0_0_rgba(255,255,255,0.04)_inset]"
    >
      {/* <div aria-hidden className="pointer-events-none absolute inset-0 -z-10">
        <div className="absolute -top-20 -left-12 size-56 rounded-full bg-blue-500/15 blur-3xl dark:bg-blue-500/20" />
        <div className="absolute -right-12 -bottom-24 size-64 rounded-full bg-purple-500/15 blur-3xl dark:bg-purple-500/20" />
      </div> */}

      <div className="flex items-center gap-6">
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <div className="flex items-baseline gap-2.5">
            <h3 className="text-[17px] font-semibold tracking-tight text-balance text-foreground">
              Upgrade to Pro from <br /> €49/
              <span className=" text-muted-foreground tabular-nums">Lifetime</span>
            </h3>
          </div>
          <p className="max-w-md text-[13.5px] leading-normal text-pretty text-muted-foreground">
            Unlock unlimited projects and domains. Pay once, keep it forever.
          </p>
        </div>

        <Button size="lg" variant="glow" onClick={() => void openUrl(BUY_URL)}>
          Upgrade
          <ArrowRightIcon data-icon="inline-end" />
        </Button>
      </div>
    </motion.div>
  );
};
