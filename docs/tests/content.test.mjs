import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import {
  distRoot,
  docsRoot,
  markdownFor,
  projectRoot,
} from "../scripts/content.mjs";

const fragments = JSON.parse(
  readFileSync(join(docsRoot, ".vitepress/generated/fragments.json"), "utf8"),
);

test("Markdown exports use the same generated version and changelog as the site", () => {
  const changelog = readFileSync(
    join(distRoot, "agent-markdown/changelog.md"),
    "utf8",
  );
  assert.ok(changelog.includes(fragments["{{ changelog }}"]));
  assert.ok(
    changelog.includes("https://github.com/jolars/badness/releases/tag/v0.1.0"),
  );
  const installation = readFileSync(
    join(distRoot, "agent-markdown/guide/installation.md"),
    "utf8",
  );
  assert.ok(!installation.includes("{{ badness-version }}"));
});

test("Markdown includes expand without interpreting code examples as HTML", () => {
  const content = markdownFor("development/contributing.md", fragments);
  const original = readFileSync(join(projectRoot, "CONTRIBUTING.md"), "utf8");
  assert.ok(content.includes(original));
  assert.equal(
    readFileSync(
      join(distRoot, "agent-markdown/development/contributing.md"),
      "utf8",
    ).trim(),
    original.trim(),
  );
});

test("benchmark exports retain results tables and omit chart payloads", () => {
  const content = readFileSync(
    join(distRoot, "agent-markdown/guide/performance.md"),
    "utf8",
  );
  assert.match(content, /<table>/);
  assert.match(content, /texlab/i);
  assert.doesNotMatch(content, /data-bench=|<script|<noscript>/);
  assert.equal(
    content,
    readFileSync(join(distRoot, "guide/performance.md"), "utf8"),
  );
});
