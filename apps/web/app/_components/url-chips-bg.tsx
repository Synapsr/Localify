"use client";

import type { CSSProperties } from "react";

import { useEffect, useRef } from "react";

import { LockIcon } from "@/components/icons";

type Hue = "orange" | "blue" | "violet" | "rose" | "sky" | "amber" | "emerald" | "indigo";

type Chip = {
  prefix: string;
  angle: number;
  rotate: number;
  scale?: number;
  hue: Hue;
  live?: boolean;
};

const COLORS: Record<Hue, { gradient: string; solid: string }> = {
  blue: {
    gradient: "linear-gradient(135deg, #60a5fa 0%, #2563eb 100%)",
    solid: "#3b82f6",
  },
  violet: {
    gradient: "linear-gradient(135deg, #a78bfa 0%, #7c3aed 100%)",
    solid: "#8b5cf6",
  },
  orange: {
    gradient: "linear-gradient(135deg, #fb923c 0%, #ea580c 100%)",
    solid: "#f97316",
  },
  rose: {
    gradient: "linear-gradient(135deg, #fb7185 0%, #e11d48 100%)",
    solid: "#f43f5e",
  },
  sky: {
    gradient: "linear-gradient(135deg, #7dd3fc 0%, #0284c7 100%)",
    solid: "#0ea5e9",
  },
  amber: {
    gradient: "linear-gradient(135deg, #fbbf24 0%, #d97706 100%)",
    solid: "#f59e0b",
  },
  emerald: {
    gradient: "linear-gradient(135deg, #6ee7b7 0%, #059669 100%)",
    solid: "#10b981",
  },
  indigo: {
    gradient: "linear-gradient(135deg, #a5b4fc 0%, #4338ca 100%)",
    solid: "#6366f1",
  },
};

const ELLIPSE = { cx: 50, cy: 50, rx: 50, ry: 50 };

const CHIPS: Chip[] = [
  { prefix: "docs", angle: 250, rotate: -2, hue: "sky" },
  { prefix: "webhooks", angle: 290, rotate: 4, hue: "orange" },
  { prefix: "api", angle: 215, rotate: -3, hue: "blue" },
  { prefix: "dashboard", angle: 325, rotate: 3, hue: "violet" },
  {
    prefix: "app",
    angle: 180,
    rotate: 2,
    scale: 1.05,
    hue: "emerald",
    live: true,
  },
  { prefix: "admin", angle: 0, rotate: -2, hue: "rose" },
  { prefix: "staging", angle: 145, rotate: -2, scale: 0.95, hue: "amber" },
  { prefix: "auth", angle: 35, rotate: 2, scale: 0.95, hue: "indigo" },
];

const POSITIONS = CHIPS.map((c) => {
  const rad = (c.angle * Math.PI) / 180;
  return {
    ...c,
    left: ELLIPSE.cx + ELLIPSE.rx * Math.cos(rad),
    top: ELLIPSE.cy + ELLIPSE.ry * Math.sin(rad),
  };
});

const RADIUS = 240;
const STRENGTH = 22;
const EASE = 0.12;

const POP_START_MS = 900;
const POP_WINDOW_MS = 500;
const POP_DURATION_MS = 340;
const CHIP_OPACITY = 0.85;

