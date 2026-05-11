//! Wire-protocol-local constants.

/// Maximum number of blocks in a single `BlocksByRoot` request or response.
///
/// Equal to `1 << 10 = 1024`. Enforced at construction time
/// ([`crate::BlocksByRootRequest::new`] /
/// [`crate::BlocksByRootResponse::new`]) and again at decode time by the
/// SSZ list helpers.
pub const MAX_REQUEST_BLOCKS: usize = 1 << 10;
