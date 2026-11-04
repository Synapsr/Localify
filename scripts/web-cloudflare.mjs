#!/usr/bin/env node

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseEnv } from "node:util";

const rootDir = resolve(import.meta.dirname, "..");
const appDir = join(rootDir, "apps", "web");
const command = process.argv[2];
const extraArgs = process.argv.slice(3).filter((value) => value !== "--");
const maskDir = join(rootDir, ".tmp", "cloudflare-env-mask");
const manifestPath = join(maskDir, "manifest.json");
const publicEnvFromFiles = readPublicEnvFromFiles();
let activeChild;
let maskedFiles = [];

const commandArgs = {
  build: ["build"],
  deploy: ["deploy", "--", "--keep-vars"],
  preview: ["preview"],
  upload: ["upload", "--", "--keep-vars"],
};

if (!command || !(command in commandArgs)) {
  fail("Usage: node scripts/web-cloudflare.mjs <build|preview|deploy|upload> [args...]");
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    if (activeChild) {
      activeChild.kill(signal);
    }
    restoreEnvFiles(maskedFiles);
    process.exit(signal === "SIGINT" ? 130 : 143);
  });
}

restoreStaleEnvFiles();
maskedFiles = maskEnvFiles();
try {
  if (command === "deploy" || command === "upload" || command === "preview") {
    await run("pnpm", ["--filter", "@localify/web", "exec", "opennextjs-cloudflare", "build"]);
  }
  await run("pnpm", [
    "--filter",
    "@localify/web",
    "exec",
    "opennextjs-cloudflare",
    ...commandArgs[command],
    ...extraArgs,
  ]);
} finally {
  restoreEnvFiles(maskedFiles);
}

function maskEnvFiles() {
  const candidates = [
    ".env",
    ".env.local",
    ".env.production",
    ".env.production.local",
    ".env.development",
    ".env.development.local",
    ".env.test",
    ".env.test.local",
  ];
  const directories = [rootDir, appDir];
  const masked = [];

  rmSync(maskDir, { recursive: true, force: true });
  mkdirSync(maskDir, { recursive: true });

  for (const directory of directories) {
    for (const fileName of candidates) {
      const sourcePath = join(directory, fileName);
      if (!existsSync(sourcePath)) {
        continue;
      }

      const token = `${masked.length}-${directory === rootDir ? "root" : "web"}-${fileName}`;
      const maskedPath = join(maskDir, token);
      renameSync(sourcePath, maskedPath);
      masked.push({ maskedPath, sourcePath });
      writeManifest(masked);
    }
  }

  return masked;
}

function restoreEnvFiles(maskedFiles) {
  for (const { maskedPath, sourcePath } of maskedFiles.reverse()) {
    if (existsSync(maskedPath)) {
      renameSync(maskedPath, sourcePath);
    }
  }

  rmSync(maskDir, { recursive: true, force: true });
}

function restoreStaleEnvFiles() {
  if (!existsSync(manifestPath)) {
    return;
  }

  const staleFiles = JSON.parse(readFileSync(manifestPath, "utf8"));
  restoreEnvFiles(staleFiles);
}

function writeManifest(maskedFiles) {
  writeFileSync(`${manifestPath}.tmp`, JSON.stringify(maskedFiles, null, 2));
  renameSync(`${manifestPath}.tmp`, manifestPath);
}

function run(commandName, args) {
  return new Promise((resolveRun, rejectRun) => {
    activeChild = spawn(commandName, args, {
      cwd: rootDir,
      env: buildCleanEnv(),
      stdio: "inherit",
    });

    activeChild.on("error", rejectRun);
    activeChild.on("exit", (code, signal) => {
      activeChild = undefined;
      if (code === 0) {
        resolveRun();
        return;
      }

      rejectRun(
        new Error(
          signal
            ? `${commandName} ${args.join(" ")} exited with ${signal}.`
            : `${commandName} ${args.join(" ")} failed.`,
        ),
      );
    });
  });
}

function buildCleanEnv() {
  const cleanEnv = {
    HOME: process.env.HOME,
    LANG: process.env.LANG,
    LC_ALL: process.env.LC_ALL,
    LOGNAME: process.env.LOGNAME,
    NEXT_TELEMETRY_DISABLED: "1",
    NODE_ENV: "production",
    PATH: process.env.PATH,
    PWD: rootDir,
    SHELL: process.env.SHELL,
    TMPDIR: process.env.TMPDIR,
    USER: process.env.USER,
    ...publicEnvFromFiles,
  };

  for (const [key, value] of Object.entries(process.env)) {
    if (
      key === "CLOUDFLARE_ACCOUNT_ID" ||
      key === "CLOUDFLARE_API_TOKEN" ||
      key === "WRANGLER_SEND_METRICS" ||
      key.startsWith("NEXT_PUBLIC_")
    ) {
      cleanEnv[key] = value;
    }
  }

  return Object.fromEntries(Object.entries(cleanEnv).filter(([, value]) => value !== undefined));
}

function readPublicEnvFromFiles() {
  const publicEnv = {};
  const productionEnvOrder = [".env", ".env.production", ".env.local", ".env.production.local"];

  for (const directory of [rootDir, appDir]) {
    for (const fileName of productionEnvOrder) {
      const envPath = join(directory, fileName);
      if (!existsSync(envPath)) {
        continue;
      }

      for (const [key, value] of Object.entries(parseEnv(readFileSync(envPath, "utf8")))) {
        if (key.startsWith("NEXT_PUBLIC_")) {
          publicEnv[key] = value;
        }
      }
    }
  }

  return publicEnv;
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
