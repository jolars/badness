import assert from "node:assert/strict";
import test from "node:test";
import worker, { acceptsMarkdown, markdownPath } from "./markdown-worker.mjs";

test("maps chapter URLs and leaves non-pages alone", () => {
  assert.equal(markdownPath("/"), "/agent-markdown/index.md");
  assert.equal(markdownPath("/guide/installation.html"), "/agent-markdown/guide/installation.md");
  assert.equal(markdownPath("/guide/installation.md"), "/agent-markdown/guide/installation.md");
  assert.equal(markdownPath("/guide/"), "/agent-markdown/guide/index.md");
  assert.equal(markdownPath("/install"), null);
  assert.equal(markdownPath("/agent-markdown/index.md"), null);
});

test("recognizes an explicitly acceptable Markdown media type", () => {
  assert.equal(acceptsMarkdown("text/html, text/markdown;q=0.8"), true);
  assert.equal(acceptsMarkdown("TEXT/MARKDOWN"), true);
  assert.equal(acceptsMarkdown("text/markdown;q=0"), false);
  assert.equal(acceptsMarkdown("text/html, */*"), false);
});

test("serves Markdown and preserves HTML as the default", async () => {
  const requests = [];
  const previousFetch = globalThis.fetch;
  globalThis.fetch = async (request) => {
    const url = typeof request === "string" ? request : request.url;
    requests.push(url);
    return url.includes("/agent-markdown/")
      ? new Response("# Installation\n", { headers: { "Content-Type": "text/plain" } })
      : new Response("<h1>Installation</h1>", { headers: { "Content-Type": "text/html", "Vary": "Accept-Encoding" } });
  };
  try {
    const url = "https://badness.dev/guide/installation.html";
    const markdown = await worker.fetch(new Request(url, { headers: { Accept: "text/markdown" } }));
    assert.equal(markdown.headers.get("Content-Type"), "text/markdown; charset=utf-8");
    assert.equal(markdown.headers.get("Vary"), "Accept");
    assert.equal(await markdown.text(), "# Installation\n");
    const html = await worker.fetch(new Request(url));
    assert.equal(html.headers.get("Content-Type"), "text/html");
    assert.equal(html.headers.get("Vary"), "Accept-Encoding, Accept");
    assert.equal(await html.text(), "<h1>Installation</h1>");
    assert.deepEqual(requests, ["https://badness.dev/agent-markdown/guide/installation.md", url]);
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test("Markdown chapter links still open HTML in browsers", async () => {
  const previousFetch = globalThis.fetch;
  let requestedUrl;
  globalThis.fetch = async (request) => {
    requestedUrl = request.url;
    return new Response("<h1>Installation</h1>", { headers: { "Content-Type": "text/html" } });
  };
  try {
    const response = await worker.fetch(new Request("https://badness.dev/guide/installation.md"));
    assert.equal(requestedUrl, "https://badness.dev/guide/installation.html");
    assert.equal(response.headers.get("Content-Type"), "text/html");
  } finally {
    globalThis.fetch = previousFetch;
  }
});
