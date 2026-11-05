// GitHub's device flow with the OAuth App's client ID alone. The Worker keeps
// no GitHub token: it reads the user's id and login, then drops it.

import { ApiError } from "./http";

const USER_AGENT = "localify-cloud";
const GRANT_TYPE = "urn:ietf:params:oauth:grant-type:device_code";

export type DeviceCode = {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  interval: number;
  expiresIn: number;
};

export type DevicePoll =
  | { kind: "token"; accessToken: string }
  | { kind: "pending" }
  | { kind: "slow_down"; interval: number | null }
  | { kind: "expired" }
  | { kind: "denied" }
  | { kind: "invalid_code" };

export type GitHubUser = { id: number; login: string };

export async function startDeviceFlow(clientId: string): Promise<DeviceCode> {
  const body = await postForm("https://github.com/login/device/code", { client_id: clientId });
  if (typeof body.error === "string") throw configurationError(body.error);
  const { device_code, user_code, verification_uri, interval, expires_in } = body;
  if (
    typeof device_code !== "string" ||
    typeof user_code !== "string" ||
    typeof verification_uri !== "string" ||
    typeof interval !== "number" ||
    typeof expires_in !== "number"
  ) {
    throw githubUnavailable();
  }
  return {
    deviceCode: device_code,
    userCode: user_code,
    verificationUri: verification_uri,
    interval,
    expiresIn: expires_in,
  };
}

export async function pollDeviceFlow(clientId: string, deviceCode: string): Promise<DevicePoll> {
  const body = await postForm("https://github.com/login/oauth/access_token", {
    client_id: clientId,
    device_code: deviceCode,
    grant_type: GRANT_TYPE,
  });
  if (typeof body.access_token === "string" && body.access_token !== "") {
    return { kind: "token", accessToken: body.access_token };
  }
  switch (body.error) {
    case "authorization_pending":
      return { kind: "pending" };
    case "slow_down":
      return {
        kind: "slow_down",
        interval: typeof body.interval === "number" ? body.interval : null,
      };
    case "expired_token":
      return { kind: "expired" };
    case "access_denied":
      return { kind: "denied" };
    case "incorrect_device_code":
      return { kind: "invalid_code" };
    case undefined:
      throw githubUnavailable();
    default:
      throw configurationError(String(body.error));
  }
}

export async function fetchGitHubUser(accessToken: string): Promise<GitHubUser> {
  const response = await githubFetch("https://api.github.com/user", {
    headers: {
      accept: "application/vnd.github+json",
      authorization: `Bearer ${accessToken}`,
      "user-agent": USER_AGENT,
      "x-github-api-version": "2022-11-28",
    },
  });
  if (!response.ok) throw githubUnavailable();
  const body = (await response.json().catch(() => null)) as {
    id?: unknown;
    login?: unknown;
  } | null;
  if (typeof body?.id !== "number" || typeof body.login !== "string" || body.login === "") {
    throw githubUnavailable();
  }
  return { id: body.id, login: body.login };
}

async function postForm(
  url: string,
  fields: Record<string, string>,
): Promise<Record<string, unknown>> {
  const response = await githubFetch(url, {
    method: "POST",
    headers: {
      accept: "application/json",
      "content-type": "application/x-www-form-urlencoded",
      "user-agent": USER_AGENT,
    },
    body: new URLSearchParams(fields).toString(),
  });
  const body = (await response.json().catch(() => null)) as unknown;
  if (typeof body !== "object" || body === null) throw githubUnavailable();
  return body as Record<string, unknown>;
}

async function githubFetch(url: string, init: RequestInit): Promise<Response> {
  try {
    return await fetch(url, init);
  } catch (error) {
    console.error("GitHub request failed", url, error);
    throw githubUnavailable();
  }
}

function githubUnavailable(): ApiError {
  return new ApiError(
    502,
    "github_unavailable",
    "GitHub did not answer as expected. Try again in a minute.",
  );
}

/** GitHub refused the OAuth App itself: an operator has to fix it. */
function configurationError(code: string): ApiError {
  console.error("GitHub refused the OAuth App", code);
  const message =
    code === "device_flow_disabled"
      ? "Device Flow is turned off for Localify Cloud's GitHub OAuth App. Tell the Localify team."
      : `GitHub refused Localify Cloud's OAuth App (${code}). Tell the Localify team.`;
  return new ApiError(500, "not_configured", message);
}
