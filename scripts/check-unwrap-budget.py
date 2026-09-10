#!/usr/bin/env python3
"""Count first-party `unwrap()` and enforce the per-crate ceiling (issue #150).

Calibre
-------
Crate roots deny `clippy::unwrap_used` for library code. `clippy.toml` turns the restriction
lints off for test code (`allow-unwrap-in-tests`, `allow-expect-in-tests`,
`allow-panic-in-tests`), and the budget counts the code those lints still see:

- only `crates/<crate>/src/**/*.rs` of the root manifest's `[workspace] members`; the
  vendored crates in `[workspace.exclude]` keep upstream shapes and are out of scope, and
  `crates/<crate>/tests/**` is a separate test target that a crate-level deny cannot reach;
- whole-line `#[cfg(test)]` before `mod <name> { ... }` removes that block, skipping
  attributes, comments and blank lines between the attribute and `mod`;
- files reachable from a crate root (`src/lib.rs`, `src/main.rs`, `src/bin/*.rs`) only
  through `#[cfg(test)]` module declarations are dropped as well, because their whole
  module tree is cfg(test)-gated (`crates/theway-tui/src/ui/tests.rs` and the files it
  pulls in under `src/ui/tests/`);
- `#[cfg(test)]` on a single item (`use`, `fn`, `static`, `impl`) does *not* remove that
  item from the non-test count. Those items stay in the number the budget gates, which
  over-counts rather than under-counts;
- `#[cfg(all(test, feature = "..."))]` and `#[cfg(any(test, ...))]` are *not* treated as
  test code: `allow-unwrap-in-tests` only recognises a standalone `#[cfg(test)]`, so clippy
  reports those unwraps and the budget has to carry them
  (rust-lang/rust-clippy#16369). `--report` names how many of them exist.

`--report` prints the non-test count next to the raw count (every `.unwrap()` under `src/`,
test code included) so both calibres stay readable. `expect(`, `panic!` and `unreachable!`
are printed as non-test trend information and never gate.

Exit codes: 0 within budget, 1 over budget, 2 the calibre itself failed (unresolvable
`mod` declaration, unbalanced braces, budget table out of sync with the workspace members).
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path
from typing import NamedTuple

ROOT = Path(__file__).resolve().parents[1]

# Ceiling of non-test `.unwrap()` per crate. The values are what
# `scripts/check-unwrap-budget.py --report` measured on 2026-09-10 at commit 356640e with the
# issue #150 convergence branch applied; they are a snapshot, not the table in issue #150,
# which counts a different calibre. Lower a ceiling whenever a convergence pass lands; raise
# one only with an issue that justifies the new number.
BUDGET = {
    # Every non-test `.unwrap()` was converted in the issue #150 convergence pass: 63 production
    # sites plus 15 inside `#[cfg(all(test, feature = "local"))]` blocks that clippy still reports.
    "theway-daemon": 0,
    # Two `#[cfg(test)]`-gated test helpers in `src/ui_state.rs` and two in
    # `src/config_payload.rs`; single-item `#[cfg(test)]` stays in the count on purpose.
    "theway-tui": 4,
    "theway-transport": 0,
    "theway-llm-provider": 0,
    "theway-extensions": 0,
    "theway-contract": 0,
    "theway-core": 0,
    "theway-storage": 0,
    "theway-mcp": 0,
    "theway-probe": 0,
    "tests-bridge-macro": 0,
}

CFG_TEST_ATTR = "#[cfg(test)]"
# `#[cfg(all(test, ...))]` / `#[cfg(any(test, ...))]`: test-only code that clippy still lints
# because `allow-unwrap-in-tests` only recognises a standalone `#[cfg(test)]`.
CFG_TEST_WITH_EXTRA_GATE_RE = re.compile(r"^#\[cfg\((?:all|any)\(\s*test\b")
MOD_HEAD_RE = re.compile(
    r"^\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*([{;])\s*$"
)
PATH_ATTR_RE = re.compile(r'^#\[\s*path\s*=\s*"([^"]+)"\s*\]$')
ATTRIBUTE_RE = re.compile(r"^#\[.*\]$")
PATTERNS = {
    "unwrap": re.compile(r"\.unwrap\s*\(\s*\)"),
    "expect": re.compile(r"\.expect\s*\("),
    "panic": re.compile(r"\bpanic!\s*\("),
    "unreachable": re.compile(r"\bunreachable!\s*\("),
}
# Entry points of a crate's module tree; every other file is reached through `mod` edges.
CRATE_ROOTS = ("lib.rs", "main.rs")


def display(path: Path) -> str:
    """`path` relative to the repository root when it lives there, else as given."""

    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


class Declaration(NamedTuple):
    """One out-of-line `mod <name>;` declaration."""

    line: int
    name: str
    path_attribute: str | None
    cfg_test: bool


class Parsed(NamedTuple):
    lines: list[str]
    kept: list[bool]
    declarations: list[Declaration]
    stripped_blocks: int
    single_item_markers: int
    single_item_unwraps: int
    feature_gated_blocks: int
    feature_gated_unwraps: int


def die(message: str) -> None:
    print(f"check-unwrap-budget: {message}", file=sys.stderr)
    raise SystemExit(2)


def scan_lines(lines: list[str], label: str) -> tuple[list[bool], list[int]]:
    """Return per line `(starts_in_code, brace depth at line start)`.

    Braces inside comments, string literals, raw strings, byte strings and char literals do
    not move the depth, so a `mod tests { ... }` block can be located even when its body
    builds `{`-bearing JSON or format strings.
    """

    text = "\n".join(lines)
    size = len(text)
    starts_in_code = [True]
    depth_at_start = [0]
    state = "code"
    block_depth = 0
    depth = 0
    raw_hashes = 0
    index = 0

    while index < size:
        char = text[index]

        if char == "\n":
            index += 1
            if state == "line_comment":
                state = "code"
            starts_in_code.append(state == "code")
            depth_at_start.append(depth)
            continue

        if state == "line_comment":
            index += 1
            continue

        if state == "block_comment":
            if text.startswith("/*", index):
                block_depth += 1
                index += 2
            elif text.startswith("*/", index):
                block_depth -= 1
                index += 2
                if block_depth == 0:
                    state = "code"
            else:
                index += 1
            continue

        if state == "string":
            if char == "\\":
                # A trailing backslash continues the literal on the next line: consume only
                # the backslash so the newline still ends the line for the depth map.
                index += 1 if text[index + 1 : index + 2] == "\n" else 2
            else:
                if char == '"':
                    state = "code"
                index += 1
            continue

        if state == "raw_string":
            if char == '"' and text.startswith("#" * raw_hashes, index + 1):
                index += 1 + raw_hashes
                state = "code"
            else:
                index += 1
            continue

        if state == "char":
            if char == "\\":
                index += 2
            else:
                if char == "'":
                    state = "code"
                index += 1
            continue

        # code
        if text.startswith("//", index):
            state = "line_comment"
            index += 2
            continue
        if text.startswith("/*", index):
            state = "block_comment"
            block_depth = 1
            index += 2
            continue
        if char == '"':
            state = "string"
            index += 1
            continue
        if char == "r" or text.startswith(("br", 'b"'), index):
            match = re.match(r'(?:b?r(#*)"|b")', text[index:])
            if match:
                token = match.group(0)
                if token == 'b"':
                    state = "string"
                    index += 2
                    continue
                raw_hashes = len(match.group(1) or "")
                state = "raw_string"
                index += len(token)
                continue
        if char == "'":
            if text[index + 1 : index + 2] == "\\":
                state = "char"
                index += 1
                continue
            if text[index + 2 : index + 3] == "'":
                index += 3
                continue
            index += 1
            continue
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
        index += 1

    if depth != 0:
        die(f"{label}: brace depth does not balance; the test-module calibre is unreliable")
    while len(starts_in_code) < len(lines):
        starts_in_code.append(state == "code")
        depth_at_start.append(depth)
    return starts_in_code, depth_at_start


def block_end(
    lines: list[str], starts_in_code: list[bool], depth_at_start: list[int], head: int
) -> int | None:
    """Exclusive end line of the brace block that opens on `head`."""

    base = depth_at_start[head]
    for line in range(head + 1, len(lines)):
        if starts_in_code[line] and depth_at_start[line] == base:
            return line
    return None


def parse_file(path: Path) -> Parsed:
    lines = path.read_text(encoding="utf-8").split("\n")
    starts_in_code, depth_at_start = scan_lines(lines, display(path))
    kept = [True] * len(lines)
    declarations: list[Declaration] = []
    stripped_blocks = 0
    single_item_markers = 0
    single_item_unwraps = 0
    feature_gated_blocks = 0
    feature_gated_unwraps = 0
    pending: list[str] = []

    index = 0
    while index < len(lines):
        stripped = lines[index].strip()

        if starts_in_code[index] and ATTRIBUTE_RE.match(stripped):
            pending.append(stripped)
            index += 1
            continue
        if not starts_in_code[index] or stripped == "" or stripped.startswith("//"):
            index += 1
            continue

        head = MOD_HEAD_RE.match(lines[index])
        if head:
            name, terminator = head.group(1), head.group(2)
            cfg_test = CFG_TEST_ATTR in pending
            feature_gated = any(
                CFG_TEST_WITH_EXTRA_GATE_RE.match(attribute) for attribute in pending
            )
            if terminator == "{":
                if cfg_test:
                    end = block_end(lines, starts_in_code, depth_at_start, index)
                    if end is None:
                        die(f"{path}: `mod {name} {{` never closes")
                    kept[index:end] = [False] * (end - index)
                    stripped_blocks += 1
                    index = end
                    pending = []
                    continue
                if feature_gated:
                    end = block_end(lines, starts_in_code, depth_at_start, index)
                    if end is None:
                        die(f"{path}: `mod {name} {{` never closes")
                    feature_gated_blocks += 1
                    feature_gated_unwraps += sum(
                        len(PATTERNS["unwrap"].findall(line)) for line in lines[index:end]
                    )
            else:
                path_attribute = next(
                    (
                        PATH_ATTR_RE.match(attribute).group(1)
                        for attribute in pending
                        if PATH_ATTR_RE.match(attribute)
                    ),
                    None,
                )
                declarations.append(
                    Declaration(index + 1, name, path_attribute, cfg_test)
                )
        elif CFG_TEST_ATTR in pending:
            # `#[cfg(test)]` on a single item: kept in the non-test count on purpose.
            single_item_markers += 1
            end = (
                block_end(lines, starts_in_code, depth_at_start, index)
                if lines[index].rstrip().endswith("{")
                else None
            )
            for line in lines[index : end if end is not None else index + 1]:
                single_item_unwraps += len(PATTERNS["unwrap"].findall(line))

        pending = []
        index += 1

    return Parsed(
        lines,
        kept,
        declarations,
        stripped_blocks,
        single_item_markers,
        single_item_unwraps,
        feature_gated_blocks,
        feature_gated_unwraps,
    )


def module_directory(path: Path) -> Path:
    """Directory that holds the submodules of the module defined by `path`."""

    if path.name in {"mod.rs", "lib.rs", "main.rs"}:
        return path.parent
    return path.parent / path.stem


def crate_roots(src: Path) -> list[Path]:
    roots = [src / name for name in CRATE_ROOTS if (src / name).is_file()]
    bin_dir = src / "bin"
    if bin_dir.is_dir():
        roots.extend(sorted(path.resolve() for path in bin_dir.glob("*.rs")))
    if not roots:
        die(f"{src}: no lib.rs, main.rs or bin/*.rs entry point")
    return roots


def resolve(declared_in: Path, src: Path, declaration: Declaration) -> Path | None:
    """Resolve one `mod <name>;` declaration to a file under `src`, or None if outside.

    `resolve` makes the returned path symlink-free, `count_crate` scans symlink-free paths,
    and `src` reaches this function resolved, so module edges join the keys the caller
    scanned even when the repository path contains a symlink.
    """

    if declaration.path_attribute is not None:
        target = declared_in.parent / declaration.path_attribute
    else:
        directory = module_directory(declared_in)
        candidates = [directory / f"{declaration.name}.rs", directory / declaration.name / "mod.rs"]
        for candidate in candidates:
            if candidate.is_file():
                target = candidate
                break
        else:
            die(
                f"{declared_in}:{declaration.line}: `mod {declaration.name};` resolves to none "
                f"of {', '.join(str(candidate) for candidate in candidates)}"
            )
    target = target.resolve()
    if not target.is_relative_to(src):
        return None
    if not target.is_file():
        die(f"{declared_in}:{declaration.line}: `mod {declaration.name};` points at missing {target}")
    return target


def test_only_files(src: Path, parsed: dict[Path, Parsed]) -> set[Path]:
    """Return the files reachable from a crate root only through `#[cfg(test)]` modules."""

    edges: dict[Path, list[tuple[Path, bool]]] = {}
    for path, result in parsed.items():
        targets: list[tuple[Path, bool]] = []
        for declaration in result.declarations:
            target = resolve(path, src, declaration)
            if target is not None:
                targets.append((target, declaration.cfg_test))
        edges[path] = targets

    production: set[Path] = set()
    frontier = [root.resolve() for root in crate_roots(src) if root.resolve() in parsed]
    production.update(frontier)
    while frontier:
        current = frontier.pop()
        for target, cfg_test in edges.get(current, ()):
            if not cfg_test and target not in production:
                production.add(target)
                frontier.append(target)

    reachable: set[Path] = set()
    frontier = list(production)
    reachable.update(frontier)
    while frontier:
        current = frontier.pop()
        for target, _ in edges.get(current, ()):
            if target not in reachable:
                reachable.add(target)
                frontier.append(target)

    return reachable - production


def count_crate(src: Path) -> tuple[dict[str, int], dict[str, int]]:
    src = src.resolve()
    files = sorted(path.resolve() for path in src.rglob("*.rs"))
    parsed = {path: parse_file(path) for path in files}
    test_only = test_only_files(src, parsed)

    counts = dict.fromkeys(PATTERNS, 0)
    raw = 0
    stats = {
        "files": len(files),
        "test_only_files": len(test_only),
        "stripped_blocks": 0,
        "single_item_markers": 0,
        "single_item_unwraps": 0,
        "feature_gated_blocks": 0,
        "feature_gated_unwraps": 0,
    }

    for path, result in parsed.items():
        raw += sum(len(PATTERNS["unwrap"].findall(line)) for line in result.lines)
        stats["stripped_blocks"] += result.stripped_blocks
        stats["single_item_markers"] += result.single_item_markers
        stats["single_item_unwraps"] += result.single_item_unwraps
        if path in test_only:
            continue
        stats["feature_gated_blocks"] += result.feature_gated_blocks
        stats["feature_gated_unwraps"] += result.feature_gated_unwraps
        for index, line in enumerate(result.lines):
            if not result.kept[index]:
                continue
            for pattern, regex in PATTERNS.items():
                counts[pattern] += len(regex.findall(line))

    counts["raw"] = raw
    return counts, stats


def workspace_crates() -> list[tuple[str, Path]]:
    with (ROOT / "Cargo.toml").open("rb") as stream:
        manifest = tomllib.load(stream)
    crates: list[tuple[str, Path]] = []
    for member in manifest["workspace"]["members"]:
        directory = ROOT / member
        name = directory.name
        src = directory / "src"
        if not src.is_dir():
            die(f"{member}: workspace member without a src/ directory")
        crates.append((name, src))
    names = [name for name, _ in crates]
    if sorted(names) != sorted(BUDGET):
        missing = sorted(set(names) - set(BUDGET))
        extra = sorted(set(BUDGET) - set(names))
        die(
            "BUDGET must list every workspace member exactly once "
            f"(missing: {missing or 'none'}; not a member: {extra or 'none'})"
        )
    return crates


def main(argv: list[str]) -> int:
    report_only = "--report" in argv
    unknown = [argument for argument in argv if argument != "--report"]
    if unknown:
        die(f"unknown argument(s): {' '.join(unknown)}")

    crates = workspace_crates()
    measured: dict[str, dict[str, int]] = {}
    stats: dict[str, dict[str, int]] = {}
    for name, src in crates:
        measured[name], stats[name] = count_crate(src)

    total_keys = ("unwrap", "raw", "expect", "panic", "unreachable")
    totals = {key: sum(row[key] for row in measured.values()) for key in total_keys}
    over_budget = [
        (name, measured[name]["unwrap"])
        for name, _ in crates
        if measured[name]["unwrap"] > BUDGET[name]
    ]

    if report_only or over_budget:
        print(f"{'crate':<22}{'unwrap':>9}{'raw':>7}{'expect':>9}{'panic!':>8}{'unreachable!':>15}")
        for name, _ in crates:
            row = measured[name]
            print(
                f"{name:<22}{row['unwrap']:>9}{row['raw']:>7}{row['expect']:>9}"
                f"{row['panic']:>8}{row['unreachable']:>15}"
            )
        print(
            f"{'TOTAL':<22}{totals['unwrap']:>9}{totals['raw']:>7}{totals['expect']:>9}"
            f"{totals['panic']:>8}{totals['unreachable']:>15}"
        )
        print("unwrap/expect/panic!/unreachable! count non-test code only; raw counts every")
        print(".unwrap() under src/, test modules included")

    if report_only:
        blocks = sum(row["stripped_blocks"] for row in stats.values())
        files = sum(row["test_only_files"] for row in stats.values())
        markers = sum(row["single_item_markers"] for row in stats.values())
        marker_unwraps = sum(row["single_item_unwraps"] for row in stats.values())
        gated = sum(row["feature_gated_blocks"] for row in stats.values())
        gated_unwraps = sum(row["feature_gated_unwraps"] for row in stats.values())
        print(
            f"cfg(test): {blocks} inline module blocks stripped, {files} test-only files "
            f"excluded, {markers} single-item `#[cfg(test)]` markers kept in the non-test "
            f"count ({marker_unwraps} unwrap() inside)"
        )
        print(
            f"cfg(all/any(test, ...)): {gated} blocks stay in the non-test count "
            f"({gated_unwraps} unwrap() inside), because clippy only exempts a standalone "
            "#[cfg(test)]"
        )
        return 0

    if over_budget:
        print("non-test unwrap() over budget:", file=sys.stderr)
        for name, count in over_budget:
            print(f"- {name}: {count} (budget {BUDGET[name]})", file=sys.stderr)
        print(
            "remove the unwrap() or raise the ceiling in BUDGET with an issue reference",
            file=sys.stderr,
        )
        return 1

    print(
        f"non-test unwrap() within budget: {totals['unwrap']} of "
        f"{sum(BUDGET.values())} across {len(crates)} workspace crates"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
