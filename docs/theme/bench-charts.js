// Renders benchmark figures with Vega-Lite. Each `.bench-chart-block` carries
// its own inline data payload and an optional `data-chart` for LSP figures.
//
// Data is injected by the `doc-utils` mdbook preprocessor as an inline
// `<script type="application/json" class="bench-data">` next to a
// `<div class="bench-chart">` (see docs/doc-utils/src/lib.rs). The Vega runtime
// is vendored under theme/vendor/ and loaded before this file via book.toml's
// `additional-js`, so nothing is fetched at view time.
//
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

  // Whole decades keep the log scale easy to read; a small margin keeps dots
  // at the domain boundaries clear of the plot edges.
  function logDomain(points, field) {
    var values = points
      .map(function (p) {
        return p[field];
      })
      .filter(function (value) {
        return Number.isFinite(value) && value > 0;
      });
    return logExtent(values.concat([1]));
  }

  function logExtent(values) {
    values = values.filter(function (value) {
      return Number.isFinite(value) && value > 0;
    });
    if (!values.length) return [0.1, 10];
    var lo = Math.floor(Math.log10(Math.min.apply(null, values)));
    var hi = Math.ceil(Math.log10(Math.max.apply(null, values)));
    if (lo === hi) {
      lo--;
      hi++;
    }
    return [Math.pow(10, lo) / 1.1, Math.pow(10, hi) * 1.1];
  }

  // Match ggplot2's log ticks: long at powers of ten, medium at five, and
  // short at the other subdivisions. Only powers of ten receive labels.
  function logAxis(domain, color, absolute) {
    var ticks = [];
    var major = [];
    var middle = [];
    for (
      var e = Math.floor(Math.log10(domain[0]));
      e <= Math.ceil(Math.log10(domain[1]));
      e++
    ) {
      for (var m = 1; m < 10; m++) {
        var value = m * Math.pow(10, e);
        if (value >= domain[0] && value <= domain[1]) {
          ticks.push(value);
          if (m === 1) major.push(value);
          if (m === 5) middle.push(value);
        }
      }
    }
    var isMajor = "indexof(" + JSON.stringify(major) + ", datum.value) >= 0";
    var isMiddle = "indexof(" + JSON.stringify(middle) + ", datum.value) >= 0";
    return {
      values: ticks,
      labelExpr: isMajor + " ? format(datum.value, ',~g') : ''",
      labelOverlap: false,
      labelPadding: 4,
      tickColor: color,
      tickSize: {
        condition: [
          { test: isMajor, value: 9 },
          { test: isMiddle, value: 6 },
        ],
        value: 3,
      },
      grid: true,
      gridOpacity: {
        condition: {
          test: isMajor + (absolute ? "" : " && datum.value !== 1"),
          value: 1,
        },
        value: 0,
      },
    };
  }

  function chartConfig() {
    var dark = isDark();
    var fg = dark ? "#c8c9db" : "#333333";
    var grid = dark ? "#3b3f5c" : "#dddddd";
    return {
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
    };
  }

  function spec(points) {
    var dark = isDark();
    var fg = dark ? "#c8c9db" : "#333333";
    var grid = dark ? "#3b3f5c" : "#dddddd";
    var formatters = orderedUnique(points, "formatter");
    var documents = orderedUnique(points, "document");
    var domain = logDomain(points, "ratio");

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
            // No xOffset: dots for every document share their tool's x position
            // so they stack vertically at their respective ratios. Color still
            // distinguishes documents, and hover disambiguates overlaps.
            y: {
              field: "ratio",
              type: "quantitative",
              title: "Time relative to badness",
              scale: { type: "log", domain: domain, nice: false },
              axis: logAxis(domain, fg),
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
      config: chartConfig(),
    };
  }

  function lspSpec(points, kind, caption, compact) {
    var memory = kind === "lsp-memory";
    var latency = kind === "lsp-latency";
    var servers = orderedUnique(points, "server");
    var metrics = orderedUnique(points, "metric");
    var color = {
      field: "server",
      type: "nominal",
      title: null,
      scale: { domain: servers, range: ["#4e79a7", "#f28e2c"] },
      legend: { orient: "top", direction: "horizontal" },
    };
    var tooltip = [
      { field: "server", title: "Server" },
      { field: "metric", title: memory ? "Milestone" : "Operation" },
    ];
    var chart = {
      $schema: "https://vega.github.io/schema/vega-lite/v5.json",
      description: caption,
      width: "container",
      height: memory ? 260 : metrics.length * 60,
      data: { values: points },
      config: chartConfig(),
    };
    if (memory) {
      tooltip.push(
        { field: "rss_mb", title: "RSS (MB)", format: ".1f" },
        { field: "pss_mb", title: "PSS (MB)", format: ".1f" },
      );
      chart.encoding = {
        x: {
          field: "metric",
          type: "nominal",
          sort: metrics,
          title: null,
          axis: { labelAngle: 0 },
        },
        xOffset: { field: "server", sort: servers },
        y: {
          field: "rss_mb",
          type: "quantitative",
          title: "Median process-tree RSS (MB)",
          scale: { zero: true },
        },
      };
      chart.layer = [
        {
          mark: { type: "bar", tooltip: true },
          encoding: { color: color, tooltip: tooltip },
        },
        {
          mark: { type: "text", dy: -8, color: chart.config.axis.labelColor },
          encoding: { text: { field: "rss_mb", format: ".1f" } },
        },
      ];
      return chart;
    }

    var lower = latency ? "median_ms" : "min_ms";
    var upper = latency ? "p95_ms" : "max_ms";
    var domain = logExtent(
      points.flatMap(function (point) {
        return [point.median_ms, point[lower], point[upper]];
      }),
    );
    tooltip.push({ field: "median_ms", title: "Median (ms)", format: ".3f" });
    if (latency) {
      tooltip.push(
        { field: "p95_ms", title: "p95 (ms)", format: ".3f" },
        { field: "samples", title: "Samples" },
        { field: "returned_work", title: "Returned work" },
        { field: "payload_bytes_median", title: "Median result size (bytes)" },
        { field: "failures", title: "Failed requests" },
        { field: "empty_results", title: "Empty results" },
      );
    } else {
      tooltip.push(
        { field: "min_ms", title: "Min (ms)", format: ".3f" },
        { field: "max_ms", title: "Max (ms)", format: ".3f" },
      );
    }
    chart.encoding = {
      y: {
        field: "metric",
        type: "nominal",
        sort: metrics,
        title: null,
        axis: {
          labelLimit: compact ? 85 : 150,
          labelExpr: compact
            ? "replace(replace(replace(replace(replace(datum.label, 'Document symbols', 'Symbols'), 'Go to definition', 'Definition'), 'Find references', 'References'), 'Workspace ready', 'Workspace'), 'Open files ready', 'Open files')"
            : "datum.label",
        },
      },
      // Separate servers within a row so equal timings remain visible.
      yOffset: { field: "server", sort: servers },
      x: {
        field: "median_ms",
        type: "quantitative",
        title: latency
          ? "Request latency (ms, log scale)"
          : "Elapsed time (ms, log scale)",
        scale: { type: "log", domain: domain, nice: false },
        axis: logAxis(domain, chart.config.axis.labelColor, true),
      },
      color: color,
      tooltip: tooltip,
    };
    chart.layer = [
      {
        transform: [
          { filter: "datum." + lower + " > 0 && datum." + upper + " > 0" },
        ],
        mark: { type: "rule", strokeWidth: 2 },
        encoding: { x: { field: lower }, x2: { field: upper } },
      },
      {
        transform: [{ filter: "datum.median_ms > 0" }],
        mark: { type: "point", filled: true, size: 85, opacity: 1 },
        encoding: { shape: { field: "server", scale: { domain: servers } } },
      },
    ];
    return chart;
  }

  const vendorBase = new URL("../vendor/", document.currentScript.src);

  function loadScript(name) {
    return new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = new URL(name, vendorBase).href;
      script.onload = resolve;
      script.onerror = () => reject(new Error(`Could not load ${script.src}`));
      document.head.appendChild(script);
    });
  }

  function renderInto(container) {
    if (!window.vegaEmbed) {
      return;
    }
    var block = container.closest(".bench-chart-block");
    var kind = block.dataset.chart;
    var vlSpec = kind
      ? lspSpec(
          container.__benchPoints,
          kind,
          block.querySelector("figcaption").textContent,
          container.clientWidth < 500,
        )
      : spec(container.__benchPoints);
    // Alt text on the container, mirroring the spec description Vega puts on the
    // rendered SVG, so the chart is labeled for assistive tech either way.
    container.setAttribute("role", "img");
    container.setAttribute("aria-label", vlSpec.description);
    window
      .vegaEmbed(container, vlSpec, { actions: false, renderer: "svg" })
      .then(function (result) {
        // The named container and adjacent data table describe the whole chart.
        container.querySelector("svg")?.setAttribute("aria-hidden", "true");
        if (container.__benchView) container.__benchView.finalize();
        container.__benchView = result.view;
        if (!container.__benchRendered) {
          block.querySelector(".bench-table").removeAttribute("open");
          container.__benchRendered = true;
        }
      })
      .catch(function (err) {
        // Leave the fallback table in place; surface the reason for debugging.
        console.error("bench-charts: failed to render", err);
      });
  }

  async function init() {
    var blocks = document.querySelectorAll(".bench-chart-block");
    if (!blocks.length) {
      return;
    }
    try {
      for (const name of [
        "vega.min.js",
        "vega-lite.min.js",
        "vega-embed.min.js",
      ]) {
        await loadScript(name);
      }
    } catch (err) {
      console.error("bench-charts: runtime unavailable", err);
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
      renderInto(container);
    });

    // Re-render on light/dark toggle so axis and legend colors track the theme.
    var observer = new MutationObserver(function () {
      document.querySelectorAll(".bench-chart").forEach(function (container) {
        if (container.__benchPoints) {
          renderInto(container);
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
