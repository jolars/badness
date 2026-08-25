#!/usr/bin/env python3
"""Fetch pinned arXiv source projects for the debug-format smoke scan."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import re
import tarfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib
from pathlib import Path, PurePosixPath

SUPPORTED_SUFFIXES = {".tex", ".sty", ".cls", ".dtx", ".ins", ".bib"}
MAX_DOWNLOAD_BYTES = 100 * 1024 * 1024
MAX_EXTRACTED_BYTES = 250 * 1024 * 1024
IDENTIFIER_RE = re.compile(
    r"^(?:[0-9]{4}\.[0-9]{4,5}|[A-Za-z.-]+/[0-9]{7})v[1-9][0-9]*$"
)


def parse_manifest(path: Path) -> list[tuple[str, str]]:
    entries: list[tuple[str, str]] = []
    seen: set[str] = set()
    for line_number, raw_line in enumerate(
        path.read_text(encoding="utf-8").splitlines(), 1
    ):
        line = raw_line.split("#", 1)[0].strip()
        if not line:
            continue
        fields = line.split()
        if len(fields) != 2:
            raise ValueError(f"{path}:{line_number}: expected ARXIV_ID SHA256")
        identifier, digest = fields
        if not IDENTIFIER_RE.fullmatch(identifier):
            raise ValueError(
                f"{path}:{line_number}: invalid versioned arXiv identifier {identifier!r}"
            )
        if not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise ValueError(f"{path}:{line_number}: invalid SHA-256 {digest!r}")
        if identifier in seen:
            raise ValueError(
                f"{path}:{line_number}: duplicate identifier {identifier!r}"
            )
        seen.add(identifier)
        entries.append((identifier, digest))
    if not entries:
        raise ValueError(f"{path}: manifest contains no sources")
    return entries


def download(url: str, user_agent: str, attempts: int = 3) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": user_agent})
    for attempt in range(1, attempts + 1):
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                chunks: list[bytes] = []
                size = 0
                while chunk := response.read(1024 * 1024):
                    size += len(chunk)
                    if size > MAX_DOWNLOAD_BYTES:
                        raise ValueError(
                            f"download exceeds {MAX_DOWNLOAD_BYTES} bytes: {url}"
                        )
                    chunks.append(chunk)
                return b"".join(chunks)
        except (TimeoutError, urllib.error.URLError):
            if attempt == attempts:
                raise
            time.sleep(attempt)
    raise AssertionError("unreachable")


def safe_member_path(name: str) -> PurePosixPath:
    path = PurePosixPath(name)
    has_control_character = any(
        ord(character) < 32 or ord(character) == 127 for character in name
    )
    if not name or path.is_absolute() or ".." in path.parts or has_control_character:
        raise ValueError(f"unsafe archive member path: {name!r}")
    return path


def extract_supported_sources(archive: bytes, destination: Path) -> int:
    try:
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:*") as source_archive:
            return extract_tar_sources(source_archive, destination)
    except tarfile.ReadError as error:
        try:
            source = gzip.decompress(archive)
        except (OSError, EOFError, zlib.error):
            raise ValueError(
                "arXiv source payload is not a supported source archive"
            ) from error
        if len(source) > MAX_EXTRACTED_BYTES:
            raise ValueError(
                f"source file exceeds {MAX_EXTRACTED_BYTES} extracted bytes"
            )
        (destination / "source.tex").write_bytes(source)
        return 1


def extract_tar_sources(source_archive: tarfile.TarFile, destination: Path) -> int:
    extracted_bytes = 0
    extracted_files = 0
    seen: set[PurePosixPath] = set()
    for member in source_archive:
        path = safe_member_path(member.name)
        if member.isdir():
            continue
        if path.suffix.lower() not in SUPPORTED_SUFFIXES:
            continue
        if not member.isfile():
            raise ValueError(f"unsupported source member type: {member.name!r}")
        if path in seen:
            raise ValueError(f"duplicate archive member: {member.name!r}")
        seen.add(path)
        extracted_bytes += member.size
        if extracted_bytes > MAX_EXTRACTED_BYTES:
            raise ValueError(
                f"supported source files exceed {MAX_EXTRACTED_BYTES} extracted bytes"
            )
        source = source_archive.extractfile(member)
        if source is None:
            raise ValueError(f"cannot read archive member: {member.name!r}")
        target = destination.joinpath(*path.parts)
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open("xb") as output:
            output.write(source.read())
        extracted_files += 1

    if extracted_files == 0:
        raise ValueError("arXiv source archive contains no supported source files")
    return extracted_files


def directory_name(identifier: str) -> str:
    return identifier.replace("/", "__")


def fetch_sources(
    manifest: Path,
    output_dir: Path,
    index_path: Path,
    base_url: str,
    user_agent: str,
    delay_seconds: float = 3.0,
) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    index_path.parent.mkdir(parents=True, exist_ok=True)
    index_lines = ["source\trevision\tproject_dir\n"]

    for index, (identifier, expected_digest) in enumerate(parse_manifest(manifest)):
        if index > 0:
            time.sleep(delay_seconds)
        url = f"{base_url.rstrip('/')}/{urllib.parse.quote(identifier, safe='/')}"
        archive = download(url, user_agent)
        actual_digest = hashlib.sha256(archive).hexdigest()
        if actual_digest != expected_digest:
            raise ValueError(
                f"checksum mismatch for {identifier}: expected {expected_digest}, got {actual_digest}"
            )
        project_dir = output_dir / directory_name(identifier)
        if project_dir.exists():
            raise ValueError(f"destination already exists: {project_dir}")
        project_dir.mkdir()
        count = extract_supported_sources(archive, project_dir)
        print(f"{identifier}: extracted {count} supported source file(s)")
        index_lines.append(
            f"arxiv:{identifier}\t{actual_digest}\t{project_dir.resolve()}\n"
        )

    index_path.write_text("".join(index_lines), encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--index", type=Path, required=True)
    parser.add_argument("--base-url", default="https://arxiv.org/src")
    parser.add_argument("--delay", type=float, default=3.0)
    parser.add_argument(
        "--user-agent",
        default="badness smoke test (https://github.com/jolars/badness)",
    )
    args = parser.parse_args()
    fetch_sources(
        args.manifest,
        args.output,
        args.index,
        args.base_url,
        args.user_agent,
        args.delay,
    )


if __name__ == "__main__":
    main()
