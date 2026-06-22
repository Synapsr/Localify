#!/usr/bin/env node

import {
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
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
    process.env[key] = value;
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

const productName = String(tauriConfig.productName).trim();
if (!productName) {
  throw new Error("Missing productName in src-tauri/tauri.conf.json.");
}

const macosBundleDir = join(rootDir, "src-tauri", "target", "release", "bundle", "macos");
const dmgBundleDir = join(rootDir, "src-tauri", "target", "release", "bundle", "dmg");
const artifacts = {
  archive: join(macosBundleDir, `${productName}.app.tar.gz`),
  signature: join(macosBundleDir, `${productName}.app.tar.gz.sig`),
  dmg: findDmgArtifact(dmgBundleDir, productName),
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
const publishedDmgName = "localify.dmg";
const legacyPublishedDmgName = "localify-desktop.dmg";
const archiveSignature = readSignatureFile(artifacts.signature);

cpSync(artifacts.archive, join(publishedDir, publishedArchiveName));
cpSync(artifacts.signature, join(publishedDir, publishedSignatureName));
removeIfExists(join(publishedDir, legacyPublishedDmgName));
cpSync(artifacts.dmg, join(publishedDir, publishedDmgName));

const latestManifest = {
  version,
  notes:
    process.env.LOCALIFY_UPDATER_NOTES?.trim() ||
    `${productName} ${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    "darwin-aarch64": {
      signature: archiveSignature,
      url: new URL(`${version}/${publishedArchiveName}`, `${updaterBaseUrl.replace(/\/+$/u, "")}/`)
        .toString(),
    },
  },
};

const latestPath = join(rootDir, "dist", "updates", "latest.json");
writeFileSync(latestPath, `${JSON.stringify(latestManifest, null, 2)}\n`);

console.log(`Published updater manifest: ${latestPath}`);
console.log(`Archive: ${join(publishedDir, basename(publishedArchiveName))}`);
console.log(`DMG: ${join(publishedDir, basename(publishedDmgName))}`);

function findDmgArtifact(directoryPath, productName) {
  if (!existsSync(directoryPath)) {
    throw new Error(`Missing DMG bundle directory: ${directoryPath}`);
  }

  const dmgFiles = readdirSync(directoryPath)
    .filter((fileName) => fileName.startsWith(`${productName}_`) && fileName.endsWith(".dmg"))
    .sort();

  if (dmgFiles.length === 0) {
    throw new Error(`Missing DMG artifact in ${directoryPath}.`);
  }

  if (dmgFiles.length > 1) {
    throw new Error(`Found multiple DMG artifacts in ${directoryPath}: ${dmgFiles.join(", ")}`);
  }

  return join(directoryPath, dmgFiles[0]);
}

function removeIfExists(filePath) {
  if (existsSync(filePath)) {
    unlinkSync(filePath);
  }
}

function readSignatureFile(filePath) {
  const signature = readFileSync(filePath, "utf8").trim();
  if (!/^[A-Za-z0-9+/=]+$/u.test(signature)) {
    throw new Error(
      `Invalid updater signature file: ${filePath}. Expected a base64 signature only.`,
    );
  }
  return signature;
}
