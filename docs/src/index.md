---
layout: home
hero:
  name: Badness
  tagline: A fast formatter, linter, and language server for LaTeX and BibTeX.
    Keep your source tidy and get feedback as you write.
  image:
    light: /images/logo.svg
    dark: /images/logo-dark.svg
    alt: Badness logo
  actions:
    - theme: brand
      text: Get started
      link: /guide/getting-started
    - theme: alt
      text: Try the playground
      link: /playground/
      target: _self
    - theme: alt
      text: View on GitHub
      link: https://github.com/jolars/badness
features:
  - title: Format your source
    details: Get consistent formatting for LaTeX and BibTeX, from papers and bibliographies to packages and expl3 code.
    link: /guide/formatting
  - title: Find and fix problems
    details: Catch syntax errors and common mistakes, apply fixes, and run the same checks locally and in CI.
    link: /guide/linting
  - title: Stay in your editor
    details: Get completions, diagnostics, navigation, and code actions through the language server, with formatting on save.
    link: /guide/editor-setup
---

## Quick start {#where-to-go-next}

Install Badness for your platform, then format and lint a file:

::: code-group

```sh [macOS/Linux]
curl --proto '=https' --tlsv1.2 -sSf https://badness.dev/install | sh
```

```powershell [Windows]
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://badness.dev/install.ps1 | iex"
```

:::

See the [installation guide](/guide/installation) for Homebrew, npm, uv, Cargo,
and other installation options.

After installation, you can run Badness from the command line:

```sh
badness format paper.tex
badness lint bibliography.bib
```

Using VS Code? Install the [Badness
extension](https://marketplace.visualstudio.com/items?itemName=jolars.badness),
which includes Badness. For other editors and configuration, see the [editor
setup guide](/guide/editor-setup).

## Built on your source {#the-architecture}

Badness parses your source into a lossless syntax tree and updates its analysis
as you type. The formatter preserves non-trivia content, and formatting an
already-formatted file changes nothing. The same parser powers the formatter,
linter, and language server.

See the [performance comparisons](/guide/performance) or try your own document
in the [playground](/playground/).

## Funding

I am grateful for the financial support offered by the [TeX Users
Group](https://tug.org/) through the [TeX Development
Fund](https://tug.org/tc/devfund/).
