// Vega is loaded only when the current page contains benchmark figures.
function isDark() {
  var c = document.documentElement.classList;
  return c.contains("dark");
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

function seriesColor(field, title, values, compact) {
  return {
    field: field,
    type: "nominal",
    title: title,
    sort: values,
    legend: {
      orient: "top",
      direction: "horizontal",
      columns: compact ? 1 : 2,
      labelLimit: 240,
    },
  };
}

function spec(points, caption, compact) {
  var dark = isDark();
  var fg = dark ? "#c8c9db" : "#333333";
  var grid = dark ? "#3b3f5c" : "#dddddd";
  var formatters = orderedUnique(points, "formatter");
  var documents = orderedUnique(points, "document");
  var domain = logDomain(points, "ratio");

  return {
    $schema: "https://vega.github.io/schema/vega-lite/v5.json",
    description: caption,
    width: "container",
    height: 340,
    data: { values: points },
    layer: [
      // The shared baseline makes ratios comparable across documents.
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
          y: {
            field: "ratio",
            type: "quantitative",
            title: "Time relative to badness",
            scale: { type: "log", domain: domain, nice: false },
            axis: logAxis(domain, fg),
          },
          color: seriesColor("document", "Document", documents, compact),
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
    height: memory ? 260 : 340,
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

  var domain = logExtent(
    points.map(function (point) {
      return point.median_ms;
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
    x: {
      field: "server",
      type: "nominal",
      sort: servers,
      title: "Server",
      axis: { labelAngle: 0 },
    },
    y: {
      field: "median_ms",
      type: "quantitative",
      title: latency
        ? "Request latency (ms, log scale)"
        : "Elapsed time (ms, log scale)",
      scale: { type: "log", domain: domain, nice: false },
      axis: logAxis(domain, chart.config.axis.labelColor, true),
    },
    color: seriesColor(
      "metric",
      latency ? "Operation" : "Wait",
      metrics,
      compact,
    ),
    tooltip: tooltip,
  };
  chart.transform = [{ filter: "datum.median_ms > 0" }];
  chart.mark = { type: "point", filled: true, size: 130, opacity: 0.9 };
  return chart;
}

let runtime;
let generation = 0;
const views = new Map();

function loadScript(name) {
  return new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = `/vendor/${name}`;
    script.onload = resolve;
    script.onerror = () => {
      script.remove();
      reject(new Error(`Could not load ${name}`));
    };
    document.head.appendChild(script);
  });
}

async function loadRuntime() {
  for (const [global, file] of [
    ["vega", "vega.min.js"],
    ["vegaLite", "vega-lite.min.js"],
    ["vegaEmbed", "vega-embed.min.js"],
  ]) {
    if (!window[global]) await loadScript(file);
  }
}

export function unmountCharts() {
  generation++;
  for (const view of views.values()) view.finalize();
  views.clear();
}

export async function mountCharts() {
  const blocks = [...document.querySelectorAll(".bench-chart-block")];
  if (!blocks.length) return;
  const current = ++generation;
  try {
    runtime ??= loadRuntime().catch((error) => {
      runtime = undefined;
      throw error;
    });
    await runtime;
    if (current !== generation) return;
    await Promise.all(
      blocks.map(async (block) => {
        const container = block.querySelector(".bench-chart");
        const points = JSON.parse(block.dataset.bench);
        if (!container || !points.length || !container.isConnected) return;
        const caption = block.querySelector("figcaption").textContent;
        const compact = container.clientWidth < 500;
        const chart = block.dataset.chart
          ? lspSpec(points, block.dataset.chart, caption, compact)
          : spec(points, caption, compact);
        container.setAttribute("role", "img");
        container.setAttribute("aria-label", chart.description);
        const { view } = await window.vegaEmbed(container, chart, {
          actions: false,
          renderer: "svg",
        });
        if (current !== generation || !container.isConnected) {
          view.finalize();
          return;
        }
        views.get(container)?.finalize();
        views.set(container, view);
        container.querySelector("svg")?.setAttribute("aria-hidden", "true");
        if (!container.dataset.rendered) {
          block.querySelector(".bench-table").removeAttribute("open");
          container.dataset.rendered = "true";
        }
      }),
    );
  } catch (error) {
    console.error("bench-charts: failed to render", error);
  }
}
