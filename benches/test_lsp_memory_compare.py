"""Unit tests for the external language-server speed and memory harness."""

import io
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lsp_memory_compare as memory


class ProcParsingTests(unittest.TestCase):
    def test_process_tree_and_sample_are_aggregated(self):
        with tempfile.TemporaryDirectory() as temp:
            proc = Path(temp)
            self._write_process(
                proc, 100, children="101", rss=1200, pss=900, ticks=(7, 5)
            )
            self._write_process(proc, 101, children="", rss=800, pss=700, ticks=(3, 2))

            self.assertEqual(memory.process_tree(100, proc), {100, 101})
            self.assertEqual(
                memory.sample_tree(100, proc),
                memory.ProcessSample(
                    rss_kb=2000, pss_kb=1600, cpu_ticks=17, processes=2
                ),
            )

    @staticmethod
    def _write_process(proc, pid, *, children, rss, pss, ticks):
        root = proc / str(pid)
        (root / "task" / str(pid)).mkdir(parents=True)
        (root / "task" / str(pid) / "children").write_text(children)
        (root / "status").write_text(f"Name:\ttest\nVmRSS:\t{rss} kB\n")
        (root / "smaps_rollup").write_text(f"Rss: 0 kB\nPss: {pss} kB\n")
        # The command deliberately contains spaces and parentheses: parsing must
        # split after the final `)`, not split the entire line on whitespace.
        fields = ["S"] + ["0"] * 10 + [str(ticks[0]), str(ticks[1])] + ["0"] * 8
        (root / "stat").write_text(f"{pid} (test worker ({pid})) " + " ".join(fields))


class ProtocolTests(unittest.TestCase):
    def test_reads_content_length_framed_message(self):
        payload = b'{"jsonrpc":"2.0","id":7,"result":null}'
        stream = io.BytesIO(b"Content-Length: %d\r\n\r\n" % len(payload) + payload)
        self.assertEqual(
            memory.read_lsp_message(stream), {"jsonrpc": "2.0", "id": 7, "result": None}
        )


