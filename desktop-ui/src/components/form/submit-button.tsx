import type { ReactNode } from "react";

import { useFormContext } from "@/hooks/form/form";

interface SubmitButtonProps {
  children: ReactNode;
  pendingLabel?: string;
}

export const SubmitButton = ({
  children,
  pendingLabel = "Saving…",
}: SubmitButtonProps) => {
  const form = useFormContext();

  return (
    <form.Subscribe
      selector={(state) => ({
        canSubmit: state.canSubmit,
        isSubmitting: state.isSubmitting,
      })}
    >
      {({ canSubmit, isSubmitting }) => (
        <button
          type="submit"
          disabled={!canSubmit || isSubmitting}
          className="inline-flex h-9 items-center rounded-md border border-[--color-ink] bg-[--color-ink] px-3 text-sm font-medium text-[--color-bg] transition-colors hover:bg-[--color-ink]/90 disabled:cursor-not-allowed disabled:opacity-50"
        >
          {isSubmitting ? pendingLabel : children}
        </button>
      )}
    </form.Subscribe>
  );
};
