//! Transient MCP failure banner state (issue: MCP errors must surface): the
//! banner text plus its expiry, set from snapshots that carry per-server MCP
//! errors and rendered between the feed and the status bar.

/// Transient MCP failure banner (issue: MCP errors must surface): shown
/// for [`MCP_ERROR_BANNER_MS`] after the first snapshot that carries
/// per-server MCP errors, then cleared. The fingerprint dedupes repeated
/// snapshot frames so the banner only appears when the error set changes.
#[derive(Clone, Debug)]
pub(crate) struct McpErrorBanner {
    pub(crate) text: String,
    pub(crate) until: tokio::time::Instant,
}

/// How long the startup MCP-error banner stays visible.
pub(crate) const MCP_ERROR_BANNER_MS: u64 = 3_000;
