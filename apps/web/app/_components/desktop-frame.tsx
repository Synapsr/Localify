"use client";

import { motion, useMotionValue, useReducedMotion, useSpring, useTransform } from "motion/react";
import { useEffect, useRef, type ReactNode } from "react";

export const DesktopFrame = ({ children }: { children: ReactNode }) => {
  const ref = useRef<HTMLDivElement>(null);
  const prefersReduced = useReducedMotion();
  const progress = useMotionValue(0);

  useEffect(() => {
    const section = ref.current?.closest("section") as HTMLElement | null;
    if (!section) return;

    const update = () => {
      const vh = window.innerHeight;
      const top = section.offsetTop;
      const height = section.offsetHeight;
      // 0 when section's top enters the viewport bottom,
      // 1 when section's center reaches the viewport center.
      const startScroll = top - vh;
      const endScroll = top + height / 2 - vh / 2;
      const range = endScroll - startScroll || 1;
      const p = (window.scrollY - startScroll) / range;
      progress.set(Math.max(0, Math.min(1, p)));
    };

    update();
    window.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);
    return () => {
      window.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
    };
  }, [progress]);

  const smooth = useSpring(progress, {
    stiffness: 140,
    damping: 28,
    mass: 0.4,
    restDelta: 0.001,
  });

  const rotateX = useTransform(smooth, [0, 1], prefersReduced ? [0, 0] : [12, 0]);
  const scale = useTransform(smooth, [0, 1], prefersReduced ? [1, 1] : [0.94, 1]);
  const translateY = useTransform(smooth, [0, 1], prefersReduced ? [0, 0] : [40, 0]);

  return (
    <motion.div
      ref={ref}
      style={{
        rotateX,
        scale,
        y: translateY,
        transformPerspective: 1600,
        transformOrigin: "50% 100%",
        aspectRatio: "1440 / 900",
        backgroundImage: "url(/mac-background.jpg)",
        backgroundSize: "contain",
        backgroundPosition: "center",
      }}
      className="relative overflow-hidden rounded-2xl border border-border shadow-[0_1px_0_rgba(13,13,13,0.03),0_60px_120px_-60px_rgba(13,13,13,0.28)]"
    >
      <div className="flex h-full items-center justify-center px-[10%] md:px-12">{children}</div>
    </motion.div>
  );
};
