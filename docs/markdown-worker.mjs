import routes from "./.vitepress/dist/markdown-routes.json" with { type: "json" };
import { redirects } from "./scripts/routes.mjs";

const HOMEPAGE_LINKS =
  '<https://badness.dev/guide/getting-started>; rel="service-doc", <https://badness.dev/agent-markdown/index.md>; rel="describedby"';

export function acceptsMarkdown(header = "") {
  return header.split(",").some((entry) => {
    const [type, ...parameters] = entry.trim().split(";");
    if (type.trim().toLowerCase() !== "text/markdown") return false;
    const quality = parameters.find((parameter) =>
      /^\s*q\s*=/i.test(parameter),
    );
    const value = quality ? Number(quality.split("=")[1]) : 1;
    return value > 0 && value <= 1;
  });
}

function pageHeaders(response, pathname, markdown = false) {
  const headers = new Headers(response.headers);
  const vary = (headers.get("Vary") || "")
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean);
  if (!vary.some((value) => ["accept", "*"].includes(value.toLowerCase())))
    vary.push("Accept");
  headers.set("Vary", vary.join(", "));
  if (pathname === "/") headers.set("Link", HOMEPAGE_LINKS);
  if (markdown) headers.set("Content-Type", "text/markdown; charset=utf-8");
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const destination =
      redirects[url.pathname] ?? routes.redirects[url.pathname];
    if (destination && ["GET", "HEAD"].includes(request.method)) {
      const target = new URL(destination, url);
      target.search = url.search;
      return Response.redirect(target.href, 301);
    }
    const markdownPath = routes.pages[url.pathname];
    const negotiate =
      markdownPath &&
      ["GET", "HEAD"].includes(request.method) &&
      acceptsMarkdown(request.headers.get("Accept") ?? "");
    if (negotiate) {
      const markdownUrl = new URL(markdownPath, url);
      const headers = new Headers(request.headers);
      // HTML validators and byte ranges do not describe the Markdown representation.
      for (const name of [
        "If-None-Match",
        "If-Modified-Since",
        "Range",
        "If-Range",
      ])
        headers.delete(name);
      const markdown = await env.ASSETS.fetch(
        new Request(markdownUrl, { method: request.method, headers }),
      );
      if (markdown.ok) return pageHeaders(markdown, url.pathname, true);
    }
    const response = await env.ASSETS.fetch(request);
    if (markdownPath) return pageHeaders(response, url.pathname);
    if (url.pathname.endsWith(".md") && response.ok) {
      const headers = new Headers(response.headers);
      headers.set("Content-Type", "text/markdown; charset=utf-8");
      return new Response(response.body, {
        status: response.status,
        statusText: response.statusText,
        headers,
      });
    }
    return response;
  },
};
