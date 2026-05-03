import { PropsWithChildren, useState } from "react";
import { MutationCache, QueryCache, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ThemeProvider } from "next-themes";
import { Toaster } from "sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { captureDesktopError } from "@/lib/analytics";
import { HotkeysProvider } from "@/providers/hotkeys-provider";

const createQueryClient = () =>
  new QueryClient({
    queryCache: new QueryCache({
      onError: (error) => {
        captureDesktopError(error, { source: "react_query" });
      },
    }),
    mutationCache: new MutationCache({
      onError: (error) => {
        captureDesktopError(error, { source: "react_mutation" });
      },
    }),
    defaultOptions: {
      queries: {
        retry: 1,
        refetchOnWindowFocus: false,
      },
    },
  });

export const SharedProvider = ({ children }: PropsWithChildren) => {
  const [queryClient] = useState(createQueryClient);

  return (
    <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
      <HotkeysProvider>
        <QueryClientProvider client={queryClient}>
          <TooltipProvider>{children}</TooltipProvider>
          <Toaster closeButton position="bottom-right" richColors />
        </QueryClientProvider>
      </HotkeysProvider>
    </ThemeProvider>
  );
};
