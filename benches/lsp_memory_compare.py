#!/usr/bin/env python3
"""Compare speed and resident memory for LaTeX language servers over stdio.

Each fresh server process receives the same editor-like session. Memory samples
cover the complete process tree and report both RSS and PSS from Linux `/proc`.
Quiescence, rather than a fixed sleep, determines the baseline and settled
milestones. After the session settles, the harness measures warm document
symbols, hover, definition, references, and rename requests.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shlex
import signal
import statistics
import subprocess
import threading
import time
from dataclasses import dataclass
from pathlib import Path

CLK_TCK = os.sysconf("SC_CLK_TCK")
IDLE_CPU_FRACTION = 0.05
SCHEMA_VERSION = 2
NAVIGATION_FILE = "Chapter1/chapter1.tex"
NAVIGATION_LINE = "Lorem Ipsum~\\citep{Aup91} has been the industry's"
NAVIGATION_SYMBOL = "Aup91"
REFERENCE_TARGET = re.compile(
    r"\\(?:[A-Za-z@]*ref|cite[A-Za-z@]*)\*?(?:\s*\[[^]]*\])*\s*\{([^,}\s]+)"
)


@dataclass(frozen=True)
class ProcessSample:
    rss_kb: int
    pss_kb: int
    cpu_ticks: int
    processes: int


def process_tree(root_pid: int, proc_root: Path = Path("/proc")) -> set[int]:
    """Return every live PID rooted at `root_pid`, including the root."""
    pids = {root_pid}
    frontier = [root_pid]
    while frontier:
        pid = frontier.pop()
        try:
            tasks = list((proc_root / str(pid) / "task").iterdir())
        except OSError:
            continue
        for task in tasks:
            try:
                children = (task / "children").read_text().split()
            except OSError:
                continue
            for child_text in children:
                child = int(child_text)
                if child not in pids:
                    pids.add(child)
                    frontier.append(child)
    return pids


def read_process(
    pid: int, proc_root: Path = Path("/proc")
) -> tuple[int, int, int] | None:
    """Read RSS, PSS, and CPU ticks for one process that may be exiting."""
    root = proc_root / str(pid)
    try:
        status = (root / "status").read_text()
        stat = (root / "stat").read_text()
    except OSError:
        return None

    rss_kb = 0
    for line in status.splitlines():
        if line.startswith("VmRSS:"):
            rss_kb = int(line.split()[1])
            break

    try:
        pss_kb = 0
        for line in (root / "smaps_rollup").read_text().splitlines():
            if line.startswith("Pss:"):
                pss_kb = int(line.split()[1])
                break
    except OSError:
        # Some hardened `/proc` mounts deny smaps access. RSS remains useful,
        # and treating it as PSS makes the fallback explicit and conservative.
        pss_kb = rss_kb

    # The command name can contain both spaces and parentheses. Fields 14 and
    # 15 are therefore located relative to the final closing parenthesis.
    fields = stat[stat.rindex(")") + 2 :].split()
    cpu_ticks = int(fields[11]) + int(fields[12])
    return rss_kb, pss_kb, cpu_ticks


def sample_tree(root_pid: int, proc_root: Path = Path("/proc")) -> ProcessSample:
    """Sum one memory and CPU sample over a live process tree."""
    rss_kb = pss_kb = cpu_ticks = processes = 0
    for pid in process_tree(root_pid, proc_root):
        reading = read_process(pid, proc_root)
        if reading is None:
            continue
        rss_kb += reading[0]
        pss_kb += reading[1]
        cpu_ticks += reading[2]
        processes += 1
    return ProcessSample(rss_kb, pss_kb, cpu_ticks, processes)


class Sampler(threading.Thread):
    """Poll a process tree, retain history, and track its peak."""

    def __init__(self, pid: int, interval: float):
        super().__init__(daemon=True)
        self.pid = pid
        self.interval = interval
        self.stop_event = threading.Event()
        self.started_at = time.monotonic()
        self.samples: list[tuple[float, ProcessSample]] = []
        self.peak_rss_kb = 0
        self.peak_pss_kb = 0

    def run(self) -> None:
        while not self.stop_event.is_set():
            sample = sample_tree(self.pid)
            if sample.processes:
                elapsed = time.monotonic() - self.started_at
                self.samples.append((elapsed, sample))
                self._include_in_peak(sample)
            self.stop_event.wait(self.interval)

    def _include_in_peak(self, sample: ProcessSample) -> None:
        self.peak_rss_kb = max(self.peak_rss_kb, sample.rss_kb)
        self.peak_pss_kb = max(self.peak_pss_kb, sample.pss_kb)

    def milestone(self) -> dict[str, int | float]:
        sample = sample_tree(self.pid)
        self._include_in_peak(sample)
        return sample_to_json(sample)

    def quiet_since(self, seconds: float, not_before: float = 0.0) -> float | None:
        """Return the start of the current quiet window when it is long enough."""
        if len(self.samples) < 3:
            return None
        cutoff = max(self.samples[-1][0] - seconds, not_before)
        window = [sample for sample in self.samples if sample[0] >= cutoff]
        if len(window) < 3:
            return None
        span = window[-1][0] - window[0][0]
        # The oldest sample in the cutoff window can be almost one interval
        # newer than the cutoff. Allow only that sampling slop—not a shortened
        # quiet period.
        if span < seconds - self.interval * 1.5:
            return None
        cpu_ticks = window[-1][1].cpu_ticks - window[0][1].cpu_ticks
        if max(0.0, cpu_ticks / CLK_TCK) / span >= IDLE_CPU_FRACTION:
            return None
        return window[0][0]


def sample_to_json(sample: ProcessSample) -> dict[str, int | float]:
    return {
        "rss_mb": round(sample.rss_kb / 1024, 1),
        "pss_mb": round(sample.pss_kb / 1024, 1),
        "processes": sample.processes,
    }


def read_lsp_message(stream) -> dict | None:
    """Read one Content-Length-framed LSP message, or EOF."""
    content_length = None
    while True:
        line = stream.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        name, separator, value = line.partition(b":")
        if separator and name.lower() == b"content-length":
            content_length = int(value.strip())
    if content_length is None:
        raise RuntimeError("LSP message omitted Content-Length")
    payload = stream.read(content_length)
    if len(payload) != content_length:
        raise RuntimeError("LSP message ended before Content-Length bytes arrived")
    return json.loads(payload)


class Client:
    """A small LSP client for a deterministic benchmark session."""

    def __init__(
        self, command: list[str], cwd: Path, stderr_path: Path, env: dict[str, str]
    ):
        stderr_path.parent.mkdir(parents=True, exist_ok=True)
        self.stderr_file = stderr_path.open("wb")
        self.proc = subprocess.Popen(
            command,
            cwd=cwd,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self.stderr_file,
            start_new_session=True,
        )
        if self.proc.stdin is None or self.proc.stdout is None:
            raise RuntimeError("failed to create language-server pipes")
        self.stdin = self.proc.stdin
        self.stdout = self.proc.stdout
        self.next_id = 1
        self.write_lock = threading.Lock()
        self.state = threading.Condition()
        self.responses: dict[int, dict] = {}
        self.notifications: list[dict] = []
        self.reader_error: str | None = None
        self.alive = True
        threading.Thread(target=self._read_loop, daemon=True).start()

    def _read_loop(self) -> None:
        try:
            while (message := read_lsp_message(self.stdout)) is not None:
                with self.state:
                    if "id" in message and ("result" in message or "error" in message):
                        self.responses[message["id"]] = message
                    elif "method" in message:
                        self.notifications.append(message)
                        if "id" in message:
                            self._answer(message)
                    self.state.notify_all()
        except (RuntimeError, ValueError, json.JSONDecodeError) as error:
            self.reader_error = str(error)
        finally:
            with self.state:
                self.alive = False
                self.state.notify_all()

    def _answer(self, request: dict) -> None:
        if request["method"] == "workspace/configuration":
            items = (request.get("params") or {}).get("items") or []
            result = [None] * len(items)
        elif request["method"] == "window/showDocument":
            result = {"success": False}
        else:
            result = None
        self._send({"jsonrpc": "2.0", "id": request["id"], "result": result})

    def _send(self, message: dict) -> None:
        payload = json.dumps(message, separators=(",", ":")).encode()
        with self.write_lock:
            try:
                self.stdin.write(b"Content-Length: %d\r\n\r\n" % len(payload))
                self.stdin.write(payload)
                self.stdin.flush()
            except (BrokenPipeError, OSError, ValueError):
                pass

    def notify(self, method: str, params) -> None:
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def request(self, method: str, params, timeout: float) -> dict:
        with self.state:
            request_id = self.next_id
            self.next_id += 1
        self._send(
            {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        )
        deadline = time.monotonic() + timeout
        with self.state:
            while request_id not in self.responses:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise RuntimeError(f"{method} timed out after {timeout:g} seconds")
                if not self.alive:
                    detail = f": {self.reader_error}" if self.reader_error else ""
                    raise RuntimeError(
                        f"server exited during {method} (status {self.proc.poll()}){detail}"
                    )
                self.state.wait(min(remaining, 1.0))
            response = self.responses.pop(request_id)
        if "error" in response:
            raise RuntimeError(f"{method} returned an LSP error: {response['error']}")
        return response

    def published_diagnostics(self) -> int:
        with self.state:
            return sum(
                message.get("method") == "textDocument/publishDiagnostics"
                for message in self.notifications
            )

    def stop(self) -> None:
        try:
            os.killpg(os.getpgid(self.proc.pid), signal.SIGKILL)
        except (ProcessLookupError, PermissionError, OSError):
            pass
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        self.stderr_file.close()


CAPABILITIES = {
    "general": {"positionEncodings": ["utf-8", "utf-16"]},
    "workspace": {
        "workspaceFolders": True,
        "configuration": True,
        "didChangeWatchedFiles": {"dynamicRegistration": True},
        "workspaceEdit": {
            "documentChanges": True,
            "resourceOperations": ["create", "rename", "delete"],
        },
    },
    "window": {"showDocument": {"support": True}},
    "textDocument": {
        "synchronization": {"dynamicRegistration": True, "didSave": True},
        "publishDiagnostics": {"relatedInformation": True, "versionSupport": True},
        "diagnostic": {"dynamicRegistration": True, "relatedDocumentSupport": True},
        "documentSymbol": {"hierarchicalDocumentSymbolSupport": True},
        "hover": {"contentFormat": ["markdown", "plaintext"]},
        "definition": {"dynamicRegistration": True, "linkSupport": True},
        "references": {"dynamicRegistration": True},
        "rename": {"dynamicRegistration": True, "prepareSupport": True},
    },
}


def wait_until_quiet(
    sampler: Sampler,
    client: Client,
    quiet_seconds: float,
    timeout: float,
    phase: str,
    not_before: float,
) -> float:
    """Return the quiet window's start, or fail when the phase does not settle."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        status = client.proc.poll()
        if status is not None:
            raise RuntimeError(f"server exited during {phase} (status {status})")
        quiet_since = sampler.quiet_since(quiet_seconds, not_before)
        if quiet_since is not None:
            return quiet_since
        time.sleep(min(0.5, sampler.interval))
    raise RuntimeError(f"server remained busy during {phase} for {timeout:g} seconds")


