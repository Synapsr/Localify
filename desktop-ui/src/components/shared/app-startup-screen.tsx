import { SharedLogo } from "@/components/shared/shared-logo";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

interface AppStartupScreenProps {
  eyebrow?: string;
  title: string;
  message: string;
  detail?: string | null;
  error?: string | null;
  primaryActionLabel?: string;
  secondaryActionLabel?: string;
  onPrimaryAction?: () => void;
  onSecondaryAction?: () => void;
}

export function AppStartupScreen({
  eyebrow,
  title,
  message,
  detail,
  error,
  primaryActionLabel,
  secondaryActionLabel,
  onPrimaryAction,
  onSecondaryAction,
}: AppStartupScreenProps) {
  return (
    <div className="flex h-full items-center justify-center px-6 py-12">
      <Card className="w-full max-w-xl overflow-hidden border-[--color-border-strong] bg-[--color-surface] shadow-[0_24px_80px_rgba(15,23,42,0.08)]">
        <div className="border-b border-[--color-border] bg-[linear-gradient(135deg,rgba(15,23,42,0.045),rgba(15,23,42,0.01))] px-6 py-5">
          <div className="flex items-center gap-3">
            <div className="flex size-11 items-center justify-center rounded-2xl border border-[--color-border] bg-[--color-bg] text-base font-semibold text-[--color-ink]">
              <SharedLogo />
            </div>
            <div className="space-y-1">
              <p className="text-[11px] font-semibold tracking-[0.24em] text-[--color-muted] uppercase">
                {eyebrow ?? "Desktop startup"}
              </p>
              <h1 className="text-lg font-semibold text-[--color-ink]">{title}</h1>
            </div>
          </div>
        </div>
        <div className="space-y-4 px-6 py-6">
          <p className="text-sm leading-6 text-[--color-ink]">{message}</p>
          {detail ? <p className="text-sm text-[--color-muted]">{detail}</p> : null}
          {error ? (
            <div className="rounded-xl border border-[--color-danger]/20 bg-[--color-danger]/5 px-4 py-3 text-sm text-[--color-danger]">
              {error}
            </div>
          ) : null}
          {primaryActionLabel || secondaryActionLabel ? (
            <div className="flex flex-wrap gap-2 pt-2">
              {primaryActionLabel && onPrimaryAction ? (
                <Button onClick={onPrimaryAction}>{primaryActionLabel}</Button>
              ) : null}
              {secondaryActionLabel && onSecondaryAction ? (
                <Button variant="outline" onClick={onSecondaryAction}>
                  {secondaryActionLabel}
                </Button>
              ) : null}
            </div>
          ) : null}
        </div>
      </Card>
    </div>
  );
}
