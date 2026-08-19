//! Shared session-id matching semantics.

/// Result of resolving an exact id or a unique, non-empty prefix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrefixMatch<'a> {
    None,
    Unique(&'a str),
    Ambiguous,
}

/// Resolve `query` against session ids.
///
/// Exact matches always win. Otherwise a non-empty prefix must match exactly
/// one id; empty and ambiguous prefixes do not resolve.
pub fn resolve_unique_prefix<'a>(
    ids: impl IntoIterator<Item = &'a str>,
    query: &str,
) -> PrefixMatch<'a> {
    if query.is_empty() {
        return PrefixMatch::None;
    }

    let mut prefix = None;
    let mut ambiguous = false;
    for id in ids {
        if id == query {
            return PrefixMatch::Unique(id);
        }
        if id.starts_with(query) {
            if prefix.is_some() {
                ambiguous = true;
            } else {
                prefix = Some(id);
            }
        }
    }

    match (prefix, ambiguous) {
        (Some(_), true) => PrefixMatch::Ambiguous,
        (Some(id), false) => PrefixMatch::Unique(id),
        (None, _) => PrefixMatch::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_wins_over_other_prefixes() {
        let ids = ["abc", "abcdef"];
        assert_eq!(
            resolve_unique_prefix(ids, "abc"),
            PrefixMatch::Unique("abc")
        );
    }

    #[test]
    fn unique_prefix_resolves_but_empty_and_ambiguous_do_not() {
        let ids = ["abc-one", "abc-two", "def"];
        assert_eq!(
            resolve_unique_prefix(ids, "def"),
            PrefixMatch::Unique("def")
        );
        assert_eq!(resolve_unique_prefix(ids, "abc"), PrefixMatch::Ambiguous);
        assert_eq!(resolve_unique_prefix(ids, ""), PrefixMatch::None);
    }
}
