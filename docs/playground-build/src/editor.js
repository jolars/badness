// Re-export entry for the vendored CodeMirror bundle used by the docs
// playground (docs/src/playground/app.js). esbuild bundles and tree-shakes
// this into docs/src/playground/vendor/codemirror.js — keep the export list
// to what app.js actually uses.

export {
  EditorView,
  keymap,
  lineNumbers,
  highlightActiveLine,
} from "@codemirror/view";
export { EditorState, Compartment } from "@codemirror/state";
export { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
export {
  StreamLanguage,
  syntaxHighlighting,
  defaultHighlightStyle,
} from "@codemirror/language";
export { stex } from "@codemirror/legacy-modes/mode/stex";
export { setDiagnostics, lintGutter } from "@codemirror/lint";
export { oneDark } from "@codemirror/theme-one-dark";
