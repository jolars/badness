#!/usr/bin/env python3
"""Unit tests for the external language-server memory harness."""

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
            self._run(10, 20, 24, 9, 18),
            self._run(12, 22, 28, 11, 20),
            self._run(11, 21, 26, 10, 19),
        ]
        summary = memory.summarize_runs(runs)
        self.assertEqual(summary["baseline_rss_mb"], 11)
        self.assertEqual(summary["settled_rss_mb"], 21)
        self.assertEqual(summary["settled_pss_mb"], 19)
        self.assertEqual(summary["peak_rss_mb"], 26)

        records = [
            {"key": "badness", "summary": summary},
            {"key": "texlab", "summary": {**summary, "settled_rss_mb": 42}},
        ]
        memory.add_relative_memory(records)
        self.assertEqual(records[0]["summary"]["relative_to_badness"], 1.0)
        self.assertEqual(records[1]["summary"]["relative_to_badness"], 2.0)

    def test_quiet_window_covers_the_requested_duration(self):
        sampler = memory.Sampler(1, interval=0.15)
        idle = memory.ProcessSample(rss_kb=1, pss_kb=1, cpu_ticks=0, processes=1)
        sampler.samples = [(0.0, idle), (2.5, idle), (4.0, idle)]
        self.assertFalse(sampler.is_quiet(5.0))

        sampler.samples.append((5.0, idle))
        self.assertTrue(sampler.is_quiet(5.0))

    @staticmethod
    def _run(baseline_rss, settled_rss, peak_rss, baseline_pss, settled_pss):
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
        }


if __name__ == "__main__":
    unittest.main()
