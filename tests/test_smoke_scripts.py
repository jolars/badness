import gzip
import hashlib
import importlib.util
import io
import os
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FETCH_SCRIPT = ROOT / "scripts" / "fetch_arxiv_sources.py"
SCAN_SCRIPT = ROOT / "scripts" / "smoke_scan_project.sh"
SMOKE_WORKFLOW = ROOT / ".github" / "workflows" / "smoke-test.yml"
SPEC = importlib.util.spec_from_file_location("fetch_arxiv_sources", FETCH_SCRIPT)
assert SPEC and SPEC.loader
FETCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FETCH)


def tar_bytes(files: dict[str, bytes]) -> bytes:
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for name, content in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(content)
            archive.addfile(info, io.BytesIO(content))
    return output.getvalue()


class FetchArxivSourcesTests(unittest.TestCase):
    def test_fetches_all_supported_project_files(self) -> None:
        archive = tar_bytes(
            {
                "paper/main.tex": b"main\n",
                "paper/chapters/one.tex": b"chapter\n",
                "paper/local.sty": b"style\n",
                "paper/figure.pdf": b"ignored\n",
            }
        )
        digest = hashlib.sha256(archive).hexdigest()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            downloads = root / "downloads"
            downloads.mkdir()
            (downloads / "2401.00001v1").write_bytes(archive)
            manifest = root / "manifest.tsv"
            manifest.write_text(f"2401.00001v1 {digest}\n", encoding="utf-8")
            index = root / "index.tsv"
            output = root / "output"

            FETCH.fetch_sources(manifest, output, index, downloads.as_uri(), "test")

            project = output / "2401.00001v1" / "paper"
            self.assertEqual((project / "main.tex").read_bytes(), b"main\n")
            self.assertEqual(
                (project / "chapters" / "one.tex").read_bytes(), b"chapter\n"
            )
            self.assertEqual((project / "local.sty").read_bytes(), b"style\n")
            self.assertFalse((project / "figure.pdf").exists())
            self.assertIn("arxiv:2401.00001v1", index.read_text(encoding="utf-8"))

    def test_rejects_unsafe_archive_paths(self) -> None:
        archive = tar_bytes({"../escape.tex": b"bad\n"})
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "output"
            destination.mkdir()
            with self.assertRaisesRegex(ValueError, "unsafe archive member path"):
                FETCH.extract_supported_sources(archive, destination)

    def test_accepts_a_single_compressed_source_file(self) -> None:
        archive = gzip.compress(b"\\documentclass{article}\n")
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary)
            count = FETCH.extract_supported_sources(archive, destination)
            self.assertEqual(count, 1)
            self.assertEqual(
                (destination / "source.tex").read_bytes(),
                b"\\documentclass{article}\n",
            )


class SmokeScanProjectTests(unittest.TestCase):
    def test_scans_nested_files_and_records_skips_and_failures(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            project = root / "project"
            (project / "chapter").mkdir(parents=True)
            (project / "main.tex").write_text("pass\n", encoding="utf-8")
            (project / "chapter" / "bad.tex").write_text("fail\n", encoding="utf-8")
            (project / "legacy.tex").write_bytes(b"bad\x95encoding\n")
            fake_badness = root / "badness"
            fake_badness.write_text(
                "#!/usr/bin/env bash\n"
                'if [[ "$*" == *bad.tex* ]]; then\n'
                "  echo 'Debug check failed (losslessness)'\n"
                "  exit 1\n"
                "fi\n"
                "echo 'All checks passed'\n",
                encoding="utf-8",
            )
            fake_badness.chmod(0o755)
            results = root / "results"
            environment = {
                **os.environ,
                "BADNESS_BIN": str(fake_badness),
                "BADNESS_SHA": "badness-sha",
                "BADNESS_VERSION": "badness test",
                "ALLOWLIST": "",
            }

            subprocess.run(
                [
                    str(SCAN_SCRIPT),
                    str(results),
                    "arxiv",
                    "arxiv:2401.00001v1",
                    "archive-sha",
                    str(project),
                    "all",
                    "none",
                ],
                check=True,
                env=environment,
            )

            failures = (results / "failures.tsv").read_text(encoding="utf-8")
            self.assertIn("arxiv:2401.00001v1\tlosslessness\tchapter/bad.tex", failures)
            skipped = (results / "skipped.tsv").read_text(encoding="utf-8")
            self.assertIn("arxiv:2401.00001v1\tlegacy.tex\tnon-utf8", skipped)
            self.assertEqual((results / "scanned_count").read_text().strip(), "2")
            self.assertEqual((results / "skipped_count").read_text().strip(), "1")

    def test_tracked_mode_ignores_untracked_files_and_applies_allowlist(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            project = root / "project"
            project.mkdir()
            (project / "tracked.tex").write_text("fail\n", encoding="utf-8")
            (project / "untracked.tex").write_text("fail\n", encoding="utf-8")
            subprocess.run(["git", "init", "--quiet", str(project)], check=True)
            subprocess.run(
                ["git", "-C", str(project), "add", "tracked.tex"], check=True
            )
            fake_badness = root / "badness"
            fake_badness.write_text(
                "#!/usr/bin/env bash\n"
                "echo 'Debug check failed (format-error)'\n"
                "exit 1\n",
                encoding="utf-8",
            )
            fake_badness.chmod(0o755)
            results = root / "results"
            environment = {
                **os.environ,
                "BADNESS_BIN": str(fake_badness),
                "BADNESS_SHA": "badness-sha",
                "BADNESS_VERSION": "badness test",
                "ALLOWLIST": "owner/repo|tracked.tex|format-error",
            }

            subprocess.run(
                [
                    str(SCAN_SCRIPT),
                    str(results),
                    "github",
                    "owner/repo",
                    "commit-sha",
                    str(project),
                    "tracked",
                    "project",
                ],
                check=True,
                env=environment,
            )

            self.assertEqual(
                len((results / "failures.tsv").read_text().splitlines()), 1
            )
            self.assertEqual((results / "scanned_count").read_text().strip(), "1")
            self.assertEqual((results / "suppressed_count").read_text().strip(), "1")


class SmokeWorkflowTests(unittest.TestCase):
    def test_repository_clones_stay_outside_the_uploaded_results(self) -> None:
        workflow = SMOKE_WORKFLOW.read_text(encoding="utf-8")

        self.assertIn(
            'REPOS_DIR="$RUNNER_TEMP/badness-debug-format-repos/github"',
            workflow,
        )
        self.assertNotIn('REPOS_DIR="$RESULTS_DIR/repos"', workflow)


if __name__ == "__main__":
    unittest.main()
