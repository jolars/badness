import { readFileSync } from "node:fs";
import { join } from "node:path";
import { origin, pageUrl, projectRoot } from "./content.mjs";

const png = readFileSync(join(projectRoot, "branding/og.png"));
export const description =
  "Badness is a formatter, linter, and language server for LaTeX.";
export function metadata(path, title, summary = description) {
  const url = origin + pageUrl(path);
  return [
    ["link", { rel: "canonical", href: url }],
    [
      "link",
      {
        rel: "alternate",
        type: "text/markdown",
        href: `${origin}/agent-markdown/${path}`,
      },
    ],
    ...Object.entries({
      "og:type": "website",
      "og:site_name": "Badness",
      "og:title": title,
      "og:description": summary,
      "og:url": url,
      "og:image": `${origin}/images/og.png`,
      "og:image:type": "image/png",
      "og:image:width": String(png.readUInt32BE(16)),
      "og:image:height": String(png.readUInt32BE(20)),
      "og:image:alt": description,
      "twitter:card": "summary_large_image",
      "twitter:title": title,
      "twitter:description": summary,
      "twitter:image": `${origin}/images/og.png`,
      "twitter:image:alt": description,
    }).map(([name, content]) => [
      "meta",
      { [name.startsWith("og:") ? "property" : "name"]: name, content },
    ]),
  ];
}
