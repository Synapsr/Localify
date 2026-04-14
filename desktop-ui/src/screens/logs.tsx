import { useState } from "react";
import { Button } from "../components/ui/button";
import { Card } from "../components/ui/card";
import { useLogs } from "../lib/queries";

export function LogsScreen() {
  const [lines, setLines] = useState(200);
  const { data, isFetching, refetch } = useLogs({ lines, clear: false });

  return (
    <section className="space-y-6">
      <div className="flex items-end justify-between">
        <div className="space-y-1">
          <h1 className="text-xl font-semibold tracking-tight">Logs</h1>
          <p className="text-sm text-[--color-muted]">
            Last {lines} lines from the Roxy daemon.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <select
            value={lines}
            onChange={(e) => setLines(Number(e.target.value))}
            className="h-9 rounded-md border border-[--color-border] bg-[--color-surface] px-3 text-sm"
          >
            <option value={50}>50 lines</option>
            <option value={200}>200 lines</option>
            <option value={500}>500 lines</option>
            <option value={1000}>1000 lines</option>
          </select>
          <Button
            variant="secondary"
            loading={isFetching}
            onClick={() => refetch()}
          >
            Refresh
          </Button>
        </div>
      </div>

      <Card>
        {data?.exists ? (
          <pre className="max-h-[60vh] overflow-auto whitespace-pre-wrap p-4 font-mono text-xs text-[--color-ink]">
            {data.content || "No log lines yet."}
          </pre>
        ) : (
          <div className="p-8 text-center text-sm text-[--color-muted]">
            Log file not found at {data?.path ?? "unknown path"}.
          </div>
        )}
      </Card>
    </section>
  );
}
