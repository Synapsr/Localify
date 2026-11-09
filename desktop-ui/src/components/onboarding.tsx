import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AnimatePresence,
  motion,
  useReducedMotion,
  type Transition,
  type Variants,
} from "motion/react";
import { useEffect, useState, type ReactNode } from "react";
import { toast } from "sonner";

import type { BootstrapDto } from "@/types/localify";

import { DaemonPortConflicts } from "@/components/app/daemon-port-conflicts";
import {
  CheckIcon,
  ChevronRightIcon,
  CloudIcon,
  CopyIcon,
  DangerTriangleIcon,
  LockIcon,
  MonitorIcon,
  PowerIcon,
  RefreshIcon,
  ServersIcon,
  SparklesIcon,
  TerminalIcon,
  ZapIcon,
} from "@/components/icons";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { captureDesktopEvent } from "@/lib/analytics";
import {
  bootstrapQueries,
  configMutations,
  daemonMutations,
  systemMutations,
  systemQueries,
} from "@/lib/queries";
import { tauriClient } from "@/lib/tauri-client";
import { assertInstallCompleted } from "@/lib/util.install-result";
import { cn } from "@/lib/utils";

interface OnboardingProps {
  bootstrap: BootstrapDto;
  onActivated: () => void;
  /** Someone who has already been through onboarding only needs the approval. */
  startAtActivation?: boolean;
}

type StepId = "welcome" | "why" | "how" | "activate";
const STEPS: readonly StepId[] = ["welcome", "why", "how", "activate"] as const;
export function Onboarding({ bootstrap, onActivated, startAtActivation }: OnboardingProps) {
  const [stepIndex, setStepIndex] = useState(startAtActivation ? STEPS.length - 1 : 0);
  const [direction, setDirection] = useState<0 | 1 | -1>(0);
  const reduceMotion = useReducedMotion();

  const currentStep = STEPS[stepIndex]!;
  const queryClient = useQueryClient();
  const updateConfig = useMutation(configMutations.update(queryClient));
  const [connectAgents, setConnectAgents] = useState(bootstrap.config.agent.connectAgents);

  // Activation reads this setting from the config, so it is saved as soon as it changes.
  const changeConnectAgents = (next: boolean) => {
    setConnectAgents(next);
    updateConfig.mutate(
      { agent: { ...bootstrap.config.agent, connectAgents: next } },
      {
        onError: (error) => {
          setConnectAgents(!next);
          toast.error("Could not change the agent setting", {
            description: error instanceof Error ? error.message : String(error),
          });
        },
      },
    );
  };

  useEffect(() => {
    captureDesktopEvent("desktop_onboarding_step_viewed", { step: currentStep });
  }, [currentStep]);

  const go = (next: number) => {
    if (next === stepIndex) return;
    setDirection(next > stepIndex ? 1 : -1);
    setStepIndex(next);
  };

  const stepVariants: Variants = reduceMotion
    ? {
        initial: { opacity: 0 },
        animate: { opacity: 1 },
        exit: { opacity: 0 },
      }
    : {
        initial: (dir: 0 | 1 | -1) =>
          dir === 0
            ? { opacity: 1, x: 0, filter: "blur(0px)" }
            : { opacity: 0, x: dir * 24, filter: "blur(8px)" },
        animate: { opacity: 1, x: 0, filter: "blur(0px)" },
        exit: (dir: 0 | 1 | -1) => ({
          opacity: 0,
          x: (dir || 1) * -24,
          filter: "blur(8px)",
        }),
      };

  const stepTransition: Transition = reduceMotion
    ? { duration: 0.15 }
    : { duration: 0.4, ease: [0.22, 1, 0.36, 1] };

  return (
    <div className="relative flex h-full flex-col overflow-hidden bg-background">
      <BackgroundDecor />

      <div className="relative flex flex-1 items-center justify-center px-6 py-10">
        <div className="w-full max-w-2xl">
          <AnimatePresence mode="wait" custom={direction}>
            <motion.div
              key={currentStep}
              custom={direction}
              variants={stepVariants}
              initial="initial"
              animate="animate"
              exit="exit"
              transition={stepTransition}
            >
              {currentStep === "welcome" && (
                <WelcomeStep bootstrap={bootstrap} onContinue={() => go(1)} />
              )}
              {currentStep === "why" && (
                <WhyStep bootstrap={bootstrap} onBack={() => go(0)} onContinue={() => go(2)} />
              )}
              {currentStep === "how" && (
                <HowItWorksStep
                  bootstrap={bootstrap}
                  connectAgents={connectAgents}
                  onConnectAgentsChange={changeConnectAgents}
                  onBack={() => go(1)}
                  onContinue={() => go(3)}
                />
              )}
              {currentStep === "activate" && (
                <ActivateStep
                  bootstrap={bootstrap}
                  connectAgents={connectAgents}
                  onActivated={onActivated}
                  onBack={() => go(2)}
                />
              )}
            </motion.div>
          </AnimatePresence>
        </div>
      </div>

      <StepIndicator current={stepIndex} count={STEPS.length} onSelect={go} />
    </div>
  );
}

