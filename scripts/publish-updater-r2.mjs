#!/usr/bin/env node

import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const rootDir = resolve(import.meta.dirname, "..");
loadDotEnv(join(rootDir, ".env"));

const dryRun = process.argv.includes("--dry-run");
const bucket = process.env.LOCALIFY_R2_BUCKET?.trim();
const jurisdiction = process.env.LOCALIFY_R2_JURISDICTION?.trim();
const updatesDir = join(rootDir, "dist", "updates");
const latestPath = join(updatesDir, "latest.json");

if (!bucket) {
  fail("LOCALIFY_R2_BUCKET is required.");
}

if (!existsSync(latestPath)) {
  fail("Missing dist/updates/latest.json. Run `pnpm tauri:build` first.");
}

const latestManifest = JSON.parse(readFileSync(latestPath, "utf8"));
const version = String(latestManifest.version ?? "").trim();
if (!version) {
  fail("dist/updates/latest.json is missing a version field.");
}

const versionDir = join(updatesDir, version);
if (!existsSync(versionDir)) {
  fail(`Missing versioned updater directory: ${versionDir}`);
}

const versionFiles = listFiles(versionDir);
if (versionFiles.length === 0) {
  fail(`No updater artifacts found in ${versionDir}`);
}

for (const filePath of versionFiles) {
  const objectKey = relative(updatesDir, filePath).replaceAll("\\", "/");
  uploadObject({
    bucket,
    jurisdiction,
    objectKey,
    filePath,
    dryRun,
  });
}

uploadObject({
  bucket,
  jurisdiction,
  objectKey: "latest.json",
  filePath: latestPath,
  dryRun,
});

if (dryRun) {
  console.log("Dry run completed. No files were uploaded.");
} else {
  console.log(`Uploaded updater manifest and ${versionFiles.length} artifact(s) to R2 bucket '${bucket}'.`);
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
    if (!(key in process.env)) {
      process.env[key] = value;
    }
  }
}

function listFiles(directoryPath) {
  const entries = readdirSync(directoryPath, { withFileTypes: true });
  const files = [];

  for (const entry of entries) {
    const entryPath = join(directoryPath, entry.name);
    if (entry.isDirectory()) {
      files.push(...listFiles(entryPath));
      continue;
    }

    if (entry.isFile()) {
      files.push(entryPath);
    }
  }

  return files.sort();
}

function uploadObject({ bucket, jurisdiction, objectKey, filePath, dryRun }) {
  const objectPath = `${bucket}/${objectKey}`;
  const contentType = detectContentType(filePath);
  const cacheControl = detectCacheControl(objectKey);

  const args = [
    "dlx",
    "wrangler",
    "r2",
    "object",
    "put",
    objectPath,
    "--file",
    filePath,
    "--remote",
    "--content-type",
    contentType,
    "--cache-control",
    cacheControl,
  ];

  if (jurisdiction) {
    args.push("--jurisdiction", jurisdiction);
  }

  const summary = `pnpm ${args.join(" ")}`;
  if (dryRun) {
    console.log(summary);
    return;
  }

  console.log(`Uploading ${objectKey} (${formatSize(statSync(filePath).size)})`);
  const result = spawnSync("pnpm", args, {
    cwd: rootDir,
    stdio: "inherit",
    env: process.env,
  });

  if (result.status !== 0) {
    fail(`Failed to upload ${objectKey} to R2.`);
  }
}

function detectContentType(filePath) {
  if (filePath.endsWith(".json")) {
    return "application/json; charset=utf-8";
  }

  if (filePath.endsWith(".sig")) {
    return "text/plain; charset=utf-8";
  }

  if (filePath.endsWith(".tar.gz")) {
    return "application/gzip";
  }

  return "application/octet-stream";
}

function detectCacheControl(objectKey) {
  if (objectKey === "latest.json") {
    return "no-store, no-cache, must-revalidate";
  }

  return "public, max-age=31536000, immutable";
}

function formatSize(size) {
  if (size < 1024 * 1024) {
    return `${Math.round(size / 1024)} KB`;
  }

  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
