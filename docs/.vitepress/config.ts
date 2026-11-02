import { legacyAlias } from "../scripts/anchors.mjs";
import { defineConfig } from "vitepress";
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { description, metadata } from "../scripts/metadata.mjs";
import {
  docsRoot,
  origin,
  replaceFragments,
  sourceRoot,
} from "../scripts/content.mjs";

const fragments = JSON.parse(
  readFileSync(join(docsRoot, ".vitepress/generated/fragments.json"), "utf8"),
);
const repository = "https://github.com/jolars/badness";
const guide = [
  ["Formatting", "formatting"],
  ["Linting", "linting"],
  ["Configuration", "configuration"],
  ["Editor Setup", "editor-setup"],
  ["Language Server", "language-server"],
  ["Integrations", "integrations"],
  ["Performance", "performance"],
];
const reference = [
  ["CLI Reference", "cli"],
  ["Configuration Reference", "configuration"],
  ["Editor Configuration", "editor-configuration"],
  ["Lint Rules", "linter-rules"],
  ["BibTeX Lint Rules", "bib-linter-rules"],
];

export default defineConfig({
  title: "Badness",
  description,
  lang: "en-US",
  srcDir: "src",
  cleanUrls: true,
  lastUpdated: true,
  markdown: {
    typographer: true,
    languageAlias: { bib: "bibtex" },
    config(md) {
      md.core.ruler.before("normalize", "badness-fragments", (state) => {
        state.src = replaceFragments(state.src, fragments);
      });
      const renderHeading = md.renderer.rules.heading_open;
      md.renderer.rules.heading_open = (
        tokens,
        index,
        options,
        env,
        renderer,
      ) => {
        const legacy = legacyAlias(tokens[index].attrGet("id") ?? "");
        const heading = renderHeading
          ? renderHeading(tokens, index, options, env, renderer)
          : renderer.renderToken(tokens, index, options);
        return (
          heading +
          (legacy
            ? `<span id="${md.utils.escapeHtml(legacy)}" aria-hidden="true"></span>`
            : "")
        );
      };
    },
  },
  head: [
    ["link", { rel: "icon", href: "/favicon.svg", type: "image/svg+xml" }],
  ],
  transformPageData(pageData) {
    if (pageData.relativePath === "404.md") return;
    const title =
      pageData.title === "Badness" ? "Badness" : `${pageData.title} | Badness`;
    pageData.frontmatter.head ??= [];
    pageData.frontmatter.head.push(
      ...metadata(
        pageData.relativePath,
        title,
        pageData.description || description,
      ),
    );
  },
  sitemap: {
    hostname: origin,
    transformItems(items) {
      items.push({ url: "playground/" });
      return items.map((item) => {
        const path =
          item.url === "playground/"
            ? "public/playground/index.html"
            : item.url === ""
              ? "index.md"
              : item.url.replace(/\.html$/, "") + ".md";
        try {
          const date = execFileSync(
            "git",
            ["log", "-1", "--format=%cs", "--", join(sourceRoot, path)],
            { encoding: "utf8" },
          ).trim();
          if (date) item.lastmod = date;
        } catch {
          /* A source archive has no Git history. */
        }
        item.url = item.url.replace(/\.html$/, "");
        return item;
      });
    },
  },
  themeConfig: {
    logo: { light: "/images/logo.svg", dark: "/images/logo-dark.svg", alt: "" },
    nav: [
      { text: "Guide", link: "/guide/getting-started" },
      { text: "Reference", link: "/reference/cli" },
      { text: "Playground", link: "/playground/", target: "_self" },
    ],
    sidebar: [
      {
        text: "Introduction",
        items: [
          { text: "Getting Started", link: "/guide/getting-started" },
          { text: "Installation", link: "/guide/installation" },
        ],
      },
      {
        text: "Guide",
        items: guide.map(([text, path]) => ({ text, link: `/guide/${path}` })),
      },
      {
        text: "Reference",
        items: reference.map(([text, path]) => ({
          text,
          link: `/reference/${path}`,
        })),
      },
      {
        text: "Development",
        items: [
          { text: "Contributing", link: "/development/contributing" },
          { text: "Architecture", link: "/development/architecture" },
        ],
      },
      { text: "Project", items: [{ text: "Changelog", link: "/changelog" }] },
    ],
    socialLinks: [{ icon: "github", link: repository }],
    editLink: { pattern: `${repository}/edit/main/docs/src/:path` },
    search: { provider: "local" },
    footer: { message: "Released under the MIT license." },
  },
});
