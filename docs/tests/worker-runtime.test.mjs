import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { readFileSync } from "node:fs";
import { createServer } from "node:net";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { after, before, test } from "node:test";
import { docsRoot, projectRoot } from "../scripts/content.mjs";
import { redirects } from "../scripts/routes.mjs";
import routes from "../.vitepress/dist/markdown-routes.json" with { type: "json" };

let worker, exited, origin;
let output = "";
before(async () => {
  const socket = createServer();
  socket.listen(0, "127.0.0.1");
  await once(socket, "listening");
  const { port } = socket.address();
  await new Promise((resolve) => socket.close(resolve));
  origin = `http://127.0.0.1:${port}`;
  worker = spawn(
    process.execPath,
    [
      join(docsRoot, "node_modules/wrangler/bin/wrangler.js"),
      "dev",
      "--local",
      "--ip",
      "127.0.0.1",
      "--port",
      String(port),
      "--show-interactive-dev-session=false",
    ],
    {
      cwd: docsRoot,
      env: { ...process.env, WRANGLER_SEND_METRICS: "false" },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  exited = once(worker, "exit");
  worker.stdout.on("data", (chunk) => {
    output += chunk;
  });
  worker.stderr.on("data", (chunk) => {
    output += chunk;
  });
  const deadline = Date.now() + 60000;
  while (Date.now() < deadline && worker.exitCode === null) {
    try {
      if ((await fetch(origin, { signal: AbortSignal.timeout(1000) })).ok)
        return;
    } catch {
      /* Wait for the local listener. */
    }
    await delay(100);
  }
  throw new Error(`Worker did not become ready:\n${output}`);
});
after(async () => {
  if (!worker || worker.exitCode !== null) return;
  worker.kill("SIGTERM");
  await Promise.race([exited, delay(3000)]);
  if (worker.exitCode === null) {
    worker.kill("SIGKILL");
    await exited;
  }
});

test("runtime redirects every legacy URL without dropping queries or destination fragments", async () => {
  for (const [from, to] of Object.entries({
    ...routes.redirects,
    ...redirects,
  })) {
    const response = await fetch(origin + from + "?source=legacy", {
      redirect: "manual",
    });
    assert.equal(response.status, 301, from);
    const destination = new URL(to, origin);
    destination.search = "?source=legacy";
    assert.equal(response.headers.get("location"), destination.href, from);
    assert.equal((await fetch(destination)).status, 200, to);
  }
});

test("runtime negotiates HTML and Markdown, including HEAD and borrowed HTML validators", async () => {
  const html = await fetch(origin + "/guide/installation");
  assert.equal(html.status, 200);
  assert.match(html.headers.get("content-type"), /text\/html/);
  assert.match(html.headers.get("vary"), /Accept/i);
  for (const method of ["GET", "HEAD"]) {
    const response = await fetch(origin + "/guide/installation", {
      method,
      headers: {
        Accept: "text/markdown",
        "If-None-Match": html.headers.get("etag"),
        Range: "bytes=0-8",
      },
    });
    assert.equal(response.status, 200);
    assert.match(response.headers.get("content-type"), /text\/markdown/);
    const body = await response.text();
    if (method === "HEAD") assert.equal(body, "");
    else assert.match(body, /^# Installation/);
  }
  const home = await fetch(origin, { headers: { Accept: "text/markdown" } });
  assert.match(home.headers.get("link"), /rel="service-doc"/);
  for (const path of [
    "/guide/installation.md",
    "/agent-markdown/guide/installation.md",
  ]) {
    const response = await fetch(origin + path);
    assert.match(response.headers.get("content-type"), /text\/markdown/);
    assert.match(await response.text(), /^# Installation/);
  }
});

test("runtime serves exact installers and schema, and genuine 404s", async () => {
  for (const [path, source] of [
    ["/install", "scripts/badness-installer.sh"],
    ["/install.ps1", "scripts/badness-installer.ps1"],
    ["/badness.schema.json", "badness.schema.json"],
  ]) {
    const response = await fetch(origin + path);
    assert.equal(response.status, 200);
    assert.deepEqual(
      Buffer.from(await response.arrayBuffer()),
      readFileSync(join(projectRoot, source)),
    );
    if (path.endsWith(".json")) {
      assert.equal(response.headers.get("access-control-allow-origin"), "*");
      assert.match(response.headers.get("content-type"), /application\/json/);
    }
  }
  for (const path of [
    "/missing",
    "/missing.html",
    "/guide/installation/unknown",
    "/markdown-routes.json",
  ]) {
    const response = await fetch(origin + path, {
      headers: { Accept: "text/markdown" },
    });
    assert.equal(response.status, 404, path);
  }
  for (const path of [
    "/playground/",
    "/playground/pkg/badness_wasm_bg.wasm",
    "/playground/vendor/codemirror.js",
  ])
    assert.equal((await fetch(origin + path)).status, 200, path);
  const playground = await fetch(origin + "/playground", {
    redirect: "manual",
  });
  assert.ok([301, 307, 308].includes(playground.status));
  assert.equal(
    new URL(playground.headers.get("location"), origin).pathname,
    "/playground/",
  );
});
