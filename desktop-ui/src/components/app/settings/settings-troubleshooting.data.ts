import type { BootstrapDto } from "@/types/roxy";

export type TroubleshootingIssueTone = "success" | "warning" | "info";

export type TroubleshootingAction =
  | "install"
  | "setupCliAccess"
  | "setupNodeTrust"
  | "startDaemon";

export interface TroubleshootingIssue {
  id: string;
  title: string;
  tone: TroubleshootingIssueTone;
  summary: string;
  symptoms: string[];
  solutions: string[];
  commands?: string[];
  action?: {
    id: TroubleshootingAction;
    label: string;
  };
}

export const buildTroubleshootingIssues = (
  data: BootstrapDto,
): TroubleshootingIssue[] => {
  const appName = data.environment.appName;
  const cliCommand = appName.toLowerCase();
  const nodeTrust = data.environment.nodeTrust;
  const caCertPath = nodeTrust.caCertPath || `${data.config.paths.dataDir}/ca.crt`;
  const setupComplete =
    data.environment.cliHelperReady &&
    data.environment.cliAccessReady &&
    data.daemon.caInstalled;

  return [
    {
      id: "browser-cert-authority-invalid",
      title: "Chrome shows net::ERR_CERT_AUTHORITY_INVALID",
      tone: "warning",
      summary:
        "This means the browser does not trust the certificate chain currently served for this .localify domain.",
      symptoms: [
        "Chrome shows “Your connection is not private”.",
        "The error details mention `net::ERR_CERT_AUTHORITY_INVALID`.",
        "The warning can appear for a local domain such as `porteportfolio.localify`.",
      ],
      solutions: [
        "Run the HTTPS repair so Localify refreshes macOS trust, regenerates local HTTPS certificates, and restarts the background service.",
        "Fully quit and reopen the browser after the repair so it reloads the macOS trust state.",
        "If the warning still appears after the repair and browser restart, contact support@localify.dev.",
      ],
      action: {
        id: "install",
        label: "Repair HTTPS",
      },
    },
    {
      id: "ca-trust",
      title: "Local HTTPS is trusted by macOS",
      tone: data.daemon.caInstalled ? "success" : "warning",
      summary: data.daemon.caInstalled
        ? `${appName}'s local certificate authority is trusted in this macOS account.`
        : `${appName}'s local certificate authority is not trusted yet, so .localify HTTPS will show certificate errors.`,
      symptoms: data.daemon.caInstalled
        ? ["Browsers and SSL-aware local clients can trust .localify certificates."]
        : [
            "Browsers show certificate warnings on .localify domains.",
            "HTTPS requests to local domains fail before the app even responds.",
          ],
      solutions: data.daemon.caInstalled
        ? [
            "If a browser still shows a warning, fully quit and reopen it so it reloads the macOS trust list.",
          ]
        : [
            "Run setup again so Localify can install its CA into your macOS keychain.",
            "After trust is installed, fully quit and reopen your browsers and developer tools.",
          ],
      action: data.daemon.caInstalled
        ? undefined
        : {
            id: "install",
            label: "Run setup again",
          },
    },
    {
      id: "cli-access",
      title: "Terminal command is available",
      tone: data.environment.cliAccessReady ? "success" : "warning",
      summary: data.environment.cliAccessReady
        ? `The ${cliCommand} command is already available from your shell.`
        : `The ${cliCommand} command is not installed in your shell PATH yet.`,
      symptoms: data.environment.cliAccessReady
        ? ["Scripts, terminals, and editor tasks can invoke the Localify CLI directly."]
        : [
            `Running ${cliCommand} returns “command not found”.`,
            "Editor tasks cannot find the Localify CLI.",
          ],
      solutions: data.environment.cliAccessReady
        ? [
            "If one terminal still cannot find the command, restart that shell or reopen your IDE.",
          ]
        : [
            "Install the CLI command from Settings so shells and IDE tasks can find it.",
            "Restart open terminals after installation if the command still is not picked up.",
          ],
      action: data.environment.cliAccessReady
        ? undefined
        : {
            id: "setupCliAccess",
            label: "Install CLI command",
          },
    },
    {
      id: "daemon",
      title: "Background service is running",
      tone: data.daemon.running ? "success" : setupComplete ? "warning" : "info",
      summary: data.daemon.running
        ? `${appName} is actively routing local domains on ports ${data.daemon.httpPort} and ${data.daemon.httpsPort}.`
        : setupComplete
          ? `${appName} is set up, but the background service is currently stopped.`
          : `${appName} still needs setup before the background service can route your local domains.`,
      symptoms: data.daemon.running
        ? ["Localify can answer .localify requests while the desktop app stays open."]
        : [
            `Localify domains do not resolve to your app even though your project is running on localhost.`,
            "Requests hang or fail because nothing is listening on Localify's routing ports.",
          ],
      solutions: data.daemon.running
        ? [
            "If a specific project still fails, check that the project target is running and reachable.",
          ]
        : setupComplete
          ? [
              "Start the background service again from Settings.",
              "If startup fails with a port conflict, use the blocking-process controls in the System section.",
            ]
          : [
              "Finish the initial Localify setup first, then start the background service.",
            ],
      action: data.daemon.running
        ? undefined
        : {
            id: setupComplete ? "startDaemon" : "install",
            label: setupComplete ? "Start service" : "Finish setup",
          },
    },
    {
      id: "node-trust",
      title: "Node.js rejects a trusted .localify certificate",
      tone: nodeTrust.ready ? "success" : "warning",
      summary: nodeTrust.ready
        ? "Localify already configured a Node.js trust integration for new terminals and restarted apps."
        : "Node can reject Localify certificates even when browsers already trust them, because Node does not always read the macOS trust store by default.",
      symptoms: nodeTrust.ready
        ? [
            "Recent Node.js terminals inherit Localify certificate trust automatically.",
            "If one app still fails, it was likely opened before the integration was enabled.",
          ]
        : [
            "Next.js or another Node dev server logs `UNABLE_TO_VERIFY_LEAF_SIGNATURE`.",
            "Server-side fetch calls fail against an https://*.localify URL while the browser opens it fine.",
          ],
      solutions: nodeTrust.ready
        ? [
            "Restart already-open terminals and IDEs so they inherit the managed Node.js environment.",
            "If a shell still fails, run Repair to rewrite the Localify Node.js trust integration.",
          ]
        : [
            "Enable Localify's Node.js trust integration so new terminals and restarted apps trust .localify certificates automatically.",
            "If needed, point Node directly at Localify's CA with NODE_EXTRA_CA_CERTS while debugging older environments.",
            "Restart the terminal or IDE after changing Node environment variables.",
          ],
      commands: [
        `NODE_USE_SYSTEM_CA=1 pnpm dev-localify`,
        `NODE_EXTRA_CA_CERTS="${caCertPath}" pnpm dev-localify`,
      ],
      action: nodeTrust.ready
        ? undefined
        : {
            id: "setupNodeTrust",
            label: "Repair Node trust",
          },
    },
    {
      id: "browser-cache",
      title: "A browser still shows a warning after setup",
      tone: "info",
      summary:
        "Browsers cache certificate trust state. After the first CA install, the browser process may need a full restart before it notices the new root CA.",
      symptoms: [
        "Setup completed successfully, but a browser still shows a lock warning.",
        "A browser tab keeps serving an older certificate state after trust was installed.",
      ],
      solutions: [
        "Quit the browser completely, then reopen it.",
        "If the warning persists, verify the domain resolves through Localify and that the app is serving the expected hostname.",
      ],
    },
    {
      id: "port-conflicts",
      title: "Port 80 or 443 is already in use",
      tone: "info",
      summary:
        "Another local process can block Localify from binding its routing ports, which prevents setup or startup from finishing cleanly.",
      symptoms: [
        "Setup fails with an “already in use” error.",
        "Starting the background service fails immediately.",
      ],
      solutions: [
        "Open the blocking-process panel in the System section and stop the conflicting process if appropriate.",
        "If the process should stay running, move that app to different ports and retry Localify.",
      ],
    },
  ];
};
