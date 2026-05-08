//! Basis-point newtype with `0..=10_000` range invariant.
//!
//! A basis point is 1/100th of a percent: `10_000 bps = 100%`. The
//! [`BasisPoint`] newtype enforces the inclusive range at construction time
//! via [`BasisPoint::new`].

use crate::error::TypesError;

/// Maximum allowed value for a [`BasisPoint`] (`10_000` bps == 100%).
///
/// # Example
/// ```
/// use types::MAX_BASIS_POINT;
/// assert_eq!(MAX_BASIS_POINT, 10_000);
/// ```
pub const MAX_BASIS_POINT: u64 = 10_000;

/// Basis-point value, guaranteed by construction to lie in `0..=10_000`.
///
/// # Example
/// ```
/// use types::{BasisPoint, TypesError};
/// # fn main() -> Result<(), TypesError> {
/// let half = BasisPoint::new(5_000)?;
/// assert_eq!(half.get(), 5_000);
///
/// assert!(BasisPoint::new(10_001).is_err());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct BasisPoint(u64);

impl BasisPoint {
    /// Constructs a [`BasisPoint`], rejecting values strictly greater than
    /// [`MAX_BASIS_POINT`] (`10_000`).
    ///
    /// # Errors
    /// Returns [`TypesError::BasisPointOutOfRange`] when `value > 10_000`.
    ///
    /// # Example
    /// ```
    /// use types::BasisPoint;
    /// assert!(BasisPoint::new(0).is_ok());
    /// assert!(BasisPoint::new(10_000).is_ok());
    /// assert!(BasisPoint::new(10_001).is_err());
    /// ```
    pub const fn new(value: u64) -> Result<Self, TypesError> {
        if value > MAX_BASIS_POINT {
            return Err(TypesError::BasisPointOutOfRange { value });
        }
        Ok(Self(value))
    }

    /// Returns the underlying `u64` value.
    ///
    /// # Example
    /// ```
    /// use types::BasisPoint;
    /// assert_eq!(BasisPoint::new(2_500).unwrap().get(), 2_500);
    /// ```
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn new_accepts_zero() {
        assert_eq!(BasisPoint::new(0).unwrap().get(), 0);
    }

    #[test]
    fn new_accepts_max() {
        assert_eq!(
            BasisPoint::new(MAX_BASIS_POINT).unwrap().get(),
            MAX_BASIS_POINT
        );
    }

    #[test]
    fn new_rejects_one_over_max() {
        assert!(matches!(
            BasisPoint::new(MAX_BASIS_POINT + 1),
            Err(TypesError::BasisPointOutOfRange { value })
                if value == MAX_BASIS_POINT + 1
        ));
    }

    #[test]
    fn new_rejects_u64_max() {
        assert!(matches!(
            BasisPoint::new(u64::MAX),
            Err(TypesError::BasisPointOutOfRange { .. })
        ));
    }

    #[test]
    fn default_is_zero() {
        assert_eq!(BasisPoint::default().get(), 0);
    }

    proptest! {
        #[test]
        fn new_round_trip_in_range(v in 0_u64..=MAX_BASIS_POINT) {
            let bp = BasisPoint::new(v).unwrap();
            prop_assert_eq!(bp.get(), v);
        }

        #[test]
        fn new_rejects_out_of_range(v in (MAX_BASIS_POINT + 1)..=u64::MAX) {
            prop_assert!(BasisPoint::new(v).is_err());
        }
    }
}
