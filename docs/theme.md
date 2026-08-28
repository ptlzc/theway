# Theme Interface (v2 Design)

[中文](theme.zh.md)

This document designs the **v2 theme interface** for theway's TUI — the
successor to the v1 `theme.toml` subset (issues #43 + #49). The v1 surface is
a narrow hand-parsed TOML slice: color roles in `[colors]`, block layout
(`bg` / `padding` / `align`) in `[blocks.<kind>]`, and six composer colors.
It has no way to control vertical rhythm (the inter-block gap is hardcoded to
one blank line), no named colors, no component coverage beyond the composer,
and no theme variants.

The design borrows from two reference ecosystems:

| Idea | Source | What we take |
| --- | --- | --- |
| Named color palette + `p:name` references | Oh My Posh (`palette`) | One place for colors; semantic roles reference palette entries |
| Color literals beyond hex (`transparent`, ANSI names, `none`) | Oh My Posh | Optional colors can be cleared; theme files stay readable |
| Conditional palettes (light/dark) | Oh My Posh (`palettes.template`) | `follow_system` + dark/light variants, driven by terminal OSC11 |
| Theme variants + `auto` | Grok TUI (`ThemeKind` + `Auto`) | Built-in named themes with a runtime picker; "auto" resolves to dark/light |
| Per-component segment styling | Oh My Posh (segments) | Every chrome component (composer, status band, pickers, sidebar, DAG band) gets a small, flat style table |
| Quantize colors to terminal depth | Grok TUI (`color_support`) | Truecolor → 256 → 16 fallback so themes survive weak terminals |

pi's own theme file is proprietary; its *shape* (TOML, semantic roles) is
already what v1 does, so the plan keeps TOML and extends the v1 sections
rather than inventing a new format.

## Goals and invariants

1. **Backward compatible.** Every v1 file parses unchanged and renders
   identically.
2. **Default = today.** With no theme file, the render must be pixel-identical
   to the current hardcoded tokyonight constants.
3. **One source of truth.** All colors and layout numbers flow from the
   resolved `Theme`; hardcoded consts shrink to the default theme's values.
4. **Progressive.** The v2 sections land in phases; each phase is optional and
   independently useful (the first one delivers the feed gap).
5. **Forgiving.** Unknown sections/keys warn on stderr and keep the previous
   value — same posture as v1.

## File format and loading

Keep TOML at `~/.theway/theme.toml` (the runtime-state layout in AGENTS.md).
v2 upgrades parsing from the hand-rolled subset to the workspace `toml` crate,
then maps onto the same `Theme` struct.

```toml
# ~/.theway/theme.toml — user theme (v2)
theme = "groknight"          # optional: built-in variant to layer on
follow_system = false        # optional: resolve dark/light via terminal OSC11
```

### Precedence

Project overrides user, user overrides the built-in variant, variant overrides
the default theme:

```
<built-in variant>   (e.g. "groknight", "tokyonight")
  └─ ~/.theway/theme.toml        (user)
       └─ <cwd>/.theway/theme.toml  (project, optional)
```

Project files are picked up through the existing per-cwd resource discovery;
a missing project file is not an error. Follows the same merge rule as the
settings payload: each present key replaces, absent keys keep.

## Color system

### 1. Semantic roles (v1, kept)

`[colors]` stays the anchor set. Every hardcoded color in the renderers
(`feed_render`, `prompt_chrome`, and the new components) becomes a role.

### 2. Named palette (new)

```toml
[palette]
accent    = "#7AA2F7"
muted     = "#565F89"
danger    = "#F7768E"
surface   = "#24283B"
```

Any color slot accepts a reference: `p:accent`. Palette entries may reference
other entries (one level, no cycles — warn on cycles). A missing palette key
warns and falls back to the slot's default.

### 3. Color literals (new)

One `parse_color` path for every slot, accepting:

| Literal | Example | Meaning |
| --- | --- | --- |
| Hex | `"#7AA2F7"` | Truecolor (v1 format) |
| Short hex | `"#7AF"` | Expanded to `#77AAFF` |
| ANSI name | `"red"`, `"lightBlue"`, `"default"` | Terminal palette reference |
| 256 index | `"146"` | 256-color palette index |
| `"transparent"` | — | No color (clears `Option<Color>` slots) |
| `"none"` | — | Alias for `transparent` in optional slots |
| `darken(#RRGGBB, 20)` / `lighten(#RRGGBB, 20)` | — | HCL lightness shift (v2 phase 3) |

### 4. Conditional variants (v2 phase 3)

```toml
[theme.dark]   # used when follow_system = true and terminal is dark
[palette.dark]
accent = "#7AA2F7"
[theme.light]
[palette.light]
accent = "#34548A"
```

`follow_system = true` resolves via terminal background query (OSC11); when the
terminal does not answer, the last explicit `theme` wins. `theme = "auto"` is
an alias for `follow_system = true`.

## Screen viewport

`[screen]` insets the **entire** UI from the terminal edges — feed, status
band, input box, side panel, pickers and overlays all render inside the
margin, so the layout never hugs the terminal border:

```toml
[screen]
margin = 2             # uniform inset on all four sides (default 0)
margin_left = 3        # per-side overrides; e.g. extra left breathing room
margin_top = 0
```

- `margin = N` sets all four sides; `margin_top` / `margin_right` /
  `margin_bottom` / `margin_left` override individual sides (applied after
  the uniform value).
- Margins larger than the terminal collapse the viewport to zero instead of
  underflowing (saturating).
- The default is all-zero (flush), so existing themes and the no-theme
  rendering are byte-identical to before.

## Feed layout (phase 1 — delivers the feed gap)

The vertical rhythm the v1 interface cannot express. `should_separate` keeps
deciding *where* a gap goes; the theme decides *how much*.

```toml
[feed]
gap = 1                # blank lines between blocks (default 1, 0 = flush)
separator = ""         # optional line glyph between blocks, e.g. "─"
separator_style = "p:muted"
```

- `gap = 0` disables inter-block blank lines entirely (single-turn feeds get
  denser).
- `separator` renders a full-width styled line **below** the gap rows when
  non-empty (so the total spacing is `gap` blank lines + one separator row;
  with `gap = 0` the separator alone separates blocks). Empty string /
  absent = pure blank lines.
- Both flow through the feed render cache as part of `FeedRenderOptions`'s
  theme fingerprint, so changing them invalidates the cache naturally.

## Block layout (phase 2)

`[blocks.<kind>]` gains vertical controls beside the v1 `bg` / `padding` /
`align`:

```toml
[blocks.tool]
bg = "p:surface"
padding = 1
align = "left"
margin_top = 0         # extra blank lines above this block kind (default 0)
margin_bottom = 0      # extra blank lines below (default 0)
border_top = "none"    # "none" | "thin" | "thick" — styled line above the block
border_bottom = "none"
border_style = "p:muted"
```

- `margin_top` / `margin_bottom` add to (never subtract from) `[feed] gap` —
  per-kind emphasis like separating every tool call.
- Borders render inside the block's background band so they do not disturb
  the feed rhythm.
- v1 files (bg/padding/align only) keep today's exact output.

## Component coverage (phase 2)

Every remaining hardcoded color moves into the theme as a flat style table.
The naming pattern is `<component>_<part>`; each value is a color literal or
palette reference.

```toml
[composer]             # v1 keys kept; additions marked (+)
border_focused = "#4B5C8C"
border_unfocused = "#3C4B78"
prefix = "p:accent"
text = "#C0CAF5"
bg = "#24283B"
info_text = "#A9B1D6"
placeholder = "#565F89"      # (+)
hint = "#565F89"             # (+)
cursor = "#C0CAF5"           # (+)

[statusbar]
bg = "#1F2335"
fg = "#A9B1D6"
accent = "p:accent"
error = "#F7768E"
busy = "#9ECE6A"

[picker]
bg = "#1F2335"
fg = "#7DCFFF"
highlight_bg = "#7DCFFF"     # selected row
highlight_fg = "#1A1B26"
title = "#E0AF68"
dim = "#565F89"

[sidebar]
bg = "#1F2335"
fg = "#A9B1D6"
heading = "#7AA2F7"
badge = "#9ECE6A"
muted = "#565F89"

[dag_band]
bg = "transparent"
fg = "#A9B1D6"
ok = "#9ECE6A"
failed = "#F7768E"
cancelled = "#E0AF68"
running = "#7DCFFF"
pending = "#565F89"
edge = "#3B4261"
title = "#C0CAF5"

[scrollbar]
thumb = "#3B4261"
track = "transparent"
```

The theme struct grows one `StyleTable` per component; the renderers read
exclusively from it. Unset keys fall back to the default theme's values, so a
theme that only sets `[feed] gap` stays fully functional.

## Built-in variants (phase 3)

Named variants ship in the binary (Grok-night by default, plus TokyoNight and
Grok-day at minimum). `theme = "<name>"` selects one; user/project files
override it field by field. A `/theme` command and a picker entry list
available variants, mirroring the model picker flow.

## Quantization (phase 3)

On terminals without truecolor, the resolved theme is quantized once at
startup (and on live changes): truecolor → nearest 256 → ANSI. Variants
declare whether they tolerate quantization (neutral-gray palettes do;
blue-tinted ones look muddy at 256 — same rule Grok uses).

## Migration

1. v1 parser stays until phase 2; v2 uses the `toml` crate and accepts every
   v1 key with identical semantics. The `Theme::parse` test suite (defaults
   match hardcoded consts, unknown keys warn, missing sections stay default)
   carries over unchanged.
2. New default values are the current hardcoded consts — nothing shifts.
3. A generated `theme.example.toml` documents every v2 key with comments
   (`/theme example` or the docs).

## Landing plan

| Phase | Scope | Delivers |
| --- | --- | --- |
| 1 | `toml`-crate parser + `[feed] gap`/`separator` + palette basics + `transparent`/`none` literals | User's ask: adjustable inter-block spacing; foundation |
| 2 | Block margins/borders + component tables (composer extras, statusbar, picker, sidebar, dag_band, scrollbar) | Full surface coverage; every hardcoded color role-ized |
| 3 | Built-in variants + `/theme` picker + `follow_system` dark/light + quantization + `darken`/`lighten` | Theme ecosystem parity with omp/grok |

Each phase keeps the invariant that a missing theme file renders exactly like
today. Phase 1 is small and self-contained — it can land immediately after
this design is agreed.
