//! Typed req/resp wire messages with SSZ codec.
//!
//! Three payloads land here:
//!
//! - [`Status`] — fixed 80-byte container of `(finalized, head)` checkpoints.
//! - [`BlocksByRootRequest`] — bounded `List[Bytes32, MAX_REQUEST_BLOCKS]`.
//! - [`BlocksByRootResponse`] — bounded `List[SignedBlock, MAX_REQUEST_BLOCKS]`
//!   over variable-length elements.
//!
//! Length invariants are enforced at construction time
//! ("parse, don't validate"); subsequent encode calls are infallible.
//! Decode-from-wire goes through the SSZ list helpers, which cap the
//! element count before allocation — adversarial peers can't OOM the
//! receiver via a length-claim.

use protocol::{Checkpoint, SignedBlock};
use ssz::merkleize::hash_pair;
use ssz::{
    decode_bytes32_list, decode_variable_element_list, encode_bytes32_list,
    encode_variable_element_list, Decode, DecodeError, Encode, HashTreeRoot,
    BYTES_PER_LENGTH_OFFSET,
};
use types::Bytes32;

use crate::config::MAX_REQUEST_BLOCKS;
use crate::error::NetworkingError;

const CHECKPOINT_LEN: usize = 40;
const STATUS_SSZ_LEN: usize = 2 * CHECKPOINT_LEN;

/// Validates that `items.len() <= MAX_REQUEST_BLOCKS`, surfacing
/// [`NetworkingError::ListTooLarge`] otherwise.
fn enforce_list_cap<T>(items: Vec<T>, kind: &'static str) -> Result<Vec<T>, NetworkingError> {
    if items.len() > MAX_REQUEST_BLOCKS {
        return Err(NetworkingError::ListTooLarge {
            kind,
            len: items.len(),
            max: MAX_REQUEST_BLOCKS,
        });
    }
    Ok(items)
}

// =============================================================================
// Status
// =============================================================================

/// Request/response payload exchanged during the initial peer handshake.
///
/// SSZ-encoded as a fixed-size container: 40-byte `finalized` followed by
/// 40-byte `head`, total 80 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Status {
    /// Highest finalized checkpoint observed by the sender.
    pub finalized: Checkpoint,
    /// Current canonical head checkpoint observed by the sender.
    pub head: Checkpoint,
}

impl Encode for Status {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        STATUS_SSZ_LEN
    }

    fn ssz_bytes_len(&self) -> usize {
        STATUS_SSZ_LEN
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        self.finalized.ssz_append(buf);
        self.head.ssz_append(buf);
    }
}

impl Decode for Status {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        STATUS_SSZ_LEN
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != STATUS_SSZ_LEN {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: STATUS_SSZ_LEN,
            });
        }
        Ok(Self {
            finalized: Checkpoint::from_ssz_bytes(&bytes[..CHECKPOINT_LEN])?,
            head: Checkpoint::from_ssz_bytes(&bytes[CHECKPOINT_LEN..])?,
        })
    }
}

impl HashTreeRoot for Status {
    fn hash_tree_root(&self) -> [u8; 32] {
        hash_pair(
            &self.finalized.hash_tree_root(),
            &self.head.hash_tree_root(),
        )
    }
}

// =============================================================================
// BlocksByRootRequest
// =============================================================================

/// Bounded list of block roots requested from a peer.
///
/// Length is capped at [`MAX_REQUEST_BLOCKS`] by [`Self::new`] and by the
/// SSZ decode helper.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlocksByRootRequest {
    roots: Vec<Bytes32>,
}

impl BlocksByRootRequest {
    /// Constructs a validated request from any iterable of roots.
    ///
    /// # Errors
    /// [`NetworkingError::ListTooLarge`] when the iterable yields more
    /// than [`MAX_REQUEST_BLOCKS`] roots.
    pub fn new<I>(roots: I) -> Result<Self, NetworkingError>
    where
        I: IntoIterator<Item = Bytes32>,
    {
        let roots = enforce_list_cap(roots.into_iter().collect(), "blocks_by_root request")?;
        Ok(Self { roots })
    }

    /// Returns the underlying validated root slice.
    #[must_use]
    pub fn roots(&self) -> &[Bytes32] {
        &self.roots
    }

    /// Number of roots in the request.
    #[must_use]
    pub fn len(&self) -> usize {
        self.roots.len()
    }

