"use client";

import { motion, useReducedMotion } from "motion/react";
import { useTranslations } from "next-intl";
import type { ReactNode } from "react";

import { CheckIcon, LockIcon } from "@/components/icons";
import { cn } from "@/lib/utils";

const ease = [0.22, 1, 0.36, 1] as const;

const WORKTREE_URL = "https://web--fix-auth.journal.localify";

/* Shared frame: a light app panel that sits on the sky background. */
const Panel = ({ children, className }: { children: ReactNode; className?: string }) => (
  <div
    className={cn(
      "w-full overflow-hidden rounded-xl border border-black/5 bg-white/95 text-neutral-900 shadow-[0_30px_60px_-30px_rgba(13,13,13,0.45)] backdrop-blur-sm",
      className,
    )}
  >
    {children}
  </div>
);

const Stage = ({ image, children }: { image: string; children: ReactNode }) => (
  <div
    className="relative flex aspect-4/3 items-center justify-center overflow-hidden rounded-2xl border border-border p-5 md:p-8"
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
      variants={{ hidden: {}, show: { transition: { staggerChildren: reduce ? 0 : 0.45, delayChildren: 0.2 } } }}
      viewport={{ once: true, amount: 0.5 }}
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

/* 01: an agent conversation that calls Localify's MCP tools. */
export const AgentVisual = () => {
  const t = useTranslations("UseCases.agentDemo");
  const calls = [
    { tool: "service_start", detail: "cwd: ~/journal/.claude/worktrees/fix-auth" },
    { tool: "service_logs", detail: "▲ Next.js · Ready in 1.2s" },
    { tool: "get_service_url", detail: WORKTREE_URL },
  ];

  return (
    <Stage image="/sky/agents.jpg">
      <Panel className="max-w-md">
        <Sequence className="space-y-3 p-4 font-sans text-[13px] leading-snug md:p-5">
          <motion.div className="ml-auto w-fit max-w-[85%] rounded-2xl rounded-br-md bg-neutral-900 px-3.5 py-2.5 text-white" variants={step}>
            {t("prompt")}
          </motion.div>
          {calls.map((call) => (
            <motion.div className="flex items-start gap-2.5 rounded-lg border border-neutral-200 bg-neutral-50 px-3 py-2" key={call.tool} variants={step}>
              <span className="mt-0.5 inline-flex size-4 shrink-0 items-center justify-center rounded-full bg-emerald-500/15 text-emerald-600">
                <CheckIcon className="size-2.5" strokeWidth={3} />
              </span>
              <span className="min-w-0">
                <span className="block font-mono text-[12px] font-medium">localify · {call.tool}</span>
                <span className="block truncate font-mono text-[11.5px] text-neutral-500">{call.detail}</span>
              </span>
            </motion.div>
          ))}
          <motion.p className="text-neutral-700" variants={step}>
            {t("reply")}{" "}
            <span className="font-mono text-[12px] font-medium text-sky-700">{WORKTREE_URL.replace("https://", "")}</span>
          </motion.p>
        </Sequence>
      </Panel>
    </Stage>
  );
};

/* 02: one project, one URL per worktree. */
export const WorktreesVisual = () => {
  const t = useTranslations("UseCases.worktreeDemo");
  const rows = [
    { branch: t("primary"), url: "web.journal.localify", state: "running" },
    { branch: "fix-auth", url: "web--fix-auth.journal.localify", state: "running" },
    { branch: "new-onboarding", url: "web--new-onboarding.journal.localify", state: "running" },
    { branch: "new-onboarding", url: "api--new-onboarding.journal.localify", state: "stopped" },
    { branch: "dark-mode", url: "web--dark-mode.journal.localify", state: "removed" },
  ] as const;

  return (
    <Stage image="/sky/urls.jpg">
      <Panel className="max-w-md">
        <div className="flex items-center justify-between border-b border-neutral-200 px-4 py-3">
          <span className="text-[14px] font-semibold">journal</span>
          <span className="text-[12px] text-neutral-500">{t("title")}</span>
        </div>
        <Sequence className="divide-y divide-neutral-100">
          {rows.map((row) => (
            <motion.div
              className={cn("flex items-center gap-3 px-4 py-2.5", row.state === "removed" && "opacity-45")}
              key={row.url}
              variants={step}
            >
              <span
                className={cn(
                  "size-2 shrink-0 rounded-full",
                  row.state === "running" && "bg-emerald-500",
                  row.state === "stopped" && "bg-neutral-300",
                  row.state === "removed" && "bg-transparent ring-1 ring-neutral-300",
                )}
              />
              <span className="min-w-0 flex-1">
                <span className={cn("block truncate font-mono text-[12.5px] font-medium", row.state === "removed" && "line-through")}>
                  {row.url}
                </span>
                <span className="block text-[11.5px] text-neutral-500">{row.branch}</span>
              </span>
              <span className="shrink-0 text-[11.5px] text-neutral-500">{t(row.state)}</span>
            </motion.div>
          ))}
        </Sequence>
      </Panel>
    </Stage>
  );
};

/* 03: the app's terminal panel, with the checkouts that have a session. */
export const AppVisual = () => {
  const t = useTranslations("UseCases.appDemo");
  const lines = [
    { text: "$ localify dev web", tone: "text-neutral-400" },
    { text: `Starting 'web' at ${WORKTREE_URL}`, tone: "text-neutral-200" },
    { text: "  injected LOCALIFY_URL", tone: "text-neutral-400" },
    { text: "  ▲ Next.js 16.2.4", tone: "text-neutral-200" },
    { text: "  ✓ Ready in 1.2s", tone: "text-emerald-400" },
    { text: " GET / 200 in 84ms", tone: "text-neutral-400" },
    { text: " GET /login 200 in 41ms", tone: "text-neutral-400" },
  ];

  return (
    <Stage image="/sky/ui.jpg">
      <Panel className="max-w-lg">
        <div className="flex items-center gap-1.5 border-b border-neutral-200 px-3 py-2.5">
          <span className="size-2.5 rounded-full bg-[#FF5C5F]" />
          <span className="size-2.5 rounded-full bg-[#FAC800]" />
          <span className="size-2.5 rounded-full bg-[#34C759]" />
          <span className="ml-2 text-[12px] font-medium text-neutral-500">{t("terminal")}</span>
        </div>
        <div className="grid grid-cols-[34%_1fr]">
          <div className="space-y-1 border-r border-neutral-200 bg-neutral-50 p-2.5 text-[12px]">
            <p className="px-2 pb-1 text-[10.5px] font-medium tracking-wide text-neutral-400 uppercase">{t("checkouts")}</p>
            {["main", "fix-auth", "new-onboarding"].map((name) => (
              <div
                className={cn(
                  "flex items-center gap-2 truncate rounded-md px-2 py-1.5",
                  name === "fix-auth" ? "bg-white font-medium shadow-sm ring-1 ring-neutral-200" : "text-neutral-600",
                )}
                key={name}
              >
                <span className="size-1.5 shrink-0 rounded-full bg-emerald-500" />
                {name}
              </div>
            ))}
          </div>
          <Sequence className="min-h-[190px] space-y-1 bg-neutral-900 p-3 font-mono text-[11px] leading-relaxed">
            {lines.map((line) => (
              <motion.p className={cn("truncate", line.tone)} key={line.text} variants={step}>
                {line.text}
              </motion.p>
            ))}
          </Sequence>
        </div>
      </Panel>
    </Stage>
  );
};

/* 04: one project URL reaches the Mac the agents work on, from any device. */
export const MachinesVisual = () => {
  const t = useTranslations("UseCases.machinesDemo");
  const devices = [
    { name: t("laptop"), role: t("laptopRole"), url: "journal--web--fix-auth.localify.fyi" },
    { name: t("phone"), role: t("phoneRole"), url: "journal--web.localify.fyi" },
  ];

  return (
    <Stage image="/sky/urls.jpg">
      <Sequence className="flex w-full max-w-md flex-col items-stretch gap-3">
        {devices.map((device) => (
          <motion.div key={device.name} variants={step}>
            <Panel className="px-4 py-3">
              <div className="flex items-center justify-between gap-3">
                <span className="text-[13px] font-semibold">{device.name}</span>
                <span className="text-[11.5px] text-neutral-500">{device.role}</span>
              </div>
              <div className="mt-2 flex items-center gap-2 rounded-md bg-neutral-100 px-2.5 py-1.5">
                <LockIcon className="size-3 shrink-0 text-neutral-500" />
                <span className="truncate font-mono text-[12px]">{device.url}</span>
              </div>
            </Panel>
          </motion.div>
        ))}
        <motion.div className="flex items-center justify-center gap-2 py-1 text-[11.5px] font-medium text-white/90" variants={step}>
          <span className="h-px w-8 bg-white/50" />
          {t("password")}
          <span className="h-px w-8 bg-white/50" />
        </motion.div>
        <motion.div variants={step}>
          <Panel className="px-4 py-3">
            <div className="flex items-center justify-between gap-3">
              <span className="text-[13px] font-semibold">{t("studio")}</span>
              <span className="text-[11.5px] text-neutral-500">{t("studioRole")}</span>
            </div>
            <div className="mt-2 flex flex-wrap gap-1.5">
              {["web.journal", "web--fix-auth.journal", "api.journal"].map((host) => (
                <span className="rounded-md bg-emerald-500/10 px-2 py-1 font-mono text-[11.5px] text-emerald-700" key={host}>
                  {host}.localify
                </span>
              ))}
            </div>
          </Panel>
        </motion.div>
      </Sequence>
    </Stage>
  );
};
