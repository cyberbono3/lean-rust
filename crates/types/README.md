# types

Foundation primitives for the Lean Ethereum devnet0 client (Tier 0).

Pure value types — no I/O, no consensus knowledge, SSZ-compatible. Tier 0:
no project dependencies.

## Scope

- [`U128`], [`U256`] — wide unsigned integers.
- [`decode_u8_le`] / [`decode_u16_le`] / [`decode_u32_le`] /
  [`decode_u64_le`] — little-endian SSZ decode helpers for native ints.
- [`Boolean`] (alias to [`bool`]) + [`decode_boolean`] — SSZ-compatible
  boolean.
- [`BasisPoint`] / [`MAX_BASIS_POINT`] — range-checked `0..=10_000` value.
- [`ByteVector<N>`], [`Bytes32`] — fixed-width byte vectors.
- [`Signature`] (3116 B), [`PublicKey`] (52 B) — devnet-1 XMSS wire byte
  vectors. Widths are interop parameters, not permanent constants.
- [`Bytes4000`] — **deprecated** signature placeholder; [`Signature`]
  replaces it.
- [`ByteList`] / [`ByteListLimit<LIMIT>`] — variable-length byte lists
  (runtime / compile-time limit).
- [`Bitvector<N>`] / [`Bitlist<LIMIT>`] — SSZ bitfields.
- [`TypesError`] — crate error type.

[`U128`]: ./src/uint.rs
[`U256`]: ./src/uint.rs
[`decode_u8_le`]: ./src/uint.rs
[`decode_u16_le`]: ./src/uint.rs
[`decode_u32_le`]: ./src/uint.rs
[`decode_u64_le`]: ./src/uint.rs
[`Boolean`]: ./src/boolean.rs
[`decode_boolean`]: ./src/boolean.rs
[`BasisPoint`]: ./src/basispt.rs
[`MAX_BASIS_POINT`]: ./src/basispt.rs
[`ByteVector<N>`]: ./src/byte_arrays.rs
[`Bytes32`]: ./src/byte_arrays.rs
[`Signature`]: ./src/byte_arrays.rs
[`PublicKey`]: ./src/byte_arrays.rs
[`Bytes4000`]: ./src/byte_arrays.rs
[`ByteList`]: ./src/bytes.rs
[`ByteListLimit<LIMIT>`]: ./src/bytes.rs
[`Bitvector<N>`]: ./src/bitfields.rs
[`Bitlist<LIMIT>`]: ./src/bitfields.rs
[`TypesError`]: ./src/error.rs

## Tier and dependencies

Tier 0. No project dependencies — only the standard library and the minimal
external crates needed for the wide-integer / byte-array primitives.
