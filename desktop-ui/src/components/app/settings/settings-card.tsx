"use client";

import { motion } from "motion/react";
import type { ComponentPropsWithoutRef, ReactNode } from "react";

import { cn } from "@/lib/utils";

const MotionDiv = motion.create("div");

type SettingsCardProps = Omit<
  ComponentPropsWithoutRef<typeof MotionDiv>,
  "title" | "children"
> & {
  icon?: ReactNode;
  title: ReactNode;
  description?: ReactNode;
  action?: ReactNode;
  trailing?: ReactNode;
  children?: ReactNode;
  tone?: "default" | "danger";
  delay?: number;
};

export const SettingsCard = ({
  icon,
  title,
  description,
  action,
  trailing,
  children,
  tone = "default",
  delay = 0,
  className,
  ...rest
}: SettingsCardProps) => {
  return (
    <MotionDiv
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: "spring", duration: 0.5, bounce: 0, delay }}
      className={cn(
        "relative isolate overflow-hidden rounded-2xl border px-6 py-5 shadow-[0_1px_0_0_rgba(255,255,255,0.6)_inset] transition-colors duration-200 dark:shadow-[0_1px_0_0_rgba(255,255,255,0.04)_inset]",
        tone === "default" &&
          "border-border/40 bg-neutral-50/80 hover:border-border dark:bg-neutral-900/50",
        tone === "danger" &&
          "border-destructive/20 bg-destructive/4 hover:border-destructive/40 dark:bg-destructive/6",
        className,
      )}
      {...rest}
    >
      <div className="flex items-center gap-5">
        {icon ? (
          <span
            aria-hidden
            className={cn(
              "flex size-10 shrink-0 items-center justify-center rounded-full ring-1 [&>svg]:size-4",
              tone === "default"
                ? "bg-background text-foreground/80 ring-border/60"
                : "bg-destructive/10 text-destructive ring-destructive/20",
            )}
          >
            {icon}
          </span>
        ) : null}
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <h3
            className={cn(
              "text-[17px] font-semibold tracking-tight",
              tone === "danger" ? "text-destructive" : "text-foreground",
            )}
          >
            {title}
          </h3>
          {description ? (
            <p className="max-w-md text-[13.5px] leading-normal text-pretty text-muted-foreground">
              {description}
            </p>
          ) : null}
        </div>
        {trailing}
        {action}
      </div>
      {children}
    </MotionDiv>
  );
};
