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
//!
//! # The parser has no production caller yet
//!
//! [`GossipTopicRef::parse`] is deliberately ahead of its consumer. The
//! gossip ingress router still matches inbound topics against the constants
//! above rather than parsing them, which keeps that path and its failure
//! modes exactly as they were. Two consequences worth stating plainly: the
//! parser's hardening is exercised by its tests and not by live peer
//! traffic, and the spec's early wrong-fork rejection
//! ([`GossipTopicRef::parse_validated`], mirroring `from_string_validated`)
//! is implemented but NOT applied at ingress. Wiring it in is follow-up
//! work, not an oversight.

use core::fmt;

use crate::error::NetworkingError;

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
        /// This is a composition table for tests — it lets them assert the
        /// assembled strings without restating a literal. Inbound routing
        /// does NOT resolve against it: `parse_topic_name` uses an explicit
        /// `match`, for the reason given on that function.
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
        ATTESTATION_SUBNET_TOPIC_V1 = "attestation_0",
    },
}

/// Upper bound on an inbound topic string, in bytes.
///
/// Topic strings arrive from peers. The longest string this client can
/// legitimately see is a subnet topic with a 20-digit subnet id, well under
/// this cap; the bound exists so a peer cannot make the parser walk an
/// arbitrarily long buffer before rejecting it.
const MAX_TOPIC_LEN: usize = 128;

/// Prefix of an attestation-subnet topic name, before the subnet id.
///
/// leanSpec `networking/gossipsub/topic.py:100`
/// (`ATTESTATION_SUBNET_TOPIC_PREFIX`). Note this is a PREFIX, not a topic
/// name: the emitted name is `attestation_{subnet_id}`
/// (`topic.py:170`), and a bare `attestation` cannot be re-emitted at all.
pub const ATTESTATION_SUBNET_PREFIX: &str = "attestation";

/// Topic name carrying signed blocks.
///
/// leanSpec `networking/gossipsub/topic.py:93` (`BLOCK_TOPIC_NAME`).
/// Single-sourced: [`parse_topic_name`] and [`fmt::Display`] both read this
/// constant, so the parse and emit paths cannot drift apart the way
/// separate copies of the literal could. The macro invocation needs a
/// literal token and carries the third copy, which
/// `components_match_spec_constants` pins against this one.
pub const BLOCK_TOPIC_NAME: &str = "block";

/// Which message type a parsed topic carries.
///
/// The spec's third kind, `aggregation`
/// (`networking/gossipsub/topic.py:106`), is deliberately absent: this
/// client neither publishes nor subscribes to it, so an inbound
/// `aggregation` topic parses as [`NetworkingError::UnknownTopicName`]
/// until the subnet-topology work adds the variant alongside the topic
/// itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TopicKind {
    /// Signed blocks with their attestation payload.
    Block,
    /// Attestations for one attestation subnet.
    AttestationSubnet {
        /// Subnet this topic carries.
        ///
        /// Exactly one subnet exists at the pinned spec revision:
        /// `ATTESTATION_COMMITTEE_COUNT = 1` (`chain/config.py:33`) and
        /// `compute_subnet_id` is `validator_index % num_committees`
        /// (`containers/validator.py:40`), so every validator resolves to
        /// subnet 0. Parsing accepts any id; only the published constant is
        /// fixed at 0.
        subnet_id: u64,
    },
}

/// A parsed gossip topic, borrowing its fork digest from the input.
///
/// Construct with [`GossipTopicRef::parse`]. [`fmt::Display`] re-emits the
/// canonical four-component string, so `parse` then `to_string` is the
/// identity on every string this client accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GossipTopicRef<'a> {
    /// Message type carried by the topic.
    pub kind: TopicKind,
    /// Fork digest component as it appeared on the wire.
    ///
    /// UNTRUSTED and unvalidated by [`GossipTopicRef::parse`]: any non-empty
    /// `/`-free UTF-8 within the length cap reaches this field, control
    /// characters and terminal escapes included. [`fmt::Display`] re-emits
    /// it verbatim, so a caller that logs a parsed topic is logging peer
    /// input — use [`GossipTopicRef::parse_validated`] where the digest is
    /// supposed to be known.
    pub fork_digest: &'a str,
}

