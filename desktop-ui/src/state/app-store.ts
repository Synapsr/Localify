import { createDesktopClient, type DesktopClient } from "../lib/tauri-client";
import type { BootstrapDto, LogsDto } from "../types/roxy";

export type ScreenId = "overview" | "domains" | "config" | "logs" | "api";

export interface AppState {
  screen: ScreenId;
  bootstrap: BootstrapDto | null;
  logs: LogsDto | null;
  loading: boolean;
  logsLoading: boolean;
  error: string | null;
  initialized: boolean;
}

type Listener = (state: AppState) => void;

const initialState: AppState = {
  screen: "overview",
  bootstrap: null,
  logs: null,
  loading: false,
  logsLoading: false,
  error: null,
  initialized: false,
};

export class AppStore {
  private state: AppState = initialState;
  private listeners = new Set<Listener>();

  constructor(private readonly client: DesktopClient = createDesktopClient()) {}

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    listener(this.snapshot());

    return () => {
      this.listeners.delete(listener);
    };
  }

  snapshot(): AppState {
    return { ...this.state };
  }

  setScreen(screen: ScreenId): void {
    this.patch({ screen });
  }

  clearError(): void {
    if (this.state.error) {
      this.patch({ error: null });
    }
  }

  async bootstrap(): Promise<void> {
    this.patch({ loading: true, error: null });

    try {
      const bootstrap = await this.client.getBootstrap();
      this.patch({
        bootstrap,
        loading: false,
        initialized: true,
      });
    } catch (error) {
      this.patch({
        loading: false,
        initialized: true,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  }

  async refreshLogs(lines = 120): Promise<void> {
    this.patch({ logsLoading: true, error: null });

    try {
      const logs = await this.client.readLogs({ lines, clear: false });
      this.patch({
        logs,
        logsLoading: false,
      });
    } catch (error) {
      this.patch({
        logsLoading: false,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  }

  private patch(partial: Partial<AppState>): void {
    this.state = { ...this.state, ...partial };
    const snapshot = this.snapshot();
    this.listeners.forEach((listener) => listener(snapshot));
  }
}
