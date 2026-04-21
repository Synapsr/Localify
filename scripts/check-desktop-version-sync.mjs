#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";

const rootDir = resolve(import.meta.dirname, "..");
const packageJsonPath = join(rootDir, "package.json");
const tauriConfigPath = join(rootDir, "src-tauri", "tauri.conf.json");
const cargoTomlPath = join(rootDir, "src-tauri", "Cargo.toml");

const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8"));
const tauriConfig = JSON.parse(readFileSync(tauriConfigPath, "utf8"));
const cargoToml = readFileSync(cargoTomlPath, "utf8");

const versions = {
  packageJson: String(packageJson.version ?? "").trim(),
  tauriConfig: String(tauriConfig.version ?? "").trim(),
  cargoToml: readCargoVersion(cargoToml),
};

const uniqueVersions = new Set(Object.values(versions));
if (uniqueVersions.size !== 1) {
  console.error("Desktop version mismatch detected:");
  console.error(`- package.json: ${versions.packageJson}`);
  console.error(`- src-tauri/tauri.conf.json: ${versions.tauriConfig}`);
  console.error(`- src-tauri/Cargo.toml: ${versions.cargoToml}`);
  console.error("Run `pnpm version:desktop -- <version>` to sync them.");
  process.exit(1);
}

console.log(`Desktop version is in sync: ${versions.packageJson}`);

function readCargoVersion(fileContent) {
  const match = fileContent.match(/^\s*version = "([^"]+)"\s*$/m);
  if (!match) {
    throw new Error("Could not find version in src-tauri/Cargo.toml");
  }

  return match[1].trim();
}
