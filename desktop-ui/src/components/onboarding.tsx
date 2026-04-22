import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  AnimatePresence,
  motion,
  useReducedMotion,
  type Transition,
  type Variants,
} from "motion/react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

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
  const [direction, setDirection] = useState<0 | 1 | -1>(0);
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

  const subtitle = `A local development proxy with custom ${bootstrap.environment.fixedDomainSuffix} domains and automatic HTTPS.`;

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
          The end of <span className="font-mono">localhost:3000</span>,{" "}
          <span className="font-mono">:3001</span>, <span className="font-mono">:3002</span>…
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
        <Button variant="primary" size="lg" onClick={onContinue}>
          Continue
          <ChevronRightIcon data-icon="inline-end" />
        </Button>
      </motion.div>
    </div>
  );
}

type BrowserProject = {
  name: string;
  localPort: string;
};

const browserEase = [0.22, 1, 0.36, 1] as const;

const browserIconVariants: Variants = {
  initial: { opacity: 0, scale: 0.25, filter: "blur(4px)" },
  animate: { opacity: 1, scale: 1, filter: "blur(0px)" },
  exit: { opacity: 0, scale: 0.25, filter: "blur(4px)" },
};

function BrowserMock({ suffix }: { suffix: string }) {
  const reduceMotion = useReducedMotion();
  const projects: BrowserProject[] = [
    { name: "shop", localPort: "3000" },
    { name: "admin", localPort: "3001" },
    { name: "docs", localPort: "3002" },
  ];

  const cycleLength = projects.length + 1;
  const [step, setStep] = useState(0);

  useEffect(() => {
    if (reduceMotion) return;
    const id = setInterval(() => {
      setStep((s) => (s + 1) % cycleLength);
    }, 1800);
    return () => clearInterval(id);
  }, [reduceMotion, cycleLength]);

  const tabModes: Array<"local" | "localify"> = projects.map((_, i) =>
    i < step ? "localify" : "local",
  );
  const activeIndex = step === 0 ? 0 : step - 1;
  const secure = tabModes[activeIndex] === "localify";
  const active = projects[activeIndex]!;

  const tabLabels = projects.map((p, i) =>
    tabModes[i] === "local" ? `localhost:${p.localPort}` : `${p.name}${suffix}`,
  );
  const url = secure ? `https://${active.name}${suffix}` : `http://localhost:${active.localPort}`;
  const label = secure ? "trusted" : "not secure";

  return (
    <div className="overflow-hidden rounded-xl border border-border bg-muted/30">
      <div className="flex">
        <div className="flex items-center gap-1.5 border-b border-border px-2.5 py-2">
          <span className="size-2.5 rounded-full bg-[#FF5C5F]" />
          <span className="size-2.5 rounded-full bg-[#FAC800]" />
          <span className="size-2.5 rounded-full bg-[#34C759]" />
        </div>

        <div className="flex w-full items-end pt-1">
          {tabLabels.map((lbl, i) => {
            const isActive = i === activeIndex;
            const tabSecure = tabModes[i] === "localify";
            return (
              <div
                // biome-ignore lint/suspicious/noArrayIndexKey: tab position is stable
                key={i}
                className={cn(
                  "relative flex min-w-0 flex-1 items-center gap-2 rounded-t-lg border-b border-border px-3 py-2.5 font-mono text-[11.5px]",
                  isActive ? "border border-b-0 bg-card text-foreground" : "text-muted-foreground",
                  i > 0 && !isActive
                    ? "before:absolute before:top-1/2 before:left-0 before:h-4 before:w-px before:-translate-y-1/2 before:bg-border before:content-['']"
                    : "",
                )}
              >
                <span
                  className={cn(
                    "inline-block size-1.5 shrink-0 rounded-full transition-colors duration-300",
                    tabSecure ? "bg-emerald-500" : "bg-muted-foreground/40",
                  )}
                />
                <span className="relative min-w-0 flex-1 overflow-hidden">
                  <MorphText className="truncate" delay={i * 0.04} value={lbl} />
                </span>
              </div>
            );
          })}
        </div>
      </div>

      <div className="p-2">
        <div className="flex items-center gap-2 rounded-lg border border-border bg-muted px-3 py-2">
          <span
            aria-hidden
            className={cn(
              "relative grid size-4 shrink-0 place-items-center",
              secure ? "text-emerald-600 dark:text-emerald-400" : "text-muted-foreground",
            )}
          >
            <AnimatePresence initial={false} mode="wait">
              {secure ? (
                <motion.span
                  key="lock"
                  variants={browserIconVariants}
                  initial="initial"
                  animate="animate"
                  exit="exit"
                  transition={{ type: "spring", duration: 0.3, bounce: 0 }}
                  className="absolute inset-0 grid place-items-center"
                >
                  <LockIcon className="size-3.5" strokeWidth={2} />
                </motion.span>
              ) : (
                <motion.span
                  key="warn"
                  variants={browserIconVariants}
                  initial="initial"
                  animate="animate"
                  exit="exit"
                  transition={{ type: "spring", duration: 0.3, bounce: 0 }}
                  className="absolute inset-0 grid place-items-center"
                >
                  <DangerTriangleIcon className="size-3.5" strokeWidth={2} />
                </motion.span>
              )}
            </AnimatePresence>
          </span>

          <span className="relative min-w-0 flex-1 overflow-hidden">
            <MorphText
              className="truncate font-mono text-[12px] text-muted-foreground"
              delay={0.18}
              value={url}
            />
          </span>

          <span
            className={cn(
              "relative block min-w-[76px] overflow-hidden text-right font-mono text-[10.5px] tracking-wider uppercase transition-colors duration-300",
              secure ? "text-emerald-600 dark:text-emerald-400" : "text-muted-foreground",
            )}
          >
            <MorphText delay={0.26} value={label} />
          </span>
        </div>
      </div>
    </div>
  );
}

function MorphText({
  className,
  delay = 0,
  value,
}: {
  className?: string;
  delay?: number;
  value: string;
}) {
  return (
    <AnimatePresence initial={false} mode="popLayout">
      <motion.span
        key={value}
        className={cn("block tabular-nums", className)}
        initial={{ opacity: 0, filter: "blur(4px)", y: -3 }}
        animate={{ opacity: 1, filter: "blur(0px)", y: 0 }}
        exit={{ opacity: 0, filter: "blur(4px)", y: 3 }}
        transition={{ duration: 0.3, ease: browserEase, delay }}
      >
        {value}
      </motion.span>
    </AnimatePresence>
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
      description: "One command inside any project to claim a domain without opening the app.",
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
        <Button variant="primary" size="lg" onClick={onContinue}>
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
      <div className="truncate text-muted-foreground">→ myapp{suffix}</div>
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
      const msg = err instanceof Error ? err.message : String(err);
      setState("error");
      setMessage(msg);
      if (!msg.includes("already in use")) {
        toast("Approve with your administrator password to continue.", {
          icon: <LockIcon className="size-4" />,
        });
      }
    }
  }

  const bullets = [
    {
      icon: <CheckIcon className="size-4" />,
      title: "Create a local certificate authority",
      description:
        "A private CA is generated on your machine and trusted by your system keychain so browsers, Node.js, and local dev tools can trust Localify domains.",
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
