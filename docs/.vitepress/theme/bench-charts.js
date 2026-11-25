// Vega is loaded only when the current page contains benchmark figures.
function isDark() {
  var c = document.documentElement.classList;
  return c.contains("dark");
}

// Keep corpus and tool order in the axes and legends.
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

// Label 1, 2, and 5 in each decade, matching Arity's timing charts.
function timeAxis(points, field, title) {
  const values = points
    .map((point) => point[field])
    .filter((value) => Number.isFinite(value) && value > 0);
  const low = values.length ? Math.floor(Math.log10(Math.min(...values))) : 0;
  const high = values.length ? Math.ceil(Math.log10(Math.max(...values))) : 1;
  const ticks = [];
  for (let exponent = low; exponent < high; exponent++) {
    for (const multiple of [1, 2, 5]) {
      ticks.push(multiple * 10 ** exponent);
    }
  }
  ticks.push(10 ** high);
  return {
    field,
    type: "quantitative",
    title,
    scale: {
      type: "log",
      domain: [10 ** low / 1.1, 10 ** high * 1.1],
      nice: false,
    },
    axis: {
      values: ticks,
      labelExpr: "format(datum.value, ',~g')",
      labelBound: true,
      labelOverlap: "greedy",
      labelSeparation: 4,
      grid: false,
    },
  };
}

function chartConfig() {
  const dark = isDark();
  const fg = dark ? "#c8c9db" : "#333333";
  const axisGray = dark ? "#40434d" : "#aaaaaa";
  const gridGray = dark ? "#535764" : "#eeeeee";
  return {
    background: null,
    view: { stroke: null },
    axis: {
      labelColor: fg,
      titleColor: fg,
      gridColor: gridGray,
      domainColor: axisGray,
      tickColor: axisGray,
    },
    legend: { labelColor: fg, titleColor: fg },
  };
}

function seriesColor(field, title, values) {
  return {
    field,
    type: "nominal",
    title,
    sort: values,
    legend: { orient: "top" },
  };
}

function spec(points, caption) {
  const formatters = orderedUnique(points, "formatter");
  const documents = orderedUnique(points, "document");

  return {
    $schema: "https://vega.github.io/schema/vega-lite/v5.json",
    description: caption,
    width: "container",
    height: 340,
    data: { values: points },
    mark: { type: "point", filled: true, size: 130, opacity: 0.9 },
    encoding: {
      y: {
        field: "document",
        type: "nominal",
        title: "Input",
        sort: documents,
        axis: { grid: true },
      },
      x: timeAxis(points, "mean_ms", "Mean time (ms)"),
      color: seriesColor("formatter", "Tool", formatters),
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
    config: chartConfig(),
  };
}

function lspSpec(points, kind, caption) {
  const memory = kind === "lsp-memory";
  const latency = kind === "lsp-latency";
  const servers = orderedUnique(points, "server");
  const metrics = orderedUnique(points, "metric");
  const tooltip = [
    { field: "server", title: "Server" },
    { field: "metric", title: memory ? "Milestone" : "Operation" },
  ];
  const chart = {
    $schema: "https://vega.github.io/schema/vega-lite/v5.json",
    description: caption,
    width: "container",
    height: 340,
    data: { values: points },
    config: chartConfig(),
  };
  if (memory) {
    tooltip.push(
      { field: "rss_mb", title: "RSS (MB)", format: ".1f" },
      { field: "pss_mb", title: "PSS (MB)", format: ".1f" },
    );
    return {
      ...chart,
      mark: { type: "bar" },
      encoding: {
        y: {
          field: "metric",
          type: "nominal",
          title: "Milestone",
          sort: metrics,
        },
        yOffset: { field: "server", type: "nominal", sort: servers },
        x: {
          field: "rss_mb",
          type: "quantitative",
          title: "Median process-tree RSS (MB)",
          scale: { type: "linear", zero: true },
        },
        color: {
          field: "server",
          type: "nominal",
          title: "Server",
          sort: servers,
        },
        tooltip,
      },
    };
  }

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
  return {
    ...chart,
    transform: [{ filter: "datum.median_ms > 0" }],
    mark: { type: "point", filled: true, size: 130, opacity: 0.9 },
    encoding: {
      y: {
        field: "metric",
        type: "nominal",
        title: latency ? "Operation" : "Wait",
        sort: metrics,
        axis: { grid: true },
      },
      x: timeAxis(
        points,
        "median_ms",
        latency ? "Request latency (ms)" : "Elapsed time (ms)",
      ),
      color: seriesColor("server", "Server", servers),
      tooltip,
    },
  };
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
        const chart = block.dataset.chart
          ? lspSpec(points, block.dataset.chart, caption)
          : spec(points, caption);
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
