#!/usr/bin/env node
"use strict";

const { execFileSync, spawnSync } = require("node:child_process");

const PACKAGES = {
  "linux-x64-gnu": "@badness/linux-x64-gnu",
  "linux-arm64-gnu": "@badness/linux-arm64-gnu",
  "linux-x64-musl": "@badness/linux-x64-musl",
  "linux-arm64-musl": "@badness/linux-arm64-musl",
  "darwin-x64": "@badness/darwin-x64",
  "darwin-arm64": "@badness/darwin-arm64",
  "win32-x64": "@badness/win32-x64",
  "win32-arm64": "@badness/win32-arm64",
};

function detectLibc() {
  if (process.platform !== "linux") return null;
  try {
    const report = process.report.getReport();
    return report.header.glibcVersionRuntime ? "gnu" : "musl";
  } catch {
    return "gnu";
  }
}

// Candidate platform keys in preference order. On linux the other libc
// flavor is kept as a fallback: a host can report glibc yet be unable to
// run generic glibc binaries (NixOS has no loader at the standard path),
// while the musl build is statically linked and runs anywhere.
function candidateKeys() {
  const { platform, arch } = process;
  const libc = detectLibc();
  if (!libc) return [`${platform}-${arch}`];
  const other = libc === "gnu" ? "musl" : "gnu";
  return [`${platform}-${arch}-${libc}`, `${platform}-${arch}-${other}`];
}

function resolveCandidates() {
  const keys = candidateKeys();
  if (!PACKAGES[keys[0]]) {
    throw new Error(
      `badness does not ship a prebuilt binary for ${keys[0]}.\n` +
        `Supported platforms: linux (x64/arm64, gnu+musl), darwin (x64/arm64), win32 (x64/arm64).\n` +
        `See https://jolars.github.io/badness/ for alternative install methods.`,
    );
  }
  const binaryName = process.platform === "win32" ? "badness.exe" : "badness";
  const candidates = [];
  let firstError;
  for (const key of keys) {
    try {
      candidates.push(require.resolve(`${PACKAGES[key]}/${binaryName}`));
    } catch (err) {
      firstError = firstError ?? err;
    }
  }
  if (candidates.length === 0) {
    throw new Error(
      `badness expected the optional dependency ${PACKAGES[keys[0]]} to be installed, ` +
        `but it could not be resolved.\n` +
        `This usually means npm skipped it (e.g. \`--no-optional\` or a registry/network issue ` +
        `during install). Try reinstalling with optional dependencies enabled.\n` +
        `Original error: ${firstError.message}`,
    );
  }
  return candidates;
}

function runnable(binary) {
  const probe = spawnSync(binary, ["--version"], { stdio: "ignore" });
  return probe.error == null && probe.status === 0;
}

// With a single candidate, trust it: probing every invocation would cost an
// extra process spawn in the common case. Probing is reserved for the
// unusual setup where both libc flavors are installed side by side.
function pickBinary(candidates) {
  if (candidates.length === 1) return candidates[0];
  for (const binary of candidates) {
    if (runnable(binary)) return binary;
  }
  return candidates[0];
}

function loaderHint() {
  const libc = detectLibc();
  if (libc !== "gnu") return;
  const muslPackage = PACKAGES[`${process.platform}-${process.arch}-musl`];
  process.stderr.write(
    `The glibc build of badness cannot run on this system (no standard glibc loader; ` +
      `on NixOS this is expected).\n` +
      `The statically linked musl build works anywhere. Package managers skip it on ` +
      `glibc hosts, so add it explicitly:\n` +
      `  npm:  npm install --force ${muslPackage}\n` +
      `  pnpm: pnpm config set supportedArchitectures.libc '["glibc","musl"]' && pnpm install\n` +
      `Or install badness via other means; ` +
      `see https://jolars.github.io/badness/.\n`,
  );
}

function resolveBinary() {
  return pickBinary(resolveCandidates());
}

function main() {
  let binary;
  try {
    binary = resolveBinary();
  } catch (err) {
    process.stderr.write(`${err.message}\n`);
    process.exit(1);
  }

  try {
    execFileSync(binary, process.argv.slice(2), { stdio: "inherit" });
  } catch (err) {
    if (typeof err.status === "number") {
      // 127 is the loader stub's exit code on hosts that cannot run the
      // glibc build; probe to distinguish that from badness itself
      // exiting 127 before hinting.
      if (err.status === 127 && !runnable(binary)) {
        loaderHint();
      }
      process.exit(err.status);
    }
    if (err.signal) {
      process.kill(process.pid, err.signal);
      return;
    }
    // The binary resolved but could not be spawned; a missing dynamic
    // loader surfaces as ENOENT on hosts without NixOS's stub.
    if (err.code === "ENOENT") {
      loaderHint();
    }
    process.stderr.write(`Failed to execute badness: ${err.message}\n`);
    process.exit(1);
  }
}

main();
