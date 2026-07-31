# Badness

A language server, formatter, and linter for LaTeX.

## Quick start

1. Install the **Badness** extension.
2. Open a LaTeX (`.tex`) file.
3. The extension starts `badness lsp` automatically.

By default, the extension uses a `badness` binary that ships inside the
extension itself, so the language server starts on first activation even on
restricted or offline networks.

## Features

- Starts `badness lsp` automatically when you open supported documents.
- Formats documents using Badness's deterministic, rule-based formatter.
- Surfaces Badness diagnostics in the editor.
- Works for LaTeX (`.tex`) and related TeX/BibTeX files.

## Commands

- `Badness: Restart Server`: stops and restarts the Badness language server
  (re-reads settings and re-resolves the binary). Useful if the LSP gets wedged
  or after changing settings such as `badness.version` or
  `badness.executablePath`.

## Binary installation

By default, the extension uses a `badness` binary that ships inside the
extension (one platform-specific VSIX per OS/architecture). No download, no
GitHub round-trip, and the language server starts on first activation even on
restricted or offline networks. Behavior is controlled by
`badness.executableStrategy`:

- `bundled` (default): use the binary that ships inside the extension. If
  you're on a platform without a platform-specific build (or you've installed
  the universal VSIX), the extension falls back to downloading a matching binary
  from GitHub releases.
- `environment`: look for `badness` on the system `PATH`.
- `path`: use the binary at `badness.executablePath`.

If you set `badness.version` or `badness.releaseTag` explicitly, the bundled
binary is skipped and the requested version is downloaded from GitHub. When
`badness.version` is `latest`, the extension automatically selects the most
recent stable release that contains a matching platform asset.

## Common setup examples

Use a local binary at a fixed path:

```json
{
  "badness.executableStrategy": "path",
  "badness.executablePath": "/usr/local/bin/badness"
}
```

Use whatever `badness` is on your `PATH`:

```json
{
  "badness.executableStrategy": "environment"
}
```

Pin to a specific release:

```json
{
  "badness.version": "0.2.0",
  "badness.githubRepo": "jolars/badness"
}
```

Use `badness.releaseTag` only if you need an exact tag override:

```json
{
  "badness.releaseTag": "v0.2.0"
}
```

## Requirements and troubleshooting

- **NixOS**: the bundled binary won't run because of the dynamic loader path.
  Set `badness.executableStrategy` to `path` (with `badness.executablePath`) or
  `environment` if `badness` is on your `PATH`.
- **Offline, restricted networks, or proxies**: the bundled-binary default works
  without network access. Only the explicit-version download paths
  (`badness.version`/`badness.releaseTag`) require GitHub connectivity.
- If a download fall-through fails, the extension shows a warning and falls back
  to looking up `badness` on the system `PATH`.

## Choosing which features to use

Badness bundles a formatter, a linter, and language features (hover, completion,
navigation, and so on) behind one language server. Each can be turned off
independently, so you can use just the parts you want:

- `badness.formatting.enable` (default `true`): use Badness as a formatter. Set
  to `false` to let another extension own LaTeX formatting without disabling
  Badness.
- `badness.diagnostics.enable` (default `true`): show Badness diagnostics (the
  linter). Set to `false` to suppress **all** squiggles, including the
  syntax/parse errors that a `badness.toml` `[lint]` selection cannot silence.
- `badness.languageFeatures.enable` (default `true`): hover, completion,
  signature help, go-to-definition, references, symbols, rename, code actions,
  folding, selection ranges, and document links.

For a formatter-only setup (the common "I just want the formatter" case):

```json
{
  "badness.diagnostics.enable": false,
  "badness.languageFeatures.enable": false
}
```

The language server keeps running either way—these are client-side gates—so
formatting stays available and the toggles take effect without reinstalling
anything.

## Settings

Badness registers itself as the default formatter for `[latex]` files.

- `badness.formatting.enable`: use Badness as a formatter (default `true`).
- `badness.diagnostics.enable`: show Badness diagnostics/the linter (default
  `true`).
- `badness.languageFeatures.enable`: enable hover, completion, navigation, and
  the other language features (default `true`).
- `badness.executableStrategy`: how to locate the `badness` binary—`bundled`
  (default), `environment`, or `path`.
- `badness.executablePath`: path to the binary, used only when
  `executableStrategy` is `path`.
- `badness.version`: version to install (default: `"latest"`)
- `badness.releaseTag`: advanced exact tag override (takes precedence if
  explicitly set)
- `badness.githubRepo`: GitHub repo for downloads (default: `"jolars/badness"`)
- `badness.serverArgs`: extra args after `badness lsp`
- `badness.serverEnv`: extra environment variables
- `badness.extraPath`: extra PATH entries prepended for the language server
  process
- `badness.logLevel`: log level for the language server, mapped to `RUST_LOG`
  (`off`, `error`, `warn`, `info`, `debug`, `trace`; unset by default).
  `badness.serverEnv.RUST_LOG` overrides this if both are set.
- `badness.trace.server`: LSP trace level (`off`, `messages`, `verbose`)

## Security and trust

When `badness.executableStrategy` is `bundled` (the default), the extension
prefers the binary that shipped inside the VSIX. If no bundled binary is
available, or `badness.version`/`badness.releaseTag` is set explicitly, it
downloads from GitHub releases configured by `badness.githubRepo` (default
`jolars/badness`).
