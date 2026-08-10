//! # mermaid-rs-parser
//!
//! Extracted **parse stage** of [`mermaid-rs-renderer`](https://github.com/1jehuang/mermaid-rs-renderer)
//! (mmdr) 0.3.1 — Mermaid source text → structured IR [`Graph`], without the
//! layout engine, SVG renderer, font stack, or CLI.
//!
//! Files were copied verbatim from the upstream crate (`src/parser.rs`,
//! `src/ir.rs`, `src/error.rs`); only the crate shell (`Cargo.toml`, `lib.rs`)
//! is new. MIT licensed, upstream attribution in `LICENSE`.
//!
//! ## Usage
//!
//! ```rust
//! use mermaid_rs_parser::{parse_mermaid, Direction, NodeShape};
//!
//! let parsed = parse_mermaid(r#"
//!     graph TD
//!         A["explorer: 调研"] --> B["planner: 计划"]
//!         B -.-> C["checker: 验证"]
//! "#).unwrap();
//!
//! let g = &parsed.graph;
//! assert_eq!(g.direction, Direction::TopDown);
//! assert_eq!(g.nodes.get("A").unwrap().label, "explorer: 调研");
//! assert_eq!(g.nodes.get("A").unwrap().shape, NodeShape::Rectangle);
//! assert_eq!(g.edges.len(), 2);
//! ```
//!
//! ## Supported diagram kinds
//!
//! Flowchart (`graph`/`flowchart`), sequence, class, state, ER, pie, mindmap,
//! journey, timeline, gantt, requirement, gitGraph, C4, sankey, quadrant,
//! zenUML, block, packet, kanban, architecture, radar, treemap, xychart —
//! everything the upstream `parser.rs` handles.

pub mod error;
pub mod ir;
pub mod parser;

// Re-export commonly used types at crate root for ergonomic library usage
// (mirrors upstream lib.rs's export surface for the parse stage).
pub use error::ParseError;
pub use ir::{
    DiagramKind, Direction, Edge, EdgeArrowhead, EdgeDecoration, EdgeStyle, Graph, Node, NodeLink,
    NodeShape, SequenceActivation, SequenceActivationKind, SequenceBox, StateNote,
    StateNotePosition, Subgraph,
};
pub use parser::{ParseOutput, parse_mermaid};
