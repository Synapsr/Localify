"use client";

import { AnimatePresence, motion } from "motion/react";
import { useTranslations } from "next-intl";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { AGENTS } from "@/app/_components/agent-logos";
import { AgentsSettingsCard, AppFrame, ConnectedToast, ProjectCard, type AgentsState } from "@/app/_components/app-replicas";
import { INSTALL_PROMPT } from "@/app/_components/install-prompt-button";
import { useCycle } from "@/app/_components/use-case-visuals";
import { CheckIcon, SpinnerIcon } from "@/components/icons";
import { CLI_INSTALL_COMMAND } from "@/app/download-links";

const ease = [0.22, 1, 0.36, 1] as const;

/* Every demo is drawn at this size, then scaled down as a whole, so no state changes its frame. */
const DESIGN_WIDTH = 460;
const DESIGN_HEIGHT = 392;

const DemoFrame = ({ children }: { children: ReactNode }) => {
  const outer = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(DESIGN_WIDTH);

  useEffect(() => {
    const box = outer.current;
    if (!box) return;
    const update = () => setWidth(box.clientWidth || DESIGN_WIDTH);
    update();
    const observer = new ResizeObserver(update);
    observer.observe(box);
    return () => observer.disconnect();
  }, []);

  const scale = Math.min(1, width / DESIGN_WIDTH);

  return (
    <div className="rounded-2xl bg-muted/60 p-4 ring-1 ring-border/60">
      <div className="flex justify-center overflow-hidden" ref={outer} style={{ height: DESIGN_HEIGHT * scale }}>
        <div
          style={{
            width: DESIGN_WIDTH,
            height: DESIGN_HEIGHT,
            transform: `scale(${scale})`,
            transformOrigin: "top center",
            flexShrink: 0,
          }}
        >
          {children}
        </div>
      </div>
    </div>
  );
};

const fade = {
  initial: { opacity: 0, y: 6 },
  animate: { opacity: 1, y: 0 },
  exit: { opacity: 0, y: -6 },
  transition: { duration: 0.3, ease },
};

/* 1: the prompt is pasted, sent, and the agent runs the install. */
const PROMPT_LINES = INSTALL_PROMPT.split("\n").filter(Boolean);
const RUNS = [CLI_INSTALL_COMMAND, "localify trust", "localify agent install"];

export const PromptDemo = () => {
  const ref = useRef<HTMLDivElement>(null);
  // 0 pasted · 1 ready to send · 2-5 the agent runs each command, then holds.
  const frame = useCycle(ref, 6, 1100);
  const claude = AGENTS[0];
  const sent = frame >= 2;

  return (
    <DemoFrame>
      <div className="flex h-full flex-col overflow-hidden rounded-xl border border-neutral-200 bg-white text-neutral-900" ref={ref}>
        <div className="flex items-center gap-2 border-b border-neutral-200/80 px-4 py-2.5 text-[12.5px] font-medium text-neutral-500">
          <claude.Logo className="size-3.5" style={{ color: claude.color }} />
          {claude.name}
        </div>
        <div className="relative flex-1 p-3">
          <AnimatePresence initial={false} mode="wait">
            {sent ? (
              <motion.div className="space-y-2.5" key="run" {...fade}>
                <div className="ml-auto w-fit max-w-[85%] rounded-2xl rounded-br-md bg-neutral-900 px-3.5 py-2 text-[12px] text-white">
                  {PROMPT_LINES[0]}
                </div>
                {RUNS.map((command, index) => {
                  const done = frame - 2 > index;
                  const active = frame - 2 === index;
                  if (!done && !active) return null;
                  return (
                    <motion.div className="flex items-center gap-2.5 px-1 text-[12px]" key={command} {...fade}>
                      <span
                        className={`inline-flex size-4 shrink-0 items-center justify-center rounded-full ${
                          done ? "bg-emerald-500/15 text-emerald-600" : "text-neutral-400"
                        }`}
                      >
                        {done ? <CheckIcon className="size-2.5" strokeWidth={3} /> : <SpinnerIcon className="size-3 animate-spin" />}
                      </span>
                      <span className="truncate font-medium">{command}</span>
                    </motion.div>
                  );
                })}
              </motion.div>
            ) : (
              <motion.div
                className="flex h-full flex-col rounded-xl border border-neutral-200 bg-neutral-50 p-3 text-[11.5px] leading-snug text-neutral-700"
                key="compose"
                {...fade}
              >
                <div className="space-y-1">
                  {PROMPT_LINES.map((line) => (
                    <p key={line}>{line}</p>
                  ))}
                </div>
                <div className="mt-auto flex justify-end pt-2">
                  <span
                    className={`inline-flex size-6 items-center justify-center rounded-full text-white transition-colors duration-300 ${
                      frame === 1 ? "bg-[#D97757]" : "bg-neutral-300"
                    }`}
                  >
                    ↑
                  </span>
                </div>
              </motion.div>
            )}
          </AnimatePresence>
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
      <div className="relative h-full" ref={ref}>
        <AgentsSettingsCard state={state} />
        <AnimatePresence>
          {state === "connected" ? (
            <motion.div
              animate={{ opacity: 1, y: 0 }}
              className="absolute right-0 bottom-0 left-0 flex justify-end"
              exit={{ opacity: 0, y: 8 }}
              initial={{ opacity: 0, y: 8 }}
              transition={{ duration: 0.3, ease }}
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
      <div className="flex h-full flex-col gap-3" ref={ref}>
        <div className="ml-auto w-fit max-w-[85%] rounded-2xl rounded-br-md bg-neutral-900 px-3.5 py-2.5 text-[13px] text-white">
          {t("ask")}
        </div>
        <AppFrame flat>
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
