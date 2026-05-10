//! Variable-length byte containers for SSZ `List[byte, LIMIT]` fields.
//!
//! [`ByteList`] enforces its limit at runtime; [`ByteListLimit<const LIMIT>`]
//! enforces it at the type level. Both reject construction when
//! `bytes.len() > limit` with [`TypesError::ByteListLimitExceeded`].

use crate::error::TypesError;

/// Variable-length byte sequence with a runtime-supplied upper bound.
///
/// SSZ `List[byte, N]` shape where `N` is supplied at construction time
/// (e.g. when the limit is read from a config). Use [`ByteListLimit`] when
/// the limit is a compile-time constant.
///
/// # Example
/// ```
/// use types::{ByteList, TypesError};
/// # fn main() -> Result<(), TypesError> {
/// let b = ByteList::try_new(vec![1, 2, 3], 8)?;
/// assert_eq!(b.as_slice(), &[1, 2, 3]);
/// assert_eq!(b.limit(), 8);
///
/// assert!(ByteList::try_new(vec![0; 9], 8).is_err());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ByteList {
    bytes: Vec<u8>,
    limit: usize,
}

impl ByteList {
    /// Constructs a [`ByteList`], rejecting `bytes` longer than `limit`.
    ///
    /// # Errors
    /// Returns [`TypesError::ByteListLimitExceeded`] when
    /// `bytes.len() > limit`.
    pub fn try_new(bytes: Vec<u8>, limit: usize) -> Result<Self, TypesError> {
        if bytes.len() > limit {
            return Err(TypesError::ByteListLimitExceeded {
                limit,
                got: bytes.len(),
            });
        }
        Ok(Self { bytes, limit })
    }

    /// Returns the underlying bytes as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the byte length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns `true` when the list contains no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Returns the declared upper bound.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Consumes the [`ByteList`] and returns the inner `Vec<u8>`.
    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

/// Variable-length byte sequence with a compile-time upper bound `LIMIT`.
///
/// SSZ `List[byte, LIMIT]` shape where the limit is a const generic — used
/// when the limit is fixed by the consensus spec and known at compile time.
///
/// # Example
/// ```
/// use types::{ByteListLimit, TypesError};
/// # fn main() -> Result<(), TypesError> {
/// let b = ByteListLimit::<32>::try_new(vec![0; 16])?;
/// assert_eq!(b.len(), 16);
///
/// assert!(ByteListLimit::<32>::try_new(vec![0; 33]).is_err());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ByteListLimit<const LIMIT: usize>(Vec<u8>);

impl<const LIMIT: usize> ByteListLimit<LIMIT> {
    /// Constructs a [`ByteListLimit`], rejecting `bytes` longer than `LIMIT`.
    ///
    /// # Errors
    /// Returns [`TypesError::ByteListLimitExceeded`] when
    /// `bytes.len() > LIMIT`.
    pub fn try_new(bytes: Vec<u8>) -> Result<Self, TypesError> {
        if bytes.len() > LIMIT {
            return Err(TypesError::ByteListLimitExceeded {
                limit: LIMIT,
                got: bytes.len(),
            });
        }
        Ok(Self(bytes))
    }

    /// Returns the underlying bytes as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Returns the byte length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` when the list contains no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the compile-time upper bound.
    #[must_use]
    pub const fn limit(&self) -> usize {
        LIMIT
    }

    /// Consumes the [`ByteListLimit`] and returns the inner `Vec<u8>`.
    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

impl<const LIMIT: usize> Default for ByteListLimit<LIMIT> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // -- ByteList ----------------------------------------------------------

    #[test]
    fn byte_list_accepts_under_limit() {
        let b = ByteList::try_new(vec![1, 2, 3], 8).unwrap();
        assert_eq!(b.as_slice(), &[1, 2, 3]);
        assert_eq!(b.len(), 3);
        assert_eq!(b.limit(), 8);
        assert!(!b.is_empty());
    }

    #[test]
    fn byte_list_accepts_at_limit() {
        let b = ByteList::try_new(vec![0; 4], 4).unwrap();
        assert_eq!(b.len(), 4);
    }

    #[test]
    fn byte_list_accepts_empty_with_zero_limit() {
        let b = ByteList::try_new(vec![], 0).unwrap();
        assert!(b.is_empty());
        assert_eq!(b.limit(), 0);
    }