function BackgroundDecor() {
  const reduceMotion = useReducedMotion();
  if (reduceMotion) return null;
  return (
    <div aria-hidden className="pointer-events-none absolute inset-0 overflow-hidden">
      <motion.div
        className="absolute -top-32 left-1/2 h-[420px] w-[420px] -translate-x-1/2 rounded-full bg-primary/10 blur-3xl"
        animate={{ opacity: [0.35, 0.6, 0.35], scale: [1, 1.08, 1] }}
        transition={{ duration: 8, repeat: Infinity, ease: "easeInOut" }}
      />
      <motion.div
        className="absolute right-0 bottom-0 h-[320px] w-[320px] translate-x-1/3 translate-y-1/3 rounded-full bg-chart-1/10 blur-3xl"
        animate={{ opacity: [0.25, 0.5, 0.25] }}
        transition={{ duration: 10, repeat: Infinity, ease: "easeInOut" }}
      />
    </div>
  );
}

function StepIndicator({
  current,
  count,
  onSelect,
}: {
  current: number;
  count: number;
  onSelect: (i: number) => void;
}) {
  return (
    <div className="relative flex items-center justify-center gap-2 pb-10">
      {Array.from({ length: count }).map((_, i) => {
        const isActive = i === current;
        const isPast = i < current;
        const clickable = isPast;
        return (
          <button
            key={i}
            type="button"
            aria-label={`Step ${i + 1}`}
            aria-current={isActive ? "step" : undefined}
            onClick={clickable ? () => onSelect(i) : undefined}
            disabled={!clickable}
            className={cn(
              "h-1.5 rounded-full transition-all",
              isActive
                ? "w-8 bg-foreground"
                : isPast
                  ? "w-1.5 cursor-pointer bg-foreground/60 hover:bg-foreground/80"
                  : "w-1.5 cursor-default bg-foreground/20",
            )}
          />
        );
      })}
    </div>
  );
}

// ---------- Step 1: Welcome ----------

const welcomeEase = [0.22, 1, 0.36, 1] as const;

const welcomeContainer: Variants = {
  hidden: {},
  show: {
    transition: {
      staggerChildren: 0.12,
      delayChildren: 0.15,
    },
  },
};

const welcomeItem: Variants = {
  hidden: { opacity: 0, filter: "blur(12px)", y: 10 },
  show: {
    opacity: 1,
    filter: "blur(0px)",
    y: 0,
    transition: { duration: 0.8, ease: welcomeEase },
  },
};

const welcomeItemReduced: Variants = {
  hidden: { opacity: 0 },
  show: { opacity: 1, transition: { duration: 0.2 } },
};

