#!/usr/bin/env node
"use strict";

const { execFileSync } = require("node:child_process");

function detectLibc() {
  if (process.platform !== "linux") return null;
  try {
    const report = process.report.getReport();
    return report.header.glibcVersionRuntime ? "gnu" : "musl";
  } catch {
    return "gnu";
  }
}

function platformPackage() {
  const { platform, arch } = process;
  const libc = detectLibc();

  const map = {
    "linux-x64-gnu": "@badness/linux-x64-gnu",
    "linux-arm64-gnu": "@badness/linux-arm64-gnu",
    "linux-x64-musl": "@badness/linux-x64-musl",
    "linux-arm64-musl": "@badness/linux-arm64-musl",
    "darwin-x64": "@badness/darwin-x64",
    "darwin-arm64": "@badness/darwin-arm64",
    "win32-x64": "@badness/win32-x64",
    "win32-arm64": "@badness/win32-arm64",
  };

  const key = libc ? `${platform}-${arch}-${libc}` : `${platform}-${arch}`;
  return { key, name: map[key] };
}

function resolveBinary() {
  const { key, name } = platformPackage();
  if (!name) {
    throw new Error(
      `badness does not ship a prebuilt binary for ${key}.\n` +
        `Supported platforms: linux (x64/arm64, gnu+musl), darwin (x64/arm64), win32 (x64/arm64).\n` +
        `See https://jolars.github.io/badness/ for alternative install methods.`,
    );
  }
  const binaryName = process.platform === "win32" ? "badness.exe" : "badness";
  try {
    return require.resolve(`${name}/${binaryName}`);
  } catch (err) {
    throw new Error(
      `badness expected the optional dependency ${name} to be installed, ` +
        `but it could not be resolved.\n` +
        `This usually means npm skipped it (e.g. \`--no-optional\` or a registry/network issue ` +
        `during install). Try reinstalling with optional dependencies enabled.\n` +
        `Original error: ${err.message}`,
    );
  }
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
      process.exit(err.status);
    }
    if (err.signal) {
      process.kill(process.pid, err.signal);
      return;
    }
    process.stderr.write(`Failed to execute badness: ${err.message}\n`);
    process.exit(1);
  }
}

main();
