"use client";

import { motion, useReducedMotion, type Transition, type Variants } from "motion/react";

const restTransition: Transition = {
  duration: 0.35,
  ease: [0.22, 1, 0.36, 1],
};

const openTransition: Transition = {
  duration: 0.5,
  ease: [0.22, 1, 0.36, 1],
};

const pivotStyle = {
  transformOrigin: "50% 90%",
  transformBox: "view-box",
} as const;

export const ProjectsListEmptyStateIllustration = () => {
  const reduceMotion = useReducedMotion();

  const leftCardVariants: Variants = {
    rest: { rotate: -7, x: 0, y: 0, transition: restTransition },
    hover: {
      rotate: reduceMotion ? -7 : -22,
      x: reduceMotion ? 0 : -10,
      transition: openTransition,
    },
  };

  const rightCardVariants: Variants = {
    rest: { rotate: 6, x: 0, y: 0, transition: restTransition },
    hover: {
      rotate: reduceMotion ? 6 : 20,
      x: reduceMotion ? 0 : 10,
      y: reduceMotion ? 0 : -4,
      transition: openTransition,
    },
  };

  return (
    <svg
      viewBox="0 0 120 120"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden="true"
      className="size-28 text-muted-foreground"
    >
      <g fill="var(--color-background)" stroke="currentColor" strokeLinejoin="round">
        <motion.g variants={leftCardVariants} strokeDasharray="2" style={pivotStyle} opacity="0.35">
          <rect x="22" y="28" width="62" height="74" rx="8" strokeWidth="1.25" />
        </motion.g>

        <motion.g
          variants={rightCardVariants}
          strokeDasharray="2"
          style={pivotStyle}
          opacity="0.55"
        >
          <rect x="36" y="26" width="62" height="74" rx="8" strokeWidth="1.25" />
        </motion.g>

        <g>
          <rect
            x="29"
            y="29"
            width="62"
            height="74"
            rx="8"
            stroke="var(--color-accent)"
            strokeWidth="1.25"
          />
          <circle cx="40" cy="46" r="3" fill="var(--color-chart-2)" stroke="none" />
          <line
            x1="48"
            y1="46"
            x2="78"
            y2="46"
            stroke="var(--color-chart-2)"
            strokeWidth="1.25"
            strokeLinecap="round"
            opacity="0.7"
          />
          <line
            x1="40"
            y1="62"
            x2="80"
            y2="62"
            stroke="currentColor"
            strokeWidth="1.25"
            strokeLinecap="round"
            opacity="0.55"
          />
          <line
            x1="40"
            y1="72"
            x2="68"
            y2="72"
            stroke="currentColor"
            strokeWidth="1.25"
            strokeLinecap="round"
            opacity="0.55"
          />
          <line
            x1="40"
            y1="82"
            x2="74"
            y2="82"
            stroke="currentColor"
            strokeWidth="1.25"
            strokeLinecap="round"
            opacity="0.55"
          />
        </g>
      </g>
    </svg>
  );
};