impl<'a> GossipTopicRef<'a> {
    /// Parses a full topic string into its components.
    ///
    /// Validates the shape (exactly one leading `/`, exactly four
    /// components, non-empty fork digest), the prefix and the encoding
    /// postfix, and resolves the topic name. An empty prefix or encoding
    /// fails its equality check rather than the shape check, and an empty
    /// topic name resolves to nothing — see `# Errors` for which variant
    /// each produces.
    ///
    /// The fork digest is NOT validated here — the spec splits the two as
    /// well (`topic.py:192-:232` parses, `topic.py:179-:190` validates the
    /// fork); use [`Self::parse_validated`] to do both.
    ///
    /// # Divergence from the spec parser
    ///
    /// leanSpec's `parse_topic_string` (`topic.py:294-:314`) is
    /// `topic_str.lstrip("/").split("/")` with a component-count check and
    /// nothing else. This parser is tighter in three ways:
    ///
    /// 1. It requires a leading `/`; the spec accepts a topic with none.
    /// 2. It requires exactly one; the spec's `lstrip` accepts any number.
    /// 3. It rejects an empty fork-digest component; the spec accepts one.
    /// 4. It rejects the bare names `attestation` and `aggregation`. The
    ///    spec's `from_string` resolves both (`topic.py:227-:232`), but
    ///    `to_topic_id` then raises on the first (`:167-:169`) and this
    ///    client does not route the second.
    ///
    /// None of the four can reject a conformant peer, because
    /// `to_topic_id` (`topic.py:161-:173`) only ever emits the
    /// single-leading-slash, non-empty form. See also the subnet-id
    /// canonicalisation on [`parse_subnet_id`], which is tighter for a
    /// stronger reason: without it, `parse` then [`fmt::Display`] would not
    /// round-trip.
    ///
    /// # Errors
    /// - [`NetworkingError::MalformedTopic`] — wrong component count,
    ///   missing leading `/`, empty fork-digest component, or over
    ///   [`MAX_TOPIC_LEN`].
    /// - [`NetworkingError::TopicComponentMismatch`] — wrong prefix or
    ///   wrong encoding postfix, empty ones included.
    /// - [`NetworkingError::UnknownTopicName`] — a topic name this client
    ///   does not route, including an empty one and a non-canonical subnet
    ///   id.
    pub fn parse(topic: &'a str) -> Result<Self, NetworkingError> {
        if topic.len() > MAX_TOPIC_LEN {
            return Err(NetworkingError::MalformedTopic {
                reason: "topic exceeds maximum length",
            });
        }
        let body = topic
            .strip_prefix('/')
            .ok_or(NetworkingError::MalformedTopic {
                reason: "topic must start with '/'",
            })?;

        let mut parts = body.split('/');
        let (Some(prefix), Some(fork_digest), Some(topic_name), Some(encoding), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(NetworkingError::MalformedTopic {
                reason: "topic must have exactly four components",
            });
        };

        if fork_digest.is_empty() {
            return Err(NetworkingError::MalformedTopic {
                reason: "fork digest component is empty",
            });
        }
        if prefix != TOPIC_PREFIX {
            return Err(NetworkingError::TopicComponentMismatch {
                component: "prefix",
                expected: TOPIC_PREFIX,
            });
        }
        if encoding != ENCODING_POSTFIX {
            return Err(NetworkingError::TopicComponentMismatch {
                component: "encoding",
                expected: ENCODING_POSTFIX,
            });
        }

        Ok(Self {
            kind: parse_topic_name(topic_name).ok_or(NetworkingError::UnknownTopicName)?,
            fork_digest,
        })
    }

    /// Parses a topic and rejects it unless its fork digest matches.
    ///
    /// Mirrors leanSpec `from_string_validated`
    /// (`networking/gossipsub/topic.py:234-:254`). A digest mismatch is
    /// reported as [`NetworkingError::TopicComponentMismatch`] on the
    /// `"fork digest"` component — a peer on another fork is not a
    /// malformed peer, and acting on that distinction (refusing to dial)
    /// belongs to the ENR work, not here.
    ///
    /// # Errors
    /// Every error from [`Self::parse`], plus
    /// [`NetworkingError::TopicComponentMismatch`] on a digest mismatch.
    pub fn parse_validated(
        topic: &'a str,
        expected_fork_digest: &'static str,
    ) -> Result<Self, NetworkingError> {
        let parsed = Self::parse(topic)?;
        if parsed.fork_digest != expected_fork_digest {
            return Err(NetworkingError::TopicComponentMismatch {
                component: "fork digest",
                expected: expected_fork_digest,
            });
        }
        Ok(parsed)
    }
}