function WelcomeStep({
  bootstrap,
  onContinue,
}: {
  bootstrap: BootstrapDto;
  onContinue: () => void;
}) {
  const appName = bootstrap.environment.appName;
  const reduceMotion = useReducedMotion();
  const item = reduceMotion ? welcomeItemReduced : welcomeItem;

  const subtitle = `Clean ${bootstrap.environment.fixedDomainSuffix} URLs for your agents, worktrees, and machines.`;

  return (
    <motion.div
      className="flex flex-col items-center text-center"
      initial="hidden"
      animate="show"
      variants={welcomeContainer}
    >
      <motion.img
        src="/logo.png"
        alt={`${appName} logo`}
        variants={item}
        className="mb-8 size-20 shadow-foreground/10"
      />

      <motion.h1
        variants={item}
        className="mb-6 text-5xl font-semibold tracking-tight text-balance sm:text-6xl"
      >
        Welcome to {appName}
      </motion.h1>

      <motion.p
        variants={item}
        className="mx-auto mb-10 max-w-md text-base text-pretty text-muted-foreground"
      >
        {subtitle}
      </motion.p>

      <motion.div variants={item}>
        <Button variant="glow" size="lg" onClick={onContinue}>
          Get started
          <ChevronRightIcon data-icon="inline-end" />
        </Button>
      </motion.div>
    </motion.div>
  );
}

// ---------- Step 2: Why Localify ----------

function WhyStep({
  bootstrap,
  onBack,
  onContinue,
}: {
  bootstrap: BootstrapDto;
  onBack: () => void;
  onContinue: () => void;
}) {
  const suffix = bootstrap.environment.fixedDomainSuffix;

  return (
    <div>
      <motion.div
        initial={{ opacity: 0, y: 8, filter: "blur(6px)" }}
        animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
        transition={{ duration: 0.5, ease: [0.22, 1, 0.36, 1] }}
        className="mb-8 text-center"
      >
        <h2 className="mb-3 text-3xl font-semibold tracking-tight">Why Localify</h2>
        <p className="mx-auto max-w-md text-sm text-muted-foreground">
          Five agents on five worktrees, across your computers. Every one keeps a stable URL, and
          hot reload shows up wherever you look from.
        </p>
      </motion.div>

      <motion.div
        initial={{ opacity: 0, y: 10, filter: "blur(6px)" }}
        animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
        transition={{ duration: 0.55, delay: 0.15, ease: [0.22, 1, 0.36, 1] }}
      >
        <MachinesMock suffix={suffix} />
      </motion.div>

      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.4, delay: 0.4, ease: "easeOut" }}
        className="mt-8 flex items-center justify-between"
      >
        <Button variant="ghost" size="lg" onClick={onBack}>
          Back
        </Button>
        <Button variant="primary" size="lg" onClick={onContinue}>
          Continue
          <ChevronRightIcon data-icon="inline-end" />
        </Button>
      </motion.div>
    </div>
  );
}

const MACHINES = [
  {
    name: "Mac Studio",
    worktrees: ["web--fix-auth.journal", "web--onboarding.journal", "api--onboarding.journal"],
  },
  { name: "Linux server", worktrees: ["web--dark-mode.journal", "web--search.journal"] },
] as const;

const WORKTREES = MACHINES.flatMap((machine) => machine.worktrees);

