"use client";

import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";

import { ChevronDownIcon, InfoCircleIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { bootstrapQueries } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { SettingsCard } from "./settings-card";
import {
  buildTroubleshootingIssues,
  type TroubleshootingIssue,
} from "./settings-troubleshooting.data";

export const SettingsTroubleshooting = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const [open, setOpen] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);

  const issues = useMemo(
    () => (data ? buildTroubleshootingIssues(data) : []),
    [data],
  );

  if (!data) return null;

  return (
    <>
      <SettingsCard
        icon={<InfoCircleIcon />}
        title="Troubleshooting"
        description="Known Localify issues, how to recognize them, and what to try next."
        action={
          <Button size="lg" variant="outline" onClick={() => setOpen(true)}>
            Open guide
          </Button>
        }
      />

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-h-[calc(100vh-2rem)] overflow-y-auto overscroll-none sm:max-w-xl">
          <DialogHeader>
            <DialogTitle>Troubleshooting</DialogTitle>
            <DialogDescription>
              Common Localify situations, how to recognize them, and what to try next.
            </DialogDescription>
          </DialogHeader>
          <DialogBody className="pb-4">
            <ul className="divide-y divide-[--color-border]/50 rounded-xl border border-[--color-border]/60">
              {issues.map((issue) => (
                <FaqRow
                  key={issue.id}
                  issue={issue}
                  open={expanded === issue.id}
                  onToggle={() =>
                    setExpanded((current) => (current === issue.id ? null : issue.id))
                  }
                />
              ))}
            </ul>
          </DialogBody>
          <DialogFooter showCloseButton />
        </DialogContent>
      </Dialog>
    </>
  );
};

interface FaqRowProps {
  issue: TroubleshootingIssue;
  open: boolean;
  onToggle: () => void;
}

const FaqRow = ({ issue, open, onToggle }: FaqRowProps) => {
  const panelId = `troubleshoot-${issue.id}`;

  return (
    <li>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={panelId}
        onClick={onToggle}
        className="flex w-full items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-[--color-border]/15 focus-visible:bg-[--color-border]/20 focus-visible:outline-none"
      >
        <span className="min-w-0 flex-1 text-sm font-medium text-[--color-ink]">
          {issue.title}
        </span>
        <ChevronDownIcon
          className={cn(
            "size-4 shrink-0 text-[--color-muted]/70 transition-transform duration-300 ease-out",
            open && "rotate-180",
          )}
        />
      </button>

      <div
        id={panelId}
        role="region"
        className={cn(
          "grid transition-[grid-template-rows] duration-300 ease-out",
          open ? "grid-rows-[1fr]" : "grid-rows-[0fr]",
        )}
      >
        <div className="overflow-hidden">
          <div className="space-y-3 px-4 pt-1 pb-4 text-sm text-[--color-muted]">
            <p>{issue.summary}</p>

            {issue.symptoms.length > 0 ? <BulletList items={issue.symptoms} /> : null}
            <BulletList items={issue.solutions} />

            {issue.commands?.length ? (
              <div className="space-y-1.5 pt-1">
                {issue.commands.map((command) => (
                  <code
                    key={command}
                    className="block overflow-x-auto rounded-md bg-[--color-border]/25 px-2.5 py-1.5 font-mono text-[11px] text-[--color-ink]"
                  >
                    {command}
                  </code>
                ))}
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </li>
  );
};

const BulletList = ({ items }: { items: string[] }) => (
  <ul className="space-y-1">
    {items.map((item) => (
      <li key={item} className="flex gap-2">
        <span className="mt-[7px] size-1 shrink-0 rounded-full bg-[--color-muted]/60" />
        <span>{item}</span>
      </li>
    ))}
  </ul>
);
