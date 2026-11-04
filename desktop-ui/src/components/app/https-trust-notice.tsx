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
    <div className="flex items-center justify-between gap-4 rounded-t-lg border-b border-amber-500/20 bg-amber-500/10 px-4 py-2">
      <p className="min-w-0 text-xs text-amber-700 dark:text-amber-300">
        Browsers warn about HTTPS until you confirm Localify's certificate in macOS. HTTP and
        Node.js already work.
      </p>
      <Button size="sm" variant="secondary" loading={trust.isPending} onClick={handleTrust}>
        Trust HTTPS
      </Button>
    </div>
  );
};
