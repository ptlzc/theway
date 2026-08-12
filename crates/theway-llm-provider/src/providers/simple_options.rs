//! Universal `SimpleStreamOptions` translation helpers. TODO: 1:1 port of
//! `packages/ai/src/providers/simple-options.ts`. Each provider translates the universal
//! reasoning level + thinking budget knobs into its own provider-specific shape.

use crate::types::{Model, SimpleStreamOptions, StreamOptions};

/// Translate a `SimpleStreamOptions` into the lowest-common-denominator `StreamOptions`. The
/// `reasoning` and `thinking_budgets` fields are dropped here; concrete providers add their own
/// translators to read them off the simple shape and inject the right provider extras.
pub fn translate_base(_model: &Model, options: &SimpleStreamOptions) -> StreamOptions {
    options.base.clone()
}
