const downloadsBaseUrl =
  process.env.NEXT_PUBLIC_LOCALIFY_DOWNLOADS_BASE_URL?.replace(/\/+$/u, "") ??
  "https://downloads.localify.dev";

export const MACOS_APP_DOWNLOAD_URL = `${downloadsBaseUrl}/latest/localify.dmg`;

/** Installs or updates the command line: `public/install.sh`, served by this site. */
export const CLI_INSTALL_COMMAND = "curl -fsSL https://localify.dev/install.sh | sh";