/* Agents' worktrees on two machines; one at a time reloads, and the MacBook shows it. */
function MachinesMock({ suffix }: { suffix: string }) {
  const reduceMotion = useReducedMotion();
  const [active, setActive] = useState(0);

  useEffect(() => {
    if (reduceMotion) return;
    const id = setInterval(() => setActive((i) => (i + 1) % WORKTREES.length), 1800);
    return () => clearInterval(id);
  }, [reduceMotion]);

  const activeHost = WORKTREES[active]!;
  const [label, project] = activeHost.split(".");

  return (
    <div className="space-y-3">
      <div className="grid grid-cols-2 gap-3">
        {MACHINES.map((machine) => (
          <div key={machine.name} className="rounded-xl border border-border bg-card p-3">
            <div className="mb-2.5 flex items-center gap-2 text-xs font-medium text-muted-foreground">
              <ServersIcon className="size-3.5" />
              {machine.name}
            </div>
            <ul className="space-y-1.5">
              {machine.worktrees.map((host) => {
                const isActive = host === activeHost;
                return (
                  <li
                    key={host}
                    className={cn(
                      "flex items-center gap-2 rounded-full border px-2.5 py-1 text-xs transition-colors duration-300",
                      isActive ? "border-foreground/25 bg-muted/60" : "border-border",
                    )}
                  >
                    <span className="size-1.5 shrink-0 rounded-full bg-success" />
                    <span className="min-w-0 flex-1 truncate">
                      {host}
                      {suffix}
                    </span>
                    {isActive && (
                      <RefreshIcon className="size-3 shrink-0 animate-spin text-muted-foreground [animation-duration:1.2s]" />
                    )}
                  </li>
                );
              })}
            </ul>
          </div>
        ))}
      </div>

      <div className="flex items-center gap-3 rounded-xl border border-border bg-card px-3 py-2.5">
        <div className="flex shrink-0 items-center gap-2 text-xs font-medium text-muted-foreground">
          <MonitorIcon className="size-3.5" />
          MacBook
        </div>
        <div className="flex min-w-0 flex-1 items-center gap-2 rounded-full bg-muted/60 px-3 py-1 text-xs">
          <LockIcon className="size-3 shrink-0 text-muted-foreground" />
          <AnimatePresence initial={false} mode="popLayout">
            <motion.span
              key={activeHost}
              className="truncate"
              initial={{ opacity: 0, filter: "blur(4px)", y: -3 }}
              animate={{ opacity: 1, filter: "blur(0px)", y: 0 }}
              exit={{ opacity: 0, filter: "blur(4px)", y: 3 }}
              transition={{ duration: 0.3, ease: [0.22, 1, 0.36, 1] }}
            >
              {project}--{label}.localify.fyi
            </motion.span>
          </AnimatePresence>
        </div>
      </div>
    </div>
  );
}

// ---------- Step 3: How it works ----------

const DOCS_URL = "https://github.com/Synapsr/Localify/blob/main/docs/README.md";

/* The prompt that sets Localify up the way a person works, pasted into their agent. */
const SETUP_PROMPT = `Set up Localify for this project, using its MCP tools. First ask me how I work: which apps you should run, whether you work in Git worktrees, and whether I need to open them from another device. Then set the project up, start its services, and give me their URLs.`;

const openDocs = (anchor: string) => {
  const url = `${DOCS_URL}#${anchor}`;
  tauriClient.openExternalUrl(url).catch(() => window.open(url, "_blank", "noopener"));
};

function LearnMore({ anchor }: { anchor: string }) {
  return (
    <button
      type="button"
      onClick={() => openDocs(anchor)}
      className="shrink-0 text-xs font-medium text-muted-foreground underline-offset-4 transition-colors hover:text-foreground hover:underline"
    >
      Learn more
    </button>
  );
}

