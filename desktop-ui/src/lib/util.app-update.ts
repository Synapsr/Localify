import type { TerminalSessionDto, UpdateStatusDto } from "../types/localify.ts";

/**
 * What `Restart to update` closes, by name: the service runs and shells the app's tabs hold.
 * Services started in the background belong to the daemon and keep running.
 */
export interface RestartCloses {
  services: string[];
  shells: string[];
}

export function restartCloses(sessions: TerminalSessionDto[]): RestartCloses {
  const services: string[] = [];
  const shells: string[] = [];

  for (const session of sessions) {
    if (session.status !== "running") continue;
    const place = placeOf(session);
    if (!session.run) {
      shells.push(`Shell in ${place}`);
    } else if (!session.run.ended) {
      services.push(`${session.target.serviceName ?? "Service"} in ${place}`);
    }
  }

  return { services, shells };
}

// A worktree's tab names its checkout; the primary one needs no label.
function placeOf(session: TerminalSessionDto) {
  const { environmentName, environmentSlug, projectName } = session.target;
  return environmentSlug === "primary" ? projectName : `${projectName} (${environmentName})`;
}

export interface UpdateSummary {
  title: string;
  /**
   * Why it waits, why the check failed, where the bundle the last install replaced was left when
   * it couldn't go to `previous/`, or when the last check ran.
   */
  detail: string | null;
  /** Offer `Restart to update`. */
  canRestart: boolean;
}

/** The status in words, for Settings. `now` is in Unix seconds. */
export function describeUpdate(
  status: UpdateStatusDto,
  appName: string,
  now: number,
): UpdateSummary {
  const lastCheck = lastCheckLine(status.lastCheckAt, now);
  const next = status.available ? `${appName} ${status.available}` : null;

  switch (status.state) {
    case "available": {
      const canRestart = status.canInstall && status.available !== null;
      return {
        title: next ? `${next} is available` : "An update is available",
        detail: status.reason ?? (canRestart ? "Restart to update when you're ready." : lastCheck),
        canRestart,
      };
    }
    case "checking":
      return { title: "Checking for updates…", detail: lastCheck, canRestart: false };
    case "installing":
      return {
        title: `Installing ${next ?? "an update"}…`,
        detail: null,
        canRestart: false,
      };
    case "failed":
      return { title: "The last check failed", detail: status.reason, canRestart: false };
    case "upToDate":
      return {
        title: `${appName} ${status.installed} is up to date`,
        detail: status.reason ?? lastCheck,
        canRestart: false,
      };
    case "unknown":
      return {
        title: `${appName} ${status.installed}`,
        detail: status.reason ?? lastCheck,
        canRestart: false,
      };
  }
}

export function lastCheckLine(lastCheckAt: number | null, now: number): string {
  if (lastCheckAt === null) return "Not checked yet.";
  const minutes = Math.floor(Math.max(0, now - lastCheckAt) / 60);
  if (minutes < 1) return "Last checked just now.";
  if (minutes < 60) return `Last checked ${plural(minutes, "minute")} ago.`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `Last checked ${plural(hours, "hour")} ago.`;
  return `Last checked ${plural(Math.floor(hours / 24), "day")} ago.`;
}

function plural(count: number, unit: string) {
  return `${count} ${unit}${count === 1 ? "" : "s"}`;
}
