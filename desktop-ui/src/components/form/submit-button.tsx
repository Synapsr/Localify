import type { ReactNode } from "react";
import type { ComponentProps } from "react";

import { useFormContext } from "@/hooks/form/form";

import { Button } from "../ui/button";

interface SubmitButtonProps extends ComponentProps<typeof Button> {
  children: ReactNode;
  pendingLabel?: string;
}

export const SubmitButton = ({
  children,
  pendingLabel = "Saving…",
  ..._props
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
        <Button type="submit" disabled={!canSubmit} loading={isSubmitting}>
          {isSubmitting ? pendingLabel : children}
        </Button>
      )}
    </form.Subscribe>
  );
};
