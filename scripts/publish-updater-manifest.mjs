#!/usr/bin/env node

import { cpSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";

const rootDir = resolve(import.meta.dirname, "..");
const envPath = join(rootDir, ".env");
if (existsSync(envPath)) {
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
    if (!(key in process.env)) {
      process.env[key] = value;
    }
  }
}

const updaterBaseUrl = process.env.LOCALIFY_UPDATER_BASE_URL?.trim();
if (!updaterBaseUrl) {
  throw new Error("LOCALIFY_UPDATER_BASE_URL is required to publish updater metadata.");
}

const tauriConfig = JSON.parse(
  readFileSync(join(rootDir, "src-tauri", "tauri.conf.json"), "utf8"),
);

const version = String(tauriConfig.version).trim();
if (!version) {
  throw new Error("Missing desktop version in src-tauri/tauri.conf.json.");
}

const macosBundleDir = join(rootDir, "src-tauri", "target", "release", "bundle", "macos");
const artifacts = {
  archive: join(macosBundleDir, "Localify Desktop.app.tar.gz"),
  signature: join(macosBundleDir, "Localify Desktop.app.tar.gz.sig"),
};

for (const artifactPath of Object.values(artifacts)) {
  if (!existsSync(artifactPath)) {
    throw new Error(`Missing updater artifact: ${artifactPath}`);
  }
}

const publishedDir = join(rootDir, "dist", "updates", version);
mkdirSync(publishedDir, { recursive: true });

const publishedArchiveName = "localify-desktop.app.tar.gz";
const publishedSignatureName = "localify-desktop.app.tar.gz.sig";

cpSync(artifacts.archive, join(publishedDir, publishedArchiveName));
cpSync(artifacts.signature, join(publishedDir, publishedSignatureName));

const latestManifest = {
  version,
  notes:
    process.env.LOCALIFY_UPDATER_NOTES?.trim() ||
    `Localify Desktop ${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    "darwin-aarch64": {
      signature: readFileSync(artifacts.signature, "utf8").trim(),
      url: new URL(`${version}/${publishedArchiveName}`, `${updaterBaseUrl.replace(/\/+$/u, "")}/`)
        .toString(),
    },
  },
};

const latestPath = join(rootDir, "dist", "updates", "latest.json");
writeFileSync(latestPath, `${JSON.stringify(latestManifest, null, 2)}\n`);

console.log(`Published updater manifest: ${latestPath}`);
console.log(`Archive: ${join(publishedDir, basename(publishedArchiveName))}`);
