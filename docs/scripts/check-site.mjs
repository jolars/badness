import assert from "node:assert/strict";
import { readFileSync, readdirSync, existsSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { load } from "cheerio";
import {
  chapters,
  distRoot,
  docsRoot,
  origin,
  pageUrl,
  projectRoot,
} from "./content.mjs";
import { redirects } from "./routes.mjs";

function htmlFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory()
      ? htmlFiles(path)
      : path.endsWith(".html")
        ? [path]
        : [];
  });
}
const routes = JSON.parse(
  readFileSync(join(distRoot, "markdown-routes.json"), "utf8"),
);
const aliases = { ...routes.redirects, ...redirects };
const pages = new Map(
  htmlFiles(distRoot).map((path) => [path, load(readFileSync(path, "utf8"))]),
);
const errors = [];
function fileFor(path) {
  for (const candidate of [
    path,
    `${path}.html`,
    `${path.replace(/\/$/, "")}/index.html`,
  ]) {
    const file = join(distRoot, candidate);
    if (existsSync(file) && statSync(file).isFile()) return file;
  }
}
function targetFor(url) {
  for (let count = 0; count < 5 && aliases[url.pathname]; count++) {
    const destination = new URL(aliases[url.pathname], origin);
    if (!destination.hash) destination.hash = url.hash;
    url = destination;
  }
  return { url, file: fileFor(decodeURIComponent(url.pathname)) };
}
function hasId($, id) {
  return $("[id]")
    .toArray()
    .some((node) => $(node).attr("id") === id);
}
for (const [path, $] of pages) {
  const rel = relative(distRoot, path).replaceAll("\\", "/");
  const canonical = rel.endsWith("index.html")
    ? "/" + rel.slice(0, -10)
    : "/" + rel.replace(/\.html$/, "");
  if (rel !== "404.html") {
    assert.equal(
      $("link[rel=canonical]").attr("href"),
      origin + canonical,
      `canonical URL for ${rel}`,
    );
    assert.equal(
      $("meta[property='og:url']").attr("content"),
      origin + canonical,
    );
    assert.ok($("meta[property='og:image']").attr("content"));
    assert.equal(
      $("meta[name='twitter:card']").attr("content"),
      "summary_large_image",
    );
  }
  assert.doesNotMatch(
    $.html(),
    /\{\{\s*(?:badness-version|changelog|[\w-]*benchmark[\w-]*)\s*\}\}/,
    rel,
  );
  $("[href], [src]").each((_, element) => {
    const value = $(element).attr("href") ?? $(element).attr("src");
    if (!value) return;
    const url = new URL(value, origin + canonical);
    if (url.origin !== origin) return;
    const target = targetFor(url);
    if (!target.file) errors.push(`${rel}: missing ${value}`);
    else if (
      target.url.hash &&
      pages.has(target.file) &&
      !hasId(
        pages.get(target.file),
        decodeURIComponent(target.url.hash.slice(1)),
      )
    )
      errors.push(`${rel}: missing anchor ${value}`);
  });
}
const legacy = JSON.parse(
  readFileSync(join(docsRoot, "tests/legacy-anchors.json"), "utf8"),
);
for (const [path, ids] of Object.entries(legacy)) {
  const $ = pages.get(join(distRoot, path.replace(/\.md$/, ".html")));
  for (const id of ids)
    if (!hasId($, id)) errors.push(`${path}: missing legacy anchor #${id}`);
}
for (const [from, to] of Object.entries(aliases)) {
  const target = targetFor(new URL(from, origin));
  if (!target.file || aliases[target.url.pathname])
    errors.push(`Invalid redirect: ${from} -> ${to}`);
  if (
    target.url.hash &&
    target.file &&
    pages.has(target.file) &&
    !hasId(pages.get(target.file), target.url.hash.slice(1))
  )
    errors.push(`Invalid redirect fragment: ${from} -> ${to}`);
}
for (const [source, target] of [
  ["scripts/badness-installer.sh", "install"],
  ["scripts/badness-installer.ps1", "install.ps1"],
  ["badness.schema.json", "badness.schema.json"],
  ["branding/og.png", "images/og.png"],
])
  assert.deepEqual(
    readFileSync(join(distRoot, target)),
    readFileSync(join(projectRoot, source)),
  );
assert.ok(existsSync(join(distRoot, "playground/pkg/badness_wasm_bg.wasm")));
const sitemap = readFileSync(join(distRoot, "sitemap.xml"), "utf8");
for (const path of chapters()) {
  assert.ok(
    sitemap.includes(`<loc>${origin + pageUrl(path)}</loc>`),
    `sitemap entry for ${path}`,
  );
  assert.ok(existsSync(join(distRoot, "agent-markdown", path)));
}
assert.doesNotMatch(sitemap, /404\.html|\.html<\/loc>|agent-markdown/);
assert.equal(errors.length, 0, errors.join("\n"));
console.log(
  `Checked ${pages.size} HTML pages, ${Object.keys(aliases).length} redirects, legacy anchors, Markdown, and downloads.`,
);