    // ByteList::try_new rejects inputs longer than limit.
    #[test]
    fn byte_list_rejects_over_limit() {
        let err = ByteList::try_new(vec![0; 5], 4).unwrap_err();
        assert!(matches!(
            err,
            TypesError::ByteListLimitExceeded { limit: 4, got: 5 }
        ));
    }

    #[test]
    fn byte_list_rejects_one_byte_over_zero_limit() {
        let err = ByteList::try_new(vec![0], 0).unwrap_err();
        assert!(matches!(
            err,
            TypesError::ByteListLimitExceeded { limit: 0, got: 1 }
        ));
    }

    #[test]
    fn byte_list_into_inner_returns_original_vec() {
        let b = ByteList::try_new(vec![9, 8, 7], 16).unwrap();
        assert_eq!(b.into_inner(), vec![9, 8, 7]);
    }

    // -- ByteListLimit ------------------------------------------------------

    #[test]
    fn byte_list_limit_accepts_under_limit() {
        let b = ByteListLimit::<8>::try_new(vec![1, 2, 3]).unwrap();
        assert_eq!(b.as_slice(), &[1, 2, 3]);
        assert_eq!(b.limit(), 8);
    }

    #[test]
    fn byte_list_limit_accepts_at_limit() {
        let b = ByteListLimit::<4>::try_new(vec![0; 4]).unwrap();
        assert_eq!(b.len(), 4);
    }

    #[test]
    fn byte_list_limit_rejects_over_limit() {
        let err = ByteListLimit::<4>::try_new(vec![0; 5]).unwrap_err();
        assert!(matches!(
            err,
            TypesError::ByteListLimitExceeded { limit: 4, got: 5 }
        ));
    }

    #[test]
    fn byte_list_limit_default_is_empty() {
        let b: ByteListLimit<32> = ByteListLimit::default();
        assert!(b.is_empty());
        assert_eq!(b.limit(), 32);
    }

    #[test]
    fn byte_list_limit_zero_accepts_empty_only() {
        assert!(ByteListLimit::<0>::try_new(vec![]).is_ok());
        let err = ByteListLimit::<0>::try_new(vec![1]).unwrap_err();
        assert!(matches!(
            err,
            TypesError::ByteListLimitExceeded { limit: 0, got: 1 }
        ));
    }

    #[test]
    fn byte_list_limit_into_inner_round_trips() {
        let b = ByteListLimit::<16>::try_new(vec![1, 2, 3]).unwrap();
        assert_eq!(b.into_inner(), vec![1, 2, 3]);
    }

    proptest! {
        #[test]
        fn byte_list_round_trip_under_limit(
            bytes in proptest::collection::vec(any::<u8>(), 0..=64),
            extra in 0_usize..=16,
        ) {
            let limit = bytes.len() + extra;
            let b = ByteList::try_new(bytes.clone(), limit).unwrap();
            prop_assert_eq!(b.as_slice(), bytes.as_slice());
            prop_assert_eq!(b.limit(), limit);
        }

        #[test]
        fn byte_list_rejects_when_over(
            limit in 0_usize..=32,
            over in 1_usize..=8,
        ) {
            let bytes = vec![0_u8; limit + over];
            let err = ByteList::try_new(bytes, limit).unwrap_err();
            match err {
                TypesError::ByteListLimitExceeded { limit: l, got: g } => {
                    prop_assert_eq!(l, limit);
                    prop_assert_eq!(g, limit + over);
                }
                other => prop_assert!(false, "unexpected error: {other:?}"),
            }
        }

        #[test]
        fn byte_list_limit_round_trip(bytes in proptest::collection::vec(any::<u8>(), 0..=32)) {
            let b = ByteListLimit::<32>::try_new(bytes.clone()).unwrap();
            prop_assert_eq!(b.as_slice(), bytes.as_slice());
        }

        #[test]
        fn byte_list_limit_rejects_over(over in 1_usize..=16) {
            let bytes = vec![0_u8; 32 + over];
            let err = ByteListLimit::<32>::try_new(bytes).unwrap_err();
            match err {
                TypesError::ByteListLimitExceeded { limit, got } => {
                    prop_assert_eq!(limit, 32);
                    prop_assert_eq!(got, 32 + over);
                }
                other => prop_assert!(false, "unexpected error: {other:?}"),
            }
        }
    }
}