def hover_position(source: str) -> dict[str, int]:
    """Choose the first ordinary control word outside a full-line comment."""
    for line_number, line in enumerate(source.splitlines()):
        if line.lstrip().startswith("%"):
            continue
        if match := re.search(r"\\([A-Za-z@]+)", line):
            return {"line": line_number, "character": match.start(1)}
    return {"line": 0, "character": 0}


def reference_hover_position(source: str) -> dict[str, int] | None:
    """Choose the first citation or reference key with project context."""
    for line_number, line in enumerate(source.splitlines()):
        if line.lstrip().startswith("%"):
            continue
        if match := REFERENCE_TARGET.search(line):
            return {"line": line_number, "character": match.start(1)}
    return None


def hover_targets(
    documents: list[tuple[str, str]], limit: int
) -> list[tuple[str, dict[str, int]]]:
    """Prefer contextual citation/reference hovers, then fill with control words."""
    contextual = []
    fallbacks = []
    for uri, source in documents:
        if position := reference_hover_position(source):
            contextual.append((uri, position))
        else:
            fallbacks.append((uri, hover_position(source)))
    return (contextual + fallbacks)[:limit]


def navigation_target(project: Path, files: list[Path]) -> dict:
    """Resolve the pinned cross-file citation use in the benchmark corpus."""
    for path in files:
        if not path.as_posix().endswith(NAVIGATION_FILE):
            continue
        for line_number, line in enumerate(
            path.read_text(errors="replace").splitlines()
        ):
            line_start = line.find(NAVIGATION_LINE)
            if line_start < 0:
                continue
            character = line.find(NAVIGATION_SYMBOL, line_start)
            if character >= 0:
                return {
                    "uri": path.as_uri(),
                    "file": str(path.relative_to(project)),
                    "symbol": NAVIGATION_SYMBOL,
                    "position": {"line": line_number, "character": character},
                }
        break
    raise RuntimeError(
        f"navigation target {NAVIGATION_SYMBOL} in {NAVIGATION_FILE} is missing"
    )


