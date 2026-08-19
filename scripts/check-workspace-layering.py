#!/usr/bin/env python3
"""Enforce the workspace crate dependency boundaries."""

from __future__ import annotations

import sys
import tomllib
from pathlib import Path
from typing import Any, Iterator


ROOT = Path(__file__).resolve().parents[1]


def load_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as stream:
        return tomllib.load(stream)


def dependency_tables(
    value: dict[str, Any], path: tuple[str, ...] = ()
) -> Iterator[tuple[str, dict[str, Any]]]:
    for key, child in value.items():
        child_path = (*path, key)
        if key in {"dependencies", "dev-dependencies", "build-dependencies"}:
            yield ".".join(child_path), child
        elif isinstance(child, dict):
            yield from dependency_tables(child, child_path)


manifests: dict[str, tuple[Path, dict[str, Any]]] = {}
for manifest_path in sorted((ROOT / "crates").glob("*/Cargo.toml")):
    manifest = load_toml(manifest_path)
    package_name = manifest.get("package", {}).get("name")
    if package_name:
        manifests[package_name] = (manifest_path, manifest)

workspace_packages = set(manifests)
edges: dict[str, set[str]] = {name: set() for name in manifests}
edge_locations: dict[tuple[str, str], list[str]] = {}

for package_name, (manifest_path, manifest) in manifests.items():
    for table_name, dependencies in dependency_tables(manifest):
        for dependency_name, specification in dependencies.items():
            resolved_name = (
                specification.get("package", dependency_name)
                if isinstance(specification, dict)
                else dependency_name
            )
            if resolved_name not in workspace_packages:
                continue
            edges[package_name].add(resolved_name)
            edge_locations.setdefault((package_name, resolved_name), []).append(
                f"{manifest_path.relative_to(ROOT)} [{table_name}]"
            )

violations: list[str] = []


def forbid(source: str, dependencies: set[str]) -> None:
    for dependency in sorted(edges.get(source, set()) & dependencies):
        locations = ", ".join(edge_locations[(source, dependency)])
        violations.append(f"{source} must not depend on {dependency}: {locations}")


core_consumers = {
    package for package, dependencies in edges.items() if "theway-core" in dependencies
}
unexpected_core_consumers = core_consumers - {"theway-daemon"}
for package in sorted(unexpected_core_consumers):
    locations = ", ".join(edge_locations[(package, "theway-core")])
    violations.append(
        f"only theway-daemon may depend directly on theway-core: {locations}"
    )

forbid("theway-contract", workspace_packages - {"theway-contract"})
forbid(
    "theway-core",
    {"theway-daemon", "theway-storage", "theway-transport", "theway-tui"},
)
forbid(
    "theway-storage",
    {
        "theway-core",
        "theway-daemon",
        "theway-llm-provider",
        "theway-mcp",
        "theway-transport",
        "theway-tui",
    },
)
forbid(
    "theway-transport",
    {"theway-core", "theway-daemon", "theway-storage", "theway-tui"},
)
forbid(
    "theway-tui",
    {"theway-core", "theway-daemon", "theway-llm-provider"},
)

required_edges = {
    ("theway-core", "theway-contract"),
    ("theway-daemon", "theway-core"),
    ("theway-storage", "theway-contract"),
    ("theway-transport", "theway-contract"),
    ("theway-tui", "theway-storage"),
    ("theway-tui", "theway-transport"),
}
for source, dependency in sorted(required_edges):
    if dependency not in edges.get(source, set()):
        violations.append(f"{source} must depend directly on {dependency}")

if violations:
    print("workspace layering violations:", file=sys.stderr)
    for violation in violations:
        print(f"- {violation}", file=sys.stderr)
    raise SystemExit(1)

print("workspace dependency layering is valid")
