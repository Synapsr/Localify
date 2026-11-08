"use client";

import { motion, useInView, useReducedMotion } from "motion/react";
import { useTranslations } from "next-intl";
import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";

import { AppFrame, ProjectCard, TerminalPanel } from "@/app/_components/app-replicas";
import { AGENTS } from "@/app/_components/agent-logos";
import { CheckIcon, LockIcon, MobileIcon, MonitorIcon, ServersIcon } from "@/components/icons";
import { cn } from "@/lib/utils";

const ease = [0.22, 1, 0.36, 1] as const;

export const WORKTREE_URL = "https://web--fix-auth.journal.localify";

export const Stage = ({ image, children }: { image: string; children: ReactNode }) => (
  <div
    className="relative flex min-h-[340px] items-center justify-center overflow-hidden rounded-2xl border border-border p-4 md:aspect-4/3 md:min-h-0 md:p-8"
    style={{ backgroundImage: `url(${image})`, backgroundSize: "cover", backgroundPosition: "center" }}
  >
    {children}
  </div>
);

/* Reveals its children one after the other once in view. */
const Sequence = ({ children, className }: { children: ReactNode; className?: string }) => {
  const reduce = useReducedMotion();
  return (
    <motion.div
      className={className}
      initial="hidden"
      variants={{ hidden: {}, show: { transition: { staggerChildren: reduce ? 0 : 0.4, delayChildren: 0.2 } } }}
      viewport={{ once: true, amount: 0.4 }}
      whileInView="show"
    >
      {children}
    </motion.div>
  );
};

const step = {
  hidden: { opacity: 0, y: 8 },
  show: { opacity: 1, y: 0, transition: { duration: 0.45, ease } },
};

/* Steps through `length` states while in view; holds the last one when motion is reduced. */
export const useCycle = (ref: RefObject<Element | null>, length: number, ms: number) => {
  const inView = useInView(ref, { amount: 0.4 });
  const reduce = useReducedMotion();
  const [index, setIndex] = useState(0);
  useEffect(() => {
    if (reduce || !inView) return;
    const timer = window.setInterval(() => setIndex((i) => (i + 1) % length), ms);
    return () => window.clearInterval(timer);
  }, [inView, length, ms, reduce]);
  return reduce ? length - 1 : index;
};

/* 01: an agent conversation that calls Localify's MCP tools. */
export const AgentVisual = () => {
  const t = useTranslations("UseCases.agentDemo");
  const claude = AGENTS[0];
  const calls = [
    { tool: "service_start", detail: "~/journal/.claude/worktrees/fix-auth" },
    { tool: "service_logs", detail: "Next.js ready in 1.2s" },
    { tool: "get_service_url", detail: WORKTREE_URL },
  ];

  return (
    <Stage image="/sky/agents.jpg">
      <div className="w-full max-w-md overflow-hidden rounded-xl border border-black/5 bg-white/95 text-neutral-900 shadow-[0_30px_60px_-30px_rgba(13,13,13,0.45)] backdrop-blur-sm">
        <div className="flex items-center gap-2 border-b border-neutral-200/80 px-4 py-2.5 text-[12.5px] font-medium text-neutral-500">
          <claude.Logo className="size-3.5" style={{ color: claude.color }} />
          {claude.name}
        </div>
        <Sequence className="space-y-2.5 p-4 text-[13px] leading-snug md:p-5">
          <motion.div className="ml-auto w-fit max-w-[85%] rounded-2xl rounded-br-md bg-neutral-900 px-3.5 py-2.5 text-white" variants={step}>
            {t("prompt")}
          </motion.div>
          {calls.map((call) => (
            <motion.div className="flex items-start gap-2.5 px-1" key={call.tool} variants={step}>
              <span className="mt-0.5 inline-flex size-4 shrink-0 items-center justify-center rounded-full bg-emerald-500/15 text-emerald-600">
                <CheckIcon className="size-2.5" strokeWidth={3} />
              </span>
              <span className="min-w-0">
                <span className="block font-medium">
                  localify <span className="text-neutral-400">·</span> {call.tool}
                </span>
                <span className="block truncate text-[12px] text-neutral-500">{call.detail}</span>
              </span>
            </motion.div>
          ))}
          <motion.p className="px-1 pt-1 text-neutral-700" variants={step}>
            {t("reply")}{" "}
            <span className="font-medium text-sky-700">{WORKTREE_URL.replace("https://", "")}</span>
          </motion.p>
        </Sequence>
      </div>
    </Stage>
  );
};