export const UrlChipsBg = () => {
  const containerRef = useRef<HTMLDivElement>(null);
  const chipRefs = useRef<Array<HTMLDivElement | null>>([]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      chipRefs.current.forEach((el) => {
        if (!el) return;
        el.style.opacity = String(CHIP_OPACITY);
      });
      return;
    }

    const centers = CHIPS.map(() => ({ x: 0, y: 0 }));
    const states = CHIPS.map(() => ({ x: 0, y: 0, tx: 0, ty: 0 }));
    const popDelays = CHIPS.map(() => Math.random() * POP_WINDOW_MS);
    const startTime = performance.now() + POP_START_MS;
    let mouseX = -99999;
    let mouseY = -99999;
    let rafId = 0;

    const measure = () => {
      chipRefs.current.forEach((el, i) => {
        if (!el) return;
        centers[i].x = el.offsetLeft;
        centers[i].y = el.offsetTop;
      });
    };

    const onMove = (e: PointerEvent) => {
      const rect = container.getBoundingClientRect();
      mouseX = e.clientX - rect.left;
      mouseY = e.clientY - rect.top;
    };

    const onLeave = () => {
      mouseX = -99999;
      mouseY = -99999;
    };

    const tick = () => {
      const now = performance.now();
      for (let i = 0; i < POSITIONS.length; i++) {
        const el = chipRefs.current[i];
        if (!el) continue;
        const chip = POSITIONS[i];
        const center = centers[i];
        const state = states[i];

        const popElapsed = now - (startTime + popDelays[i]);
        let enterScale = 0;
        let enterOpacity = 0;
        if (popElapsed >= POP_DURATION_MS) {
          enterScale = 1;
          enterOpacity = 1;
        } else if (popElapsed > 0) {
          const p = popElapsed / POP_DURATION_MS;
          const eased = 1 - (1 - p) ** 3;
          enterScale = eased;
          enterOpacity = eased;
        }

        const dx = mouseX - center.x;
        const dy = mouseY - center.y;
        const dist = Math.hypot(dx, dy);
        if (enterScale === 1 && dist < RADIUS && dist > 0.01) {
          const pull = (1 - dist / RADIUS) ** 2;
          state.tx = (dx / dist) * STRENGTH * pull;
          state.ty = (dy / dist) * STRENGTH * pull;
        } else {
          state.tx = 0;
          state.ty = 0;
        }
        state.x += (state.tx - state.x) * EASE;
        state.y += (state.ty - state.y) * EASE;

        el.style.transform = `translate(-50%, -50%) translate(${state.x.toFixed(2)}px, ${state.y.toFixed(2)}px) rotate(${chip.rotate}deg) scale(${(chip.scale ?? 1) * enterScale})`;
        el.style.opacity = (CHIP_OPACITY * enterOpacity).toString();
      }
      rafId = requestAnimationFrame(tick);
    };

    measure();
    window.addEventListener("pointermove", onMove);
    document.documentElement.addEventListener("mouseleave", onLeave);
    window.addEventListener("resize", measure);
    rafId = requestAnimationFrame(tick);

    return () => {
      cancelAnimationFrame(rafId);
      window.removeEventListener("pointermove", onMove);
      document.documentElement.removeEventListener("mouseleave", onLeave);
      window.removeEventListener("resize", measure);
    };
  }, []);

  return (
    <div
      ref={containerRef}
      aria-hidden
      className="pointer-events-none absolute inset-x-0 top-0 hidden h-[760px] overflow-visible select-none md:block"
    >
      {POSITIONS.map((p, i) => {
        const colors = COLORS[p.hue];
        const outerStyle: CSSProperties = {
          top: `${p.top}%`,
          left: `${p.left}%`,
          transform: `translate(-50%, -50%) rotate(${p.rotate}deg) scale(0)`,
          willChange: "transform, opacity",
          opacity: 0,
        };
        const textStyle: CSSProperties = {
          backgroundImage: colors.gradient,
          WebkitBackgroundClip: "text",
          backgroundClip: "text",
          color: "transparent",
        };
        return (
          <div
            key={p.prefix}
            ref={(el) => {
              chipRefs.current[i] = el;
            }}
            style={outerStyle}
            className="absolute"
          >
            <div className="rounded-full p-px " style={{ backgroundImage: colors.gradient }}>
              <div
                className="flex items-center gap-1 rounded-full bg-background px-2 py-1.5"
                //style={{ padding: "4px 10px 4px 7px" }}
              >
                <LockIcon color={colors.solid} className="size-3" />
                <div className="font-mono text-[12px] leading-none font-medium" style={textStyle}>
                  {p.prefix}
                  <span className="opacity-55" style={textStyle}>
                    .localify
                  </span>
                </div>
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
};