class SummaryTests(unittest.TestCase):
    def test_server_summary_uses_medians_and_badness_ratio(self):
        runs = [
            self._run(10, 20, 24, 9, 18, 0.01, 0.10, 0.20, [1.0, 2.0]),
            self._run(12, 22, 28, 11, 20, 0.03, 0.30, 0.40, [3.0, 4.0]),
            self._run(11, 21, 26, 10, 19, 0.02, 0.20, 0.30, [5.0, 6.0]),
        ]
        summary = memory.summarize_runs(runs)
        self.assertEqual(summary["baseline_rss_mb"], 11)
        self.assertEqual(summary["settled_rss_mb"], 21)
        self.assertEqual(summary["settled_pss_mb"], 19)
        self.assertEqual(summary["peak_rss_mb"], 26)
        self.assertEqual(summary["initialize_seconds"], 0.02)
        self.assertEqual(summary["workspace_ready_seconds"], 0.2)
        self.assertEqual(summary["documents_ready_seconds"], 0.3)
        latency = summary["request_latencies"][0]
        self.assertEqual(latency["median_ms"], 3.5)
        self.assertEqual(latency["p95_ms"], 6.0)
        self.assertEqual(latency["result_count_min"], 1)
        self.assertEqual(latency["result_count_median"], 2.0)
        self.assertEqual(latency["result_count_max"], 3)
        self.assertEqual(latency["result_files_min"], 1)
        self.assertEqual(latency["payload_bytes_median"], 150)

        records = [
            {"key": "badness", "summary": summary},
            {"key": "texlab", "summary": {**summary, "settled_rss_mb": 42}},
        ]
        memory.add_relative_memory(records)
        self.assertEqual(records[0]["summary"]["relative_to_badness"], 1.0)
        self.assertEqual(records[1]["summary"]["relative_to_badness"], 2.0)

    def test_quiet_window_covers_the_requested_duration_and_phase_start(self):
        sampler = memory.Sampler(1, interval=0.15)
        idle = memory.ProcessSample(rss_kb=1, pss_kb=1, cpu_ticks=0, processes=1)
        sampler.samples = [(0.0, idle), (2.5, idle), (4.0, idle)]
        self.assertIsNone(sampler.quiet_since(5.0))

        sampler.samples.append((5.0, idle))
        self.assertEqual(sampler.quiet_since(5.0), 0.0)
        self.assertIsNone(sampler.quiet_since(5.0, not_before=1.0))

    def test_navigation_target_finds_pinned_citation(self):
        with tempfile.TemporaryDirectory() as temp:
            project = Path(temp)
            chapter = project / "Chapter1" / "chapter1.tex"
            chapter.parent.mkdir()
            chapter.write_text(
                "Prelude\nLorem Ipsum~\\citep{Aup91} has been the industry's\n"
            )

            target = memory.navigation_target(project, [chapter])

        self.assertEqual(target["file"], "Chapter1/chapter1.tex")
        self.assertEqual(target["symbol"], "Aup91")
        self.assertEqual(target["position"], {"line": 1, "character": 19})

    def test_result_summaries_count_nested_and_cross_file_work(self):
        symbols = [
            {"name": "chapter", "children": [{"name": "section"}]},
            {"name": "figure"},
        ]
        locations = [
            {"uri": "file:///chapter.tex", "range": {}},
            {"targetUri": "file:///references.bib", "targetRange": {}},
        ]
        edit = {
            "changes": {"file:///chapter.tex": [{}, {}]},
            "documentChanges": [
                {
                    "textDocument": {"uri": "file:///references.bib"},
                    "edits": [{}],
                }
            ],
        }
        self.assertEqual(memory.document_symbol_summary(symbols), (3, None))
        self.assertEqual(memory.location_summary(locations), (2, 2))
        self.assertEqual(memory.workspace_edit_summary(edit), (3, 2))

    def test_hover_targets_prefer_reference_keys_across_documents(self):
        documents = [
            ("file:///main.tex", "\\documentclass{book}\n"),
            ("file:///one.tex", "See~\\ref{section-one}.\n"),
            ("file:///two.tex", "Read~\\citep{Aup91}.\n"),
        ]
        self.assertEqual(
            memory.hover_targets(documents, limit=2),
            [
                (
                    "file:///one.tex",
                    {"line": 0, "character": 9},
                ),
                (
                    "file:///two.tex",
                    {"line": 0, "character": 12},
                ),
            ],
        )

    @staticmethod
    def _run(
        baseline_rss,
        settled_rss,
        peak_rss,
        baseline_pss,
        settled_pss,
        initialize_seconds,
        workspace_ready_seconds,
        documents_ready_seconds,
        latencies,
    ):
        return {
            "milestones": {
                "baseline": {
                    "rss_mb": baseline_rss,
                    "pss_mb": baseline_pss,
                    "processes": 1,
                },
                "settled": {
                    "rss_mb": settled_rss,
                    "pss_mb": settled_pss,
                    "processes": 1,
                },
                "peak": {"rss_mb": peak_rss, "pss_mb": settled_pss},
            },
            "settled_seconds": 3.0,
            "initialize_seconds": initialize_seconds,
            "workspace_ready_seconds": workspace_ready_seconds,
            "documents_ready_seconds": documents_ready_seconds,
            "request_latencies": [
                {
                    "key": "definition",
                    "label": "Go to definition",
                    "samples": len(latencies),
                    "failures": 0,
                    "empty_results": 0,
                    "targets": 1,
                    "result_unit": "location",
                    "_latencies_ms": latencies,
                    "_result_counts": [1, 2, 3],
                    "_result_files": [1, 2, 2],
                    "_payload_bytes": [100, 150, 200],
                }
            ],
        }


if __name__ == "__main__":
    unittest.main()
