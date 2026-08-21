#!/usr/bin/env python3
"""Compare resident memory for LaTeX language servers over stdio.

Each fresh server process receives the same editor-like session. Memory samples
cover the complete process tree and report both RSS and PSS from Linux `/proc`.
Quiescence, rather than a fixed sleep, determines the baseline and settled
milestones.
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
SCHEMA_VERSION = 1


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

    def is_quiet(self, seconds: float) -> bool:
        if len(self.samples) < 3:
            return False
        cutoff = self.samples[-1][0] - seconds
        window = [sample for sample in self.samples if sample[0] >= cutoff]
        if len(window) < 3:
            return False
        span = window[-1][0] - window[0][0]
        # The oldest sample in the cutoff window can be almost one interval
        # newer than the cutoff. Allow only that sampling slop—not a shortened
        # quiet period.
        if span < seconds - self.interval * 1.5:
            return False
        cpu_ticks = window[-1][1].cpu_ticks - window[0][1].cpu_ticks
        return max(0.0, cpu_ticks / CLK_TCK) / span < IDLE_CPU_FRACTION


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
    },
    "window": {"showDocument": {"support": True}},
    "textDocument": {
        "synchronization": {"dynamicRegistration": True, "didSave": True},
        "publishDiagnostics": {"relatedInformation": True, "versionSupport": True},
        "diagnostic": {"dynamicRegistration": True, "relatedDocumentSupport": True},
        "documentSymbol": {"hierarchicalDocumentSymbolSupport": True},
        "hover": {"contentFormat": ["markdown", "plaintext"]},
    },
}


def wait_until_quiet(
    sampler: Sampler, client: Client, quiet_seconds: float, timeout: float, phase: str
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        status = client.proc.poll()
        if status is not None:
            raise RuntimeError(f"server exited during {phase} (status {status})")
        if sampler.is_quiet(quiet_seconds):
            return
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
                "clientInfo": {"name": "badness-memory-benchmark", "version": "1"},
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
        result["init_seconds"] = round(time.monotonic() - started_at, 2)
        server_capabilities = (initialized.get("result") or {}).get("capabilities", {})
        pull_diagnostics = bool(server_capabilities.get("diagnosticProvider"))
        result["diagnostic_mode"] = "pull" if pull_diagnostics else "push"
        client.notify("initialized", {})

        wait_until_quiet(
            sampler, client, quiet_seconds, settle_timeout, "initialization"
        )
        result["milestones"]["baseline"] = sampler.milestone()
        result["baseline_seconds"] = round(time.monotonic() - started_at, 2)

        documents = []
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
            time.sleep(0.2)

        diagnostic_requests = 0
        if pull_diagnostics:
            for uri, _ in documents:
                client.request(
                    "textDocument/diagnostic",
                    {"textDocument": {"uri": uri}},
                    settle_timeout,
                )
                diagnostic_requests += 1

        for uri, source in documents:
            client.request(
                "textDocument/documentSymbol",
                {"textDocument": {"uri": uri}},
                settle_timeout,
            )
            client.request(
                "textDocument/hover",
                {"textDocument": {"uri": uri}, "position": hover_position(source)},
                settle_timeout,
            )

        wait_until_quiet(
            sampler, client, quiet_seconds, settle_timeout, "open-file analysis"
        )
        result["milestones"]["settled"] = sampler.milestone()
        result["settled_seconds"] = round(time.monotonic() - started_at, 2)
        result["diagnostic_requests"] = diagnostic_requests
        result["diagnostics_published"] = client.published_diagnostics()
        if not pull_diagnostics and result["diagnostics_published"] == 0:
            raise RuntimeError("push-diagnostic server published no diagnostics")

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
    def median(path: tuple[str, ...]) -> float:
        values = []
        for run in runs:
            value = run
            for part in path:
                value = value[part]
            values.append(value)
        return round(statistics.median(values), 1)

    return {
        "baseline_rss_mb": median(("milestones", "baseline", "rss_mb")),
        "baseline_pss_mb": median(("milestones", "baseline", "pss_mb")),
        "settled_rss_mb": median(("milestones", "settled", "rss_mb")),
        "settled_pss_mb": median(("milestones", "settled", "pss_mb")),
        "peak_rss_mb": median(("milestones", "peak", "rss_mb")),
        "peak_pss_mb": median(("milestones", "peak", "pss_mb")),
        "settled_seconds": median(("settled_seconds",)),
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
    parser.add_argument("--stderr-dir", required=True)
    parser.add_argument("--scratch-dir", required=True)
    parser.add_argument("--badness-version", required=True)
    parser.add_argument("--texlab-version", required=True)
    parser.add_argument("--corpus-repository", required=True)
    parser.add_argument("--corpus-tag", required=True)
    parser.add_argument("--corpus-commit", required=True)
    args = parser.parse_args()

    if platform.system() != "Linux" or not Path("/proc/self/smaps_rollup").is_file():
        parser.error("the memory benchmark requires Linux with `/proc` smaps_rollup")
    if args.runs < 1:
        parser.error("--runs must be positive")

    project = Path(args.project).resolve()
    files = [Path(path).resolve() for path in args.files]
    stderr_dir = Path(args.stderr_dir).resolve()
    scratch_dir = Path(args.scratch_dir).resolve()
    scratch_dir.mkdir(parents=True, exist_ok=True)
    missing = [str(path) for path in [project, *files] if not path.exists()]
    if missing:
        parser.error(f"missing benchmark paths: {', '.join(missing)}")

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
                "runs": runs,
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
            "open_files": [str(path.relative_to(project)) for path in files],
            "open_bytes": sum(path.stat().st_size for path in files),
            "operations": ["diagnostics", "document symbols", "hover"],
        },
        "servers": records,
    }
    Path(args.out).write_text(json.dumps(output, indent=2) + "\n")


if __name__ == "__main__":
    main()
