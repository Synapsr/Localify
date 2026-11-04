import type { ReactNode } from "react";

import * as motion from "motion/react-client";

export { SiteHeader } from "./site-header";

const fadeEase = [0.22, 1, 0.36, 1] as const;

type FadeInProps = {
  children: ReactNode;
  className?: string;
  delay?: number;
  distance?: number;
};

export const sectionClass = "site-width relative z-10 pb-28 md:pb-40";

export const SectionHeading = ({
  eyebrow,
  title,
  description,
  align = "left",
}: {
  eyebrow?: string;
  title: ReactNode;
  description?: ReactNode;
  align?: "left" | "center";
}) => {
  const alignClass = align === "center" ? "mx-auto text-center" : "";
  return (
    <div className={`max-w-4xl space-y-5 ${alignClass}`}>
      {eyebrow ? (
        <span className="text-muted-fg inline-flex items-center gap-2 font-mono text-[11px] tracking-[0.18em] uppercase">
          <span className="h-px w-6 bg-border" aria-hidden="true" />
          {eyebrow}
        </span>
      ) : null}
      <h2 className="text-heading-gradient text-[44px] leading-none font-semibold tracking-[-0.035em] text-balance md:text-[64px] lg:text-[72px]">
        {title}
      </h2>
      {description ? (
        <p
          className={`text-[17px] leading-[1.55] text-pretty text-muted-foreground md:text-[18px] ${
            align === "center" ? "mx-auto max-w-xl" : "max-w-xl"
          }`}
        >
          {description}
        </p>
      ) : null}
    </div>
  );
};

export const FadeIn = ({ children, className, delay = 0, distance = 14 }: FadeInProps) => {
  return (
    <motion.div
      className={className}
      initial={{ opacity: 0, y: distance }}
      transition={{ duration: 0.6, ease: fadeEase, delay }}
      viewport={{ once: true, amount: 0.15 }}
      whileInView={{ opacity: 1, y: 0 }}
    >
      {children}
    </motion.div>
  );
};
