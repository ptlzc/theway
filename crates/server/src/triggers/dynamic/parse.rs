//! Natural-language trigger rule parsing: splits a `when/if <condition>, run <action>`
//! spec (English or Chinese markers) into condition + action text.

pub(super) const ZH_WHEN_PREFIX: &str = "\u{5f53}";
pub(super) const ZH_IF_PREFIX: &str = "\u{5982}\u{679c}";
pub(super) const ZH_TIME_SUFFIX_LONG: &str = "\u{7684}\u{65f6}\u{5019}";
pub(super) const ZH_TIME_SUFFIX_SHORT: char = '\u{65f6}';
pub(super) const ZH_EXECUTE_PREFIX: &str = "\u{6267}\u{884c}";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedTriggerRule {
    pub condition: String,
    pub action: String,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseTriggerRuleError {
    #[error("usage: /new-trigger <when condition, run action>")]
    Empty,
    #[error(
        "could not split the trigger into a condition and action. In normal chat, ask theway to create the trigger so the model can extract them, or use `/new-trigger if condition, then action`."
    )]
    MissingAction,
    #[error("condition and action must both be non-empty")]
    EmptyPart,
}

pub fn parse_trigger_rule(spec: &str) -> Result<ParsedTriggerRule, ParseTriggerRuleError> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(ParseTriggerRuleError::Empty);
    }

    let markers: &[&str] = &[
        "\u{7684}\u{65f6}\u{5019}\u{ff0c}\u{6267}\u{884c}",
        "\u{7684}\u{65f6}\u{5019},\u{6267}\u{884c}",
        "\u{7684}\u{65f6}\u{5019} \u{6267}\u{884c}",
        "\u{7684}\u{65f6}\u{5019}\u{6267}\u{884c}",
        "\u{7684}\u{65f6}\u{5019}\u{ff0c}",
        "\u{7684}\u{65f6}\u{5019},",
        "\u{65f6}\u{ff0c}\u{6267}\u{884c}",
        "\u{65f6},\u{6267}\u{884c}",
        "\u{65f6} \u{6267}\u{884c}",
        "\u{65f6}\u{6267}\u{884c}",
        "\u{65f6}\u{ff0c}",
        "\u{65f6},",
        "\u{ff0c}\u{5219}",
        ", \u{5219}",
        ",\u{5219}",
        " \u{5219} ",
        "\u{5219}",
        "\u{ff0c}\u{5c31}",
        ", \u{5c31}",
        ",\u{5c31}",
        " \u{5c31} ",
        "\u{ff0c}\u{6267}\u{884c}",
        ", \u{6267}\u{884c}",
        ",\u{6267}\u{884c}",
        " \u{6267}\u{884c} ",
        " then ",
        " then run ",
        " then execute ",
        ", run ",
        ", execute ",
        ", do ",
        " run ",
        " execute ",
    ];

    let lower = spec.to_lowercase();
    let mut split: Option<(usize, &str)> = None;
    for &marker in markers {
        let haystack = if marker.is_ascii() {
            lower.as_str()
        } else {
            spec
        };
        if let Some(idx) = haystack.find(marker) {
            split = Some((idx, marker));
            break;
        }
    }

    let Some((idx, marker)) = split else {
        return Err(ParseTriggerRuleError::MissingAction);
    };

    let raw_condition = spec[..idx].trim();
    let raw_action = spec[idx + marker.len()..].trim();
    let condition = clean_condition(raw_condition);
    let action = clean_action(raw_action);
    if condition.is_empty() || action.is_empty() {
        return Err(ParseTriggerRuleError::EmptyPart);
    }

    Ok(ParsedTriggerRule { condition, action })
}

fn clean_condition(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some(rest) = s.strip_prefix(ZH_WHEN_PREFIX) {
        s = rest.trim();
    }
    if let Some(rest) = s.strip_prefix(ZH_IF_PREFIX) {
        s = rest.trim();
    }
    let lower = s.to_lowercase();
    if lower.starts_with("when ") {
        s = s[5..].trim();
    } else if lower.starts_with("if ") {
        s = s[3..].trim();
    }
    s.trim_end_matches(ZH_TIME_SUFFIX_LONG)
        .trim_end_matches(ZH_TIME_SUFFIX_SHORT)
        .trim()
        .to_string()
}

fn clean_action(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some(rest) = s.strip_prefix(ZH_EXECUTE_PREFIX) {
        s = rest.trim();
    }
    let lower = s.to_lowercase();
    if lower.starts_with("run ") {
        s = s[4..].trim();
    } else if lower.starts_with("execute ") {
        s = s[8..].trim();
    }
    s.to_string()
}
