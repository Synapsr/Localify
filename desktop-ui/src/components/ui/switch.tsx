import { Switch as SwitchPrimitive } from "@base-ui/react/switch";

import { cn } from "@/lib/utils";

function Switch({ className, children, ...props }: SwitchPrimitive.Root.Props) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className={cn(
        "relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent bg-border/70 transition-colors duration-200 outline-none focus-visible:ring-3 focus-visible:ring-ring/50 data-checked:bg-primary data-disabled:cursor-not-allowed data-disabled:opacity-50",
        className,
      )}
      {...props}
    >
      {children ?? <SwitchThumb />}
    </SwitchPrimitive.Root>
  );
}

function SwitchThumb({ className, ...props }: SwitchPrimitive.Thumb.Props) {
  return (
    <SwitchPrimitive.Thumb
      data-slot="switch-thumb"
      className={cn(
        "pointer-events-none inline-block size-4 translate-x-0.5 rounded-full bg-background shadow-sm ring-1 ring-black/5 transition-transform duration-200 ease-out data-checked:translate-x-[18px]",
        className,
      )}
      {...props}
    />
  );
}

export { Switch, SwitchPrimitive, SwitchThumb };