    /// `true` when no roots are requested.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

impl Encode for BlocksByRootRequest {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_bytes_len(&self) -> usize {
        self.roots.len() * 32
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        encode_bytes32_list(&self.roots, buf);
    }
}

impl Decode for BlocksByRootRequest {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        Ok(Self {
            roots: decode_bytes32_list(bytes, MAX_REQUEST_BLOCKS)?,
        })
    }
}

// =============================================================================
// BlocksByRootResponse
// =============================================================================

/// Bounded list of signed blocks returned in response to a `BlocksByRoot`
/// request.
///
/// Length is capped at [`MAX_REQUEST_BLOCKS`] by [`Self::new`] and by the
/// SSZ decode helper. The element type (`SignedBlock`) is variable-length,
/// so the encoded form carries a 4-byte offset per element.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlocksByRootResponse {
    blocks: Vec<SignedBlock>,
}

impl BlocksByRootResponse {
    /// Constructs a validated response from any iterable of signed blocks.
    ///
    /// # Errors
    /// [`NetworkingError::ListTooLarge`] when the iterable yields more
    /// than [`MAX_REQUEST_BLOCKS`] blocks.
    pub fn new<I>(blocks: I) -> Result<Self, NetworkingError>
    where
        I: IntoIterator<Item = SignedBlock>,
    {
        let blocks = enforce_list_cap(blocks.into_iter().collect(), "blocks_by_root response")?;
        Ok(Self { blocks })
    }

    /// Returns the underlying validated block slice.
    #[must_use]
    pub fn blocks(&self) -> &[SignedBlock] {
        &self.blocks
    }

