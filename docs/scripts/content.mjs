import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const docsRoot = fileURLToPath(new URL("../", import.meta.url));
export const projectRoot = resolve(docsRoot, "..");
export const sourceRoot = join(docsRoot, "src");
export const distRoot = join(docsRoot, ".vitepress/dist");
export const origin = "https://badness.dev";

export function chapters(directory = sourceRoot) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    if (entry.name === "public" || entry.name === "SUMMARY.md") return [];
    const path = join(directory, entry.name);
    return entry.isDirectory()
      ? chapters(path)
      : path.endsWith(".md")
        ? [relative(sourceRoot, path).replaceAll("\\", "/")]
        : [];
  });
}

export function pageUrl(path) {
  return "/" + path.replace(/(^|\/)index\.md$/, "$1").replace(/\.md$/, "");
}

export function replaceFragments(source, fragments) {
  for (const [marker, content] of Object.entries(fragments))
    source = source.replaceAll(marker, () => content);
  return source;
}

export function expandIncludes(source, path, stack = []) {
  return source.replace(/<!--\s*@include:\s*(.*?)\s*-->/g, (_, name) => {
    const included = resolve(dirname(path), name);
    if (stack.includes(included))
      throw new Error(`Circular Markdown include: ${included}`);
    return expandIncludes(readFileSync(included, "utf8"), included, [
      ...stack,
      included,
    ]);
  });
}

export function markdownFor(path, fragments) {
  const file = join(sourceRoot, path);
  return replaceFragments(
    expandIncludes(readFileSync(file, "utf8"), file),
    fragments,
  );
}