def percentile(values: list[float], percentile_value: float) -> float:
    """Return the nearest-rank percentile for a nonempty sample."""
    ordered = sorted(values)
    rank = max(1, int(len(ordered) * percentile_value + 0.999999999))
    return ordered[min(rank - 1, len(ordered) - 1)]


def location_summary(result) -> tuple[int, int]:
    """Count definition/reference locations and distinct target files."""
    locations = result if isinstance(result, list) else [result]
    uris = {
        location.get("uri") or location.get("targetUri")
        for location in locations
        if isinstance(location, dict)
    }
    uris.discard(None)
    return len(locations), len(uris)


def document_symbol_summary(result) -> tuple[int, None]:
    """Count symbols, including nested DocumentSymbol children."""

    def count(symbol) -> int:
        if not isinstance(symbol, dict):
            return 0
        return 1 + sum(count(child) for child in symbol.get("children") or [])

    symbols = result if isinstance(result, list) else []
    return sum(count(symbol) for symbol in symbols), None


def singleton_summary(_result) -> tuple[int, None]:
    """Count a successful singleton response, such as hover."""
    return 1, None


def workspace_edit_summary(result) -> tuple[int, int]:
    """Count text edits and distinct files in a WorkspaceEdit."""
    edits = 0
    uris = set()
    for uri, file_edits in (result.get("changes") or {}).items():
        uris.add(uri)
        edits += len(file_edits or [])
    for change in result.get("documentChanges") or []:
        document = change.get("textDocument") if isinstance(change, dict) else None
        if document is None:
            continue
        if uri := document.get("uri"):
            uris.add(uri)
        edits += len(change.get("edits") or [])
    return edits, len(uris)


