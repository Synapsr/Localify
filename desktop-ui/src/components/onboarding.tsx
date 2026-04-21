import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  AnimatePresence,
  motion,
  useReducedMotion,
  type Transition,
  type Variants,
} from "motion/react";
import { useEffect, useState } from "react";

import type { BootstrapDto } from "@/types/roxy";

import { DaemonPortConflicts } from "@/components/app/daemon-port-conflicts";
import {
  CheckIcon,
  ChevronRightIcon,
  CodeIcon,
  DangerTriangleIcon,
  FolderIcon,
  LockIcon,
  PlayIcon,
  PlusIcon,
  PowerIcon,
  TerminalIcon,
  ZapIcon,
} from "@/components/icons";
import { Button } from "@/components/ui/button";
import { bootstrapQueries, daemonMutations, systemMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";

interface OnboardingProps {
  bootstrap: BootstrapDto;
  onActivated: () => void;
}

type StepId = "welcome" | "why" | "how" | "activate";
const STEPS: readonly StepId[] = ["welcome", "why", "how", "activate"] as const;

export function Onboarding({ bootstrap, onActivated }: OnboardingProps) {
  const [stepIndex, setStepIndex] = useState(0);
  const [direction, setDirection] = useState<1 | -1>(1);
  const reduceMotion = useReducedMotion();

  const currentStep = STEPS[stepIndex]!;

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
        initial: (dir: 1 | -1) => ({
          opacity: 0,
          x: dir * 24,
          filter: "blur(8px)",
        }),
        animate: { opacity: 1, x: 0, filter: "blur(0px)" },
        exit: (dir: 1 | -1) => ({
          opacity: 0,
          x: dir * -24,
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
          <AnimatePresence mode="wait" custom={direction} initial={false}>
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
                <WhyStep
                  bootstrap={bootstrap}
                  onBack={() => go(0)}
                  onContinue={() => go(2)}
                />
              )}
              {currentStep === "how" && (
                <HowItWorksStep
                  bootstrap={bootstrap}
                  onBack={() => go(1)}
                  onContinue={() => go(3)}
                />
              )}
              {currentStep === "activate" && (
                <ActivateStep
                  bootstrap={bootstrap}
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

function WelcomeStep({
  bootstrap,
  onContinue,
}: {
  bootstrap: BootstrapDto;
  onContinue: () => void;
}) {
  const appName = bootstrap.environment.appName;
  const reduceMotion = useReducedMotion();

  const titleWords = ["Welcome", "to", appName];
  const subtitle = `A local development proxy with custom ${bootstrap.environment.fixedDomainSuffix} domains and automatic HTTPS.`;

  return (
    <div className="flex flex-col items-center text-center">
      <motion.img
        src="/logo.png"
        alt={`${appName} logo`}
        initial={reduceMotion ? { opacity: 0 } : { opacity: 0, scale: 0.85, filter: "blur(10px)" }}
        animate={reduceMotion ? { opacity: 1 } : { opacity: 1, scale: 1, filter: "blur(0px)" }}
        transition={{ duration: 0.6, ease: [0.22, 1, 0.36, 1] }}
        className="mb-8 size-20  shadow-foreground/10"
      />

      <h1 className="mb-6 text-5xl font-semibold tracking-tight sm:text-6xl">
        {titleWords.map((word, i) => (
          <BlurWord key={i} delay={0.15 + i * 0.12}>
            {word}
          </BlurWord>
        ))}
      </h1>

      <motion.p
        initial={reduceMotion ? { opacity: 0 } : { opacity: 0, y: 8, filter: "blur(6px)" }}
        animate={reduceMotion ? { opacity: 1 } : { opacity: 1, y: 0, filter: "blur(0px)" }}
        transition={{ duration: 0.7, delay: 0.6, ease: [0.22, 1, 0.36, 1] }}
        className="mx-auto mb-10 max-w-md text-base text-muted-foreground"
      >
        {subtitle}
      </motion.p>

      <motion.div
        initial={reduceMotion ? { opacity: 0 } : { opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.5, delay: 0.9, ease: "easeOut" }}
      >
        <Button size="lg" onClick={onContinue} className="px-5">
          Get started
          <ChevronRightIcon data-icon="inline-end" />
        </Button>
      </motion.div>
    </div>
  );
}

function BlurWord({ children, delay }: { children: string; delay: number }) {
  const reduceMotion = useReducedMotion();
  return (
    <motion.span
      className="mr-[0.25em] inline-block"
      initial={reduceMotion ? { opacity: 0 } : { opacity: 0, y: 16, filter: "blur(14px)" }}
      animate={reduceMotion ? { opacity: 1 } : { opacity: 1, y: 0, filter: "blur(0px)" }}
      transition={{ duration: 0.7, delay, ease: [0.22, 1, 0.36, 1] }}
    >
      {children}
    </motion.span>
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

  const paragraphs = [
    "AI makes it easy to juggle many projects at once. The problem: they all default to localhost:3000.",
    "Open a second one and it bumps to :3001, then :3002. The moment an env var hardcodes a base URL, things quietly break.",
    `Localify gives every project a fixed, memorable ${suffix} domain. No port drift, no collisions — and URLs that look good in a demo.`,
  ];

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
          The end of <span className="font-mono">localhost:3000</span>, <span className="font-mono">:3001</span>, <span className="font-mono">:3002</span>…
        </p>
      </motion.div>

      <div className="mx-auto max-w-xl space-y-4 text-[15px] leading-relaxed text-foreground/90">
        {paragraphs.map((p, i) => (
          <motion.p
            key={i}
            initial={{ opacity: 0, y: 10, filter: "blur(6px)" }}
            animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
            transition={{
              duration: 0.55,
              delay: 0.1 + i * 0.12,
              ease: [0.22, 1, 0.36, 1],
            }}
          >
            {p}
          </motion.p>
        ))}
      </div>

      <motion.div
        initial={{ opacity: 0, y: 10, filter: "blur(6px)" }}
        animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
        transition={{ duration: 0.55, delay: 0.5, ease: [0.22, 1, 0.36, 1] }}
        className="mt-8"
      >
        <BrowserMock suffix={suffix} />
      </motion.div>

      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.4, delay: 0.7, ease: "easeOut" }}
        className="mt-8 flex items-center justify-between"
      >
        <Button variant="ghost" size="lg" onClick={onBack}>
          Back
        </Button>
        <Button size="lg" onClick={onContinue} className="px-5">
          Continue
          <ChevronRightIcon data-icon="inline-end" />
        </Button>
      </motion.div>
    </div>
  );
}

type BrowserTab = {
  name: string;
  localPort: string;
};

function BrowserMock({ suffix }: { suffix: string }) {
  const reduceMotion = useReducedMotion();
  const tabs: BrowserTab[] = [
    { name: "shop", localPort: "3000" },
    { name: "admin", localPort: "3001" },
    { name: "docs", localPort: "3002" },
  ];

  const cycleLength = tabs.length * 2;
  const [step, setStep] = useState(0);

  useEffect(() => {
    if (reduceMotion) return;
    const id = setInterval(() => {
      setStep((s) => (s + 1) % cycleLength);
    }, 1800);
    return () => clearInterval(id);
  }, [reduceMotion, cycleLength]);

  const activeTab = step % tabs.length;
  const mode: "local" | "localify" = step < tabs.length ? "local" : "localify";
  const active = tabs[activeTab]!;
  const activeUrl =
    mode === "local"
      ? `http://localhost:${active.localPort}`
      : `https://${active.name}${suffix}`;

  return (
    <div className="overflow-hidden rounded-xl border border-border bg-card shadow-[0_12px_40px_-12px] shadow-foreground/10">
      {/* Window chrome */}
      <div className="flex items-center gap-1.5 border-b border-border bg-muted/40 px-3 py-2">
        <span className="size-2.5 rounded-full bg-red-400/80" />
        <span className="size-2.5 rounded-full bg-yellow-400/80" />
        <span className="size-2.5 rounded-full bg-green-400/80" />
      </div>

      {/* Tab strip */}
      <div className="flex gap-px bg-muted/40 px-2 pt-2">
        {tabs.map((tab, i) => {
          const isActive = i === activeTab;
          return (
            <div
              key={tab.name}
              className={cn(
                "relative flex min-w-0 flex-1 items-center gap-1.5 rounded-t-md px-3 py-1.5 text-xs transition-colors",
                isActive
                  ? "bg-card text-foreground"
                  : "text-muted-foreground hover:text-foreground/70",
              )}
            >
              <TabFavicon mode={mode} />
              <span className="truncate">{tab.name}</span>
              <span
                className={cn(
                  "ml-auto size-3 shrink-0 rounded-full opacity-40",
                  !isActive && "hidden",
                )}
              />
            </div>
          );
        })}
      </div>

      {/* Address bar */}
      <div className="flex items-center gap-2 border-t border-border bg-card p-2.5">
        <div className="flex flex-1 items-center gap-2 overflow-hidden rounded-md bg-muted/60 px-3 py-1.5">
          <AnimatePresence mode="wait" initial={false}>
            <motion.span
              key={`${mode}-icon`}
              initial={{ opacity: 0, scale: 0.6 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.6 }}
              transition={{ duration: 0.2 }}
              className="inline-flex shrink-0"
            >
              {mode === "local" ? (
                <DangerTriangleIcon className="size-3 text-amber-500" />
              ) : (
                <LockIcon className="size-3 text-emerald-600 dark:text-emerald-400" />
              )}
            </motion.span>
          </AnimatePresence>
          <AnimatePresence mode="wait" initial={false}>
            <motion.span
              key={activeUrl}
              initial={{ opacity: 0, y: 4, filter: "blur(4px)" }}
              animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
              exit={{ opacity: 0, y: -4, filter: "blur(4px)" }}
              transition={{ duration: 0.25, ease: [0.22, 1, 0.36, 1] }}
              className="min-w-0 truncate font-mono text-[12px] text-foreground"
            >
              <span className="text-muted-foreground">
                {mode === "local" ? "http://" : "https://"}
              </span>
              {mode === "local"
                ? `localhost:${active.localPort}`
                : `${active.name}${suffix}`}
            </motion.span>
          </AnimatePresence>
        </div>
      </div>

      {/* Caption */}
      <div className="flex items-center justify-center gap-2 border-t border-border bg-muted/20 px-3 py-2 text-[11px]">
        <AnimatePresence mode="wait" initial={false}>
          <motion.span
            key={mode}
            initial={{ opacity: 0, y: 4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -4 }}
            transition={{ duration: 0.25 }}
            className={cn(
              "flex items-center gap-1.5 font-medium",
              mode === "local"
                ? "text-destructive/80"
                : "text-emerald-600 dark:text-emerald-400",
            )}
          >
            <span
              className={cn(
                "size-1.5 rounded-full",
                mode === "local" ? "bg-destructive" : "bg-emerald-500",
              )}
            />
            {mode === "local"
              ? "3 projects fighting for the same port"
              : "3 projects, 3 clean domains"}
          </motion.span>
        </AnimatePresence>
      </div>
    </div>
  );
}

function TabFavicon({ mode }: { mode: "local" | "localify" }) {
  return (
    <span className="inline-flex size-3 shrink-0 items-center justify-center">
      <AnimatePresence mode="wait" initial={false}>
        {mode === "localify" ? (
          <motion.span
            key="lock"
            initial={{ opacity: 0, scale: 0.4 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.4 }}
            transition={{ duration: 0.2 }}
            className="inline-flex"
          >
            <LockIcon className="size-2.5 text-emerald-600 dark:text-emerald-400" />
          </motion.span>
        ) : (
          <motion.span
            key="dot"
            initial={{ opacity: 0, scale: 0.4 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.4 }}
            transition={{ duration: 0.2 }}
            className="size-1.5 rounded-full bg-amber-500/70"
          />
        )}
      </AnimatePresence>
    </span>
  );
}

// ---------- Step 3: How it works ----------

function HowItWorksStep({
  bootstrap,
  onBack,
  onContinue,
}: {
  bootstrap: BootstrapDto;
  onBack: () => void;
  onContinue: () => void;
}) {
  const suffix = bootstrap.environment.fixedDomainSuffix;

  const items: Array<{
    title: string;
    description: string;
    icon: React.ReactNode;
    media: React.ReactNode;
  }> = [
    {
      title: "Create a project manually",
      description: `Point Localify at a local port, pick a ${suffix} name, and add more domains as your project grows.`,
      icon: <PlusIcon className="size-4" />,
      media: <VideoPlaceholder />,
    },
    {
      title: "Import a monorepo folder",
      description:
        "Drop in a folder with multiple apps — Localify detects each package and wires domains automatically.",
      icon: <FolderIcon className="size-4" />,
      media: <VideoPlaceholder />,
    },
    {
      title: "Register from the CLI",
      description:
        "One command inside any project to claim a domain without opening the app.",
      icon: <TerminalIcon className="size-4" />,
      media: <CodeSnippet suffix={suffix} />,
    },
  ];

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
          Three ways to wire a project to a clean domain.
        </p>
      </motion.div>

      <ul className="space-y-4">
        {items.map((item, i) => (
          <motion.li
            key={item.title}
            initial={{ opacity: 0, y: 12, filter: "blur(8px)" }}
            animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
            transition={{
              duration: 0.55,
              delay: 0.1 + i * 0.1,
              ease: [0.22, 1, 0.36, 1],
            }}
            className="flex items-start gap-4 rounded-2xl border border-border bg-card p-4"
          >
            {item.media}
            <div className="min-w-0 flex-1 pt-1">
              <div className="flex items-center gap-2">
                <span className="inline-flex size-6 items-center justify-center rounded-md bg-muted text-muted-foreground">
                  {item.icon}
                </span>
                <h3 className="text-sm font-medium text-foreground">{item.title}</h3>
              </div>
              <p className="mt-1.5 text-sm text-muted-foreground">{item.description}</p>
            </div>
          </motion.li>
        ))}
      </ul>

      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.4, delay: 0.55, ease: "easeOut" }}
        className="mt-8 flex items-center justify-between"
      >
        <Button variant="ghost" size="lg" onClick={onBack}>
          Back
        </Button>
        <Button size="lg" onClick={onContinue} className="px-5">
          Continue
          <ChevronRightIcon data-icon="inline-end" />
        </Button>
      </motion.div>
    </div>
  );
}

