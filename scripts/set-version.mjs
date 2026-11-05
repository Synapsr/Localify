#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const rootDir = resolve(import.meta.dirname, "..");
const args = process.argv.slice(2).filter((value) => value !== "--");
const nextVersion = args[0]?.trim();

if (!nextVersion) {
  fail("Usage: pnpm version:set -- <version>");
}

if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(nextVersion)) {
  fail(`Invalid version: ${nextVersion}`);
}

const packageJsonPath = join(rootDir, "package.json");
const tauriConfigPath = join(rootDir, "src-tauri", "tauri.conf.json");
const crateDirs = [rootDir, join(rootDir, "src-tauri")];

const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8"));
const currentVersion = String(packageJson.version ?? "").trim();
packageJson.version = nextVersion;
writeFileSync(packageJsonPath, `${JSON.stringify(packageJson, null, 2)}\n`);

const tauriConfig = JSON.parse(readFileSync(tauriConfigPath, "utf8"));
tauriConfig.version = nextVersion;
writeFileSync(tauriConfigPath, `${JSON.stringify(tauriConfig, null, 2)}\n`);

for (const crateDir of crateDirs) {
  const cargoTomlPath = join(crateDir, "Cargo.toml");
  const cargoToml = readFileSync(cargoTomlPath, "utf8");
  const updatedCargoToml = cargoToml.replace(
    /^version = "([^"]+)"$/m,
    `version = "${nextVersion}"`,
  );

  if (updatedCargoToml === cargoToml && currentVersion !== nextVersion) {
    fail(`Could not update version in ${cargoTomlPath}`);
  }

  writeFileSync(cargoTomlPath, updatedCargoToml);
  execFileSync("cargo", ["update", "--workspace", "--quiet"], {
    cwd: crateDir,
    stdio: "inherit",
  });
}

console.log(`Version set to ${nextVersion}`);
console.log("- package.json");
console.log("- src-tauri/tauri.conf.json");
console.log("- Cargo.toml and Cargo.lock");
console.log("- src-tauri/Cargo.toml and src-tauri/Cargo.lock");

function fail(message) {
  console.error(message);
  process.exit(1);
}
