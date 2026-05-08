//! Foundation primitives for the Lean Ethereum devnet0 client.
//!
//! Pure value types — no I/O, no consensus knowledge, SSZ-compatible.
//!
//! # Issue #2 scope
//! - Wide unsigned integers ([`U128`], [`U256`]).
//! - Little-endian SSZ decode helpers for native `u8`/`u16`/`u32`/`u64`.
//! - SSZ-compatible [`Boolean`] (alias to [`bool`]) plus [`decode_boolean`].
//! - Range-checked [`BasisPoint`] (`0..=10_000`).
//! - The crate-wide [`TypesError`] enum.
//!
//! Byte arrays and bitfields land in subsequent issues (#3, #4).
//!
//! # Example
//! ```
//! use types::{decode_u64_le, BasisPoint, Boolean, TypesError, U128, U256};
//!
//! # fn main() -> Result<(), TypesError> {
//! let x: u64 = decode_u64_le(&42_u64.to_le_bytes())?;
//! assert_eq!(x, 42);
//!
//! let half: BasisPoint = BasisPoint::new(5_000)?;
//! assert_eq!(half.get(), 5_000);
//!
//! let _wide: U128 = U128::from(1_u64);
//! let _wider: U256 = U256::from(2_u64);
//! let _flag: Boolean = true;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

pub mod basispt;
pub mod boolean;
pub mod error;
pub mod uint;

pub use basispt::{BasisPoint, MAX_BASIS_POINT};
pub use boolean::{decode_boolean, Boolean};
pub use error::TypesError;
pub use uint::{decode_u16_le, decode_u32_le, decode_u64_le, decode_u8_le, U128, U256};
