#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const rootDir = resolve(import.meta.dirname, "..");
const args = process.argv.slice(2).filter((value) => value !== "--");
const nextVersion = args[0]?.trim();

if (!nextVersion) {
  fail("Usage: pnpm version:desktop -- <version>");
}

if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(nextVersion)) {
  fail(`Invalid desktop version: ${nextVersion}`);
}

const packageJsonPath = join(rootDir, "package.json");
const tauriConfigPath = join(rootDir, "src-tauri", "tauri.conf.json");
const cargoTomlPath = join(rootDir, "src-tauri", "Cargo.toml");

const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8"));
const currentVersion = String(packageJson.version ?? "").trim();
packageJson.version = nextVersion;
writeFileSync(packageJsonPath, `${JSON.stringify(packageJson, null, 2)}\n`);

const tauriConfig = JSON.parse(readFileSync(tauriConfigPath, "utf8"));
tauriConfig.version = nextVersion;
writeFileSync(tauriConfigPath, `${JSON.stringify(tauriConfig, null, 2)}\n`);

const cargoToml = readFileSync(cargoTomlPath, "utf8");
const updatedCargoToml = cargoToml.replace(
  /^version = "([^"]+)"$/m,
  `version = "${nextVersion}"`,
);

if (updatedCargoToml === cargoToml && currentVersion !== nextVersion) {
  fail("Could not update version in src-tauri/Cargo.toml");
}

writeFileSync(cargoTomlPath, updatedCargoToml);

console.log(`Desktop version set to ${nextVersion}`);
console.log("- package.json");
console.log("- src-tauri/tauri.conf.json");
console.log("- src-tauri/Cargo.toml");

function fail(message) {
  console.error(message);
  process.exit(1);
}