function HowItWorksStep({
  bootstrap,
  connectAgents,
  onConnectAgentsChange,
  onBack,
  onContinue,
}: {
  bootstrap: BootstrapDto;
  connectAgents: boolean;
  onConnectAgentsChange: (next: boolean) => void;
  onBack: () => void;
  onContinue: () => void;
}) {
  const suffix = bootstrap.environment.fixedDomainSuffix;
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 2000);
    return () => window.clearTimeout(timer);
  }, [copied]);

  const copyPrompt = async () => {
    try {
      await navigator.clipboard.writeText(SETUP_PROMPT);
      setCopied(true);
    } catch {
      toast.error("Could not copy the prompt");
    }
  };

  const items: Array<{ icon: ReactNode; title: string; description: string; anchor: string }> = [
    {
      icon: <ZapIcon className="size-4" />,
      title: "One worktree, one URL",
      description: `Each Git worktree gets its own ${suffix} URL, removed when the agent is done.`,
      anchor: "cleaning-up-worktrees",
    },
    {
      icon: <TerminalIcon className="size-4" />,
      title: "You watch from here",
      description: "What your agents start runs in this app's terminal.",
      anchor: "services-in-the-app-from-the-command-line",
    },
    {
      icon: <CloudIcon className="size-4" />,
      title: "Reach it from anywhere",
      description: "With Localify Cloud, one password-protected URL per project.",
      anchor: "public-urls",
    },
  ];

  const enter = (i: number) => ({
    initial: { opacity: 0, y: 12, filter: "blur(8px)" },
    animate: { opacity: 1, y: 0, filter: "blur(0px)" },
    transition: { duration: 0.5, delay: 0.1 + i * 0.08, ease: [0.22, 1, 0.36, 1] as const },
  });

  return (
    <div>
      <motion.div
        initial={{ opacity: 0, y: 8, filter: "blur(6px)" }}
        animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
        transition={{ duration: 0.5, ease: [0.22, 1, 0.36, 1] }}
        className="mb-8 text-center"
      >
        <h2 className="mb-3 text-3xl font-semibold tracking-tight">How it works</h2>
        <p className="mx-auto max-w-md text-sm text-muted-foreground">
          Your agents do the work. You see all of it.
        </p>
      </motion.div>

      <motion.div {...enter(0)} className="rounded-2xl border border-border bg-card p-4">
        <div className="flex items-start gap-3">
          <div className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
            <SparklesIcon className="size-4" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="flex items-baseline justify-between gap-3">
              <h3 className="text-sm font-medium text-foreground">Your agent sets it up</h3>
              <LearnMore anchor="coding-agents" />
            </div>
            <p className="mt-1 text-sm text-muted-foreground">
              Claude Code, Codex, and Cursor run your projects through Localify, over MCP.
            </p>
          </div>
        </div>
        <div className="mt-4 flex flex-wrap items-center justify-between gap-3 border-t border-border/60 pt-3 pl-10">
          <label className="flex cursor-pointer items-center gap-2.5 text-sm text-foreground">
            <Switch checked={connectAgents} onCheckedChange={onConnectAgentsChange} />
            Connect my agents automatically
          </label>
          <span className="text-xs text-muted-foreground">or</span>
          <Button
            variant="outline"
            size="sm"
            className="rounded-full"
            onClick={() => void copyPrompt()}
          >
            {copied ? (
              <CheckIcon data-icon="inline-start" />
            ) : (
              <CopyIcon data-icon="inline-start" />
            )}
            {copied ? "Copied" : "Copy a setup prompt"}
          </Button>
        </div>
      </motion.div>

      <ul className="mt-3 divide-y divide-border/60 rounded-2xl border border-border bg-card">
        {items.map((item, i) => (
          <motion.li key={item.title} {...enter(i + 1)} className="flex items-start gap-3 p-4">
            <div className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
              {item.icon}
            </div>
            <div className="min-w-0 flex-1">
              <div className="flex items-baseline justify-between gap-3">
                <h3 className="text-sm font-medium text-foreground">{item.title}</h3>
                <LearnMore anchor={item.anchor} />
              </div>
              <p className="mt-1 text-sm text-muted-foreground">{item.description}</p>
            </div>
          </motion.li>
        ))}
      </ul>

      <motion.p {...enter(4)} className="mt-4 text-center text-xs text-muted-foreground">
        No agent? Add a project yourself from the app.
      </motion.p>

      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.4, delay: 0.55, ease: "easeOut" }}
        className="mt-8 flex items-center justify-between"
      >
        <Button variant="ghost" size="lg" onClick={onBack}>
          Back
        </Button>
        <Button variant="primary" size="lg" onClick={onContinue}>
          Continue
          <ChevronRightIcon data-icon="inline-end" />
        </Button>
      </motion.div>
    </div>
  );
}

// ---------- Step 3: Activate ----------

type ActivateState = "idle" | "running" | "error" | "done";

const waitForNextPaint = () =>
  new Promise<void>((resolve) => {
    requestAnimationFrame(() => resolve());
  });

