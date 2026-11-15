#!/usr/bin/env node
"use strict";

// Launcher for the `thepia` npm package. It carries no binary itself: the prebuilt native binary
// for this machine arrives as one of the per-platform optionalDependencies (`@thepia/cli-<os>-<arch>`),
// which npm installs only when its `os`/`cpu` match. This script resolves that binary and execs it,
// passing through argv, stdio, and the exit status. No download, no postinstall network call — every
// byte comes over the npm registry channel (see ADR-0004). The same binary backs `thepia`, `the`,
// and `th`; it is named `thepia` inside the platform package so the process shows as `thepia`.

const { spawnSync } = require("node:child_process");
const { execFileSync } = require("node:child_process");
const os = require("node:os");

// process.platform / process.arch → the platform package that carries this machine's binary.
const PLATFORMS = {
  darwin: {
    arm64: "thepia-cli-darwin-arm64/thepia",
    x64: "thepia-cli-darwin-x64/thepia",
  },
  linux: {
    x64: "thepia-cli-linux-x64/thepia",
    arm64: "thepia-cli-linux-arm64/thepia",
  },
};

// The Linux binaries are glibc builds; they will not run on a musl libc (Alpine). Detect musl so we
// can print an honest "build from source" message instead of leaking a dynamic-linker error.
function isMuslLinux() {
  if (process.platform !== "linux") {
    return false;
  }
  try {
    const report = typeof process.report?.getReport === "function" ? process.report.getReport() : null;
    const glibcVersion = report?.header?.glibcVersionRuntime;
    if (glibcVersion) {
      return false; // glibc runtime present → not musl
    }
  } catch {
    // fall through to ldd probe
  }
  try {
    const out = execFileSync("ldd", ["--version"], { stdio: ["ignore", "pipe", "pipe"] }).toString();
    return out.includes("musl");
  } catch (err) {
    const stderr = err && err.stderr ? err.stderr.toString() : "";
    return stderr.includes("musl");
  }
}

function buildFromSourceMessage(target) {
  return [
    `thepia: no prebuilt binary for ${target}.`,
    "",
    "Install the CLI another way:",
    "  cargo install --git https://github.com/Thepia/the thepia",
    "  nix run github:Thepia/thepia -- --help",
    "  # or build from source: https://github.com/Thepia/the",
  ].join("\n");
}

function resolveBinary() {
  // Escape hatch for tests / vendored installs: point straight at a binary.
  if (process.env.THEPIA_BINARY) {
    return process.env.THEPIA_BINARY;
  }

  const platform = isMuslLinux() ? "linux-musl" : process.platform;
  const specifier = PLATFORMS[process.platform] && PLATFORMS[process.platform][process.arch];
  const target = `${platform}-${process.arch}`;

  if (!specifier) {
    process.stderr.write(buildFromSourceMessage(target) + "\n");
    process.exit(1);
  }
  if (platform === "linux-musl") {
    process.stderr.write(buildFromSourceMessage(target) + "\n");
    process.exit(1);
  }

  const pkg = specifier.slice(0, specifier.indexOf("/"));
  try {
    return require.resolve(specifier);
  } catch {
    // The platform package matched this machine but is not installed. This is almost always the npm
    // optional-dependency bug (a stale/corrupt lockfile, or `--no-optional` / `--ignore-optional`).
    process.stderr.write(
      [
        `thepia: the ${pkg} package is not installed, so the CLI binary is missing.`,
        "",
        "This usually means npm skipped an optional dependency. Reinstall to fix it:",
        "  npm install thepia          # or: pnpm install / bun install",
        "If it persists, remove node_modules and the lockfile, then reinstall.",
      ].join("\n") + "\n"
    );
    process.exit(1);
  }
}

function main() {
  const binary = resolveBinary();
  const result = spawnSync(binary, process.argv.slice(2), {
    stdio: "inherit",
    env: { ...process.env, THEPIA_DISTRIBUTION: "npm" },
  });

  if (result.error) {
    if (result.error.code === "ENOENT") {
      process.stderr.write(`thepia: cannot execute ${binary}\n`);
      process.exit(1);
    }
    throw result.error;
  }

  // A child killed by a signal reports status=null; mirror the shell's 128 + signal convention.
  if (result.signal) {
    const signalNumber = os.constants.signals[result.signal];
    process.exit(signalNumber ? 128 + signalNumber : 1);
  }
  process.exit(result.status === null ? 1 : result.status);
}

main();