def add_distribution(record: dict, prefix: str, values: list[int]) -> None:
    """Attach min, median, and max fields for a nonempty result sample."""
    if not values:
        return
    record[f"{prefix}_min"] = min(values)
    record[f"{prefix}_median"] = statistics.median(values)
    record[f"{prefix}_max"] = max(values)


def finalize_request_record(record: dict) -> dict:
    """Derive display statistics from one or more runs' raw measurements."""
    latencies = record["_latencies_ms"]
    record["median_ms"] = round(statistics.median(latencies), 3) if latencies else None
    record["p95_ms"] = round(percentile(latencies, 0.95), 3) if latencies else None
    record["samples"] = len(latencies)
    add_distribution(record, "result_count", record["_result_counts"])
    add_distribution(record, "result_files", record["_result_files"])
    if record["_payload_bytes"]:
        record["payload_bytes_median"] = round(
            statistics.median(record["_payload_bytes"])
        )
    return record


def benchmark_requests(
    client: Client,
    *,
    key: str,
    label: str,
    method: str,
    params: list[dict],
    runs: int,
    warmups: int,
    timeout: float,
    result_unit: str,
    summarize,
) -> dict:
    """Measure serial stdio round trips for one warm LSP request kind."""
    for _ in range(warmups):
        for request_params in params:
            client.request(method, request_params, timeout)

    record = {
        "key": key,
        "label": label,
        "failures": 0,
        "empty_results": 0,
        "targets": len(params),
        "result_unit": result_unit,
        "_latencies_ms": [],
        "_result_counts": [],
        "_result_files": [],
        "_payload_bytes": [],
    }
    for _ in range(runs):
        for request_params in params:
            started = time.perf_counter_ns()
            response = client.request(method, request_params, timeout)
            record["_latencies_ms"].append(
                (time.perf_counter_ns() - started) / 1_000_000
            )
            result = response.get("result")
            if result in (None, [], {}):
                record["empty_results"] += 1
                continue
            count, files = summarize(result)
            record["_result_counts"].append(count)
            if files is not None:
                record["_result_files"].append(files)
            record["_payload_bytes"].append(
                len(
                    json.dumps(
                        result, separators=(",", ":"), ensure_ascii=False
                    ).encode()
                )
            )
    return finalize_request_record(record)


