import assert from "node:assert/strict";
import test from "node:test";
import worker, { acceptsMarkdown } from "../markdown-worker.mjs";
import routes from "../.vitepress/dist/markdown-routes.json" with { type: "json" };
import { redirects } from "../scripts/routes.mjs";

const origin = "https://badness.dev";
const request = (path, init) => new Request(origin + path, init);

test("recognizes explicitly acceptable Markdown", () => {
  for (const value of [
    "text/markdown",
    "TEXT/MARKDOWN",
    "text/html, text/markdown;q=0.8",
  ])
    assert.ok(acceptsMarkdown(value));
  for (const value of [
    "text/html, */*",
    "text/markdown;q=0",
    "text/markdown;q=0.000",
    "text/markdown;q=no",
    "text/markdown;q=-1",
    "text/markdown;q=2",
  ])
    assert.ok(!acceptsMarkdown(value));
});

test("redirects every previous chapter and historical alias before fetching assets", async () => {
  for (const [from, to] of Object.entries({
    ...routes.redirects,
    ...redirects,
  })) {
    for (const method of ["GET", "HEAD"]) {
      const response = await worker.fetch(
        request(from + "?source=old", { method }),
        {
          ASSETS: {
            fetch() {
              throw new Error("redirect fetched an asset");
            },
          },
        },
      );
      const target = new URL(to, origin);
      target.search = "?source=old";
      assert.equal(response.status, 301, from);
      assert.equal(response.headers.get("Location"), target.href, from);
    }
  }
});

test("serves Markdown through ASSETS without reusing HTML validators or ranges", async () => {
  const calls = [];
  const env = {
    ASSETS: {
      async fetch(req) {
        calls.push(req);
        return new Response("# Installation\n", {
          headers: { "Content-Type": "text/plain", Vary: "Accept-Encoding" },
        });
      },
    },
  };
  const response = await worker.fetch(
    request("/guide/installation", {
      headers: {
        Accept: "text/markdown",
        "If-None-Match": '"html"',
        "If-Modified-Since": "yesterday",
        Range: "bytes=0-10",
        "If-Range": '"html"',
      },
    }),
    env,
  );
  assert.equal(await response.text(), "# Installation\n");
  assert.equal(
    response.headers.get("Content-Type"),
    "text/markdown; charset=utf-8",
  );
  assert.equal(response.headers.get("Vary"), "Accept-Encoding, Accept");
  assert.equal(
    new URL(calls[0].url).pathname,
    "/agent-markdown/guide/installation.md",
  );
  for (const name of [
    "If-None-Match",
    "If-Modified-Since",
    "Range",
    "If-Range",
  ])
    assert.equal(calls[0].headers.get(name), null);
});

test("preserves HTML defaults, HEAD responses, and conditional responses", async () => {
  for (const [method, status] of [
    ["GET", 200],
    ["HEAD", 200],
    ["GET", 304],
  ]) {
    const response = await worker.fetch(request("/", { method }), {
      ASSETS: {
        async fetch(req) {
          assert.equal(req.method, method);
          return new Response(
            method === "HEAD" || status === 304 ? null : "<h1>Badness</h1>",
            {
              status,
              headers: { "Content-Type": "text/html", Vary: "Accept-Encoding" },
            },
          );
        },
      },
    });
    assert.equal(response.status, status);
    assert.equal(response.headers.get("Vary"), "Accept-Encoding, Accept");
    assert.match(response.headers.get("Link"), /rel="service-doc"/);
    if (method === "HEAD" || status === 304)
      assert.equal(await response.text(), "");
  }
});

test("falls back to HTML when a Markdown export is unavailable", async () => {
  const response = await worker.fetch(
    request("/guide/installation", { headers: { Accept: "text/markdown" } }),
    {
      ASSETS: {
        async fetch(req) {
          return new URL(req.url).pathname.startsWith("/agent-markdown/")
            ? new Response(null, { status: 404 })
            : new Response("HTML", {
                headers: { "Content-Type": "text/html" },
              });
        },
      },
    },
  );
  assert.equal(await response.text(), "HTML");
});

test("leaves downloads, unknown paths, and unsupported methods to ASSETS", async () => {
  for (const path of [
    "/install",
    "/badness.schema.json",
    "/missing",
    "/agent-markdown/index.md",
  ]) {
    const response = await worker.fetch(
      request(path, { headers: { Accept: "text/markdown" } }),
      {
        ASSETS: {
          async fetch(req) {
            assert.equal(new URL(req.url).pathname, path);
            return new Response("asset", {
              status: path === "/missing" ? 404 : 200,
            });
          },
        },
      },
    );
    assert.equal(response.status, path === "/missing" ? 404 : 200);
  }
  const response = await worker.fetch(
    request("/guide/installation", {
      method: "POST",
      headers: { Accept: "text/markdown" },
    }),
    {
      ASSETS: {
        async fetch(req) {
          assert.equal(req.method, "POST");
          return new Response(null, { status: 405 });
        },
      },
    },
  );
  assert.equal(response.status, 405);
});
