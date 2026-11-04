"use client";

import { Button as ButtonPrimitive } from "@base-ui/react/button";
import type { VariantProps } from "class-variance-authority";

import { SpinnerIcon } from "@/components/icons";
import { buttonVariants } from "@/components/ui/button-variants";
import { cn } from "@/lib/utils";

type ButtonProps = ButtonPrimitive.Props &
  VariantProps<typeof buttonVariants> & {
    loading?: boolean;
  };

export const Button = ({
  className,
  variant = "default",
  size = "default",
  loading = false,
  disabled,
  children,
  ...props
}: ButtonProps) => {
  return (
    <ButtonPrimitive
      aria-busy={loading || undefined}
      className={cn(buttonVariants({ variant, size, className }))}
      data-slot="button"
      disabled={disabled || loading}
      {...props}
    >
      {loading ? <SpinnerIcon className="animate-spin" data-icon="inline-start" /> : null}
      {children}
    </ButtonPrimitive>
  );
};