impl fmt::Display for GossipTopicRef<'_> {
    /// Re-emits the canonical four-component string.
    ///
    /// Mirrors leanSpec `to_topic_id`
    /// (`networking/gossipsub/topic.py:161-:173`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{TOPIC_PREFIX}/{}/", self.fork_digest)?;
        match self.kind {
            TopicKind::Block => f.write_str(BLOCK_TOPIC_NAME)?,
            TopicKind::AttestationSubnet { subnet_id } => {
                write!(f, "{ATTESTATION_SUBNET_PREFIX}_{subnet_id}")?;
            }
        }
        write!(f, "/{ENCODING_POSTFIX}")
    }
}

/// Resolves a topic-name component to its [`TopicKind`].
///
/// Subnet topics are matched by prefix, as the spec does
/// (`networking/gossipsub/topic.py:213-:225`), before the plain-name
/// lookup. The prefix has one source — [`ATTESTATION_SUBNET_PREFIX`], the
/// same constant [`fmt::Display`] emits — so the parse path and the emit
/// path cannot drift.
///
/// The plain-name arm is an explicit `match`, NOT a lookup in
/// [`ALL_TOPICS`]: that table maps names to full strings, and resolving any
/// hit in it to a single kind would silently mis-route the moment a second
/// plain-named topic exists.
fn parse_topic_name(topic_name: &str) -> Option<TopicKind> {
    if let Some(raw) = topic_name
        .strip_prefix(ATTESTATION_SUBNET_PREFIX)
        .and_then(|rest| rest.strip_prefix('_'))
    {
        return parse_subnet_id(raw).map(|subnet_id| TopicKind::AttestationSubnet { subnet_id });
    }
    if topic_name == BLOCK_TOPIC_NAME {
        return Some(TopicKind::Block);
    }
    None
}

