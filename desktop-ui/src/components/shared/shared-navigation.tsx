import { CogFourSolidIcon, FileTextSolidIcon, HexagonSolidIcon } from "@/components/icons";
import { cn } from "@/lib/cn";
import { Link, useRouterState } from "@tanstack/react-router";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";

const tabs = [
  { to: "/projects", label: "Projects", icon: HexagonSolidIcon },
  { to: "/logs", label: "Logs", icon: FileTextSolidIcon },
  { to: "/settings", label: "Settings", icon: CogFourSolidIcon },
] as const;

export function SharedNavigation() {
  const {
    location: { pathname },
  } = useRouterState();

  return (
    <div className="fixed dark bottom-8 left-1/2 z-50 -translate-x-1/2 w-fit rounded-full bg-background text-foreground p-2">
      <div className="flex items-center gap-1">
        {tabs.map((tab) => {
          const active = pathname.startsWith(tab.to);
          return (
            <Tooltip key={tab.to}>
              <TooltipTrigger
                render={
                  <Link
                    to={tab.to}
                    className={cn(
                      "rounded-full p-2 text-sm font-medium transition-all duration-150 hover:bg-accent  hover:text-foreground/90",
                      active ? "stroke-3 text-foreground bg-accent" : "text-foreground/80",
                    )}
                  >
                    <tab.icon className={cn("size-6 ", active ? "stroke-2" : "stroke-2")} />
                  </Link>
                }
              ></TooltipTrigger>
              <TooltipContent side="bottom">
                <p>{tab.label}</p>
              </TooltipContent>
            </Tooltip>
          );
        })}
      </div>
    </div>
  );
}
