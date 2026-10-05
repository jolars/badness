const MARKDOWN_ROOT = "/agent-markdown";

function markdownPath(pathname) {
  if (pathname.startsWith(`${MARKDOWN_ROOT}/`)) return null;
  if (pathname === "/" || pathname === "/index.html") {
    return `${MARKDOWN_ROOT}/index.md`;
  }
  if (pathname.endsWith("/")) {
    return `${MARKDOWN_ROOT}${pathname}index.md`;
  }
  if (pathname.endsWith(".html")) {
    return `${MARKDOWN_ROOT}${pathname.slice(0, -5)}.md`;
  }
  if (pathname.endsWith(".md")) {
    return `${MARKDOWN_ROOT}${pathname}`;
  }
  return null;
}

function acceptsMarkdown(header) {
  return (header || "").split(",").some((entry) => {
    const [type, ...parameters] = entry.trim().split(";");
    if (type.trim().toLowerCase() !== "text/markdown") return false;
    return !parameters.some((parameter) => /^q\s*=\s*0(?:\.0*)?\s*$/i.test(parameter.trim()));
  });
}

function varyOnAccept(headers) {
  const vary = headers.get("Vary");
  if (!vary) {
    headers.set("Vary", "Accept");
  } else if (!vary.split(",").some((value) => value.trim().toLowerCase() === "accept")) {
    headers.set("Vary", `${vary}, Accept`);
  }
}

export default {
  async fetch(request) {
    const url = new URL(request.url);
    const path = markdownPath(url.pathname);
    if (!path || !["GET", "HEAD"].includes(request.method)) {
      return fetch(request);
    }

    if (acceptsMarkdown(request.headers.get("Accept"))) {
      const markdownUrl = new URL(path, url.origin);
      const markdown = await fetch(new Request(markdownUrl, { method: request.method }));
      if (markdown.ok) {
        const headers = new Headers(markdown.headers);
        headers.set("Content-Type", "text/markdown; charset=utf-8");
        headers.delete("Content-Encoding");
        headers.delete("Content-Length");
        headers.delete("ETag");
        headers.delete("Last-Modified");
        varyOnAccept(headers);
        return new Response(markdown.body, { status: markdown.status, headers });
      }
    }

    const htmlRequest = url.pathname.endsWith(".md")
      ? new Request(new URL(url.pathname.slice(0, -3) + ".html", url.origin), request)
      : request;
    const html = await fetch(htmlRequest);
    const headers = new Headers(html.headers);
    headers.delete("Content-Encoding");
    headers.delete("Content-Length");
    varyOnAccept(headers);
    return new Response(html.body, { status: html.status, statusText: html.statusText, headers });
  },
};

export { acceptsMarkdown, markdownPath };
