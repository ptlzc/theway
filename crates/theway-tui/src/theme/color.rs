/// `${THEWAY_DIR:-$HOME/.theway}/theme.toml`, matching the runtime-state
/// layout documented in AGENTS.md.
fn theme_toml_path() -> PathBuf {
    theway_transport::config::base_dir().join("theme.toml")
}

fn warn(msg: &str) {
    eprintln!("theway theme: {msg}");
}

// ── palette ────────────────────────────────────────────────────────────────

/// Collect the raw `[palette]` table: name → literal string.
fn raw_palette(table: &TomlTable) -> BTreeMap<String, String> {
    let mut raw = BTreeMap::new();
    let Some(toml::Value::Table(palette)) = table.get("palette") else {
        return raw;
    };
    for (name, value) in palette {
        match value.as_str() {
            Some(text) => {
                raw.insert(name.clone(), text.to_string());
            }
            None => warn(&format!(
                "palette.{name}: expected a string color, got {value:?}"
            )),
        }
    }
    raw
}

/// Resolve every palette entry to a concrete color. Entries may reference
/// other entries (`p:other`); cycles and unresolvable references warn once
/// and resolve to `None`.
fn build_palette(table: &TomlTable) -> BTreeMap<String, Option<Color>> {
    let raw = raw_palette(table);
    let mut resolved: BTreeMap<String, Option<Color>> = BTreeMap::new();
    let mut stack: Vec<&str> = Vec::new();
    for name in raw.keys() {
        resolve_palette_entry(name, &raw, &mut resolved, &mut stack);
    }
    resolved
}

fn resolve_palette_entry<'a>(
    name: &'a str,
    raw: &'a BTreeMap<String, String>,
    resolved: &mut BTreeMap<String, Option<Color>>,
    stack: &mut Vec<&'a str>,
) -> Option<Color> {
    if let Some(color) = resolved.get(name) {
        return *color;
    }
    if stack.contains(&name) {
        let cycle = stack.join(" -> ");
        warn(&format!(
            "palette.{name}: reference cycle detected ({cycle}) — ignoring"
        ));
        resolved.insert(name.to_string(), None);
        return None;
    }
    stack.push(name);
    let Some(value) = raw.get(name) else {
        resolved.insert(name.to_string(), None);
        return None;
    };
    let color = match value.strip_prefix("p:") {
        Some(referenced) => resolve_palette_entry(referenced, raw, resolved, stack),
        None => parse_literal_color(value),
    };
    stack.pop();
    resolved.insert(name.to_string(), color);
    color
}

// ── color literals ─────────────────────────────────────────────────────────

/// `#RRGGBB` / `#RGB` / ANSI names / 256-palette index → [`Color`]; anything
/// else → `None` for the caller to warn on.
fn parse_literal_color(value: &str) -> Option<Color> {
    if let Some(hex) = value.strip_prefix('#') {
        let digits: Vec<char> = hex.chars().collect();
        let expanded: String = match digits.len() {
            6 => hex.to_string(),
            3 => digits.iter().flat_map(|c| [*c, *c]).collect(),
            _ => return None,
        };
        if expanded.len() != 6 || !expanded.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        return u32::from_str_radix(&expanded, 16).ok().map(Color::from_u32);
    }
    match value.to_ascii_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "gray" | "darkgray" => Some(Color::DarkGray),
        "lightred" => Some(Color::LightRed),
        "lightgreen" => Some(Color::LightGreen),
        "lightyellow" => Some(Color::LightYellow),
        "lightblue" => Some(Color::LightBlue),
        "lightmagenta" => Some(Color::LightMagenta),
        "lightcyan" => Some(Color::LightCyan),
        "default" => Some(Color::Reset),
        _ => value.parse::<u8>().ok().map(Color::Indexed),
    }
}

/// Resolve a slot value: `p:name` looks up the (already resolved) palette;
/// anything else parses as a literal.
fn resolve_slot_color(value: &str, palette: &BTreeMap<String, Option<Color>>) -> Option<Color> {
    if let Some(name) = value.strip_prefix("p:") {
        return palette.get(name).copied().flatten();
    }
    parse_literal_color(value)
}

fn set_color(slot: &mut Color, key: &str, value: &str, palette: &BTreeMap<String, Option<Color>>) {
    match resolve_slot_color(value, palette) {
        Some(color) => *slot = color,
        None => warn(&format!(
            "{key}: invalid color {value:?} — keeping the current value"
        )),
    }
}

/// Optional slots additionally accept `transparent` / `none` to clear the
/// color (no background).
fn set_opt_color(
    slot: &mut Option<Color>,
    key: &str,
    value: &str,
    palette: &BTreeMap<String, Option<Color>>,
) {
    if matches!(value, "transparent" | "none") {
        *slot = None;
        return;
    }
    match resolve_slot_color(value, palette) {
        Some(color) => *slot = Some(color),
        None => warn(&format!(
            "{key}: invalid color {value:?} — keeping the current value"
        )),
    }
}

/// Parse a block border weight literal (`none | thin | thick`).
fn parse_block_border(value: &str) -> Option<BlockBorder> {
    match value {
        "none" => Some(BlockBorder::None),
        "thin" => Some(BlockBorder::Thin),
        "thick" => Some(BlockBorder::Thick),
        _ => None,
    }
}

/// String value of a toml value, warning when it is not a string.
fn as_str<'a>(key: &str, value: &'a toml::Value) -> Option<&'a str> {
    match value.as_str() {
        Some(text) => Some(text),
        None => {
            warn(&format!("{key}: expected a string value, got {value:?}"));
            None
        }
    }
}

/// Non-negative integer value: accepts a toml integer or a numeric string.
fn as_u16(value: &toml::Value) -> Option<u16> {
    match value {
        toml::Value::Integer(i) => u16::try_from(*i).ok(),
        _ => value.as_str().and_then(|s| s.parse().ok()),
    }
}
