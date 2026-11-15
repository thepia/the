#!/usr/bin/env node
// Assemble the npm distribution for `thepia` (ADR-0004): a tiny launcher package `thepia` plus one
// per-platform package `@thepia/cli-<os>-<arch>` carrying the prebuilt native binary, selected at
// install time by npm's `os`/`cpu` gating via optionalDependencies. No binary lives in the launcher;
// no CDN or postinstall download exists. This script is the single source of truth for the target set
// and keeps every version pinned to the Rust workspace version in Cargo.toml.
//
// Usage:
//   node scripts/build-npm.mjs sync                       # pin launcher version + optionalDependencies to Cargo.toml
//   node scripts/build-npm.mjs platform <triple> <binary> # build npm/platforms/<pkg> from one built binary
//   node scripts/build-npm.mjs assemble [--from <dir>]    # sync + build every platform whose binary is present
//   node scripts/build-npm.mjs pack                       # npm pack launcher + platform packages into npm/dist
//
// `<dir>` defaults to `target/<triple>/release`. `assemble` skips targets whose binary is absent, so CI
// can run it after a partial build matrix and a developer can run it with just the host target built.

import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const npmDir = join(repoRoot, "npm");
const launcherDir = join(npmDir, "thepia");
const platformsDir = join(npmDir, "platforms");
const distDir = join(npmDir, "dist");

// Rust target triple → npm platform package. `os`/`cpu` must equal Node's process.platform/process.arch
// so npm installs exactly the matching package. The binary is named `thepia` inside each package.
// UNSCOPED platform package names. `@thepia` is Thepia's PRIVATE scope (routed to GitHub Packages in
// Thepia/corporate .npmrc), so scoping the public binary packages under it makes npm resolve them from
// the wrong registry and silently skip them (the launcher then can't find its binary). Unscoped names
// can't be misrouted by any scope config. Revisit a dedicated public scope later if wanted.
const TARGETS = [
  { triple: "aarch64-apple-darwin", pkg: "thepia-cli-darwin-arm64", os: "darwin", cpu: "arm64" },
  { triple: "x86_64-apple-darwin", pkg: "thepia-cli-darwin-x64", os: "darwin", cpu: "x64" },
  { triple: "x86_64-unknown-linux-gnu", pkg: "thepia-cli-linux-x64", os: "linux", cpu: "x64" },
  { triple: "aarch64-unknown-linux-gnu", pkg: "thepia-cli-linux-arm64", os: "linux", cpu: "arm64" },
];

function workspaceVersion() {
  const cargo = readFileSync(join(repoRoot, "Cargo.toml"), "utf8");
  const section = cargo.slice(cargo.indexOf("[workspace.package]"));
  const match = section.match(/version\s*=\s*"([^"]+)"/);
  if (!match) {
    throw new Error("could not read [workspace.package] version from Cargo.toml");
  }
  return match[1];
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function writeJson(path, value) {
  writeFileSync(path, JSON.stringify(value, null, 2) + "\n");
}

// Pin the launcher's own version and every optionalDependency to the workspace version, so the three
// (launcher, platform packages, Rust binary) never drift. Idempotent. `registry` is the publish target
// (default npmjs.org; a verdaccio URL for `npm-smoke`).
function sync(version, registry) {
  const manifest = readJson(join(launcherDir, "package.json"));
  manifest.version = version;
  manifest.optionalDependencies = Object.fromEntries(TARGETS.map((t) => [t.pkg, version]));
  manifest.publishConfig = { access: "public", registry };
  writeJson(join(launcherDir, "package.json"), manifest);
  console.error(`synced thepia@${version} (registry ${registry}) with ${TARGETS.length} optionalDependencies`);
}

function platformManifest(target, version, registry) {
  return {
    name: target.pkg,
    version,
    description: `Prebuilt \`thepia\` CLI binary for ${target.os}/${target.cpu}.`,
    homepage: "https://thepia.com",
    repository: { type: "git", url: "git+https://github.com/Thepia/the.git", directory: "npm/platforms" },
    license: "MIT OR Apache-2.0",
    engines: { node: ">=16" },
    os: [target.os],
    cpu: [target.cpu],
    // Pin the publish target so the scoped `@thepia/*` publish is not misrouted to a GitHub Packages
    // `@thepia:registry` in a local .npmrc. Defaults to PUBLIC npmjs.org so the launcher's
    // optionalDependencies resolve for every public/corporate-mirror install (ADR-0004); `npm-smoke`
    // overrides it to a local verdaccio URL.
    publishConfig: { access: "public", registry },
  };
}

function targetFor(triple) {
  const target = TARGETS.find((t) => t.triple === triple);
  if (!target) {
    throw new Error(`unknown target triple: ${triple} (known: ${TARGETS.map((t) => t.triple).join(", ")})`);
  }
  return target;
}

