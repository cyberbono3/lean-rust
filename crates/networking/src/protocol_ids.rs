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

/// Protocol ID for the status handshake.
///
/// leanSpec `networking/reqresp/message.py:17`.
pub const STATUS_PROTOCOL_V1: ProtocolId = ProtocolId("/leanconsensus/req/status/1/ssz_snappy");

/// Protocol ID for block recovery by root.
///
/// leanSpec `networking/reqresp/message.py:43`. The resource name is
/// `blocks_by_root` with no client-specific prefix, and the `ssz_snappy`
/// encoding suffix is mandatory.
pub const BLOCKS_BY_ROOT_PROTOCOL_V1: ProtocolId =
    ProtocolId("/leanconsensus/req/blocks_by_root/1/ssz_snappy");

/// Every req/resp protocol this client serves.
///
/// Mirrors leanSpec `REQRESP_PROTOCOL_IDS`
/// (`networking/reqresp/handler.py:225-:231`) — exactly `Status` and
/// `BlocksByRoot`. Advertising anything else is a conformance break, which
/// is why this is a single list rather than two call sites that could
/// drift.
pub const REQRESP_PROTOCOLS: &[ProtocolId] = &[STATUS_PROTOCOL_V1, BLOCKS_BY_ROOT_PROTOCOL_V1];

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn protocol_ids_match_spec() {
        // leanSpec networking/reqresp/message.py:17 and :43.
        let cases = [
            (STATUS_PROTOCOL_V1, "/leanconsensus/req/status/1/ssz_snappy"),
            (
                BLOCKS_BY_ROOT_PROTOCOL_V1,
                "/leanconsensus/req/blocks_by_root/1/ssz_snappy",
            ),
        ];
        for (id, want) in cases {
            assert_eq!(id, want);
        }
    }

    /// leanSpec `networking/reqresp/handler.py:225-:231` serves exactly
    /// these two protocols. A third entry here is a conformance break.
    #[test]
    fn served_protocol_set_matches_spec() {
        assert_eq!(REQRESP_PROTOCOLS.len(), 2);
        assert!(REQRESP_PROTOCOLS.contains(&STATUS_PROTOCOL_V1));
        assert!(REQRESP_PROTOCOLS.contains(&BLOCKS_BY_ROOT_PROTOCOL_V1));
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