def public_record(value):
    """Remove raw measurement arrays while retaining each fresh run's summary."""
    if isinstance(value, dict):
        return {
            key: public_record(item)
            for key, item in value.items()
            if not key.startswith("_")
        }
    if isinstance(value, list):
        return [public_record(item) for item in value]
    return value


def run_session(
    *,
    key: str,
    command: list[str],
    project: Path,
    files: list[Path],
    run_number: int,
    settle_timeout: float,
    quiet_seconds: float,
    sample_interval: float,
    latency_runs: int,
    latency_warmups: int,
    navigation: dict,
    stderr_dir: Path,
    scratch_dir: Path,
) -> dict:
    stderr_path = stderr_dir / f"{key}-{run_number}.stderr.log"
    env = os.environ.copy()
    if key == "badness":
        config = scratch_dir / f"badness-{run_number}.toml"
        config.write_text("")
        env["BADNESS_CONFIG"] = str(config)

    client = Client(command, project, stderr_path, env)
    sampler = Sampler(client.proc.pid, sample_interval)
    sampler.start()
    started_at = time.monotonic()
    result: dict = {"run": run_number, "milestones": {}}

    try:
        initialized = client.request(
            "initialize",
            {
                "processId": os.getpid(),
                "clientInfo": {"name": "badness-lsp-benchmark", "version": "2"},
                "rootUri": project.as_uri(),
                "rootPath": str(project),
                "capabilities": CAPABILITIES,
                "workspaceFolders": [{"uri": project.as_uri(), "name": project.name}],
                "initializationOptions": {
                    "badness": {
                        "forwardSearch": {
                            "ipcDir": str(scratch_dir / f"ipc-{key}-{run_number}")
                        }
                    }
                },
            },
            settle_timeout,
        )
        result["initialize_seconds"] = round(time.monotonic() - started_at, 3)
        server_capabilities = (initialized.get("result") or {}).get("capabilities", {})
        pull_diagnostics = bool(server_capabilities.get("diagnosticProvider"))
        result["diagnostic_mode"] = "pull" if pull_diagnostics else "push"
        client.notify("initialized", {})
        baseline_phase = time.monotonic() - sampler.started_at

        baseline_ready = wait_until_quiet(
            sampler,
            client,
            quiet_seconds,
            settle_timeout,
            "initialization",
            baseline_phase,
        )
        result["milestones"]["baseline"] = sampler.milestone()
        result["baseline_seconds"] = round(time.monotonic() - started_at, 2)
        result["workspace_ready_seconds"] = round(
            sampler.started_at + baseline_ready - started_at, 3
        )

        documents = []
        documents_started = time.monotonic()
        documents_phase = documents_started - sampler.started_at
        for path in files:
            source = path.read_text(errors="replace")
            uri = path.as_uri()
            documents.append((uri, source))
            client.notify(
                "textDocument/didOpen",
                {
                    "textDocument": {
                        "uri": uri,
                        "languageId": "latex",
                        "version": 1,
                        "text": source,
                    }
                },
            )

        diagnostic_requests = 0
        if pull_diagnostics:
            for uri, _ in documents:
                client.request(
                    "textDocument/diagnostic",
                    {"textDocument": {"uri": uri}},
                    settle_timeout,
                )
                diagnostic_requests += 1

        selected_hovers = hover_targets(documents, limit=3)
        for uri, _position in selected_hovers:
            client.request(
                "textDocument/documentSymbol",
                {"textDocument": {"uri": uri}},
                settle_timeout,
            )
        for uri, position in selected_hovers:
            client.request(
                "textDocument/hover",
                {"textDocument": {"uri": uri}, "position": position},
                settle_timeout,
            )

        documents_ready = wait_until_quiet(
            sampler,
            client,
            quiet_seconds,
            settle_timeout,
            "open-file analysis",
            documents_phase,
        )
        result["milestones"]["settled"] = sampler.milestone()
        result["settled_seconds"] = round(time.monotonic() - started_at, 2)
        result["documents_ready_seconds"] = round(
            sampler.started_at + documents_ready - documents_started, 3
        )
        result["diagnostic_requests"] = diagnostic_requests
        result["diagnostics_published"] = client.published_diagnostics()
        if not pull_diagnostics and result["diagnostics_published"] == 0:
            raise RuntimeError("push-diagnostic server published no diagnostics")

        navigation_params = {
            "textDocument": {"uri": navigation["uri"]},
            "position": navigation["position"],
        }
        result["request_latencies"] = [
            benchmark_requests(
                client,
                key="document_symbol",
                label="Document symbols",
                method="textDocument/documentSymbol",
                params=[
                    {"textDocument": {"uri": uri}} for uri, _position in selected_hovers
                ],
                runs=latency_runs,
                warmups=latency_warmups,
                timeout=settle_timeout,
                result_unit="symbol",
                summarize=document_symbol_summary,
            ),
            benchmark_requests(
                client,
                key="hover",
                label="Hover",
                method="textDocument/hover",
                params=[
                    {"textDocument": {"uri": uri}, "position": position}
                    for uri, position in selected_hovers
                ],
                runs=latency_runs,
                warmups=latency_warmups,
                timeout=settle_timeout,
                result_unit="result",
                summarize=singleton_summary,
            ),
            benchmark_requests(
                client,
                key="definition",
                label="Go to definition",
                method="textDocument/definition",
                params=[navigation_params],
                runs=latency_runs,
                warmups=latency_warmups,
                timeout=settle_timeout,
                result_unit="location",
                summarize=location_summary,
            ),
            benchmark_requests(
                client,
                key="references",
                label="Find references",
                method="textDocument/references",
                params=[{**navigation_params, "context": {"includeDeclaration": True}}],
                runs=latency_runs,
                warmups=latency_warmups,
                timeout=settle_timeout,
                result_unit="location",
                summarize=location_summary,
            ),
            benchmark_requests(
                client,
                key="rename",
                label="Rename",
                method="textDocument/rename",
                params=[{**navigation_params, "newName": "badnessBenchmarkAup91"}],
                runs=latency_runs,
                warmups=latency_warmups,
                timeout=settle_timeout,
                result_unit="edit",
                summarize=workspace_edit_summary,
            ),
        ]
        for latency in result["request_latencies"]:
            print(
                f"    {latency['label'].lower():<17} "
                f"{latency['median_ms']:.3f} ms median "
                f"({latency['p95_ms']:.3f} ms p95)",
                flush=True,
            )

        sampler.stop_event.set()
        sampler.join(timeout=2)
        result["milestones"]["peak"] = {
            "rss_mb": round(sampler.peak_rss_kb / 1024, 1),
            "pss_mb": round(sampler.peak_pss_kb / 1024, 1),
        }
        result["samples"] = len(sampler.samples)

        client.request("shutdown", None, min(15.0, settle_timeout))
        client.notify("exit", None)
        return result
    finally:
        sampler.stop_event.set()
        sampler.join(timeout=2)
        client.stop()


