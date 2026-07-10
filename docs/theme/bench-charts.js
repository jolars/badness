// Renders the benchmark dot plot(s) on the Benchmarks page with Vega-Lite.
// One block per `.bench-chart-block`, so the formatter and linter charts share
// this code (each carries its own inline data payload).
//
// Data is injected by the `doc-utils` mdbook preprocessor as an inline
// `<script type="application/json" class="bench-data">` next to a
// `<div class="bench-chart">` (see docs/doc-utils/src/lib.rs). The Vega runtime
// is vendored under theme/vendor/ and loaded before this file via book.toml's
// `additional-js`, so nothing is fetched at view time.
//
// Chart: x = tool, y = time relative to badness (log scale, baseline = 1),
// color = document, one dot per (document, tool), with a hover tooltip.
(function () {
  "use strict";

  // mdBook keeps the active theme as a class on <html>; these three are dark.
  function isDark() {
    var c = document.documentElement.classList;
    return c.contains("coal") || c.contains("navy") || c.contains("ayu");
  }

  // Unique values in first-appearance (corpus / results) order, so the axis and
  // legend read badness -> tex-fmt -> latexindent rather than alphabetized.
  function orderedUnique(rows, key) {
    var seen = Object.create(null);
    var out = [];
    rows.forEach(function (r) {
      if (!(r[key] in seen)) {
        seen[r[key]] = true;
        out.push(r[key]);
      }
    });
    return out;
  }

  function spec(points) {
    var dark = isDark();
    var fg = dark ? "#c8c9db" : "#333333";
    var grid = dark ? "#3b3f5c" : "#dddddd";
    var formatters = orderedUnique(points, "formatter");
    var documents = orderedUnique(points, "document");

    return {
      $schema: "https://vega.github.io/schema/vega-lite/v5.json",
      description:
        "Dot plot of formatting speed relative to badness. Each dot is one " +
        "document formatted by one tool; the vertical axis is mean time as a " +
        "ratio to badness on a log scale, with badness on a dashed baseline " +
        "at 1, faster tools below and slower tools above. See the data table " +
        "for the underlying numbers.",
      width: "container",
      height: 340,
      data: { values: points },
      layer: [
        // Baseline at 1.0 (badness); everything below is faster, above slower.
        {
          mark: { type: "rule", strokeDash: [4, 4], color: grid },
          encoding: { y: { datum: 1, type: "quantitative" } },
        },
        {
          mark: { type: "point", filled: true, size: 130, opacity: 0.9 },
          encoding: {
            x: {
              field: "formatter",
              type: "nominal",
              title: "Tool",
              sort: formatters,
              axis: { labelAngle: 0 },
            },
            // Dodge dots of different documents so same-ratio points (all the
            // badness dots sit at 1.0) don't stack on top of each other.
            xOffset: { field: "document", type: "nominal", sort: documents },
            y: {
              field: "ratio",
              type: "quantitative",
              title: "Time relative to badness",
              scale: { type: "log" },
              axis: { format: "~s" },
            },
            color: {
              field: "document",
              type: "nominal",
              title: "Document",
              sort: documents,
            },
            tooltip: [
              { field: "document", title: "Document" },
              { field: "formatter", title: "Tool" },
              { field: "mean_ms", title: "Mean (ms)", format: ".3f" },
              { field: "ratio_label", title: "Relative" },
              { field: "min_ms", title: "Min (ms)", format: ".3f" },
              { field: "max_ms", title: "Max (ms)", format: ".3f" },
              { field: "stddev_ms", title: "Std dev (ms)", format: ".3f" },
            ],
          },
        },
      ],
      config: {
        background: null,
        view: { stroke: null },
        axis: {
          labelColor: fg,
          titleColor: fg,
          gridColor: grid,
          domainColor: grid,
          tickColor: grid,
        },
        legend: { labelColor: fg, titleColor: fg },
      },
    };
  }

  function renderInto(container, points) {
    if (!window.vegaEmbed) {
      return;
    }
    var vlSpec = spec(points);
    // Alt text on the container, mirroring the spec description Vega puts on the
    // rendered SVG, so the chart is labeled for assistive tech either way.
    container.setAttribute("role", "img");
    container.setAttribute("aria-label", vlSpec.description);
    window
      .vegaEmbed(container, vlSpec, { actions: false, renderer: "svg" })
      .catch(function (err) {
        // Leave the fallback table in place; surface the reason for debugging.
        console.error("bench-charts: failed to render", err);
      });
  }

  function init() {
    var blocks = document.querySelectorAll(".bench-chart-block");
    if (!blocks.length) {
      return;
    }
    blocks.forEach(function (block) {
      var container = block.querySelector(".bench-chart");
      var data = block.querySelector("script.bench-data");
      if (!container || !data) {
        return;
      }
      var points;
      try {
        points = JSON.parse(data.textContent);
      } catch (err) {
        console.error("bench-charts: bad data payload", err);
        return;
      }
      if (!Array.isArray(points) || !points.length) {
        return;
      }
      container.__benchPoints = points;
      renderInto(container, points);
    });

    // Re-render on light/dark toggle so axis and legend colors track the theme.
    var observer = new MutationObserver(function () {
      document.querySelectorAll(".bench-chart").forEach(function (container) {
        if (container.__benchPoints) {
          renderInto(container, container.__benchPoints);
        }
      });
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
