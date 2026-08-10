# mermaid-rs-parser

Vendored **parse stage** of [`mermaid-rs-renderer`](https://github.com/1jehuang/mermaid-rs-renderer)
(mmd) 0.3.1: Mermaid source → structured IR [`Graph`](src/ir.rs), without the
layout engine / SVG renderer / font stack / CLI.

- Upstream license: MIT (see [LICENSE](LICENSE), copyright held by the
  mermaid-rs-renderer contributors)
- Vendoring: `src/parser.rs` + `src/ir.rs` + `src/error.rs` copied verbatim —
  the crate shell (`Cargo.toml` / `src/lib.rs`) is the only new code. Upstream
  clippy noise is `#![allow]`ed at the `parser.rs` module header to keep the
  vendored diff at zero.
- Verification: `cargo test` — all 98 upstream inline unit tests + 1 doctest
  pass unchanged.

Consumed by `theway-core`'s `graph_engineering::graph::parse_mermaid` (the
dag_plan mermaid subset): a preprocess line scan normalizes dag_plan hyphen
ids (`impl-api` → `impl_api`, mmdr treats `-` as edge syntax), the vendored
parser does the actual parsing, and a postprocess maps ids back, splits
`agent: task` labels, derives `depends_on`, and cross-checks the parsed node
set against the declared one so mmdr's silent tolerance of unknown lines /
stray commas surfaces as explicit line-numbered errors.

## Dependency footprint

```text
anyhow, json5, once_cell, regex, serde_json, thiserror   (6 in total)
```

The full mmdr renderer additionally pulls `clap / fontdb / resvg /
ttf-parser / usvg / serde / criterion`; the extracted parse side only has the
lightweight six. `Graph.nodes` is a `BTreeMap<String, Node>` (no serde, pure
std).

## dag_plan subset compatibility (verified, see examples/dag_plan_demo.rs)

| dag_plan syntax | Result |
|---|---|
| `graph TD\|LR` / `flowchart TD\|LR` | ✅ correct direction (TopDown/LeftRight; TB/RL/BT also parse) |
| `A["agent: task"]` | ✅ label lands in `Node.label`, shape=Rectangle |
| `A --> B` | ✅ `EdgeStyle::Solid` |
| `A -.-> B` | ✅ `EdgeStyle::Dotted` (distinguishable in the IR) |
| `A --> B & C` (standard mermaid multi-target) | ✅ 2 edges |
| `A --> B, C` (former dag_plan extension syntax) | ❌ non-standard; **removed from dag_plan** (pi-src cab2995) — now an explicit error in `theway`'s parser |
| `%%` comments (line / trailing) | ✅ |
| Semicolon-separated / chained `A --> B --> C` | ✅ |

### Comma syntax removal (2026-08 decision)

dag_plan used to declare `A --> B, C` as multi-target edges — not standard
mermaid. Removed in pi-src commit `cab2995`: the parser now uses standard `&`,
and stray-comma input is an **explicit error** (no more silent garbage). No
preprocessing is needed when consuming this lib — standard syntax only.
