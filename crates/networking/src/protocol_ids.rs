//! libp2p protocol-ID constants.
//!
//! Format follows the consensus networking spec:
//! `/leanconsensus/req/<name>/<version>/<encoding>`. The exact strings
//! below are part of the cross-client wire contract — every conformant
//! client advertises them verbatim at libp2p Identify. Modifying any
//! byte breaks interoperability.

use core::fmt;

/// Newtype wrapping a libp2p protocol identifier string.
///
/// Carrying the constants as a typed newtype (rather than `&'static str`)
/// catches "I passed the wrong string" at downstream libp2p call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProtocolId(&'static str);

impl ProtocolId {
    /// Returns the underlying canonical string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl AsRef<str> for ProtocolId {
    fn as_ref(&self) -> &str {
        self.0
    }
}

impl fmt::Display for ProtocolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl PartialEq<&str> for ProtocolId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

/// Protocol ID for the devnet0 status exchange.
pub const STATUS_PROTOCOL_V1: ProtocolId = ProtocolId("/leanconsensus/req/status/1/ssz_snappy");

/// Protocol ID for block recovery by root.
///
/// Resource name is `lean_blocks_by_root` (not `blocks_by_root`); the
/// encoding suffix `ssz_snappy` is mandatory.
pub const BLOCKS_BY_ROOT_PROTOCOL_V1: ProtocolId =
    ProtocolId("/leanconsensus/req/lean_blocks_by_root/1/ssz_snappy");

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn status_protocol_v1_matches_canonical_string() {
        assert_eq!(STATUS_PROTOCOL_V1, "/leanconsensus/req/status/1/ssz_snappy");
    }

    #[test]
    fn blocks_by_root_protocol_v1_matches_canonical_string() {
        assert_eq!(
            BLOCKS_BY_ROOT_PROTOCOL_V1,
            "/leanconsensus/req/lean_blocks_by_root/1/ssz_snappy",
        );
    }

    #[test]
    fn protocol_ids_are_distinct() {
        assert_ne!(STATUS_PROTOCOL_V1, BLOCKS_BY_ROOT_PROTOCOL_V1);
    }

    #[test]
    fn protocol_id_display_matches_as_str() {
        assert_eq!(STATUS_PROTOCOL_V1.to_string(), STATUS_PROTOCOL_V1.as_str(),);
    }

    #[test]
    fn protocol_id_as_ref_str() {
        let id: &str = STATUS_PROTOCOL_V1.as_ref();
        assert_eq!(id, STATUS_PROTOCOL_V1.as_str());
    }
}
