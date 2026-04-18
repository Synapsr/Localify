import { formatForDisplay } from "@tanstack/react-hotkeys";
import { Link, useRouterState } from "@tanstack/react-router";

import { cn } from "@/lib/cn";
import { useAppHotkeys } from "@/providers/hotkeys-provider";

import { Kbd } from "../ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";
import { sharedNavigationTabs } from "./shared-navigation.constants";

export function SharedNavigation() {
  const { hotkeys } = useAppHotkeys();
  const {
    location: { pathname },
  } = useRouterState();

  return (
    <div className="dark fixed bottom-4 left-1/2 z-50 w-fit -translate-x-1/2 rounded-full bg-background/90 p-2 text-foreground backdrop-blur-3xl dark:bg-accent/20">
      <div className="flex items-center gap-1">
        {sharedNavigationTabs.map((tab) => {
          const active = pathname.startsWith(tab.to);
          return (
            <Tooltip key={tab.to}>
              <TooltipTrigger
                render={
                  <Link
                    to={tab.to}
                    className={cn(
                      "rounded-full p-2 text-sm font-medium transition-all duration-150 hover:bg-accent  hover:text-foreground/90",
                      active ? "bg-accent stroke-3 text-foreground" : "text-foreground/80",
                    )}
                  >
                    <tab.icon className={cn("size-6 ", active ? "stroke-2" : "stroke-2")} />
                  </Link>
                }
              ></TooltipTrigger>
              <TooltipContent side="bottom" className="flex items-center gap-2">
                <span>{tab.label}</span>
                <Kbd>{formatForDisplay(hotkeys[tab.hotkeyId])}</Kbd>
              </TooltipContent>
            </Tooltip>
          );
        })}
      </div>
    </div>
  );
}
