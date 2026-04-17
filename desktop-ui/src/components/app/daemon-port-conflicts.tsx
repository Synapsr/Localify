"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { daemonMutations, daemonQueries } from "@/lib/queries";

interface DaemonPortConflictsProps {
  visible: boolean;
}

export const DaemonPortConflicts = ({
  visible,
}: DaemonPortConflictsProps) => {
  const queryClient = useQueryClient();
  const conflictsQuery = useQuery({
    ...daemonQueries.portConflicts(),
    enabled: visible,
  });
  const killConflict = useMutation(daemonMutations.killPortConflict(queryClient));
  const [armedPid, setArmedPid] = useState<number | null>(null);

  if (!visible) return null;

  const conflicts = conflictsQuery.data ?? [];
  const error =
    conflictsQuery.error instanceof Error ? conflictsQuery.error.message : null;
  const successMessage =
    killConflict.data
      ? `Sent SIGTERM to PID ${killConflict.data.pid} on ${conflictLabel(killConflict.data)}.`
      : null;

  return (
    <Card className="border-[--color-danger]/20 bg-[--color-danger]/5 p-4">
      <div className="space-y-3">
        <div className="space-y-1">
          <h3 className="text-sm font-semibold text-[--color-ink]">
            Blocking Process
          </h3>
          <p className="text-xs text-[--color-muted]">
            {conflicts.length > 0
              ? "Localify cannot start until these ports are freed. The action below sends SIGTERM to the selected process."
              : "Localify reported a port conflict, but no current listener was found on the configured ports."}
          </p>
        </div>

        {error ? <p className="text-xs text-[--color-danger]">{error}</p> : null}
        {successMessage ? (
          <p className="text-xs text-emerald-600">{successMessage}</p>
        ) : null}
        {killConflict.error instanceof Error ? (
          <p className="text-xs text-[--color-danger]">
            {killConflict.error.message}
          </p>
        ) : null}

        <div className="space-y-2">
          {conflicts.map((conflict) => {
            const isArmed = armedPid === conflict.pid;
            const isLoading =
              killConflict.isPending && killConflict.variables?.pid === conflict.pid;

            return (
              <div
                key={`${conflict.protocol}-${conflict.port}-${conflict.pid}`}
                className="rounded-lg border border-[--color-border] bg-[--color-surface] p-3"
              >
                <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                  <div className="min-w-0 space-y-1">
                    <p className="text-sm font-medium text-[--color-ink]">
                      {conflictLabel(conflict)}
                    </p>
                    <p className="text-xs text-[--color-muted]">
                      PID {conflict.pid}
                      {conflict.user ? ` • ${conflict.user}` : ""}
                      {conflict.processName ? ` • ${conflict.processName}` : ""}
                    </p>
                    <code className="block overflow-x-auto rounded bg-[--color-border]/30 px-2 py-1 font-mono text-[11px] text-[--color-ink]">
                      {conflict.command}
                    </code>
                  </div>

                  <div className="flex shrink-0 gap-2">
                    {isArmed ? (
                      <>
                        <Button
                          variant="ghost"
                          size="sm"
                          disabled={isLoading}
                          onClick={() => setArmedPid(null)}
                        >
                          Cancel
                        </Button>
                        <Button
                          variant="danger"
                          size="sm"
                          loading={isLoading}
                          onClick={() =>
                            killConflict.mutate({
                              protocol: conflict.protocol,
                              port: conflict.port,
                              pid: conflict.pid,
                            })
                          }
                        >
                          Kill PID {conflict.pid}
                        </Button>
                      </>
                    ) : (
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => setArmedPid(conflict.pid)}
                      >
                        Stop process
                      </Button>
                    )}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </Card>
  );
};

const conflictLabel = ({
  protocol,
  port,
}: {
  protocol: "http" | "https" | "dns";
  port: number;
}) => `${protocol.toUpperCase()} port ${port}`;
