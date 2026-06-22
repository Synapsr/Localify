#!/usr/bin/env node

import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const rootDir = resolve(import.meta.dirname, "..");
loadDotEnv(join(rootDir, ".env"));

const args = parseArgs(process.argv.slice(2).filter((value) => value !== "--"));
const packageJson = JSON.parse(readFileSync(join(rootDir, "package.json"), "utf8"));
const currentVersion = String(packageJson.version ?? "").trim();
const nextVersion = args.version ?? incrementVersion(currentVersion, args.bump);

assertVersion(nextVersion);

if (args.dryRun) {
  console.log(`Current desktop version: ${currentVersion}`);
  console.log(`Next desktop version: ${nextVersion}`);
  console.log("Dry run only. No files will be changed, no build will run, and nothing will be uploaded.");
  console.log(`pnpm version:desktop -- ${nextVersion}`);
  console.log("pnpm tauri:build");
  console.log("pnpm publish:update:r2");
  process.exit(0);
}

requireEnv("LOCALIFY_UPDATER_BASE_URL");
requireEnv("LOCALIFY_R2_BUCKET");

console.log(`Publishing Localify Desktop ${currentVersion} -> ${nextVersion}`);

run("pnpm", ["version:desktop", "--", nextVersion]);
run("pnpm", ["tauri:build"]);
run("pnpm", ["publish:update:r2"]);

console.log(`Published Localify Desktop ${nextVersion}`);

function parseArgs(rawArgs) {
  const parsed = {
    bump: "patch",
    dryRun: false,
    version: undefined,
  };

  for (let index = 0; index < rawArgs.length; index += 1) {
    const arg = rawArgs[index];

    if (arg === "--dry-run") {
      parsed.dryRun = true;
      continue;
    }

    if (arg === "--version") {
      parsed.version = readOptionValue(rawArgs, index, arg);
      index += 1;
      continue;
    }

    if (arg.startsWith("--version=")) {
      parsed.version = arg.slice("--version=".length);
      continue;
    }

    if (arg === "--bump") {
      parsed.bump = readOptionValue(rawArgs, index, arg);
      index += 1;
      continue;
    }

    if (arg.startsWith("--bump=")) {
      parsed.bump = arg.slice("--bump=".length);
      continue;
    }

    if (!arg.startsWith("-") && !parsed.version) {
      parsed.version = arg;
      continue;
    }

    fail(`Unknown option: ${arg}`);
  }

  if (!["patch", "minor", "major"].includes(parsed.bump)) {
    fail(`Invalid bump type: ${parsed.bump}. Expected patch, minor, or major.`);
  }

  return parsed;
}

function readOptionValue(rawArgs, index, optionName) {
  const value = rawArgs[index + 1]?.trim();
  if (!value || value.startsWith("-")) {
    fail(`Missing value for ${optionName}.`);
  }
  return value;
}

function incrementVersion(version, bump) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (!match) {
    fail(`Cannot auto-bump non-stable version: ${version}. Pass --version <version> explicitly.`);
  }

  const major = Number(match[1]);
  const minor = Number(match[2]);
  const patch = Number(match[3]);

  if (bump === "major") {
    return `${major + 1}.0.0`;
  }

  if (bump === "minor") {
    return `${major}.${minor + 1}.0`;
  }

  return `${major}.${minor}.${patch + 1}`;
}

function assertVersion(version) {
  if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
    fail(`Invalid desktop version: ${version}`);
  }
}

function requireEnv(name) {
  if (!process.env[name]?.trim()) {
    fail(`${name} is required. Add it to .env or export it before publishing.`);
  }
}

function loadDotEnv(envPath) {
  if (!existsSync(envPath)) {
    return;
  }

  const envContent = readFileSync(envPath, "utf8");
  for (const rawLine of envContent.split(/\r?\n/u)) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#")) {
      continue;
    }

    const separatorIndex = line.indexOf("=");
    if (separatorIndex <= 0) {
      continue;
    }

    const key = line.slice(0, separatorIndex).trim();
    const value = line.slice(separatorIndex + 1).trim().replace(/^(['"])(.*)\1$/u, "$2");
    process.env[key] = value;
  }
}

function run(command, commandArgs) {
  const result = spawnSync(command, commandArgs, {
    cwd: rootDir,
    env: process.env,
    stdio: "inherit",
  });

  if (result.status !== 0) {
    fail(`${command} ${commandArgs.join(" ")} failed.`);
  }
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
