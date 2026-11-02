export const redirects = {
  "/reference/benchmarks.html": "/guide/performance",
  "/guide/pre-commit.html": "/guide/integrations#pre-commit",
  "/reference/wrap-modes.html": "/reference/configuration#wrap",
  "/playground.html": "/playground/",
  "/development/parser.html": "/development/architecture#the-parser",
  "/development/formatter.html": "/development/architecture#the-formatter",
  "/development/linter.html": "/development/architecture#the-linter",
  "/development/lsp.html": "/development/architecture#the-language-server",
};
// These paths may already have been normalized by a proxy or browser.
for (const [from, to] of Object.entries(redirects)) {
  redirects[from.replace(/\.html$/, "")] = to;
}
