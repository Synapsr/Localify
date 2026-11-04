import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { bootstrapQueries, systemMutations } from "@/lib/queries";

/** The install leaves certificate trust for later, so this is where people finish it. */
export const HttpsTrustNotice = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const trust = useMutation(systemMutations.trustCa(queryClient));

  if (!data?.daemon.caTrustPending) return null;

  async function handleTrust() {
    try {
      await trust.mutateAsync();
      toast.success("HTTPS is trusted", {
        description: "If a browser still warns, quit and reopen it.",
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      toast.error("HTTPS is not trusted yet", { description: message });
    }
  }

  return (
    <div className="border-t border-[--color-border] bg-[--color-surface]">
      <div className="mx-auto flex w-full max-w-4xl items-center justify-between gap-4 px-8 py-2">
        <p className="text-xs text-[--color-muted]">
          Browsers warn about HTTPS until you confirm this certificate in macOS. HTTP and Node.js
          already work.
        </p>
        <Button size="sm" variant="secondary" loading={trust.isPending} onClick={handleTrust}>
          Trust HTTPS
        </Button>
      </div>
    </div>
  );
};
