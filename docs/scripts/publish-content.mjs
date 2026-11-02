import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { load } from "cheerio";
import {
  chapters,
  distRoot,
  docsRoot,
  markdownFor,
  pageUrl,
} from "./content.mjs";
import { metadata } from "./metadata.mjs";

const fragments = JSON.parse(
  readFileSync(join(docsRoot, ".vitepress/generated/fragments.json"), "utf8"),
);
const routes = { pages: {}, redirects: {} };
for (const path of [...chapters(), "playground/index.md"]) {
  const content =
    path === "playground/index.md"
      ? "# Playground\n\nTry Badness formatting and linting in the [interactive playground](https://badness.dev/playground/).\n"
      : markdownFor(path, fragments);
  let markdown = content;
  if (path === "guide/performance.md") {
    markdown = markdown
      .replace(/ data-bench="[^"]*"/g, "")
      .replace(/<div class="bench-chart"><\/div>\n?/g, "")
      .replace(/<noscript>[\s\S]*?<\/noscript>\n?/g, "");
  }
  if (path === "index.md") {
    markdown = markdown.replace(
      /^<img[^>]+class="[^"]*logo-(?:light|dark)[^"]*"[^>]*>\n?/gm,
      "",
    );
  }
  for (const root of [distRoot, join(distRoot, "agent-markdown")]) {
    const target = join(root, path);
    mkdirSync(join(target, ".."), { recursive: true });
    writeFileSync(target, markdown);
  }
  const canonical = pageUrl(path);
  routes.pages[canonical] = `/agent-markdown/${path}`;
  routes.redirects[`/${path.replace(/\.md$/, ".html")}`] = canonical;
}
writeFileSync(
  join(distRoot, "markdown-routes.json"),
  JSON.stringify(routes, null, 2) + "\n",
);
writeFileSync(join(distRoot, ".assetsignore"), "markdown-routes.json\n");

const playgroundPath = join(distRoot, "playground/index.html");
const $ = load(readFileSync(playgroundPath, "utf8"));
for (const [tag, attributes] of metadata(
  "playground/index.md",
  $("title").text(),
  $("meta[name=description]").attr("content"),
))
  $("head").append($(`<${tag}>`).attr(attributes));
writeFileSync(playgroundPath, $.html());