def summarize_runs(runs: list[dict]) -> dict:
    def median(path: tuple[str, ...], digits: int = 1) -> float:
        values = []
        for run in runs:
            value = run
            for part in path:
                value = value[part]
            values.append(value)
        return round(statistics.median(values), digits)

    request_latencies = []
    for request_index, first in enumerate(runs[0]["request_latencies"]):
        combined = {
            key: value
            for key, value in first.items()
            if not key.startswith("_")
            and key
            not in {
                "median_ms",
                "p95_ms",
                "samples",
                "result_count_min",
                "result_count_median",
                "result_count_max",
                "result_files_min",
                "result_files_median",
                "result_files_max",
                "payload_bytes_median",
            }
        }
        combined["failures"] = sum(
            run["request_latencies"][request_index]["failures"] for run in runs
        )
        combined["empty_results"] = sum(
            run["request_latencies"][request_index]["empty_results"] for run in runs
        )
        for field in (
            "_latencies_ms",
            "_result_counts",
            "_result_files",
            "_payload_bytes",
        ):
            combined[field] = [
                value
                for run in runs
                for value in run["request_latencies"][request_index][field]
            ]
        request_latencies.append(public_record(finalize_request_record(combined)))

    return {
        "baseline_rss_mb": median(("milestones", "baseline", "rss_mb")),
        "baseline_pss_mb": median(("milestones", "baseline", "pss_mb")),
        "settled_rss_mb": median(("milestones", "settled", "rss_mb")),
        "settled_pss_mb": median(("milestones", "settled", "pss_mb")),
        "peak_rss_mb": median(("milestones", "peak", "rss_mb")),
        "peak_pss_mb": median(("milestones", "peak", "pss_mb")),
        "settled_seconds": median(("settled_seconds",)),
        "initialize_seconds": median(("initialize_seconds",), 3),
        "workspace_ready_seconds": median(("workspace_ready_seconds",), 3),
        "documents_ready_seconds": median(("documents_ready_seconds",), 3),
        "request_latencies": request_latencies,
    }


