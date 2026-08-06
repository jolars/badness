// Add a "Playground" link to the menu bar on every page (the playground is a
// standalone page outside the book, so it cannot live in SUMMARY.md). Loaded
// via additional-js in book.toml; runs after the menu bar exists.
"use strict";

(() => {
  const buttons = document.querySelector(".menu-bar .right-buttons");
  if (!buttons) return;
  // mdBook defines `path_to_root` as a page-level global.
  const root = typeof path_to_root === "string" ? path_to_root : "";
  const link = document.createElement("a");
  link.href = `${root}playground/index.html`;
  link.title = "Try the formatter in your browser";
  link.textContent = "Playground";
  link.classList.add("playground-link");
  buttons.prepend(link);
})();