function ActivateStep({
  bootstrap,
  connectAgents,
  onActivated,
  onBack,
}: {
  bootstrap: BootstrapDto;
  connectAgents: boolean;
  onActivated: () => void;
  onBack: () => void;
}) {
  const queryClient = useQueryClient();
  const install = useMutation(systemMutations.install(queryClient));
  const startDaemon = useMutation(daemonMutations.start(queryClient));
  // On for a new install; an older config that predates the setting keeps it off.
  const { data: updateStatus } = useQuery(systemQueries.updateStatus());
  const appName = bootstrap.environment.appName;

  const [state, setState] = useState<ActivateState>("idle");
  const [portConflictMessage, setPortConflictMessage] = useState<string | null>(null);
  const [phase, setPhase] = useState<"install" | "start" | null>(null);

  const showPortConflicts = state === "error" && Boolean(portConflictMessage);

  async function activate() {
    let activationPhase: "install" | "start" | null = null;
    setState("running");
    setPortConflictMessage(null);
    captureDesktopEvent("desktop_onboarding_activation_started");
    try {
      activationPhase = "install";
      setPhase("install");
      await waitForNextPaint();
      const installResult = await install.mutateAsync();
      assertInstallCompleted(installResult);
      activationPhase = "start";
      setPhase("start");
      await startDaemon.mutateAsync({ verbose: false });
      await queryClient.fetchQuery(bootstrapQueries.read());
      setState("done");
      setPhase(null);
      captureDesktopEvent("desktop_onboarding_activation_completed");
      onActivated();
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setState("error");
      captureDesktopEvent("desktop_onboarding_activation_failed", {
        phase: activationPhase,
        reason:
          activationPhase === "start" && msg.includes("already in use")
            ? "port_conflict"
            : "unknown",
      });
      if (activationPhase === "start" && msg.includes("already in use")) {
        setPortConflictMessage(msg);
      } else {
        setPortConflictMessage(null);
        toast.error("Activation failed", {
          description: msg,
          icon: <DangerTriangleIcon className="size-4" />,
        });
      }
    }
  }

  const suffix = bootstrap.environment.fixedDomainSuffix;
  const checks = [
    {
      icon: <CheckIcon className="size-3.5" />,
      label: "A local certificate authority, for trusted HTTPS",
    },
    {
      icon: <ZapIcon className="size-3.5" />,
      label: `DNS for ${suffix} domains, without /etc/hosts`,
    },
    { icon: <PowerIcon className="size-3.5" />, label: `${appName} runs as you, not as root` },
    ...(updateStatus?.automatic
      ? [
          {
            icon: <RefreshIcon className="size-3.5" />,
            label: "Updates install themselves, unless you turn them off in Settings",
          },
        ]
      : []),
    ...(connectAgents
      ? [
          {
            icon: <SparklesIcon className="size-3.5" />,
            label: "Your coding agents, connected over MCP",
          },
        ]
      : []),
  ];

  return (
    <div className="mx-auto max-w-md text-center">
      <motion.div
        initial={{ opacity: 0, y: 8, filter: "blur(6px)" }}
        animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
        transition={{ duration: 0.5, delay: 0.05, ease: [0.22, 1, 0.36, 1] }}
        className="mb-7"
      >
        <h2 className="mb-3 text-3xl font-semibold tracking-tight">Activate {appName}</h2>
        <p className="text-sm text-muted-foreground">
          One administrator password, once. After that, nothing of {appName}'s runs as root.
        </p>
      </motion.div>

      <motion.ul
        initial={{ opacity: 0, y: 10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.5, delay: 0.15, ease: [0.22, 1, 0.36, 1] }}
        className="space-y-2.5 text-left"
      >
        {checks.map((check) => (
          <li key={check.label} className="flex items-center gap-3 text-sm text-foreground/90">
            <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-muted text-muted-foreground">
              {check.icon}
            </span>
            {check.label}
          </li>
        ))}
      </motion.ul>

      {showPortConflicts && (
        <div className="mt-4">
          <DaemonPortConflicts visible />
        </div>
      )}

      <div className="mt-8 flex items-center justify-between">
        <Button
          variant="ghost"
          size="lg"
          onClick={onBack}
          disabled={state === "running" || state === "done"}
        >
          Back
        </Button>
        <Button
          variant="primary"
          size="lg"
          onClick={activate}
          loading={state === "running"}
          disabled={state === "running" || state === "done"}
        >
          {state === "running" ? (
            phase === "install" ? (
              "Applying secure setup…"
            ) : (
              "Starting Localify…"
            )
          ) : state === "done" ? (
            <>
              <CheckIcon data-icon="inline-start" />
              Activated
            </>
          ) : state === "error" ? (
            "Try again"
          ) : (
            `Activate ${appName}`
          )}
        </Button>
      </div>
    </div>
  );
}
