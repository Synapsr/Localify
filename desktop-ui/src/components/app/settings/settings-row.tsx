"use client";

interface SettingsRowProps {
  label: string;
  value: string;
  mono?: boolean;
}

export const SettingsRow = ({ label, value, mono }: SettingsRowProps) => {
  return (
    <div className="min-w-0 space-y-0.5">
      <dt className="text-xs text-[--color-muted]">{label}</dt>
      <dd
        className={
          mono
            ? "truncate font-mono text-xs text-[--color-ink]"
            : "truncate text-sm text-[--color-ink]"
        }
      >
        {value}
      </dd>
    </div>
  );
};