    /// Number of blocks in the response.
    #[must_use]
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// `true` when no blocks are returned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

impl Encode for BlocksByRootResponse {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_bytes_len(&self) -> usize {
        let offsets = self.blocks.len() * BYTES_PER_LENGTH_OFFSET;
        let payload: usize = self.blocks.iter().map(SignedBlock::ssz_bytes_len).sum();
        offsets + payload
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        encode_variable_element_list(&self.blocks, buf);
    }
}

impl Decode for BlocksByRootResponse {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        Ok(Self {
            blocks: decode_variable_element_list::<SignedBlock>(bytes, MAX_REQUEST_BLOCKS)?,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

// Fixtures here still build the deprecated `Bytes4000` placeholder. `expect`
// rather than `allow` so it retires itself when the fixture moves to
// `Signature`.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[expect(deprecated)]
mod tests {
    use super::*;
    use protocol::{Block, BlockBody, Slot, ValidatorIndex};
    use ssz::{decode, encode};
    use static_assertions::{assert_impl_all, const_assert_eq};
    use types::Bytes4000;

    // -- compile-time witnesses ---------------------------------------------

    const_assert_eq!(MAX_REQUEST_BLOCKS, 1024);
    const_assert_eq!(STATUS_SSZ_LEN, 80);
    assert_impl_all!(Status: Copy, Default, Send, Sync);
    assert_impl_all!(BlocksByRootRequest: Default, Send, Sync);
    assert_impl_all!(BlocksByRootResponse: Default, Send, Sync);

    #[test]
    fn status_ssz_fixed_len_runtime_check() {
        assert_eq!(<Status as Encode>::ssz_fixed_len(), STATUS_SSZ_LEN);
    }

    // -- helpers ------------------------------------------------------------

    fn sample_status() -> Status {
        Status {
            finalized: Checkpoint::new(Bytes32::new([0xaa; 32]), Slot::ZERO),
            head: Checkpoint::new(Bytes32::new([0xbb; 32]), Slot::new(7)),
        }
    }

    fn sample_signed_block(seed: u8) -> SignedBlock {
        SignedBlock {
            message: Block {
                slot: Slot::new(u64::from(seed)),
                proposer_index: ValidatorIndex::new(u64::from(seed)),
                parent_root: Bytes32::new([seed; 32]),
                state_root: Bytes32::new([seed.wrapping_add(1); 32]),
                body: BlockBody::default(),
            },
            signature: Bytes4000::new([seed; 4000]),
        }
    }

    fn assert_round_trips<T>(value: &T, expected_len: Option<usize>)
    where
        T: Encode + Decode + PartialEq + std::fmt::Debug,
    {
        let bytes = encode(value);
        if let Some(len) = expected_len {
            assert_eq!(bytes.len(), len, "encoded length mismatch");
        }
        assert_eq!(&decode::<T>(&bytes).unwrap(), value);
    }

    // -- Status -------------------------------------------------------------

    #[test]
    fn status_round_trips() {
        assert_round_trips(&sample_status(), Some(80));
        assert_round_trips(&Status::default(), Some(80));
    }

    #[test]
    fn status_rejects_wrong_length() {
        let err = <Status as Decode>::from_ssz_bytes(&[0_u8; 79]).unwrap_err();
        assert!(matches!(
            err,
            DecodeError::InvalidByteLength {
                len: 79,
                expected: 80
            }
        ));
    }

    // -- BlocksByRootRequest ------------------------------------------------

    #[test]
    fn request_construction_respects_cap() {
        // under, at, and over the cap.
        let under = BlocksByRootRequest::new([Bytes32::new([1; 32]); 5]).unwrap();
        assert_eq!(under.len(), 5);
        assert!(!under.is_empty());

        let at_cap =
            BlocksByRootRequest::new(std::iter::repeat_n(Bytes32::zero(), MAX_REQUEST_BLOCKS))
                .unwrap();
        assert_eq!(at_cap.len(), MAX_REQUEST_BLOCKS);

        let err =
            BlocksByRootRequest::new(std::iter::repeat_n(Bytes32::zero(), MAX_REQUEST_BLOCKS + 1))
                .unwrap_err();
        assert!(matches!(
            err,
            NetworkingError::ListTooLarge { kind, len, max }
                if kind == "blocks_by_root request"
                    && len == MAX_REQUEST_BLOCKS + 1
                    && max == MAX_REQUEST_BLOCKS
        ));
    }

    #[test]
    fn request_round_trips() {
        assert_round_trips(
            &BlocksByRootRequest::new([Bytes32::new([0xab; 32]); 3]).unwrap(),
            Some(96),
        );
        assert_round_trips(&BlocksByRootRequest::default(), Some(0));
    }

    #[test]
    fn request_accepts_vec_array_and_iter_chain() {
        // Witness for the `impl IntoIterator<Item = Bytes32>` flexibility.
        let _vec = BlocksByRootRequest::new(vec![Bytes32::zero()]).unwrap();
        let _array = BlocksByRootRequest::new([Bytes32::zero(); 2]).unwrap();
        let _iter = BlocksByRootRequest::new((0_u8..3).map(|i| Bytes32::new([i; 32]))).unwrap();
    }

    #[test]
    fn request_decode_rejects_over_cap_at_wire_boundary() {
        // Build bytes that would decode to MAX + 1 roots.
        let bytes = vec![0_u8; (MAX_REQUEST_BLOCKS + 1) * 32];
        let err = <BlocksByRootRequest as Decode>::from_ssz_bytes(&bytes).unwrap_err();
        assert!(matches!(err, DecodeError::BytesInvalid(_)));
    }

    // -- BlocksByRootResponse -----------------------------------------------

    #[test]
    fn response_round_trips() {
        assert_round_trips(
            &BlocksByRootResponse::new([sample_signed_block(1)]).unwrap(),
            None,
        );
        assert_round_trips(
            &BlocksByRootResponse::new((1_u8..=3).map(sample_signed_block)).unwrap(),
            None,
        );
        assert_round_trips(&BlocksByRootResponse::default(), Some(0));
    }

    #[test]
    fn response_rejects_over_cap() {
        let blocks = std::iter::repeat_with(SignedBlock::default).take(MAX_REQUEST_BLOCKS + 1);
        let err = BlocksByRootResponse::new(blocks).unwrap_err();
        assert!(matches!(
            err,
            NetworkingError::ListTooLarge { kind, len, max }
                if kind == "blocks_by_root response"
                    && len == MAX_REQUEST_BLOCKS + 1
                    && max == MAX_REQUEST_BLOCKS
        ));
    }

    #[test]
    fn response_accepts_vec_array_and_iter_chain() {
        let _vec = BlocksByRootResponse::new(vec![sample_signed_block(1)]).unwrap();
        let _array = BlocksByRootResponse::new([sample_signed_block(2)]).unwrap();
        let _iter = BlocksByRootResponse::new((1_u8..=2).map(sample_signed_block)).unwrap();
    }
}
