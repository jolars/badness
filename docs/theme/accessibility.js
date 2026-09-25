"use strict";

(() => {
  // mdBook's empty unload handler prevents the browser's back/forward cache.
  window.onunload = null;

  const toggle = document.getElementById("mdbook-sidebar-toggle");
  const checkbox = document.getElementById("mdbook-sidebar-toggle-anchor");
  if (toggle?.tagName === "BUTTON" && checkbox) {
    // The postbuild step replaces the label; retain mdBook's checkbox state flow.
    toggle.addEventListener("click", () => checkbox.click());
  }

  const menu = document.getElementById("mdbook-menu-bar");
  menu?.setAttribute("role", "navigation");
  menu?.setAttribute("aria-label", "Documentation controls");
  document.querySelectorAll("pre > code").forEach((code) => {
    code.tabIndex = 0;
  });

  const sidebar = document.getElementById("mdbook-sidebar");
  if (!sidebar) return;
  const enhanceFolds = () => {
    // Keep every control out of the tab order while the sidebar closes.
    sidebar.inert = sidebar.getAttribute("aria-hidden") === "true";
    sidebar.querySelectorAll("a.chapter-fold-toggle").forEach((anchor) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = anchor.className;
      button.innerHTML = anchor.innerHTML;
      const title = anchor.parentElement
        .querySelector("a:not(.chapter-fold-toggle)")
        ?.textContent.trim();
      button.setAttribute("aria-label", `Toggle ${title || "section"}`);
      button.addEventListener("click", () => {
        button.closest("li").classList.toggle("expanded");
      });
      anchor.replaceWith(button);
    });
    sidebar.querySelectorAll("button.chapter-fold-toggle").forEach((button) => {
      button.setAttribute(
        "aria-expanded",
        button.closest("li").classList.contains("expanded"),
      );
    });
  };
  enhanceFolds();
  new MutationObserver(enhanceFolds).observe(sidebar, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["class", "aria-hidden"],
  });
})();
