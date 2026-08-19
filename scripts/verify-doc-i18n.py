#!/usr/bin/env python3
"""Verify English-first documentation pairs for the workspace crates."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


DEFAULT_ROOT = Path(__file__).resolve().parents[1]
POLICY_SOURCES = (
    "docs/i18n/README.md",
    "docs/i18n/translation-rules.md",
)
HEX_HASH = re.compile(r"^[0-9a-f]{40,64}$")
HEADING = re.compile(r"^(#{1,6})\s+")
UNORDERED_ITEM = re.compile(r"^(\s*)[-+*]\s+")
ORDERED_ITEM = re.compile(r"^(\s*)(\d+)[.)]\s+")
LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)\s]+)(?:\s+[^)]*)?\)")
FENCE = re.compile(r"^\s*(`{3,}|~{3,})(.*)$")


@dataclass(frozen=True)
class PairPaths:
    source: str
    zh: str
    meta: str


@dataclass(frozen=True)
class MarkdownStructure:
    headings: tuple[int, ...]
    lists: tuple[tuple[str, int, int | None], ...]
    tables: tuple[int, ...]
    links: tuple[str, ...]
    code_blocks: tuple[tuple[str, str, str], ...]


def run_git(root: Path, args: list[str], data: bytes | None = None) -> bytes:
    result = subprocess.run(
        ["git", *args],
        cwd=root,
        input=data,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        message = result.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(f"git {' '.join(args)} failed: {message}")
    return result.stdout


def read_file(root: Path, path: str, cached: bool) -> bytes | None:
    if cached:
        result = subprocess.run(
            ["git", "show", f":{path}"],
            cwd=root,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        return result.stdout if result.returncode == 0 else None
    target = root / path
    return target.read_bytes() if target.is_file() else None


def repository_files(root: Path, cached: bool) -> set[str]:
    args = ["ls-files", "-z"] if cached else ["ls-files", "-co", "--exclude-standard", "-z"]
    output = run_git(root, args)
    return {item.decode("utf-8") for item in output.split(b"\0") if item}


def workspace_members(root: Path, cached: bool) -> list[str]:
    content = read_file(root, "Cargo.toml", cached)
    if content is None:
        raise RuntimeError("Cargo.toml is missing")
    manifest = tomllib.loads(content.decode("utf-8"))
    members = manifest.get("workspace", {}).get("members", [])
    if not isinstance(members, list) or not all(isinstance(item, str) for item in members):
        raise RuntimeError("Cargo.toml [workspace].members must be a string array")
    if any("*" in item or "?" in item or "[" in item for item in members):
        raise RuntimeError("verify-doc-i18n requires explicit Cargo workspace members")
    return sorted(members)


def discover_sources(root: Path, cached: bool) -> list[str]:
    files = repository_files(root, cached)
    sources = set(POLICY_SOURCES)
    for member in workspace_members(root, cached):
        sources.add(f"{member}/README.md")
        sources.add(f"{member}/docs/architecture.md")
        prefix = f"{member}/docs/"
        for path in files:
            if path.startswith(prefix) and path.endswith(".md") and not path.endswith(".zh.md"):
                sources.add(path)
    return sorted(sources)


def pair_paths(source: str) -> PairPaths:
    source_path = Path(source)
    if source.endswith(".zh.md"):
        source_path = Path(source.removesuffix(".zh.md") + ".md")
    elif source.endswith(".i18n.yaml"):
        source_path = Path(source.removesuffix(".i18n.yaml") + ".md")
    elif source_path.suffix != ".md":
        source_path = source_path.with_suffix(".md")
    stem = source_path.name.removesuffix(".md")
    parent = source_path.parent
    return PairPaths(
        source=source_path.as_posix(),
        zh=(parent / f"{stem}.zh.md").as_posix(),
        meta=(parent / f"{stem}.i18n.yaml").as_posix(),
    )


def switcher_lines(paths: PairPaths) -> tuple[str, str]:
    return (
        f"English | [中文]({Path(paths.zh).name})",
        f"[English]({Path(paths.source).name}) | 中文",
    )


def has_switcher(text: str, expected: str) -> bool:
    lines = text.splitlines()
    return len(lines) >= 3 and lines[0].startswith("# ") and lines[1] == "" and lines[2] == expected


def table_columns(line: str) -> int:
    content = line.strip()
    if not (content.startswith("|") and content.endswith("|")):
        return 0
    return len(re.findall(r"(?<!\\)\|", content)) - 1


def markdown_structure(text: str, switcher: str) -> MarkdownStructure:
    headings: list[int] = []
    lists: list[tuple[str, int, int | None]] = []
    tables: list[int] = []
    links: list[str] = []
    code_blocks: list[tuple[str, str, str]] = []
    fence_char: str | None = None
    fence_len = 0
    fence_info = ""
    fence_body: list[str] = []

    for line in text.splitlines():
        fence_match = FENCE.match(line)
        if fence_char is not None:
            if fence_match and fence_match.group(1)[0] == fence_char and len(fence_match.group(1)) >= fence_len:
                code_blocks.append((fence_char * fence_len, fence_info, "\n".join(fence_body)))
                fence_char = None
                fence_body = []
            else:
                fence_body.append(line)
            continue
        if fence_match:
            marker = fence_match.group(1)
            fence_char = marker[0]
            fence_len = len(marker)
            fence_info = fence_match.group(2)
            continue

        heading = HEADING.match(line)
        if heading:
            headings.append(len(heading.group(1)))
        unordered = UNORDERED_ITEM.match(line)
        ordered = ORDERED_ITEM.match(line)
        if unordered:
            lists.append(("unordered", len(unordered.group(1)), None))
        elif ordered:
            lists.append(("ordered", len(ordered.group(1)), int(ordered.group(2))))
        columns = table_columns(line)
        if columns:
            tables.append(columns)
        if line != switcher:
            links.extend(match.group(1) for match in LINK.finditer(line))

    if fence_char is not None:
        code_blocks.append((fence_char * fence_len, fence_info, "\n".join(fence_body)))
    return MarkdownStructure(
        headings=tuple(headings),
        lists=tuple(lists),
        tables=tuple(tables),
        links=tuple(links),
        code_blocks=tuple(code_blocks),
    )


def structure_errors(paths: PairPaths, source_text: str, zh_text: str) -> list[str]:
    source_switcher, zh_switcher = switcher_lines(paths)
    source = markdown_structure(source_text, source_switcher)
    zh = markdown_structure(zh_text, zh_switcher)
    errors: list[str] = []
    for field in ("headings", "lists", "tables", "links", "code_blocks"):
        if getattr(source, field) != getattr(zh, field):
            errors.append(f"{paths.source} ↔ {paths.zh}: {field} structure differs")
    return errors


def git_blob_hash(root: Path, content: bytes, write: bool = False) -> str:
    args = ["hash-object"]
    if write:
        args.append("-w")
    args.append("--stdin")
    return run_git(root, args, content).decode("ascii").strip()


def render_record(paths: PairPaths, source_hash: str, zh_hash: str) -> str:
    return (
        "# English-first bilingual documentation record. Update both files, then run:\n"
        f"#   scripts/verify-doc-i18n.py --write {paths.source}\n"
        f"{Path(paths.source).name}: {source_hash}\n"
        f"{Path(paths.zh).name}: {zh_hash}\n"
    )


def parse_record(paths: PairPaths, text: str) -> tuple[str, str] | None:
    values: dict[str, str] = {}
    for line in text.splitlines():
        if not line or line.startswith("#") or ": " not in line:
            continue
        key, value = line.split(": ", 1)
        values[key] = value
    source_hash = values.get(Path(paths.source).name)
    zh_hash = values.get(Path(paths.zh).name)
    if source_hash and zh_hash and HEX_HASH.fullmatch(source_hash) and HEX_HASH.fullmatch(zh_hash):
        return source_hash, zh_hash
    return None


def pair_content_errors(root: Path, paths: PairPaths, cached: bool) -> tuple[list[str], bytes | None, bytes | None]:
    source = read_file(root, paths.source, cached)
    zh = read_file(root, paths.zh, cached)
    errors: list[str] = []
    missing = [path for path, content in ((paths.source, source), (paths.zh, zh)) if content is None]
    if missing:
        errors.append(f"{paths.source}: incomplete bilingual pair; missing {', '.join(missing)}")
        return errors, source, zh
    assert source is not None and zh is not None
    source_text = source.decode("utf-8")
    zh_text = zh.decode("utf-8")
    source_switcher, zh_switcher = switcher_lines(paths)
    if not has_switcher(source_text, source_switcher):
        errors.append(f"{paths.source}: expected language switcher `{source_switcher}` after the H1")
    if not has_switcher(zh_text, zh_switcher):
        errors.append(f"{paths.zh}: expected language switcher `{zh_switcher}` after the H1")
    errors.extend(structure_errors(paths, source_text, zh_text))
    return errors, source, zh


def verify_pair(root: Path, paths: PairPaths, cached: bool) -> list[str]:
    errors, source, zh = pair_content_errors(root, paths, cached)
    meta = read_file(root, paths.meta, cached)
    if meta is None:
        errors.append(f"{paths.source}: incomplete bilingual pair; missing {paths.meta}")
        return errors
    record = parse_record(paths, meta.decode("utf-8"))
    if record is None:
        errors.append(f"{paths.meta}: malformed bilingual consistency record")
        return errors
    if source is None or zh is None:
        return errors
    actual = (git_blob_hash(root, source), git_blob_hash(root, zh))
    for path, current, recorded in zip((paths.source, paths.zh), actual, record):
        if current != recorded:
            errors.append(
                f"{path}: out of sync with {paths.meta}; update the counterpart and run "
                f"`scripts/verify-doc-i18n.py --write {paths.source}`"
            )
    return errors


def normalize_anchors(root: Path, anchors: Iterable[str]) -> list[str]:
    normalized: list[str] = []
    for anchor in anchors:
        path = Path(anchor)
        if path.is_absolute():
            path = path.relative_to(root)
        normalized.append(pair_paths(path.as_posix()).source)
    return normalized


def write_records(root: Path, sources: list[str]) -> int:
    errors: list[str] = []
    writable: list[tuple[PairPaths, bytes, bytes]] = []
    for source in sources:
        paths = pair_paths(source)
        pair_errors, source_content, zh_content = pair_content_errors(root, paths, False)
        errors.extend(pair_errors)
        if not pair_errors and source_content is not None and zh_content is not None:
            writable.append((paths, source_content, zh_content))
    if errors:
        for error in errors:
            print(f"verify-doc-i18n: {error}", file=sys.stderr)
        return 1
    for paths, source_content, zh_content in writable:
        source_hash = git_blob_hash(root, source_content, write=True)
        zh_hash = git_blob_hash(root, zh_content, write=True)
        (root / paths.meta).write_text(render_record(paths, source_hash, zh_hash), encoding="utf-8")
        print(f"verify-doc-i18n: recorded {paths.meta}")
    return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pairs", nargs="*", help="source, translation, record, or bare pair paths")
    parser.add_argument("--write", action="store_true", help="record confirmed worktree pairs")
    parser.add_argument("--all", action="store_true", help="select the complete corpus with --write")
    parser.add_argument("--list", action="store_true", help="report pair state without failing")
    parser.add_argument("--cached", action="store_true", help="check the staged Git index")
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if args.write and args.cached:
        parser.error("--write and --cached cannot be combined")
    if args.write and args.all == bool(args.pairs):
        parser.error("--write requires either named pairs or --all")
    if args.all and not args.write:
        parser.error("--all is valid only with --write")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    root = args.root.resolve()
    available = discover_sources(root, args.cached)
    selected = normalize_anchors(root, args.pairs) if args.pairs else available
    unknown = sorted(set(selected) - set(available))
    if unknown:
        for source in unknown:
            print(f"verify-doc-i18n: {source} is not an in-scope document", file=sys.stderr)
        return 2
    if args.write:
        return write_records(root, available if args.all else selected)

    failed = False
    for source in selected:
        errors = verify_pair(root, pair_paths(source), args.cached)
        state = "ok" if not errors else "out-of-sync"
        if args.list:
            print(f"{state}\t{source}")
        elif errors:
            for error in errors:
                print(f"verify-doc-i18n: {error}", file=sys.stderr)
        failed = failed or bool(errors)
    if args.list:
        return 0
    if failed:
        return 1
    print(f"verify-doc-i18n: {len(selected)} bilingual pair(s) are synchronized")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