/// Parses a subnet id, accepting only the canonical decimal form.
///
/// Deliberately stricter than the spec, which parses the component with
/// Python's `int()` (`networking/gossipsub/topic.py:218`) and therefore
/// accepts `007`, `+7`, `1_0` and non-ASCII digits. Rust's own
/// `str::parse::<u64>` still accepts `007` and `+7`, and since the topic is
/// re-emitted canonically as `attestation_7`, a permissive parse would
/// break `parse` then [`fmt::Display`] round-tripping. Rejecting
/// non-canonical forms cannot reject a conformant peer: the spec only ever
/// EMITS the canonical form.
fn parse_subnet_id(raw: &str) -> Option<u64> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if raw.len() > 1 && raw.starts_with('0') {
        return None;
    }
    raw.parse::<u64>().ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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
    fn block_topic_round_trips() {
        let parsed = GossipTopicRef::parse(BLOCK_TOPIC_V1).expect("block topic parses");
        assert_eq!(parsed.kind, TopicKind::Block);
        assert_eq!(parsed.fork_digest, FORK_DIGEST);
        assert_eq!(parsed.to_string(), BLOCK_TOPIC_V1);
    }

    #[test]
    fn subnet_topic_round_trips() {
        let topic = format!("/{TOPIC_PREFIX}/{FORK_DIGEST}/attestation_0/{ENCODING_POSTFIX}");
        let parsed = GossipTopicRef::parse(&topic).expect("subnet topic parses");
        assert_eq!(parsed.kind, TopicKind::AttestationSubnet { subnet_id: 0 });
        assert_eq!(parsed.to_string(), topic);
    }

    /// The attestation topic is the spec's subnet form, not a bare name.
    /// leanSpec `networking/gossipsub/topic.py:167-:170` emits
    /// `attestation_{subnet_id}`; a bare `attestation` cannot be re-emitted,
    /// and no spec node subscribes to `vote`.
    ///
    /// This replaces `vote_topic_name_does_not_resolve`, which was the only
    /// test that could tell the explicit-`match` `parse_topic_name` apart
    /// from a blanket `ALL_TOPICS` lookup. That discrimination is no longer
    /// observable: `attestation_0` is intercepted by the subnet-prefix
    /// branch before any table lookup, and `block` maps to `Block` under
    /// both forms. The explicit `match` is now held by the doc comment on
    /// `parse_topic_name` and by review.
    #[test]
    fn attestation_topic_is_the_subnet_form() {
        let parsed =
            GossipTopicRef::parse(ATTESTATION_SUBNET_TOPIC_V1).expect("subnet topic parses");
        assert_eq!(parsed.kind, TopicKind::AttestationSubnet { subnet_id: 0 });
        assert_eq!(parsed.to_string(), ATTESTATION_SUBNET_TOPIC_V1);
    }

    #[test]
    fn parser_rejects_malformed_shape() {
        let cases = [
            ("leanconsensus/devnet0/block/ssz_snappy", "no leading slash"),
            ("/leanconsensus/devnet0/block", "three components"),
            (
                "/leanconsensus/devnet0/block/ssz_snappy/x",
                "five components",
            ),
            ("/leanconsensus//block/ssz_snappy", "empty fork digest"),
        ];
        for (input, case) in cases {
            assert!(
                matches!(
                    GossipTopicRef::parse(input),
                    Err(NetworkingError::MalformedTopic { .. })
                ),
                "case {case}: expected MalformedTopic",
            );
        }
    }

    #[test]
    fn parser_rejects_wrong_prefix_and_encoding() {
        let cases = [
            (
                format!("/ethconsensus/{FORK_DIGEST}/block/{ENCODING_POSTFIX}"),
                "prefix",
            ),
            (
                format!("/{TOPIC_PREFIX}/{FORK_DIGEST}/block/ssz"),
                "encoding",
            ),
        ];
        for (input, component) in cases {
            match GossipTopicRef::parse(&input) {
                Err(NetworkingError::TopicComponentMismatch { component: got, .. }) => {
                    assert_eq!(got, component);
                }
                other => panic!("case {component}: expected mismatch, got {other:?}"),
            }
        }
    }

    #[test]
    fn parser_rejects_unknown_topic_name() {
        // `aggregation` is a real spec topic this client does not route
        // (owned by the subnet-topology work); `nonsense` is not a topic at
        // all; a bare `attestation` cannot be re-emitted and so is not one
        // either.
        for name in ["aggregation", "nonsense", "attestation"] {
            let topic = format!("/{TOPIC_PREFIX}/{FORK_DIGEST}/{name}/{ENCODING_POSTFIX}");
            assert!(
                matches!(
                    GossipTopicRef::parse(&topic),
                    Err(NetworkingError::UnknownTopicName)
                ),
                "topic name {name} must not resolve",
            );
        }
    }

    /// The canonical-form guard: every accepted subnet component must
    /// re-emit identically, so non-canonical spellings are rejected rather
    /// than normalised. See the doc comment on `parse_subnet_id`.
    #[test]
    fn parser_rejects_non_canonical_subnet_ids() {
        for raw in [
            "007",
            "+7",
            "-1",
            "1_0",
            "\u{667}",
            "",
            " 7",
            "18446744073709551616",
        ] {
            let topic =
                format!("/{TOPIC_PREFIX}/{FORK_DIGEST}/attestation_{raw}/{ENCODING_POSTFIX}");
            assert!(
                matches!(
                    GossipTopicRef::parse(&topic),
                    Err(NetworkingError::UnknownTopicName)
                ),
                "subnet id {raw:?} must be rejected",
            );
        }
    }

    #[test]
    fn parser_rejects_oversized_topic() {
        let topic = format!(
            "/{TOPIC_PREFIX}/{}/block/{ENCODING_POSTFIX}",
            "d".repeat(MAX_TOPIC_LEN)
        );
        assert!(matches!(
            GossipTopicRef::parse(&topic),
            Err(NetworkingError::MalformedTopic { .. })
        ));
    }

    #[test]
    fn parse_validated_rejects_foreign_fork_digest() {
        let topic = format!("/{TOPIC_PREFIX}/devnet9/block/{ENCODING_POSTFIX}");
        match GossipTopicRef::parse_validated(&topic, FORK_DIGEST) {
            Err(NetworkingError::TopicComponentMismatch {
                component,
                expected,
            }) => {
                assert_eq!(component, "fork digest");
                assert_eq!(expected, FORK_DIGEST);
            }
            other => panic!("expected fork-digest mismatch, got {other:?}"),
        }
        assert!(
            GossipTopicRef::parse(&topic).is_ok(),
            "parse alone must not validate the fork",
        );
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
