import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { load } from "cheerio";
import { createMarkdownRenderer } from "vitepress";
import {
  chapters,
  distRoot,
  docsRoot,
  markdownFor,
  pageUrl,
  sourceRoot,
} from "./content.mjs";
import { metadata } from "./metadata.mjs";

const fragments = JSON.parse(
  readFileSync(join(docsRoot, ".vitepress/generated/fragments.json"), "utf8"),
);
const routes = { pages: {}, redirects: {} };
const renderer = await createMarkdownRenderer(sourceRoot);
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
    const env = {};
    renderer.render(content, env);
    const { hero, features } = env.frontmatter;
    markdown =
      [
        `# ${hero.name}`,
        hero.text,
        hero.tagline,
        hero.actions.map(({ text, link }) => `- [${text}](${link})`).join("\n"),
        ...features.map(
          ({ title, details, link }) =>
            `## ${title}\n\n${details}\n\n[Learn more](${link})`,
        ),
        env.content.trim(),
      ].join("\n\n") + "\n";
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
