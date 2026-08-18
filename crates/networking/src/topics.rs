//! Gossipsub topic identifiers for the consensus networking layer.
//!
//! Topic strings are the canonical identifiers libp2p hashes into a
//! `TopicHash` and that the deterministic message-id function
//! ([`crate::compute_gossipsub_message_id`]) folds into the SHA-256 input.
//! Centralising them here keeps `lean-p2p-host` free of protocol-level
//! constants — `lean-p2p-host::gossip::Topic` is a typed wrapper that
//! delegates to these values.
//!
//! # Composition
//!
//! Every topic is the four-component string the consensus networking spec
//! specifies:
//!
//! ```text
//! /{TOPIC_PREFIX}/{FORK_DIGEST}/{topic_name}/{ENCODING_POSTFIX}
//! ```
//!
//! The components are declared once, in the `lean_topics!` invocation
//! below, and the full strings are assembled from them at compile time.
//! They stay `const` because the gossip ingress router matches on them as
//! pattern arms.

/// Declares the gossip topic components and assembles the full topic
/// strings from them at compile time.
///
/// Each component literal is written exactly once, at the invocation.
/// `concat!` accepts the substituted literal tokens, which is what lets the
/// results stay `const` — and therefore usable as `match` pattern arms,
/// which a runtime-built `String` could not be.
///
/// Expands to: one constant per component, one `&'static str` per topic,
/// the `ALL_TOPICS` lookup table, and a const-evaluated assertion that
/// every assembled string starts with `/` (the libp2p `IdentTopic` /
/// `StreamProtocol` invariant — a violation fails the build, not the
/// test suite).
macro_rules! lean_topics {
    (
        prefix: $prefix:literal,
        digest: $digest:literal,
        encoding: $encoding:literal,
        topics: { $($konst:ident = $name:literal),+ $(,)? } $(,)?
    ) => {
        /// Network prefix identifying this consensus network in topic strings.
        ///
        /// leanSpec `networking/gossipsub/topic.py:79` (`TOPIC_PREFIX`).
        pub const TOPIC_PREFIX: &str = $prefix;

        /// Fork identifier bound into every gossip topic string.
        ///
        /// This is an interop-negotiated string, not a computed digest:
        /// leanSpec's reference node hardcodes the same value at
        /// `src/lean_spec/__main__.py:64` with the note that it "must match
        /// the fork string used by ream and other clients". Changing it
        /// stops gossip crossing silently — no error, no log — so it is a
        /// contract value recorded in the README interop table.
        ///
        /// Not to be confused with the 4-byte ENR `eth2` fork digest
        /// (leanSpec `networking/enr/eth2.py:35`), which is a different
        /// encoding of the same concept and is owned by the ENR work.
        pub const FORK_DIGEST: &str = $digest;

        /// Encoding suffix — SSZ payloads with Snappy compression.
        ///
        /// leanSpec `networking/gossipsub/topic.py:86` (`ENCODING_POSTFIX`).
        pub const ENCODING_POSTFIX: &str = $encoding;

        $(
            #[doc = concat!("Full gossipsub topic string for `", $name, "`.")]
            #[doc = ""]
            #[doc = concat!(
                "Assembled as `/", $prefix, "/", $digest, "/", $name, "/", $encoding, "`."
            )]
            pub const $konst: &str =
                concat!("/", $prefix, "/", $digest, "/", $name, "/", $encoding);
        )+

        /// Every topic this crate defines, as `(topic_name, full_string)`.
        ///
        /// The parser resolves an inbound `topic_name` component against
        /// this table; tests use it to assert composition without restating
        /// any literal.
        pub const ALL_TOPICS: &[(&str, &str)] = &[$(($name, $konst)),+];

        // Compile-time enforcement of the libp2p `StreamProtocol` /
        // `IdentTopic` invariant: topic strings must start with `/`.
        // Stated once, applied to every topic by construction.
        const _: () = {
            $( assert!($konst.as_bytes()[0] == b'/'); )+
        };
    };
}

lean_topics! {
    prefix: "leanconsensus",
    digest: "devnet0",
    encoding: "ssz_snappy",
    topics: {
        BLOCK_TOPIC_V1 = "block",
        VOTE_TOPIC_V1 = "vote",
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The components match the spec's own constants. This is the only
    /// place a spec literal is restated, and it is restated against the
    /// component — never against an assembled string.
    ///
    /// All FOUR components are pinned. Pinning three of them leaves the
    /// topic name free: `ALL_TOPICS` feeds the composition test the same
    /// literal it composes from, so a `"block"` -> `"blocks"` slip would
    /// otherwise pass every test in this crate.
    #[test]
    fn components_match_spec_constants() {
        // leanSpec networking/gossipsub/topic.py:79, :86.
        assert_eq!(TOPIC_PREFIX, "leanconsensus");
        assert_eq!(ENCODING_POSTFIX, "ssz_snappy");
        // leanSpec src/lean_spec/__main__.py:64.
        assert_eq!(FORK_DIGEST, "devnet0");
        // leanSpec networking/gossipsub/topic.py:93 (BLOCK_TOPIC_NAME).
        // Singular. The spec's prose docs say `blocks`
        // (docs/client/networking.md:71); the code is normative.
        assert_eq!(ALL_TOPICS[0].0, "block");
    }

    /// Every topic is the four-component form, composed from the
    /// components rather than copied. A test that restated the full string
    /// could only fail if someone edited one line and not the other.
    #[test]
    fn topics_are_composed_from_components() {
        for (name, full) in ALL_TOPICS {
            assert_eq!(
                *full,
                format!("/{TOPIC_PREFIX}/{FORK_DIGEST}/{name}/{ENCODING_POSTFIX}"),
                "topic {name}: not the four-component form",
            );
        }
    }

    #[test]
    fn topic_strings_are_distinct() {
        for (i, (_, a)) in ALL_TOPICS.iter().enumerate() {
            for (_, b) in &ALL_TOPICS[i + 1..] {
                assert_ne!(a, b, "duplicate topic string");
            }
        }
    }
}
