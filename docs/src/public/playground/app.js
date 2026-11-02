// The docs playground: two CodeMirror editors over the badness-wasm formatter.
// Positions from the wasm side are UTF-16 code units, which are exactly
// CodeMirror document positions — no conversion here.

import {
  Compartment,
  EditorState,
  EditorView,
  StreamLanguage,
  defaultHighlightStyle,
  defaultKeymap,
  highlightActiveLine,
  history,
  historyKeymap,
  keymap,
  lintGutter,
  lineNumbers,
  setDiagnostics,
  stex,
  syntaxHighlighting,
  oneDark,
} from "./vendor/codemirror.js";
import initWasm, { check, format } from "./pkg/badness_wasm.js";

const SAMPLE_TEX = `\\documentclass{article}

\\begin{document}

\\section{Introduction}

Badness is a formatter, linter, and language server for LaTeX. It reflows prose to the configured line width, normalizes spacing in math, and never touches verbatim regions.
Every line break is decided by the layout engine, so the same input always produces the same output.

\\begin{itemize}
\\item A first item written as one deliberately long line so that the formatter has something to wrap the moment the page loads.
  \\item A short one.
\\end{itemize}

The Taylor series of $f$ about $a$:
\\[ f(x)=\\sum_{n=0}^{\\infty}\\frac{f^{(n)}(a)}{n!}(x-a)^n \\]

\\end{document}
`;

const SAMPLE_BIB = `@article{shannon1948,
author={Shannon, Claude E.}, title = {A Mathematical Theory of Communication},
  journal={Bell System Technical Journal},   year=1948, volume = {27},
 pages={379--423}
}

@book{knuth1984, author = {Knuth, Donald E.},
title={The {\\TeX}book}, publisher = {Addison-Wesley}, year = {1984}
}
`;

const DEBOUNCE_MS = 200;
let wasmReady;

