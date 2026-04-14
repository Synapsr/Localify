import type { ReactNode } from "react";

interface EmptyStateProps {
  title: string;
  description?: string;
  action?: ReactNode;
}

export function EmptyState({ title, description, action }: EmptyStateProps) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 rounded-xl border border-dashed border-[--color-border] bg-[--color-surface]/40 px-8 py-16 text-center">
      <h3 className="text-base font-semibold text-[--color-ink]">{title}</h3>
      {description && (
        <p className="max-w-sm text-sm text-[--color-muted]">{description}</p>
      )}
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}
