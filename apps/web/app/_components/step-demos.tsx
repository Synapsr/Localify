"use client";

import { AnimatePresence, motion } from "motion/react";
import { useTranslations } from "next-intl";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { AGENTS } from "@/app/_components/agent-logos";
import { AgentsSettingsCard, AppFrame, ConnectedToast, ProjectCard, type AgentsState } from "@/app/_components/app-replicas";
import { INSTALL_PROMPT } from "@/app/_components/install-prompt-button";
import { useCycle } from "@/app/_components/use-case-visuals";

const ease = [0.22, 1, 0.36, 1] as const;

const DESIGN_WIDTH = 460;
const FRAME_HEIGHT = 260;

/*
 * Renders a demo at a fixed width, scaled down to fit the card, on a plain background.
 * The height follows the tallest state seen, so a demo that changes size does not jump.
 */
const DemoFrame = ({ children }: { children: ReactNode }) => {
  const outer = useRef<HTMLDivElement>(null);
  const inner = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(DESIGN_WIDTH);
  const [natural, setNatural] = useState(0);

  useEffect(() => {
    const box = outer.current;
    const content = inner.current;
    if (!box || !content) return;
    const update = () => {
      setWidth(box.clientWidth || DESIGN_WIDTH);
      setNatural((h) => Math.max(h, content.offsetHeight));
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(box);
    observer.observe(content);
    return () => observer.disconnect();
  }, []);

  const scale = Math.min(1, width / DESIGN_WIDTH, natural > 0 ? FRAME_HEIGHT / natural : 1);

  return (
    <div className="rounded-2xl bg-muted/60 p-4 ring-1 ring-border/60">
      <div className="flex items-center justify-center overflow-hidden" ref={outer} style={{ height: FRAME_HEIGHT }}>
        <div className="overflow-hidden" style={{ width: DESIGN_WIDTH * scale, height: natural * scale }}>
          <div ref={inner} style={{ transform: `scale(${scale})`, transformOrigin: "top left", width: DESIGN_WIDTH }}>
            {children}
          </div>
        </div>
      </div>
    </div>
  );
};

/* 1: the install prompt, pasted into the agent's composer. */
export const PromptDemo = () => {
  const ref = useRef<HTMLDivElement>(null);
  const frame = useCycle(ref, 3, 1400);
  const claude = AGENTS[0];
  const lines = INSTALL_PROMPT.split("\n").filter(Boolean);

  return (
    <DemoFrame>
      <div className="w-full max-w-md overflow-hidden rounded-xl border border-black/5 bg-white/95 text-neutral-900 shadow-[0_30px_60px_-30px_rgba(13,13,13,0.45)]" ref={ref}>
        <div className="flex items-center gap-2 border-b border-neutral-200/80 px-4 py-2.5 text-[12.5px] font-medium text-neutral-500">
          <claude.Logo className="size-3.5" style={{ color: claude.color }} />
          {claude.name}
        </div>
        <div className="p-3">
          <div className="space-y-1.5 rounded-xl border border-neutral-200 bg-neutral-50 p-3 text-[12px] leading-snug text-neutral-700">
            {lines.map((line) => (
              <p key={line}>{line}</p>
            ))}
            <div className="flex justify-end pt-1">
              <span
                className={`inline-flex size-6 items-center justify-center rounded-full text-white transition-colors ${
                  frame === 2 ? "bg-[#D97757]" : "bg-neutral-300"
                }`}
              >
                ↑
              </span>
            </div>
          </div>
        </div>
      </div>
    </DemoFrame>
  );
};

/* 2: Settings → AI agents, the way the app connects them. */
const AGENT_STATES: AgentsState[] = ["ready", "connecting", "connected", "connected"];

export const ConnectDemo = () => {
  const ref = useRef<HTMLDivElement>(null);
  const frame = useCycle(ref, AGENT_STATES.length, 1500);
  const state = AGENT_STATES[frame];

  return (
    <DemoFrame>
      <div className="relative w-full max-w-lg" ref={ref}>
        <AgentsSettingsCard state={state} />
        <AnimatePresence>
          {state === "connected" ? (
            <motion.div
              animate={{ opacity: 1, y: 0 }}
              className="absolute right-3 bottom-3 left-3 flex justify-end"
              exit={{ opacity: 0, y: 8 }}
              initial={{ opacity: 0, y: 8 }}
              transition={{ duration: 0.35, ease }}
            >
              <ConnectedToast />
            </motion.div>
          ) : null}
        </AnimatePresence>
      </div>
    </DemoFrame>
  );
};

/* 3: the agent starts a worktree, and its URL shows up in the project card. */
export const AskDemo = () => {
  const t = useTranslations("HowItWorks.demo");
  const ref = useRef<HTMLDivElement>(null);
  const frame = useCycle(ref, 4, 1500);

  return (
    <DemoFrame>
      <div className="flex w-full max-w-lg flex-col gap-3" ref={ref}>
        <div className="ml-auto w-fit max-w-[85%] rounded-2xl rounded-br-md bg-neutral-900 px-3.5 py-2.5 text-[13px] text-white shadow-lg">
          {t("ask")}
        </div>
        <AppFrame>
          <ProjectCard
            domains={[{ domain: "web.journal.localify", port: "3000" }]}
            environments={
              frame === 0
                ? []
                : [
                    {
                      slug: "fix-auth",
                      branch: "fix/auth-callback",
                      open: true,
                      services: [{ domain: "web--fix-auth.journal.localify", state: frame === 1 ? "pending" : "running" }],
                    },
                  ]
            }
            name="journal"
          />
        </AppFrame>
      </div>
    </DemoFrame>
  );
};
