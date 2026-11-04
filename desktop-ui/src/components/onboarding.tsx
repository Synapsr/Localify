import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  AnimatePresence,
  motion,
  useReducedMotion,
  type Transition,
  type Variants,
} from "motion/react";
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { toast } from "sonner";

import type { BootstrapDto } from "@/types/localify";

import { DaemonPortConflicts } from "@/components/app/daemon-port-conflicts";
import {
  CheckIcon,
  ChevronRightIcon,
  DangerTriangleIcon,
  LockIcon,
  PowerIcon,
  ZapIcon,
} from "@/components/icons";
import { Button } from "@/components/ui/button";
import { captureDesktopEvent } from "@/lib/analytics";
import { bootstrapQueries, daemonMutations, systemMutations } from "@/lib/queries";
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
const FLOW_VIDEOS = [
  "/videos/flow-2-web.mp4",
  "/videos/flow-3-web.mp4",
  "/videos/flow-4-web.mp4",
  "/videos/agent.mp4",
] as const;
const warmedFlowVideos = new Map<string, HTMLVideoElement>();

export function Onboarding({ bootstrap, onActivated, startAtActivation }: OnboardingProps) {
  const [stepIndex, setStepIndex] = useState(startAtActivation ? STEPS.length - 1 : 0);
  const [direction, setDirection] = useState<0 | 1 | -1>(0);
  const reduceMotion = useReducedMotion();

  const currentStep = STEPS[stepIndex]!;

  useEffect(() => {
    captureDesktopEvent("desktop_onboarding_step_viewed", { step: currentStep });
  }, [currentStep]);

  useEffect(() => {
    warmFlowVideos();
  }, []);

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

function warmFlowVideos() {
  if (typeof document === "undefined") return;

  for (const src of FLOW_VIDEOS) {
    if (warmedFlowVideos.has(src)) continue;

    const video = document.createElement("video");
    video.src = src;
    video.preload = "auto";
    video.muted = true;
    video.playsInline = true;
    video.load();
    warmedFlowVideos.set(src, video);
  }
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
  const httpsPort = bootstrap.config.daemon.httpsPort;

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
        <BrowserMock suffix={suffix} httpsPort={httpsPort} />
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

function BrowserMock({ suffix, httpsPort }: { suffix: string; httpsPort: number }) {
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
  const httpsPortSegment = httpsPort === 443 ? "" : `:${httpsPort}`;

  const tabLabels = projects.map((p, i) =>
    tabModes[i] === "local" ? `localhost:${p.localPort}` : `${p.name}${suffix}`,
  );
  const url = secure
    ? `https://${active.name}${suffix}${httpsPortSegment}`
    : `http://localhost:${active.localPort}`;
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
  const [openVideo, setOpenVideo] = useState<string | null>(null);

  const items: Array<{
    title: string;
    description: string;
    video: string;
  }> = [
    {
      title: "Pick a folder, get a project",
      description: `Localify scans the folder, detects each app, and proposes ${suffix} hostnames. Works for a single project or a whole monorepo.`,
      video: FLOW_VIDEOS[0],
    },
    {
      title: "Or run localify init-project from your editor",
      description:
        "Same result, interactive prompts. The project shows up in the app, ready to go.",
      video: FLOW_VIDEOS[1],
    },
    {
      title: "Projects by name, not by port",
      description: `Every project gets a fixed ${suffix} hostname with trusted HTTPS. No port drift, demos that look like a product.`,
      video: FLOW_VIDEOS[2],
    },
    {
      title: "Or let your agent do it — /localify",
      description:
        "In any project, run /localify with Claude, Codex, or Cursor. It sets everything up, then you just talk to your agent to fix anything.",
      video: FLOW_VIDEOS[3],
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
          From a folder to a clean, trusted local URL.
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
            className="flex items-center gap-5 rounded-2xl border border-border bg-card p-4"
          >
            <FlowVideo src={item.video} onOpen={() => setOpenVideo(item.video)} />
            <div className="min-w-0 flex-1">
              <h3 className="text-[15px] font-medium text-foreground">{item.title}</h3>
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

      <FlowVideoModal src={openVideo} onClose={() => setOpenVideo(null)} />
    </div>
  );
}

function FlowVideo({ src, onOpen }: { src: string; onOpen: () => void }) {
  return (
    <motion.button
      type="button"
      onClick={onOpen}
      layoutId={`flow-video-${src}`}
      transition={flowVideoTransition}
      className="group relative aspect-4/3 w-30 shrink-0 cursor-pointer overflow-hidden rounded-lg border border-border bg-muted ring-1 ring-black/5 focus-visible:ring-2 focus-visible:ring-foreground focus-visible:outline-none dark:ring-white/10"
    >
      <video
        className="h-full w-full object-cover"
        src={src}
        autoPlay
        loop
        muted
        playsInline
        preload="auto"
      />
      <div className="pointer-events-none absolute inset-0 grid place-items-center bg-black/0 opacity-0 transition duration-200 group-hover:bg-black/40 group-hover:opacity-100 group-focus-visible:bg-black/40 group-focus-visible:opacity-100">
        <div className="flex items-center gap-1.5 rounded-full bg-background/90 px-2.5 py-1 text-[11px] font-medium text-foreground shadow-sm backdrop-blur">
          <MaximizeIcon className="size-3" />
          Watch
        </div>
      </div>
    </motion.button>
  );
}

const flowVideoTransition: Transition = {
  type: "spring",
  stiffness: 320,
  damping: 32,
  mass: 0.9,
};

function FlowVideoModal({ src, onClose }: { src: string | null; onClose: () => void }) {
  useEffect(() => {
    if (!src) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [src, onClose]);

  return createPortal(
    <AnimatePresence>
      {src && (
        <motion.div
          className="fixed inset-0 z-50 grid place-items-center p-8"
          onClick={onClose}
          role="dialog"
          aria-modal="true"
        >
          <motion.div
            className="absolute inset-0 bg-black/70 backdrop-blur-sm"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.2 }}
          />
          <motion.div
            layoutId={`flow-video-${src}`}
            transition={flowVideoTransition}
            onClick={(e) => e.stopPropagation()}
            className="relative aspect-4/3 w-full max-w-3xl overflow-hidden rounded-2xl border border-border bg-muted shadow-2xl ring-1 ring-black/10 dark:ring-white/10"
          >
            <video
              className="h-full w-full object-cover"
              src={src}
              autoPlay
              loop
              muted
              playsInline
              preload="auto"
            />
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  );
}

function MaximizeIcon({ className }: { className?: string }) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
    >
      <polyline points="15 3 21 3 21 9" />
      <polyline points="9 21 3 21 3 15" />
      <line x1="21" y1="3" x2="14" y2="10" />
      <line x1="3" y1="21" x2="10" y2="14" />
    </svg>
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

  const bullets = [
    {
      icon: <CheckIcon className="size-4" />,
      title: "Create a local certificate authority",
      description:
        "A private CA is generated on your machine. Node.js trusts it right away, and browsers trust it once you confirm in macOS.",
    },
    {
      icon: <ZapIcon className="size-4" />,
      title: "Configure DNS for .localify domains",
      description: `A resolver is added so your system sends ${bootstrap.environment.fixedDomainSuffix} lookups to Localify — without touching /etc/hosts.`,
    },
    {
      icon: <PowerIcon className="size-4" />,
      title: `Run ${appName} as you, not as root`,
      description:
        "macOS opens ports 80 and 443 and hands them to Localify, which runs under your account from then on. Updates need no password.",
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
          Localify needs an administrator's password once to set up its domains and background
          service. After that, nothing of Localify's runs as root.
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