/* 02: the project card of the app, one environment per worktree. */
export const WorktreesVisual = () => (
  <Stage image="/sky/urls.jpg">
    <Sequence className="w-full max-w-lg">
      <motion.div variants={step}>
        <AppFrame>
          <ProjectCard
            domains={[
              { domain: "web.journal.localify", port: "3000" },
              { domain: "api.journal.localify", port: "4000" },
            ]}
            environments={[
              {
                slug: "fix-auth",
                branch: "fix/auth-callback",
                open: true,
                services: [
                  { name: "web", domain: "web--fix-auth.journal.localify" },
                  { name: "api", domain: "api--fix-auth.journal.localify", state: "stopped" },
                ],
              },
              {
                slug: "new-onboarding",
                services: [
                  { domain: "web--new-onboarding.journal.localify" },
                  { domain: "api--new-onboarding.journal.localify" },
                ],
              },
              { slug: "dark-mode", services: [{ domain: "web--dark-mode.journal.localify", state: "stopped" }] },
            ]}
            name="journal"
          />
        </AppFrame>
      </motion.div>
    </Sequence>
  </Stage>
);

/* 03: the app's terminal panel, where what an agent starts runs. */
const TERMINAL_LINES: Array<{ text: string; color: string }> = [
  { text: "$ localify dev web", color: "#6b7280" },
  { text: `Starting 'web' at ${WORKTREE_URL}`, color: "#dddddd" },
  { text: "  injected LOCALIFY_URL", color: "#6b7280" },
  { text: "  ▲ Next.js 16.2.4", color: "#dddddd" },
  { text: "  ✓ Ready in 1.2s", color: "#4ade80" },
  { text: " GET / 200 in 84ms", color: "#7aa2f7" },
  { text: " GET /login 200 in 41ms", color: "#7aa2f7" },
  { text: " POST /api/auth/callback 302 in 63ms", color: "#facc15" },
];

export const AppVisual = () => (
  <Stage image="/sky/ui.jpg">
    <div className="w-full max-w-xl">
      <TerminalPanel
        checkouts={[
          { project: "journal", label: "primary", tabs: 2 },
          { project: "journal", label: "fix-auth", worktree: true, tabs: 1, active: true },
          { project: "journal", label: "new-onboarding", worktree: true, tabs: 2 },
        ]}
        lines={
          <Sequence className="space-y-0.5">
            {TERMINAL_LINES.map((line) => (
              <motion.p className="truncate" key={line.text} style={{ color: line.color }} variants={step}>
                {line.text}
              </motion.p>
            ))}
          </Sequence>
        }
        tabs={[{ label: "web", active: true }, { label: "Shell" }]}
      />
    </div>
  </Stage>
);

/* 04: devices reach the Mac the agents work on, through one protected project URL. */
const Browser = ({ url, children, className }: { url: string; children?: ReactNode; className?: string }) => (
  <div className={cn("overflow-hidden rounded-lg border border-black/5 bg-white shadow-[0_20px_40px_-20px_rgba(13,13,13,0.5)]", className)}>
    <div className="flex items-center gap-1.5 border-b border-neutral-200/80 bg-neutral-50 px-2 py-1.5">
      <span className="size-1.5 rounded-full bg-neutral-300" />
      <span className="size-1.5 rounded-full bg-neutral-300" />
      <span className="flex min-w-0 flex-1 items-center gap-1 rounded-md bg-white px-1.5 py-0.5 text-[10px] text-neutral-600 ring-1 ring-neutral-200">
        <LockIcon className="size-2.5 shrink-0 text-neutral-400" />
        <span className="truncate">{url}</span>
      </span>
    </div>
    <div className="space-y-1.5 p-2.5">{children}</div>
  </div>
);