export async function mountPlayground(root, signal) {
  const els = {
    filetype: root.querySelector("#pg-filetype"),
    lineWidth: root.querySelector("#pg-line-width"),
    indentWidth: root.querySelector("#pg-indent-width"),
    wrap: root.querySelector("#pg-wrap"),
    mathWrap: root.querySelector("#pg-math-wrap"),
    share: root.querySelector("#pg-share"),
    copy: root.querySelector("#pg-copy"),
    status: root.querySelector("#pg-status"),
    errors: root.querySelector("#pg-errors"),
    input: root.querySelector("#pg-input"),
    output: root.querySelector("#pg-output"),
  };

  // VitePress can navigate away while the shared wasm module is still loading.
  await (wasmReady ??= initWasm());
  if (signal.aborted) return;

  const isDark = () => document.documentElement.classList.contains("dark");
  const themeExt = () =>
    isDark()
      ? oneDark
      : syntaxHighlighting(defaultHighlightStyle, { fallback: true });

  const stexLang = StreamLanguage.define(stex);
  const inputTheme = new Compartment();
  const outputTheme = new Compartment();
  const inputLang = new Compartment();
  const outputLang = new Compartment();
  const langExt = () => (els.filetype.value === "bib" ? [] : stexLang);

  let debounce;
  const inputView = new EditorView({
    parent: els.input,
    state: EditorState.create({
      doc: SAMPLE_TEX,
      extensions: [
        EditorView.contentAttributes.of({
          "aria-labelledby": "pg-input-label",
          tabindex: "0",
        }),
        lineNumbers(),
        history(),
        highlightActiveLine(),
        keymap.of([...defaultKeymap, ...historyKeymap]),
        inputLang.of(stexLang),
        inputTheme.of(themeExt()),
        lintGutter(),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            clearTimeout(debounce);
            debounce = setTimeout(run, DEBOUNCE_MS);
          }
        }),
      ],
    }),
  });
  const outputView = new EditorView({
    parent: els.output,
    state: EditorState.create({
      doc: "",
      extensions: [
        EditorView.contentAttributes.of({
          "aria-labelledby": "pg-output-label",
          tabindex: "0",
        }),
        lineNumbers(),
        outputLang.of(stexLang),
        outputTheme.of(themeExt()),
        EditorState.readOnly.of(true),
        EditorView.editable.of(false),
      ],
    }),
  });

  const themeObserver = new MutationObserver(() => {
    inputView.dispatch({ effects: inputTheme.reconfigure(themeExt()) });
    outputView.dispatch({ effects: outputTheme.reconfigure(themeExt()) });
  });
  themeObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["class"],
  });

  const setStatus = (text) => {
    if (!signal.aborted) els.status.textContent = text;
  };

  const options = () => ({
    ft: els.filetype.value,
    lineWidth: clampedNumber(els.lineWidth),
    indentWidth: clampedNumber(els.indentWidth),
    wrap: els.wrap.value,
    mathWrap: els.mathWrap.value,
  });

  function run() {
    const text = inputView.state.doc.toString();
    const { ft, lineWidth, indentWidth, wrap, mathWrap } = options();

    let diags;
    try {
      diags = check(text, ft).map((d) => {
        const plain = {
          message: d.message,
          start: d.start,
          end: d.end,
          line: d.line,
          column: d.column,
        };
        d.free();
        return plain;
      });
    } catch (e) {
      setStatus(e.message ?? String(e));
      return;
    }

    inputView.dispatch(
      setDiagnostics(
        inputView.state,
        diags.map((d) => ({
          from: d.start,
          to: Math.max(d.end, d.start + 1),
          severity: "error",
          message: d.message,
        })),
      ),
    );
    els.errors.replaceChildren(
      ...diags.map((d) => {
        const li = document.createElement("li");
        li.textContent = `${d.line}:${d.column} ${d.message}`;
        return li;
      }),
    );

    if (diags.length > 0) {
      setStatus(
        `${diags.length} parse error${diags.length === 1 ? "" : "s"} — the formatter needs a clean parse.`,
      );
      return;
    }

    try {
      const out = format(text, ft, lineWidth, indentWidth, wrap, mathWrap);
      outputView.dispatch({
        changes: { from: 0, to: outputView.state.doc.length, insert: out },
      });
      setStatus("");
    } catch (e) {
      setStatus(e.message ?? String(e));
    }
  }

  function clampedNumber(el) {
    const n = Number.parseInt(el.value, 10);
    if (Number.isNaN(n)) return undefined;
    return Math.min(Math.max(n, Number(el.min)), Number(el.max));
  }

  // Each file type has its own default wrap (mirroring the CLI); annotate that
  // option's label, and gray out the LaTeX-only controls for .bib.
  const defaultWrapFor = (ft) => (ft === "tex" ? "reflow" : "preserve");

  function syncControls() {
    const ft = els.filetype.value;
    const defaultWrap = defaultWrapFor(ft);
    for (const opt of els.wrap.options) {
      opt.textContent =
        opt.value === defaultWrap ? `${opt.value} (default)` : opt.value;
    }
    const isBib = ft === "bib";
    els.wrap.disabled = isBib;
    els.mathWrap.disabled = isBib;
  }

  function swapSampleIfPristine(previous, next) {
    const doc = inputView.state.doc.toString();
    const pristine = doc === SAMPLE_TEX || doc === SAMPLE_BIB;
    const wantBib = next === "bib";
    const wasBib = previous === "bib";
    if (pristine && wantBib !== wasBib) {
      inputView.dispatch({
        changes: {
          from: 0,
          to: inputView.state.doc.length,
          insert: wantBib ? SAMPLE_BIB : SAMPLE_TEX,
        },
      });
    }
  }

  // Options (not the document) round-trip through the URL so a configuration
  // can be shared.
  const PARAMS = [
    { key: "ft", el: els.filetype },
    { key: "lw", el: els.lineWidth },
    { key: "iw", el: els.indentWidth },
    { key: "wrap", el: els.wrap },
    { key: "mw", el: els.mathWrap },
  ];

  function applyQueryString() {
    const params = new URLSearchParams(window.location.search);
    for (const { key, el } of PARAMS) {
      const value = params.get(key);
      if (value === null) continue;
      const valid =
        el.tagName === "SELECT"
          ? [...el.options].some((o) => o.value === value)
          : value !== "";
      if (valid) el.value = value;
    }
  }

  function updateQueryString() {
    const params = new URLSearchParams();
    for (const { key, el } of PARAMS) {
      if (el.value !== "" && !el.disabled) params.set(key, el.value);
    }
    const url = new URL(window.location.href);
    url.search = params.toString();
    // NB: `history` is CodeMirror's history extension here; qualify the global.
    window.history.replaceState(window.history.state, "", url);
  }

  let previousFt = els.filetype.value;
  for (const { el } of PARAMS) {
    el.addEventListener(
      "change",
      () => {
        if (el === els.filetype) {
          swapSampleIfPristine(previousFt, els.filetype.value);
          previousFt = els.filetype.value;
          els.wrap.value = defaultWrapFor(els.filetype.value);
          inputView.dispatch({ effects: inputLang.reconfigure(langExt()) });
          outputView.dispatch({ effects: outputLang.reconfigure(langExt()) });
        }
        syncControls();
        updateQueryString();
        run();
      },
      { signal },
    );
  }

  els.copy.addEventListener(
    "click",
    async () => {
      await navigator.clipboard.writeText(outputView.state.doc.toString());
      setStatus("Output copied to clipboard.");
    },
    { signal },
  );
  els.share.addEventListener(
    "click",
    async () => {
      await navigator.clipboard.writeText(window.location.href);
      setStatus("Link copied to clipboard.");
    },
    { signal },
  );

  applyQueryString();
  if (els.filetype.value !== previousFt) {
    swapSampleIfPristine(previousFt, els.filetype.value);
    previousFt = els.filetype.value;
    inputView.dispatch({ effects: inputLang.reconfigure(langExt()) });
    outputView.dispatch({ effects: outputLang.reconfigure(langExt()) });
  }
  // A file type from the URL brings its own wrap default along unless the URL
  // pinned one explicitly.
  if (!new URLSearchParams(window.location.search).has("wrap")) {
    els.wrap.value = defaultWrapFor(els.filetype.value);
  }
  syncControls();
  run();

  return () => {
    clearTimeout(debounce);
    themeObserver.disconnect();
    inputView.destroy();
    outputView.destroy();
  };
}
