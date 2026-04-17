import { useFieldContext } from "@/hooks/form/form";
import { cn } from "@/lib/cn";

interface CheckboxFieldProps {
  label: string;
  description?: string;
  className?: string;
}

export const CheckboxField = ({
  label,
  description,
  className,
}: CheckboxFieldProps) => {
  const field = useFieldContext<boolean>();
  const fieldId = `field-${field.name}`;
  const descriptionId = `${fieldId}-description`;

  return (
    <div className={cn("space-y-1", className)}>
      <label className="flex items-center gap-2 text-sm text-[--color-muted]">
        <input
          id={fieldId}
          type="checkbox"
          checked={field.state.value ?? false}
          onChange={(event) => field.handleChange(event.target.checked)}
          onBlur={field.handleBlur}
          aria-describedby={description ? descriptionId : undefined}
          className="h-4 w-4 rounded border-[--color-border] accent-[--color-ink]"
        />
        {label}
      </label>
      {description && (
        <p id={descriptionId} className="pl-6 text-xs text-[--color-muted]">
          {description}
        </p>
      )}
    </div>
  );
};
