import type { ErrorInfo, ReactNode } from "react";

import { Component } from "react";

import { DangerTriangleIcon, RefreshIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { captureDesktopError } from "@/lib/analytics";

interface DesktopErrorBoundaryProps {
  children: ReactNode;
}

interface DesktopErrorBoundaryState {
  error: Error | null;
}

export class DesktopErrorBoundary extends Component<
  DesktopErrorBoundaryProps,
  DesktopErrorBoundaryState
> {
  state: DesktopErrorBoundaryState = {
    error: null,
  };

  static getDerivedStateFromError(error: Error): DesktopErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    captureDesktopError(error, {
      source: "react_error_boundary",
      componentStack: errorInfo.componentStack,
    });
  }

  private reload = () => {
    window.location.reload();
  };

  render() {
    const { error } = this.state;

    if (!error) {
      return this.props.children;
    }

    return (
      <div
        data-tauri-drag-region
        className="flex h-full flex-col bg-neutral-50 p-1.5 pt-8 text-foreground dark:bg-neutral-950/40"
      >
        <div className="flex flex-1 items-center justify-center rounded-lg bg-background px-6 py-12">
          <div className="w-full max-w-md">
            <div className="flex flex-col items-center gap-5 text-center">
              <div className="flex size-12 items-center justify-center rounded-2xl border border-destructive/20 bg-destructive/10 text-destructive">
                <DangerTriangleIcon className="size-5" />
              </div>
              <div className="space-y-2">
                <p className="text-[11px] font-semibold tracking-[0.24em] text-muted-foreground uppercase">
                  Unexpected error
                </p>
                <h1 className="text-xl font-semibold tracking-tight">
                  Localify ran into a problem
                </h1>
                <p className="text-sm leading-6 text-muted-foreground">
                  The error was reported automatically. Reload the app to continue.
                </p>
              </div>
              {error.message ? (
                <div className="w-full rounded-xl border border-destructive/20 bg-destructive/5 px-4 py-3 text-left font-mono text-xs leading-5 wrap-break-word text-destructive">
                  {error.message}
                </div>
              ) : null}
              <Button type="button" onClick={this.reload}>
                <RefreshIcon />
                Reload app
              </Button>
            </div>
          </div>
        </div>
      </div>
    );
  }
}
