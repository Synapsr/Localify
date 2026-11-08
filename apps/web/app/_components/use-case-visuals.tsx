"use client";

import { motion, useInView, useReducedMotion } from "motion/react";
import { useTranslations } from "next-intl";
import { useEffect, useState, type ReactNode, type RefObject } from "react";

import { AppFrame, ProjectCard, TerminalPanel } from "@/app/_components/app-replicas";
import { AGENTS } from "@/app/_components/agent-logos";
import { ArrowRightIcon, CheckIcon, LockIcon, MonitorIcon, ServersIcon } from "@/components/icons";

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

/* 04: each local URL of the server has a public twin you open from your laptop. */
const MACHINE_ROWS = [
  { local: "web.journal.localify", public: "journal--web.localify.fyi" },
  { local: "web--fix-auth.journal.localify", public: "journal--web--fix-auth.localify.fyi" },
  { local: "api.journal.localify", public: "journal--api.localify.fyi" },
];

const MachineHeader = ({ icon, name, role }: { icon: ReactNode; name: string; role: string }) => (
  <div className="flex items-center gap-2.5">
    <span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-background text-foreground/80 ring-1 ring-border">
      {icon}
    </span>
    <div className="min-w-0">
      <p className="text-[13px] font-semibold text-foreground">{name}</p>
      <p className="truncate text-[11.5px] text-muted-foreground">{role}</p>
    </div>
  </div>
);

export const MachinesVisual = () => {
  const t = useTranslations("UseCases.machinesDemo");

  return (
    <div className="relative flex items-center justify-center overflow-hidden rounded-2xl bg-muted/60 px-4 py-8 ring-1 ring-border/60 md:px-8 md:py-10">
      <Sequence className="w-full max-w-lg space-y-4">
        <motion.div className="grid grid-cols-[minmax(0,1fr)_1.25rem_minmax(0,1fr)] items-center gap-2" variants={step}>
          <MachineHeader icon={<ServersIcon className="size-4" />} name={t("server")} role={t("serverRole")} />
          <span />
          <MachineHeader icon={<MonitorIcon className="size-4" />} name={t("laptop")} role={t("laptopRole")} />
        </motion.div>

        <div className="space-y-2">
          {MACHINE_ROWS.map((row) => (
            <motion.div
              className="grid grid-cols-[minmax(0,1fr)_1.25rem_minmax(0,1fr)] items-center gap-2"
              key={row.local}
              variants={step}
            >
              <span className="flex min-w-0 items-center gap-2 rounded-full bg-background px-3 py-2 text-[12px] text-foreground ring-1 ring-border">
                <span className="size-2 shrink-0 rounded-full bg-success" />
                <span className="truncate">{row.local}</span>
              </span>
              <ArrowRightIcon className="size-4 text-muted-foreground" />
              <span className="flex min-w-0 items-center gap-2 rounded-full bg-background px-3 py-2 text-[12px] text-foreground ring-1 ring-border">
                <LockIcon className="size-3.5 shrink-0 text-muted-foreground" />
                <span className="truncate">{row.public}</span>
              </span>
            </motion.div>
          ))}
        </div>

        <motion.p className="text-center text-[12px] text-muted-foreground" variants={step}>
          {t("password")}
        </motion.p>
      </Sequence>
    </div>
  );
};