// Build npm/platforms/<pkg> from one built binary: the gating package.json, the binary (named `thepia`,
// executable), a README, and the dual licenses.
function buildPlatform(target, binaryPath, version, registry) {
  if (!existsSync(binaryPath)) {
    throw new Error(`binary not found for ${target.triple}: ${binaryPath}`);
  }
  const outDir = join(platformsDir, target.pkg.replace("@thepia/", ""));
  rmSync(outDir, { recursive: true, force: true });
  mkdirSync(outDir, { recursive: true });

  writeJson(join(outDir, "package.json"), platformManifest(target, version, registry));
  copyFileSync(binaryPath, join(outDir, "thepia"));
  chmodSync(join(outDir, "thepia"), 0o755);

  const readme = `# ${target.pkg}\n\nPrebuilt \`thepia\` CLI binary for ${target.os}/${target.cpu}. Installed automatically as an optional\ndependency of the [\`thepia\`](https://www.npmjs.com/package/thepia) package; do not depend on it directly.\n`;
  writeFileSync(join(outDir, "README.md"), readme);
  for (const license of ["LICENSE-MIT", "LICENSE-APACHE"]) {
    const src = join(repoRoot, "docs", license);
    if (existsSync(src)) {
      copyFileSync(src, join(outDir, license));
    }
  }
  console.error(`built ${target.pkg}@${version} from ${binaryPath}`);
  return outDir;
}

function defaultBinary(triple) {
  return join(repoRoot, "target", triple, "release", "thepia");
}

function pack(version) {
  rmSync(distDir, { recursive: true, force: true });
  mkdirSync(distDir, { recursive: true });
  const packInto = (pkgDir) => {
    const out = execFileSync("npm", ["pack", "--pack-destination", distDir, pkgDir], { cwd: repoRoot }).toString().trim();
    console.error(`packed ${out}`);
  };
  packInto(launcherDir);
  for (const target of TARGETS) {
    const outDir = join(platformsDir, target.pkg.replace("@thepia/", ""));
    if (existsSync(outDir)) {
      packInto(outDir);
    }
  }
  console.error(`packed thepia@${version} + present platform packages into ${distDir}`);
}

// Pull `--pre <label>` and `--registry <url>` out of argv; leave the rest (command, positionals, and
// `--from <dir>`) in order. `--pre beta.0` makes the version `<cargo>-beta.0`; `--registry` sets the
// publish target. Both also read env (THEPIA_NPM_PRE / THEPIA_NPM_REGISTRY).
function parseArgs(argv) {
  const positional = [];
  let pre = process.env.THEPIA_NPM_PRE || null;
  let registry = process.env.THEPIA_NPM_REGISTRY || "https://registry.npmjs.org";
  let versionOverride = process.env.THEPIA_NPM_VERSION || null;
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--pre") pre = argv[++i];
    else if (a.startsWith("--pre=")) pre = a.slice("--pre=".length);
    else if (a === "--registry") registry = argv[++i];
    else if (a.startsWith("--registry=")) registry = a.slice("--registry=".length);
    else if (a === "--version") versionOverride = argv[++i];
    else if (a.startsWith("--version=")) versionOverride = a.slice("--version=".length);
    else positional.push(a);
  }
  return { positional, pre, registry, versionOverride };
}

function main() {
  const { positional, pre, registry, versionOverride } = parseArgs(process.argv.slice(2));
  const [cmd, ...rest] = positional;
  // --version publishes a specific already-built release regardless of the current Cargo.toml (used by
  // the dev-machine npm-publish-release job, which assembles from a downloaded RC's binaries).
  const base = versionOverride || workspaceVersion();
  const version = pre ? `${base}-${pre}` : base;

  switch (cmd) {
    case "sync":
      sync(version, registry);
      break;
    case "platform": {
      const [triple, binary] = rest;
      if (!triple) {
        throw new Error("usage: build-npm.mjs platform <triple> [binary] [--pre label] [--registry url]");
      }
      sync(version, registry);
      buildPlatform(targetFor(triple), binary ? resolve(binary) : defaultBinary(triple), version, registry);
      break;
    }
    case "assemble": {
      const fromIdx = rest.indexOf("--from");
      const fromDir = fromIdx >= 0 ? resolve(rest[fromIdx + 1]) : null;
      sync(version, registry);
      // Clean the whole platforms dir so renamed/removed packages never linger and get published.
      rmSync(platformsDir, { recursive: true, force: true });
      let built = 0;
      for (const target of TARGETS) {
        const binary = fromDir ? join(fromDir, target.triple, "thepia") : defaultBinary(target.triple);
        if (existsSync(binary)) {
          buildPlatform(target, binary, version, registry);
          built += 1;
        } else {
          console.error(`skip ${target.pkg}: no binary at ${binary}`);
        }
      }
      if (built === 0) {
        throw new Error("assemble built nothing: no target binaries found");
      }
      break;
    }
    case "pack":
      pack(version);
      break;
    default:
      console.error(
        "usage: build-npm.mjs <sync|platform <triple> [binary]|assemble [--from dir]|pack> [--pre label] [--registry url]"
      );
      process.exit(2);
  }
}

main();
