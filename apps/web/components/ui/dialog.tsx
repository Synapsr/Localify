"use client";

import { Dialog as DialogPrimitive } from "@base-ui/react/dialog";
import { RiCloseLine } from "@remixicon/react";
import * as React from "react";

import { ProgressiveBlur } from "@/components/progresive-blur";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

const useDialogScrollOverflow = () => {
  const footerRef = React.useRef<HTMLDivElement | null>(null);
  const [showFooterBlur, setShowFooterBlur] = React.useState(false);

  React.useEffect(() => {
    const footerElement = footerRef.current;
    if (!footerElement) {
      return;
    }

    const dialogContent = footerElement.closest<HTMLElement>('[data-slot="dialog-content"]');
    if (!dialogContent) {
      return;
    }

    const updateScrollState = () => {
      const hasScrollOverflow = dialogContent.scrollHeight > dialogContent.clientHeight;
      const isScrolledToBottom =
        dialogContent.scrollTop + dialogContent.clientHeight >= dialogContent.scrollHeight - 1;

      setShowFooterBlur(hasScrollOverflow && !isScrolledToBottom);
    };

    updateScrollState();

    const resizeObserver = new ResizeObserver(updateScrollState);
    resizeObserver.observe(dialogContent);
    const mutationObserver = new MutationObserver(updateScrollState);
    mutationObserver.observe(dialogContent, {
      childList: true,
      subtree: true,
      characterData: true,
      attributes: true,
    });
    dialogContent.addEventListener("scroll", updateScrollState, { passive: true });
    window.addEventListener("resize", updateScrollState);

    return () => {
      resizeObserver.disconnect();
      mutationObserver.disconnect();
      dialogContent.removeEventListener("scroll", updateScrollState);
      window.removeEventListener("resize", updateScrollState);
    };
  }, []);

  return { footerRef, showFooterBlur };
};

export const Dialog = (props: DialogPrimitive.Root.Props) => {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />;
};

export const DialogTrigger = (props: DialogPrimitive.Trigger.Props) => {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />;
};

export const DialogPortal = (props: DialogPrimitive.Portal.Props) => {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />;
};

export const DialogClose = (props: DialogPrimitive.Close.Props) => {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />;
};

export const DialogOverlay = ({
  className,
  ...props
}: DialogPrimitive.Backdrop.Props) => {
  return (
    <DialogPrimitive.Backdrop
      className={cn(
        "fixed inset-0 isolate z-50 bg-black/10 duration-100 supports-backdrop-filter:backdrop-blur-xs data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0",
        className,
      )}
      data-slot="dialog-overlay"
      {...props}
    />
  );
};

type DialogContentProps = DialogPrimitive.Popup.Props & {
  showCloseButton?: boolean;
};

export const DialogContent = ({
  className,
  children,
  showCloseButton = true,
  ...props
}: DialogContentProps) => {
  return (
    <DialogPortal>
      <DialogOverlay />
      <DialogPrimitive.Popup
        className={cn(
          "fixed top-1/2 left-1/2 z-50 grid w-full max-w-[calc(100%-2rem)] -translate-x-1/2 -translate-y-1/2 gap-6 rounded-xl bg-popover text-sm text-popover-foreground ring-1 ring-foreground/10 duration-100 outline-none [-webkit-app-region:no-drag] sm:max-w-md data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95",
          className,
        )}
        data-slot="dialog-content"
        {...props}
      >
        {children}
        {showCloseButton ? (
          <DialogPrimitive.Close
            data-slot="dialog-close"
            render={<Button className="absolute top-4 right-4" size="icon-sm" variant="ghost" />}
          >
            <RiCloseLine />
            <span className="sr-only">Close</span>
          </DialogPrimitive.Close>
        ) : null}
      </DialogPrimitive.Popup>
    </DialogPortal>
  );
};

export const DialogHeader = ({ className, ...props }: React.ComponentProps<"div">) => {
  return (
    <div
      className={cn("flex flex-col gap-2 p-6 pb-0", className)}
      data-slot="dialog-header"
      {...props}
    />
  );
};

export const DialogBody = ({ className, ...props }: React.ComponentProps<"div">) => {
  return <div className={cn("px-6", className)} data-slot="dialog-body" {...props} />;
};

type DialogFooterProps = React.ComponentProps<"div"> & {
  showCloseButton?: boolean;
};

export const DialogFooter = ({
  className,
  showCloseButton = false,
  children,
  ...props
}: DialogFooterProps) => {
  const { footerRef, showFooterBlur } = useDialogScrollOverflow();

  return (
    <div
      ref={footerRef}
      className={cn("sticky bottom-0 px-6 py-4", className)}
      data-slot="dialog-footer"
      {...props}
    >
      <div className="relative z-10 flex flex-col-reverse justify-between gap-2 sm:flex-row">
        {children}
      </div>
      {showFooterBlur ? (
        <ProgressiveBlur
          backgroundColor="var(--color-background)"
          className="absolute !bottom-0 left-0 h-4 w-full"
          height="150px"
          position="bottom"
        />
      ) : null}
      {showCloseButton ? (
        <DialogPrimitive.Close render={<Button variant="outline" />}>Close</DialogPrimitive.Close>
      ) : null}
    </div>
  );
};

export const DialogTitle = ({
  className,
  ...props
}: DialogPrimitive.Title.Props) => {
  return (
    <DialogPrimitive.Title
      className={cn("leading-none font-medium", className)}
      data-slot="dialog-title"
      {...props}
    />
  );
};

export const DialogDescription = ({
  className,
  ...props
}: DialogPrimitive.Description.Props) => {
  return (
    <DialogPrimitive.Description
      className={cn(
        "text-sm text-muted-foreground *:[a]:underline *:[a]:underline-offset-3 *:[a]:hover:text-foreground",
        className,
      )}
      data-slot="dialog-description"
      {...props}
    />
  );
};

export { DialogPrimitive };
