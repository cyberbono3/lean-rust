# ssz

SSZ encode/decode + SHA-256 merkleization facade (Tier 1).

The single project-internal SSZ entry point — a thin facade over
`ethereum_ssz`. Downstream crates depend on `ssz`, never on the upstream
crate directly, so the upstream choice is swappable in one place.

## Scope

- [`Encode`] / [`Decode`] — re-exported from the upstream SSZ crate.
- [`encode`] — convenience free function returning `Vec<u8>`.
- [`decode`] — convenience free function returning `Result<T, SszError>`.
- [`SszError`] / [`DecodeErrorAdapter`] — facade error type that wraps the
  upstream `DecodeError` into the `std::error::Error::source` chain and adds
  merkleization-specific variants.
- [`merkleize`](./src/merkleize.rs) — SHA-256 merkleization helpers
  (`HashTreeRoot`, chunking, padding to power-of-two leaf counts).
- [`lists`](./src/lists.rs) — SSZ list encode/decode helpers.

[`Encode`]: ./src/lib.rs
[`Decode`]: ./src/lib.rs
[`encode`]: ./src/encode.rs
[`decode`]: ./src/decode.rs
[`SszError`]: ./src/error.rs
[`DecodeErrorAdapter`]: ./src/error.rs

## Tier and dependencies

Tier 1. Depends on `types` and the upstream `ethereum_ssz` facade only — no
consensus or runtime imports.