const PageSkeleton = ({ accent }: { accent: string }) => (
  <>
    <div className={cn("h-2 w-1/2 rounded-full", accent)} />
    <div className="h-1.5 w-5/6 rounded-full bg-neutral-200" />
    <div className="h-1.5 w-2/3 rounded-full bg-neutral-200" />
    <div className="mt-2 h-5 w-16 rounded-md bg-neutral-900" />
  </>
);

/* A dot that travels down a short vertical line, from the devices to the Mac. */
const Flow = () => {
  const reduce = useReducedMotion();
  return (
    <div aria-hidden="true" className="relative mx-auto h-7 w-px bg-white/60">
      {reduce ? null : (
        <motion.span
          animate={{ y: [0, 24], opacity: [0, 1, 0] }}
          className="absolute -left-[3px] size-[7px] rounded-full bg-white shadow"
          transition={{ duration: 1.4, repeat: Infinity, ease: "easeInOut" }}
        />
      )}
    </div>
  );
};

export const MachinesVisual = () => {
  const t = useTranslations("UseCases.machinesDemo");

  return (
    <Stage image="/sky/urls.jpg">
      <Sequence className="flex w-full max-w-md flex-col">
        <motion.div className="grid grid-cols-[1fr_auto] items-end gap-3" variants={step}>
          <div>
            <p className="mb-1.5 flex items-center gap-1.5 text-[11.5px] font-medium text-white">
              <MonitorIcon className="size-3.5" /> {t("laptop")} · {t("laptopRole")}
            </p>
            <Browser url="journal--web--fix-auth.localify.fyi">
              <PageSkeleton accent="bg-sky-400/70" />
            </Browser>
          </div>
          <div className="w-28">
            <p className="mb-1.5 flex items-center gap-1.5 text-[11.5px] font-medium text-white">
              <MobileIcon className="size-3.5" /> {t("phone")}
            </p>
            <div className="rounded-[18px] bg-neutral-900 p-1 shadow-[0_20px_40px_-20px_rgba(13,13,13,0.6)]">
              <Browser className="rounded-[14px] border-0" url="journal--web.localify.fyi">
                <PageSkeleton accent="bg-emerald-400/70" />
              </Browser>
            </div>
          </div>
        </motion.div>

        <motion.div variants={step}>
          <Flow />
          <div className="mx-auto flex w-fit items-center gap-1.5 rounded-full bg-white/95 px-3 py-1.5 text-[12px] font-medium text-neutral-800 shadow-lg">
            <LockIcon className="size-3.5 text-neutral-500" />
            localify.fyi
            <span className="text-neutral-400">·</span>
            <span className="text-neutral-500">{t("password")}</span>
          </div>
          <Flow />
        </motion.div>

        <motion.div variants={step}>
          <p className="mb-1.5 flex items-center gap-1.5 text-[11.5px] font-medium text-white">
            <ServersIcon className="size-3.5" /> {t("studio")} · {t("studioRole")}
          </p>
          <div className="rounded-xl bg-white/95 p-1 shadow-[0_20px_40px_-20px_rgba(13,13,13,0.5)]">
            <ProjectCard
              domains={[
                { domain: "web.journal.localify", isPublic: true },
                { domain: "web--fix-auth.journal.localify", isPublic: true },
              ]}
              isPublic
              name="journal"
            />
          </div>
        </motion.div>
      </Sequence>
    </Stage>
  );
};
