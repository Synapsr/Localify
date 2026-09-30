import type {
  BootstrapDto,
  LaunchProfileInfoDto,
  LaunchSessionDto,
  ProjectEnvironmentInfoDto,
  TerminalTargetRequest,
} from "@/types/localify";

/** Profile `localify dev` uses when none is chosen. It can never be dangerous. */
export const DEFAULT_LAUNCH_PROFILE = "default";

export interface ServiceLaunchProfiles {
  /** Profiles offered next to the launch action, without `default`. */
  profiles: LaunchProfileInfoDto[];
  /** Why `localify.toml` could not be read. */
  profilesError: string | null;
  /** Launch session the service is currently running with. */
  session: LaunchSessionDto | null;
}

const noLaunchProfiles: ServiceLaunchProfiles = {
  profiles: [],
  profilesError: null,
  session: null,
};

/** Launch profiles and running launch session of a service terminal target. */
export function serviceLaunchProfiles(
  data: BootstrapDto | undefined,
  target: TerminalTargetRequest | null,
): ServiceLaunchProfiles {
  const serviceId = target?.kind === "service" ? target.serviceId : null;
  const environment =
    target && serviceId
      ? data?.projectEnvironments.find(
          (candidate) =>
            candidate.projectId === target.projectId && candidate.slug === target.environmentSlug,
        )
      : undefined;

  if (!data || !environment || !serviceId) {
    return noLaunchProfiles;
  }

  return {
    profiles: environment.launchProfiles.filter(
      (profile) => profile.name !== DEFAULT_LAUNCH_PROFILE,
    ),
    profilesError: environment.launchProfilesError,
    session: findLaunchSession(data.launchSessions, environment, serviceId),
  };
}

/**
 * Sessions without an environment slug belong to the primary environment. If
 * several sessions match, a dangerous one wins so its warning is never hidden.
 */
function findLaunchSession(
  sessions: LaunchSessionDto[],
  environment: ProjectEnvironmentInfoDto,
  serviceId: string,
): LaunchSessionDto | null {
  const matches = sessions.filter(
    (session) =>
      session.projectId === environment.projectId &&
      session.serviceId === serviceId &&
      (session.environmentSlug === environment.slug ||
        (session.environmentSlug === null && environment.primary)),
  );

  return matches.find((session) => session.danger) ?? matches[0] ?? null;
}

/** A service running on the default profile gets no badge. */
export function hasLaunchSessionBadge(session: LaunchSessionDto): boolean {
  return session.danger || session.profile !== DEFAULT_LAUNCH_PROFILE;
}

export function describeLaunchSession(session: LaunchSessionDto): string {
  const running = session.danger
    ? `Running on the dangerous launch profile “${session.profile}” (production data).`
    : `Running on the launch profile “${session.profile}”.`;

  return session.expiresAt === null
    ? running
    : `${running} Stops at ${formatUnixTime(session.expiresAt)}.`;
}

function formatUnixTime(seconds: number): string {
  const date = new Date(seconds * 1000);
  const today = date.toDateString() === new Date().toDateString();

  return date.toLocaleString(
    undefined,
    today ? { timeStyle: "short" } : { dateStyle: "medium", timeStyle: "short" },
  );
}
