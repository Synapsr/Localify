import type { InputHTMLAttributes } from "react";
import { cn } from "../../lib/cn";

export function Input({
  className,
  ...rest
}: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      {...rest}
      className={cn(
        "h-9 w-full rounded-md border border-[--color-border] bg-[--color-surface] px-3 text-sm text-[--color-ink] placeholder:text-[--color-muted] focus:outline-none focus:border-[--color-border-strong] focus:ring-2 focus:ring-[--color-ink]/10",
        className,
      )}
    />
  );
}

export function Label({
  className,
  ...rest
}: React.LabelHTMLAttributes<HTMLLabelElement>) {
  return (
    <label
      {...rest}
      className={cn(
        "text-xs font-medium text-[--color-muted] uppercase tracking-wide",
        className,
      )}
    />
  );
}
