//! Foundation primitives for the Lean Ethereum devnet0 client.
//!
//! Pure value types — no I/O, no consensus knowledge, SSZ-compatible.
//!
//! # Scope
//! - Wide unsigned integers ([`U128`], [`U256`]).
//! - Little-endian SSZ decode helpers for native `u8`/`u16`/`u32`/`u64`.
//! - SSZ-compatible [`Boolean`] (alias to [`bool`]) plus [`decode_boolean`].
//! - Range-checked [`BasisPoint`] (`0..=10_000`).
//! - Fixed-width byte vectors: [`ByteVector<N>`], [`Bytes32`], the devnet-1
//!   XMSS wire types [`Signature`] and [`PublicKey`], and the deprecated
//!   [`Bytes4000`] placeholder that [`Signature`] replaces.
//! - Variable-length byte lists: [`ByteList`] (runtime limit) and
//!   [`ByteListLimit<const LIMIT: usize>`] (compile-time limit).
//! - SSZ bitfields: [`Bitvector<const N: usize>`] (fixed length) and
//!   [`Bitlist<const LIMIT: usize>`] (variable length, delimiter bit).
//! - The crate-wide [`TypesError`] enum.
//!
//! # Example
//! ```
//! use types::{
//!     decode_u64_le, BasisPoint, Bitlist, Bitvector, Boolean, ByteList, Bytes32, TypesError,
//!     U128, U256,
//! };
//!
//! # fn main() -> Result<(), TypesError> {
//! let x: u64 = decode_u64_le(&42_u64.to_le_bytes())?;
//! assert_eq!(x, 42);
//!
//! let half: BasisPoint = BasisPoint::new(5_000)?;
//! assert_eq!(half.get(), 5_000);
//!
//! let root: Bytes32 = Bytes32::zero();
//! assert_eq!(root.as_slice().len(), 32);
//!
//! let payload = ByteList::try_new(vec![1, 2, 3], 1024)?;
//! assert_eq!(payload.len(), 3);
//!
//! let mut bv: Bitvector<8> = Bitvector::new();
//! bv.set(0, true)?;
//! assert_eq!(bv.count_ones(), 1);
//!
//! let mut bl: Bitlist<32> = Bitlist::new();
//! bl.set(5, true)?;
//! assert_eq!(bl.len(), 6);
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
pub mod bitfields;
pub mod boolean;
pub mod byte_arrays;
pub mod bytes;
pub mod error;
pub mod uint;

pub use basispt::{BasisPoint, MAX_BASIS_POINT};
pub use bitfields::{Bitlist, Bitvector};
pub use boolean::{decode_boolean, Boolean};
pub use byte_arrays::{ByteVector, Bytes32, PublicKey, Signature};
// Split out of the group above: an attribute cannot be applied to one name
// inside a brace list, and re-exporting a deprecated item warns. Retires with
// the alias itself.
#[allow(deprecated)]
pub use byte_arrays::Bytes4000;
pub use bytes::{ByteList, ByteListLimit};
pub use error::TypesError;
pub use uint::{decode_u16_le, decode_u32_le, decode_u64_le, decode_u8_le, U128, U256};
