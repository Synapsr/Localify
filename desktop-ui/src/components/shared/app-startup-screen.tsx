import { Button } from "@/components/ui/button";

interface AppStartupScreenProps {
  eyebrow?: string;
  title: string;
  message: string;
  detail?: string | null;
  error?: string | null;
  primaryActionLabel?: string;
  secondaryActionLabel?: string;
  primaryActionLoading?: boolean;
  secondaryActionLoading?: boolean;
  onPrimaryAction?: () => void;
  onSecondaryAction?: () => void;
}

export const AppStartupScreen = ({
  eyebrow,
  title,
  message,
  detail,
  error,
  primaryActionLabel,
  secondaryActionLabel,
  primaryActionLoading = false,
  secondaryActionLoading = false,
  onPrimaryAction,
  onSecondaryAction,
}: AppStartupScreenProps) => {
  return (
    <div className="screen-wrapper flex min-h-full items-center justify-center px-6 py-14">
      <section className="w-full max-w-2xl space-y-6">
        <div className="min-w-0 space-y-1.5">
          <p className="text-[11px] font-medium tracking-wider text-muted-foreground uppercase">
            {eyebrow ?? "Desktop startup"}
          </p>
          <h1 className="text-[22px] leading-tight font-semibold tracking-tight text-foreground">
            {title}
          </h1>
        </div>

        <div className="space-y-5 border-t border-border/70 pt-6">
          <p className="max-w-xl text-sm leading-6 text-foreground">{message}</p>
          {detail ? (
            <p className="max-w-xl text-sm leading-6 text-muted-foreground">{detail}</p>
          ) : null}
          {error ? (
            <div className="max-w-2xl border-l-2 border-destructive/50 bg-destructive/4 px-4 py-3 text-sm leading-6 text-foreground whitespace-pre-wrap dark:bg-destructive/6">
              {error}
            </div>
          ) : null}
          {primaryActionLabel || secondaryActionLabel ? (
            <div className="flex flex-wrap items-center gap-2 pt-1">
              {primaryActionLabel && onPrimaryAction ? (
                <Button size="lg" loading={primaryActionLoading} onClick={onPrimaryAction}>
                  {primaryActionLabel}
                </Button>
              ) : null}
              {secondaryActionLabel && onSecondaryAction ? (
                <Button
                  size="lg"
                  variant="outline"
                  loading={secondaryActionLoading}
                  onClick={onSecondaryAction}
                >
                  {secondaryActionLabel}
                </Button>
              ) : null}
            </div>
          ) : null}
        </div>
      </section>
    </div>
  );
};
