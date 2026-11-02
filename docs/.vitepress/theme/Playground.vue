<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { withBase } from "vitepress";

const root = ref<HTMLElement | null>(null);
const status = ref<HTMLElement | null>(null);
let controller: AbortController | undefined;
let dispose: (() => void) | undefined;

onMounted(async () => {
  controller = new AbortController();
  const { signal } = controller;
  try {
    const { mountPlayground } = await import(
      /* @vite-ignore */ withBase("/playground/app.js")
    );
    if (signal.aborted || !root.value) return;
    const cleanup = await mountPlayground(root.value, signal);
    if (signal.aborted) cleanup?.();
    else dispose = cleanup;
  } catch (error) {
    if (!signal.aborted && status.value) {
      status.value.textContent = `Failed to load the playground: ${error instanceof Error ? error.message : error}`;
    }
  }
});

onBeforeUnmount(() => {
  controller?.abort();
  dispose?.();
});
</script>

<template>
  <section ref="root" class="badness-playground" aria-labelledby="pg-title">
    <header class="pg-header">
      <div>
        <h1 id="pg-title">Playground</h1>
        <p>Try Badness on LaTeX or BibTeX. Your code stays in your browser.</p>
      </div>
      <a :href="withBase('/reference/configuration')">Options reference</a>
    </header>
    <noscript>The playground requires JavaScript and WebAssembly.</noscript>
    <div class="pg-controls">
      <label>
        File type
        <select id="pg-filetype">
          <option value="tex" selected>.tex</option>
          <option value="sty-cls">.sty / .cls</option>
          <option value="dtx">.dtx</option>
          <option value="bib">.bib</option>
        </select>
      </label>
      <label>
        Line width (0 = unlimited)
        <input id="pg-line-width" type="number" min="0" max="200" value="80" />
      </label>
      <label>
        Indent
        <input id="pg-indent-width" type="number" min="0" max="8" value="2" />
      </label>
      <label>
        Wrap
        <select id="pg-wrap">
          <option value="reflow" selected>reflow (default)</option>
          <option value="stable">stable</option>
          <option value="sentence">sentence</option>
          <option value="semantic">semantic</option>
          <option value="preserve">preserve</option>
        </select>
      </label>
      <label>
        Math wrap
        <select id="pg-math-wrap">
          <option value="auto" selected>auto</option>
          <option value="preserve">preserve</option>
          <option value="single-line">single-line</option>
          <option value="break">break</option>
        </select>
      </label>
      <button
        id="pg-share"
        class="pg-share"
        type="button"
        title="Copy a link with the current options"
      >
        Share
      </button>
    </div>
    <div class="pg-panes">
      <div class="pg-pane">
        <div class="pg-pane-title"><span id="pg-input-label">Input</span></div>
        <div id="pg-input" class="pg-editor"></div>
      </div>
      <div class="pg-pane">
        <div class="pg-pane-title">
          <span id="pg-output-label">Output</span>
          <button id="pg-copy" type="button" title="Copy the formatted output">
            Copy
          </button>
        </div>
        <div id="pg-output" class="pg-editor"></div>
      </div>
    </div>
    <div id="pg-status" ref="status" role="status" aria-live="polite">
      Loading the playground…
    </div>
    <ul id="pg-errors" class="pg-errors"></ul>
  </section>
</template>

<style scoped>
.badness-playground {
  display: flex;
  flex-direction: column;
  gap: 16px;
  width: 100%;
  max-width: 1600px;
  margin: 0 auto;
  padding: 32px 24px 24px;
}

.pg-header {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 8px;
}

.pg-header h1 {
  font-size: 28px;
  font-weight: 600;
  line-height: 1.4;
}

.pg-header p {
  margin-top: 8px;
  color: var(--vp-c-text-2);
}

.pg-header a {
  color: var(--vp-c-brand-1);
  font-size: 14px;
}

.pg-header a:hover {
  text-decoration: underline;
}

.pg-controls {
  display: flex;
  flex-wrap: wrap;
  align-items: end;
  gap: 12px;
}

.pg-controls label {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 14px;
}

.pg-controls select,
.pg-controls input {
  color: var(--vp-c-text-1);
  background: var(--vp-c-bg);
  border: 1px solid var(--vp-c-divider);
  border-radius: 6px;
  padding: 4px 8px;
  font: inherit;
}

.pg-controls input[type="number"] {
  width: 72px;
}

.pg-controls select:disabled {
  opacity: 0.5;
}

.pg-share {
  margin-left: auto;
}

button {
  padding: 4px 12px;
  color: var(--vp-c-text-1);
  background: var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-divider);
  border-radius: 6px;
  font-size: 14px;
  cursor: pointer;
}

button:hover {
  border-color: var(--vp-c-brand-1);
}

button:focus-visible,
select:focus-visible,
input:focus-visible {
  outline: 2px solid var(--vp-c-brand-1);
  outline-offset: 2px;
}

.pg-panes {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: 16px;
  height: max(28rem, calc(100dvh - var(--vp-nav-height) - 18rem));
}

.pg-pane {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  border: 1px solid var(--vp-c-divider);
  border-radius: 8px;
  overflow: hidden;
}

.pg-pane-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
  min-height: 42px;
  padding: 4px 12px;
  background: var(--vp-c-bg-soft);
  border-bottom: 1px solid var(--vp-c-divider);
  font-size: 13px;
  font-weight: 600;
}

.pg-editor {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

.pg-editor :deep(.cm-editor) {
  height: 100%;
  background: var(--vp-c-bg);
  font-size: 13px;
}

.pg-editor :deep(.cm-scroller) {
  overflow: auto;
  font-family: var(--vp-font-family-mono);
}

.pg-editor :deep(.cm-editor.cm-focused) {
  outline: 2px solid var(--vp-c-brand-1);
  outline-offset: -2px;
}

#pg-status {
  min-height: 24px;
  font-size: 14px;
}

.pg-errors {
  margin: 0;
  padding-left: 24px;
  font-family: var(--vp-font-family-mono);
  font-size: 14px;
  list-style: disc;
}

.pg-errors:empty {
  display: none;
}

@media (max-width: 900px) {
  .pg-panes {
    grid-template-columns: minmax(0, 1fr);
    height: auto;
  }

  .pg-editor {
    flex: none;
    height: 24rem;
  }
}
</style>
