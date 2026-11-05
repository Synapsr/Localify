#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";

const rootDir = resolve(import.meta.dirname, "..");

const packageJson = JSON.parse(readFileSync(join(rootDir, "package.json"), "utf8"));
const tauriConfig = JSON.parse(
  readFileSync(join(rootDir, "src-tauri", "tauri.conf.json"), "utf8"),
);

const versions = {
  "package.json": String(packageJson.version ?? "").trim(),
  "src-tauri/tauri.conf.json": String(tauriConfig.version ?? "").trim(),
  "Cargo.toml": readCargoVersion("Cargo.toml"),
  "src-tauri/Cargo.toml": readCargoVersion("src-tauri/Cargo.toml"),
};

if (new Set(Object.values(versions)).size !== 1) {
  console.error("Version mismatch detected:");
  for (const [file, version] of Object.entries(versions)) {
    console.error(`- ${file}: ${version}`);
  }
  console.error("Run `pnpm version:set -- <version>` to sync them.");
  process.exit(1);
}

console.log(`Version is in sync: ${versions["package.json"]}`);

function readCargoVersion(relativePath) {
  const fileContent = readFileSync(join(rootDir, relativePath), "utf8");
  const match = fileContent.match(/^\s*version = "([^"]+)"\s*$/m);
  if (!match) {
    throw new Error(`Could not find version in ${relativePath}`);
  }

  return match[1].trim();
}
