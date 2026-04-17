import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, "..");
const isRelease = !process.argv.includes("--debug");
const profile = isRelease ? "release" : "debug";
const extension = process.platform === "win32" ? ".exe" : "";
const binaryName = `localify${extension}`;

const targetTriple = resolveTargetTriple();
const cargoArgs = ["build", "--bin", "localify"];

if (isRelease) {
  cargoArgs.push("--release");
}

if (targetTriple) {
  cargoArgs.push("--target", targetTriple);
}

execFileSync("cargo", cargoArgs, {
  cwd: repoRoot,
  stdio: "inherit",
});

const targetDir = targetTriple
  ? resolve(repoRoot, "target", targetTriple, profile)
  : resolve(repoRoot, "target", profile);
const sourcePath = resolve(targetDir, binaryName);
const binariesDir = resolve(repoRoot, "src-tauri", "binaries");
const outputPath = resolve(
  binariesDir,
  `localify-${targetTriple}${extension}`,
);

if (!existsSync(sourcePath)) {
  throw new Error(`Missing CLI binary at ${sourcePath}`);
}

mkdirSync(binariesDir, { recursive: true });
copyFileSync(sourcePath, outputPath);

if (process.platform !== "win32") {
  chmodSync(outputPath, 0o755);
}

console.log(`Prepared Tauri sidecar: ${outputPath}`);

function resolveTargetTriple() {
  const explicitTarget = process.env.CARGO_BUILD_TARGET?.trim();
  if (explicitTarget) {
    return explicitTarget;
  }

  const hostTuple = runRustc(["--print", "host-tuple"]);
  if (hostTuple) {
    return hostTuple;
  }

  const verbose = runRustc(["-vV"]);
  const hostLine = verbose
    .split("\n")
    .find((line) => line.startsWith("host: "));

  if (!hostLine) {
    throw new Error("Failed to determine Rust target triple");
  }

  return hostLine.replace("host: ", "").trim();
}

function runRustc(args) {
  try {
    return execFileSync("rustc", args, {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return "";
  }
}
