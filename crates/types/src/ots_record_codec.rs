//! The shared fixed-width codec behind the two OTS records.
//!
//! [`crate::OtsKeyState`] (secret-bearing, on-disk) and [`crate::OtsWatermark`]
//! (seed-free, durable) have byte-identical layouts: a 32-byte head — the seed
//! in one, its commitment in the other — followed by three little-endian `u64`
//! fields. This module owns that layout so the two records cannot drift into
//! mutually undecodable encodings; each record keeps its own public length
//! constant and decode error, and delegates the byte work here.
//!
//! Crate-private: the shape is an implementation detail of the two records, not
//! a `types` API.

/// SSZ-layout byte length shared by both OTS records: 32 (head) + 8 + 8 + 8.
pub(crate) const OTS_RECORD_SSZ_LEN: usize = 32 + 8 + 8 + 8;

/// Encodes `head || a || b || c`, each integer little-endian.
pub(crate) fn encode(head: &[u8; 32], a: u64, b: u64, c: u64) -> [u8; OTS_RECORD_SSZ_LEN] {
    let mut out = [0_u8; OTS_RECORD_SSZ_LEN];
    out[0..32].copy_from_slice(head);
    out[32..40].copy_from_slice(&a.to_le_bytes());
    out[40..48].copy_from_slice(&b.to_le_bytes());
    out[48..56].copy_from_slice(&c.to_le_bytes());
    out
}

/// Inverse of [`encode`], returning `(head, a, b, c)`.
///
/// `None` when `bytes.len() != OTS_RECORD_SSZ_LEN` — the only failure mode. The
/// caller maps it to its own record-specific length error, so neither record's
/// public error type leaks into the other's.
pub(crate) fn decode(bytes: &[u8]) -> Option<([u8; 32], u64, u64, u64)> {
    if bytes.len() != OTS_RECORD_SSZ_LEN {
        return None;
    }
    let mut head = [0_u8; 32];
    head.copy_from_slice(&bytes[0..32]);
    // Length verified above, so each 8-byte slice decodes; reuse the crate's
    // canonical LE decoder rather than re-implementing it. The `ok()?` arms are
    // unreachable given the length check but keep the path panic-free.
    let field = |offset: usize| crate::decode_u64_le(&bytes[offset..offset + 8]).ok();
    Some((head, field(32)?, field(40)?, field(48)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let head = [0xab_u8; 32];
        let bytes = encode(&head, 1, 2, 3);
        assert_eq!(bytes.len(), OTS_RECORD_SSZ_LEN);
        assert_eq!(decode(&bytes), Some((head, 1, 2, 3)));
    }

    #[test]
    fn decode_rejects_wrong_length() {
        assert_eq!(decode(&[0_u8; 10]), None);
        assert_eq!(decode(&[0_u8; OTS_RECORD_SSZ_LEN + 1]), None);
    }

    /// The two records are wire-compatible by construction: this is what makes
    /// `OtsWatermark`'s "layout mirrors `OtsKeyState` byte-for-byte" claim a
    /// property of the code rather than of two hand-kept-in-sync copies.
    #[test]
    fn both_records_share_this_layout() {
        assert_eq!(
            crate::ots_key_state::OTS_KEY_STATE_SSZ_LEN,
            OTS_RECORD_SSZ_LEN
        );
        assert_eq!(
            crate::ots_watermark::OTS_WATERMARK_SSZ_LEN,
            OTS_RECORD_SSZ_LEN
        );
    }
}
