import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { docsRoot, projectRoot, sourceRoot } from "./content.mjs";

const generated = join(docsRoot, ".vitepress/generated");
mkdirSync(generated, { recursive: true });
const fragments = execFileSync(
  "cargo",
  [
    "run",
    "--quiet",
    "--manifest-path",
    join(docsRoot, "doc-utils/Cargo.toml"),
    "--",
    projectRoot,
  ],
  {
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
    stdio: ["ignore", "pipe", "inherit"],
  },
);
JSON.parse(fragments);
writeFileSync(join(generated, "fragments.json"), fragments);
for (const [source, destination] of [
  ["scripts/badness-installer.sh", "install"],
  ["scripts/badness-installer.ps1", "install.ps1"],
  ["badness.schema.json", "badness.schema.json"],
  ["branding/og.png", "images/og.png"],
])
  copyFileSync(
    join(projectRoot, source),
    join(sourceRoot, "public", destination),
  );
