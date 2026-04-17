import { useStore } from "@tanstack/react-form";
import type { ReactNode } from "react";

import { Input } from "@/components/ui/input";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupInput,
} from "@/components/ui/input-group";
import { Label } from "@/components/ui/label";
import { useFieldContext } from "@/hooks/form/form";
import { cn } from "@/lib/cn";

interface TextFieldProps {
  label: string;
  placeholder?: string;
  description?: string | ((value: string) => string);
  rightAction?: ReactNode;
  inputSuffix?: ReactNode;
  autoFocus?: boolean;
  className?: string;
}

export const TextField = ({
  label,
  placeholder,
  description,
  rightAction,
  inputSuffix,
  autoFocus,
  className,
}: TextFieldProps) => {
  const field = useFieldContext<string>();
  const value = useStore(field.store, (state) => state.value ?? "");
  const errors = useStore(field.store, (state) => state.meta.errors);
  const fieldId = `field-${field.name}`;
  const errorId = `${fieldId}-error`;
  const descriptionId = `${fieldId}-description`;
  const errorMessage = formatError(errors);
  const hasError = Boolean(errorMessage);
  const descriptionText = typeof description === "function" ? description(value) : description;

  const describedBy = [
    hasError ? errorId : null,
    !hasError && descriptionText ? descriptionId : null,
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div className={cn("space-y-1.5", className)}>
      <div className="flex items-center justify-between gap-3">
        <Label htmlFor={fieldId}>{label}</Label>
        {rightAction}
      </div>
      {inputSuffix !== undefined ? (
        <InputGroup>
          <InputGroupInput
            id={fieldId}
            value={value}
            onChange={(event) => field.handleChange(event.target.value)}
            onBlur={field.handleBlur}
            placeholder={placeholder}
            autoFocus={autoFocus}
            spellCheck={false}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            aria-invalid={hasError || undefined}
            aria-describedby={describedBy.length > 0 ? describedBy : undefined}
          />
          <InputGroupAddon align="inline-end">{inputSuffix}</InputGroupAddon>
        </InputGroup>
      ) : (
        <Input
          id={fieldId}
          value={value}
          onChange={(event) => field.handleChange(event.target.value)}
          onBlur={field.handleBlur}
          placeholder={placeholder}
          autoFocus={autoFocus}
          spellCheck={false}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          aria-invalid={hasError || undefined}
          aria-describedby={describedBy.length > 0 ? describedBy : undefined}
        />
      )}
      {hasError ? (
        <p id={errorId} className="text-xs text-[--color-danger]">
          {errorMessage}
        </p>
      ) : descriptionText ? (
        <p id={descriptionId} className="font-mono text-xs text-[--color-muted]">
          {descriptionText}
        </p>
      ) : null}
    </div>
  );
};

function formatError(errors: unknown[]): string | null {
  for (const error of errors) {
    if (!error) continue;
    if (typeof error === "string") return error;
    if (typeof error === "object" && "message" in error) {
      const message = (error as { message?: unknown }).message;
      if (typeof message === "string" && message.length > 0) return message;
    }
  }
  return null;
}
