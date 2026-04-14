import type { HTMLAttributes } from "react";
import { cn } from "../../lib/cn";

type Tone = "neutral" | "accent" | "muted" | "danger";

interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  tone?: Tone;
}

const tones: Record<Tone, string> = {
  neutral:
    "bg-[--color-surface] text-[--color-ink] border border-[--color-border]",
  accent: "bg-[--color-accent]/15 text-[--color-accent] border border-transparent",
  muted: "bg-transparent text-[--color-muted] border border-[--color-border]",
  danger: "bg-[--color-danger]/10 text-[--color-danger] border border-transparent",
};

export function Badge({ tone = "neutral", className, ...rest }: BadgeProps) {
  return (
    <span
      {...rest}
      className={cn(
        "inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-medium",
        tones[tone],
        className,
      )}
    />
  );
}
