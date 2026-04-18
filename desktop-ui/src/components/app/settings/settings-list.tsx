"use client";

import type { ComponentProps, ReactNode } from "react";

import { ChevronRightIcon } from "@/components/icons";
import { cn } from "@/lib/utils";

export const SettingsSection = ({
  label,
  icon,
  description,
  children,
}: {
  label?: string;
  icon?: ReactNode;
  description?: ReactNode;
  children: ReactNode;
}) => {
  return (
    <section className="space-y-2">
      {label ? (
        <div className="flex items-center gap-1.5 px-1">
          {icon ? <span className="text-[--color-muted] [&>svg]:size-3.5">{icon}</span> : null}
          <h2 className="text-[11px] font-medium tracking-wider text-[--color-muted] uppercase">
            {label}
          </h2>
        </div>
      ) : null}
      {description ? <p className="px-1 text-xs text-[--color-muted]">{description}</p> : null}
      {children}
    </section>
  );
};

export const SettingsGroup = ({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) => {
  return (
    <div
      className={cn(
        "divide-y divide-[--color-border]/50 overflow-hidden rounded-xl border border-[--color-border]/70 bg-neutral-50/60 dark:bg-neutral-800/60",
        className,
      )}
    >
      {children}
    </div>
  );
};

interface SettingsRowProps extends Omit<ComponentProps<"div">, "title"> {
  icon?: ReactNode;
  title: ReactNode;
  description?: ReactNode;
  value?: ReactNode;
  trailing?: ReactNode;
  onActivate?: () => void;
  disabled?: boolean;
  danger?: boolean;
}

export const SettingsRow = ({
  icon,
  title,
  description,
  value,
  trailing,
  onActivate,
  disabled,
  danger,
  className,
  ...rest
}: SettingsRowProps) => {
  const interactive = Boolean(onActivate);

  const content = (
    <>
      {icon ? (
        <span
          aria-hidden
          className={cn(
            "mt-0.5 flex size-5 shrink-0 items-center justify-center text-[--color-ink]/80 [&>svg]:size-4",
            danger && "text-[--color-danger]",
            disabled && "opacity-50",
          )}
        >
          {icon}
        </span>
      ) : null}
      <div className="min-w-0 flex-1 space-y-0.5">
        <div
          className={cn(
            "truncate text-sm font-medium text-[--color-ink]",
            danger && "text-[--color-danger]",
            disabled && "opacity-60",
          )}
        >
          {title}
        </div>
        {description ? <div className="text-xs text-[--color-muted]">{description}</div> : null}
      </div>
      {value ? <div className="shrink-0 truncate text-sm text-[--color-muted]">{value}</div> : null}
      {trailing ? <div className="shrink-0">{trailing}</div> : null}
      {interactive ? (
        <ChevronRightIcon className="size-4 shrink-0 text-[--color-muted]/70" />
      ) : null}
    </>
  );

  if (interactive) {
    return (
      <button
        type="button"
        onClick={onActivate}
        disabled={disabled}
        className={cn(
          "group flex w-full items-start gap-3 px-4 py-3 text-left transition-colors",
          "hover:bg-[--color-border]/20 focus-visible:bg-[--color-border]/20 focus-visible:outline-none",
          "disabled:pointer-events-none disabled:opacity-60",
          className,
        )}
      >
        {content}
      </button>
    );
  }

  return (
    <div {...rest} className={cn("flex items-start gap-3 px-4 py-3", className)}>
      {content}
    </div>
  );
};