def add_relative_memory(records: list[dict]) -> None:
    badness = next(record for record in records if record["key"] == "badness")
    baseline = badness["summary"]["settled_rss_mb"]
    for record in records:
        record["summary"]["relative_to_badness"] = round(
            record["summary"]["settled_rss_mb"] / baseline, 2
        )


def parse_server(spec: str) -> tuple[str, list[str]]:
    key, separator, command = spec.partition("=")
    if not separator or not key or not command:
        raise argparse.ArgumentTypeError("servers must use KEY=COMMAND")
    return key, shlex.split(command)


def host_metadata() -> dict:
    cpu = "unknown"
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                cpu = line.partition(":")[2].strip()
                break
        mem_total_kb = next(
            int(line.split()[1])
            for line in Path("/proc/meminfo").read_text().splitlines()
            if line.startswith("MemTotal:")
        )
    except (OSError, StopIteration, ValueError):
        mem_total_kb = 0
    return {
        "hostname": platform.node(),
        "os": f"{platform.system()} {platform.machine()}",
        "cpu": cpu,
        "memory_gb": round(mem_total_kb / 1024 / 1024, 1),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True)
    parser.add_argument("--files", nargs="+", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--server", action="append", type=parse_server, required=True)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--sample-interval", type=float, default=0.15)
    parser.add_argument("--quiet-seconds", type=float, default=5.0)
    parser.add_argument("--settle-timeout", type=float, default=60.0)
    parser.add_argument("--latency-runs", type=int, default=20)
    parser.add_argument("--latency-warmups", type=int, default=2)
    parser.add_argument("--stderr-dir", required=True)
    parser.add_argument("--scratch-dir", required=True)
    parser.add_argument("--badness-version", required=True)
    parser.add_argument("--texlab-version", required=True)
    parser.add_argument("--corpus-repository", required=True)
    parser.add_argument("--corpus-tag", required=True)
    parser.add_argument("--corpus-commit", required=True)
    args = parser.parse_args()

    if platform.system() != "Linux" or not Path("/proc/self/smaps_rollup").is_file():
        parser.error("the LSP benchmark requires Linux with `/proc` smaps_rollup")
    if args.runs < 1:
        parser.error("--runs must be positive")
    if args.latency_runs < 1:
        parser.error("--latency-runs must be positive")
    if args.latency_warmups < 0:
        parser.error("--latency-warmups must be nonnegative")

    project = Path(args.project).resolve()
    files = [Path(path).resolve() for path in args.files]
    stderr_dir = Path(args.stderr_dir).resolve()
    scratch_dir = Path(args.scratch_dir).resolve()
    scratch_dir.mkdir(parents=True, exist_ok=True)
    missing = [str(path) for path in [project, *files] if not path.exists()]
    if missing:
        parser.error(f"missing benchmark paths: {', '.join(missing)}")
    navigation = navigation_target(project, files)

    records = []
    labels = {"badness": "Badness", "texlab": "TexLab"}
    for key, command in args.server:
        runs = []
        for run_number in range(1, args.runs + 1):
            print(
                f"==> {labels.get(key, key)} run {run_number}/{args.runs}", flush=True
            )
            run = run_session(
                key=key,
                command=command,
                project=project,
                files=files,
                run_number=run_number,
                settle_timeout=args.settle_timeout,
                quiet_seconds=args.quiet_seconds,
                sample_interval=args.sample_interval,
                latency_runs=args.latency_runs,
                latency_warmups=args.latency_warmups,
                navigation=navigation,
                stderr_dir=stderr_dir,
                scratch_dir=scratch_dir,
            )
            runs.append(run)
            settled = run["milestones"]["settled"]["rss_mb"]
            peak = run["milestones"]["peak"]["rss_mb"]
            print(f"    settled {settled} MB RSS (peak {peak} MB)", flush=True)
            time.sleep(1)
        records.append(
            {
                "key": key,
                "label": labels.get(key, key),
                "command": [Path(command[0]).name, *command[1:]],
                "summary": summarize_runs(runs),
                "runs": public_record(runs),
            }
        )
    add_relative_memory(records)

    output = {
        "schema_version": SCHEMA_VERSION,
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "host": host_metadata(),
        "versions": {"badness": args.badness_version, "texlab": args.texlab_version},
        "corpus": {
            "repository": args.corpus_repository,
            "tag": args.corpus_tag,
            "commit": args.corpus_commit,
            "source_files": len(list(project.rglob("*.tex")))
            + len(list(project.rglob("*.bib")))
            + len(list(project.rglob("*.cls")))
            + len(list(project.rglob("*.sty"))),
            "source_bytes": sum(
                path.stat().st_size
                for suffix in ("*.tex", "*.bib", "*.cls", "*.sty")
                for path in project.rglob(suffix)
            ),
        },
        "session": {
            "runs_per_server": args.runs,
            "sample_interval_seconds": args.sample_interval,
            "quiet_seconds": args.quiet_seconds,
            "settle_timeout_seconds": args.settle_timeout,
            "latency_runs": args.latency_runs,
            "latency_warmups": args.latency_warmups,
            "latency_files": min(3, len(files)),
            "open_files": [str(path.relative_to(project)) for path in files],
            "open_bytes": sum(path.stat().st_size for path in files),
            "navigation_target": {
                key: value for key, value in navigation.items() if key != "uri"
            },
            "operations": [
                "diagnostics",
                "document symbols",
                "hover",
                "go to definition",
                "find references",
                "rename",
            ],
        },
        "servers": records,
    }
    Path(args.out).write_text(json.dumps(output, indent=2) + "\n")


if __name__ == "__main__":
    main()
