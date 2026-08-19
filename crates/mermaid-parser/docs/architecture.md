# Mermaid parser architecture

English | [中文](architecture.zh.md)

## Responsibility

`mermaid-rs-parser` owns parsing from source text into an intermediate representation. It identifies the diagram header, validates initialization directives, dispatches to a diagram-specific parser, and returns `ParseOutput`; it performs neither terminal nor graphical layout and does not define theway's DAG schema.

## Parsing pipeline

[`parse_mermaid`](../src/parser.rs) validates the Mermaid initialization directive and detects [`DiagramKind`](../src/ir.rs), then dispatches to the flowchart, sequence, class, state, ER, pie, mindmap, journey, timeline, Gantt, requirement, GitGraph, C4, Sankey, quadrant, ZenUML, block, packet, Kanban, architecture, radar, treemap, or XY chart parser.

[`ParseOutput`](../src/parser.rs) contains a [`Graph`](../src/ir.rs) and optional initialization JSON. `Graph` stores nodes in a `BTreeMap`, edges and subgraphs in ordered vectors, and also retains direction, type, and diagram-specific collections. Parsing failures use `anyhow::Result`; [`ParseError`](../src/error.rs) provides a typed error vocabulary for stricter callers.

## DAG adapter boundary

[`theway-core/src/multiagent/graph/mermaid.rs`](../../theway-core/src/multiagent/graph/mermaid.rs) owns the `dag_plan` flowchart subset. Its preprocessor classifies source lines, normalizes hyphenated identifiers for the vendored parser, and collects line-numbered diagnostics; its postprocessor restores identifiers, splits `agent: task` labels, derives dependencies, and rejects parser output that differs from the declared node set.

This crate does not absorb DAG-specific labels, identifier rewriting, dependency rules, or user-correction policy. The adapter remains separate so the vendored parser can continue serving the broader Mermaid intermediate representation.

## Source ownership

[`src/parser.rs`](../src/parser.rs), [`src/ir.rs`](../src/ir.rs), and [`src/error.rs`](../src/error.rs) are the vendored source group. [`src/lib.rs`](../src/lib.rs), [`Cargo.toml`](../Cargo.toml), the example, and the documentation belong to the local shell. `parser.rs` is exempt from the workspace file-size limit and stays monolithic for source comparison.