function VideoPlaceholder() {
  return (
    <div className="relative h-20 w-32 shrink-0 overflow-hidden rounded-lg bg-muted">
      <div className="absolute inset-0 bg-linear-to-br from-foreground/5 to-foreground/10" />
      <div className="absolute inset-0 flex items-center justify-center">
        <div className="flex h-8 w-8 items-center justify-center rounded-full bg-background/80 shadow-sm backdrop-blur">
          <PlayIcon className="size-3.5 translate-x-px text-foreground" />
        </div>
      </div>
    </div>
  );
}

function CodeSnippet({ suffix }: { suffix: string }) {
  return (
    <div className="relative flex h-20 w-32 shrink-0 flex-col justify-center gap-1 overflow-hidden rounded-lg border border-border bg-muted/60 px-2.5 py-2 font-mono text-[10px]">
      <div className="flex items-center gap-1 text-muted-foreground">
        <span className="text-emerald-600 dark:text-emerald-400">$</span>
        <span className="truncate text-foreground">cd ~/myapp</span>
      </div>
      <div className="flex items-center gap-1 text-muted-foreground">
        <span className="text-emerald-600 dark:text-emerald-400">$</span>
        <span className="truncate text-foreground">localify add</span>
      </div>
      <div className="truncate text-muted-foreground">
        → myapp{suffix}
      </div>
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
  onActivated,
  onBack,
}: {
  bootstrap: BootstrapDto;
  onActivated: () => void;
  onBack: () => void;
}) {
  const queryClient = useQueryClient();
  const install = useMutation(systemMutations.install(queryClient));
  const startDaemon = useMutation(daemonMutations.start(queryClient));
  const appName = bootstrap.environment.appName;

  const [state, setState] = useState<ActivateState>("idle");
  const [message, setMessage] = useState<string | null>(null);
  const [phase, setPhase] = useState<"install" | "start" | null>(null);

  const showPortConflicts =
    state === "error" && phase === "start" && Boolean(message?.includes("already in use"));

  async function activate() {
    setState("running");
    setMessage(null);
    try {
      setPhase("install");
      await waitForNextPaint();
      await install.mutateAsync();
      setPhase("start");
      await startDaemon.mutateAsync({ verbose: false });
      queryClient.setQueryData<BootstrapDto>(bootstrapQueries.all(), (current) =>
        current
          ? {
              ...current,
              daemon: {
                ...current.daemon,
                caInstalled: true,
                running: true,
              },
            }
          : current,
      );
      setState("done");
      setPhase(null);
      onActivated();
      void queryClient.invalidateQueries({ queryKey: bootstrapQueries.all() });
    } catch (err) {
      setState("error");
      setMessage(err instanceof Error ? err.message : String(err));
    }
  }

  const bullets = [
    {
      icon: <CheckIcon className="size-4" />,
      title: "Create a local certificate authority",
      description:
        "A private CA is generated on your machine and trusted by your system keychain so browsers can show a lock on local domains.",
    },
    {
      icon: <ZapIcon className="size-4" />,
      title: "Configure DNS for .localify domains",
      description: `A resolver is added so your system sends ${bootstrap.environment.fixedDomainSuffix} lookups to Localify — without touching /etc/hosts.`,
    },
    {
      icon: <PowerIcon className="size-4" />,
      title: `Install the ${appName} helper`,
      description:
        "A tiny privileged helper lets Localify bind to ports 80 and 443. macOS prompts once, then the desktop app reuses it.",
    },
  ];

  return (
    <div>
      <motion.div
        initial={{ opacity: 0, y: 8, filter: "blur(6px)" }}
        animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
        transition={{ duration: 0.5, ease: [0.22, 1, 0.36, 1] }}
        className="mb-8 text-center"
      >
        <h2 className="mb-3 text-3xl font-semibold tracking-tight">Activate {appName}</h2>
        <p className="mx-auto max-w-md text-sm text-muted-foreground">
          Localify needs your administrator password once, to set up secure local HTTPS. Here's
          exactly what happens on your machine.
        </p>
      </motion.div>

      <ul className="mb-6 space-y-3">
        {bullets.map((b, i) => (
          <motion.li
            key={b.title}
            initial={{ opacity: 0, y: 10, filter: "blur(6px)" }}
            animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
            transition={{
              duration: 0.5,
              delay: 0.1 + i * 0.08,
              ease: [0.22, 1, 0.36, 1],
            }}
            className="flex items-start gap-3 rounded-xl border border-border bg-card p-4"
          >
            <div className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
              {b.icon}
            </div>
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium text-foreground">{b.title}</div>
              <p className="mt-1 text-sm text-muted-foreground">{b.description}</p>
            </div>
          </motion.li>
        ))}
      </ul>

      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.4, delay: 0.4, ease: "easeOut" }}
        className="rounded-xl border border-border bg-muted/30 p-3 text-xs text-muted-foreground"
      >
        <span className="font-medium text-foreground">Nothing leaves your computer.</span>{" "}
        Certificates and DNS settings stay on this Mac. You can undo everything from{" "}
        <span className="inline-flex items-center gap-1 font-mono">
          <CodeIcon className="size-3" /> Settings → Uninstall
        </span>
        .
      </motion.div>

      {state === "error" && message && (
        <div className="mt-4 rounded-xl border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
          {phase === "install" ? "Could not install system trust." : "The daemon could not start."}
          <div className="mt-1 font-mono text-xs opacity-80">{message}</div>
        </div>
      )}

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
          size="lg"
          onClick={activate}
          loading={state === "running"}
          disabled={state === "running" || state === "done"}
          className="px-5"
        >
          {state === "running" ? (
            phase === "install" ? (
              "Installing system trust…"
            ) : (
              "Starting daemon…"
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
