#!/usr/bin/env python3
"""Copy juicefront's static tree and stamp every ES module import with the
compiled asset version.

A browser keeps one module graph for the lifetime of a tab, and module imports
do not inherit the query string of their parent. Without a fresh specifier on
each import, a CDN can serve a mix of old and new modules after a deploy. This
mirrors the FNV-1a hash in crates/juicefront/build.rs, copies the source tree to
the destination, and appends that version to relative .js/.mjs specifiers.
"""

from __future__ import annotations

import argparse
import re
import shutil
from pathlib import Path

FNV_OFFSET = 0xCBF29CE484222325
FNV_PRIME = 0x100000001B3
MODULE_SPECIFIER = re.compile(
    r'(?P<prefix>\bfrom\s+|\bimport\s*\(\s*|\bimport\s+)'
    r'(?P<quote>["\'])(?P<path>\.{1,2}/[^"\']+?\.(?:js|mjs))'
    r'(?:\?v=[^"\']*)?(?P=quote)'
)


def static_tree_hash(root: Path) -> str:
    files = sorted(
        (path for path in root.rglob("*") if path.is_file()),
        key=lambda path: str(path),
    )
    value = FNV_OFFSET
    for path in files:
        relative = path.relative_to(root).as_posix()
        for byte in relative.encode():
            value ^= byte
            value = (value * FNV_PRIME) & 0xFFFFFFFFFFFFFFFF
        for byte in path.read_bytes():
            value ^= byte
            value = (value * FNV_PRIME) & 0xFFFFFFFFFFFFFFFF
    return f"{value:016x}"


def package_version(repo_root: Path) -> str:
    cargo_toml = (repo_root / "Cargo.toml").read_text()
    match = re.search(
        r'\[workspace\.package\].*?^\s*version\s*=\s*"([^"]+)"',
        cargo_toml,
        re.MULTILINE | re.DOTALL,
    )
    if not match:
        raise SystemExit("could not find workspace.package.version in Cargo.toml")
    return match.group(1)


def stamp_modules(root: Path, version: str) -> tuple[int, int]:
    files = 0
    specifiers = 0
    for path in sorted(root.rglob("*")):
        if path.suffix not in {".js", ".mjs"} or not path.is_file():
            continue
        source = path.read_text()
        updated, count = MODULE_SPECIFIER.subn(
            lambda match: (
                f"{match.group('prefix')}{match.group('quote')}"
                f"{match.group('path')}?v={version}{match.group('quote')}"
            ),
            source,
        )
        if count:
            path.write_text(updated)
            files += 1
            specifiers += count
    return files, specifiers


def find_repo_root() -> Path:
    for parent in Path(__file__).resolve().parents:
        cargo_toml = parent / "Cargo.toml"
        if cargo_toml.is_file() and "[workspace]" in cargo_toml.read_text():
            return parent
    raise SystemExit("could not find the workspace root")


def main() -> None:
    repo_root = find_repo_root()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source",
        type=Path,
        default=repo_root / "crates/juicefront/static",
        help="source static tree (default: crates/juicefront/static)",
    )
    parser.add_argument(
        "--dest",
        type=Path,
        required=True,
        help="destination static tree, for example /var/lib/juicebox/current/static",
    )
    args = parser.parse_args()

    source = args.source.resolve()
    dest = args.dest.resolve()
    version = f"{package_version(repo_root)}-{static_tree_hash(source)}"

    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(source, dest)
    files, specifiers = stamp_modules(dest, version)
    print(f"asset_version={version}")
    print(f"stamped {specifiers} module specifiers in {files} files")


if __name__ == "__main__":
    main()
