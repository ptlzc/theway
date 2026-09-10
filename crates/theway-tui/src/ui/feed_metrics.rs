//! Feed-derived counters behind the busy-band meters (issue #38): cumulative
//! text bytes and estimated tokens across the wire feed blocks, sampled once
//! per spinner tick.

/// Cumulative text bytes across the feed blocks — the monotonic counter the
/// busy-band char/s meter samples each spinner tick (issue #38).
pub(super) fn feed_text_bytes(blocks: &[theway_transport::feed::WireFeedBlock]) -> usize {
    use theway_transport::feed::WireFeedBlock as Block;
    blocks
        .iter()
        .map(|block| match block {
            Block::User { text, .. }
            | Block::Assistant { text, .. }
            | Block::Thinking { text, .. }
            | Block::Plain { text, .. } => text.len(),
            Block::ToolCall { name, args, .. } => name.len() + args.len(),
            Block::Error { message, .. } => message.len(),
            Block::ToolResult { lines, .. } => lines.iter().map(String::len).sum(),
        })
        .sum()
}

/// Rough token estimate for a text slice (~4 chars per token for ASCII, ~1
/// token per character for non-ASCII), matching `theway-core`'s estimator
/// so the busy band's `tps` figure tracks real token accounting.
fn estimate_token_chars(text: &str) -> usize {
    let mut ascii = 0usize;
    let mut non_ascii = 0usize;
    for c in text.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            non_ascii += 1;
        }
    }
    ascii.div_ceil(4) + non_ascii
}

/// Cumulative *estimated token* count across the feed blocks — the counter
/// the busy-band token-per-second meter samples each spinner tick.
pub(super) fn feed_text_tokens(blocks: &[theway_transport::feed::WireFeedBlock]) -> usize {
    use theway_transport::feed::WireFeedBlock as Block;
    blocks
        .iter()
        .map(|block| match block {
            Block::User { text, .. }
            | Block::Assistant { text, .. }
            | Block::Thinking { text, .. }
            | Block::Plain { text, .. } => estimate_token_chars(text),
            Block::ToolCall { name, args, .. } => {
                estimate_token_chars(name) + estimate_token_chars(args)
            }
            Block::Error { message, .. } => estimate_token_chars(message),
            Block::ToolResult { lines, .. } => lines.iter().map(|l| estimate_token_chars(l)).sum(),
        })
        .sum()
}
