# `protocol` crate cleanup — duplication removal

Refactor pass to remove duplication and tighten the shared SSZ vocabulary across the `protocol` crate. No behavior change — pure cleanup.

## Changes

- **`crates/ssz/src/lib.rs`** — added `impl<const N: usize> HashTreeRoot for types::ByteVector<N>` (single generic covers `Bytes32`, `Bytes4000`, future widths). Removes the need for per-call `bytes_vector_hash_tree_root(x.as_slice())` in callers.
- **`crates/protocol/src/internal.rs`** — added centralized SSZ-length constants (`U64_LEN`, `BYTES32_LEN`, `BYTES4000_LEN`, `SLOT_LEN`, `VALIDATOR_INDEX_LEN`, `CHECKPOINT_LEN`, `BLOCK_HEADER_LEN`); replaced two near-identical list-HTR helpers (`bytes32_list_hash_tree_root` + `attestations_hash_tree_root`) with one generic `list_hash_tree_root<T: HashTreeRoot>`; dropped now-redundant `bytes_vector_hash_tree_root` and `encode_bytes32_list`/`decode_bytes32_list` callers.
- **`crates/protocol/src/block.rs`** — uses centralized constants; `BlockBody::hash_tree_root` calls the generic `list_hash_tree_root`; signature HTR via `self.signature.hash_tree_root()`.
- **`crates/protocol/src/vote.rs`** — uses centralized constants; `Vote::from_ssz_bytes` and `SignedVote::from_ssz_bytes` rewritten via `ensure_len` + `read_fixed`/`read_byte_array` (matching `block.rs`/`state.rs` style); signature HTR via `self.signature.hash_tree_root()`.
- **`crates/protocol/src/checkpoint.rs`** — uses centralized `BYTES32_LEN` and `CHECKPOINT_LEN`; dropped local `ROOT_LEN`/`SLOT_LEN`.
- **`crates/protocol/src/state.rs`** — uses centralized constants; `state::HashTreeRoot` calls the generic `list_hash_tree_root`.
- **`crates/protocol/src/test_fixtures.rs`** (new) — shared `cfg(test)` sample-value helpers (`sample_block_header`, `sample_block`, `sample_signed_block`, `sample_signed_vote`); consumed by `block.rs` and `state.rs` test modules.

## Plan steps

| # | Step | Status |
|---|---|---|
| 1 | Centralize SSZ-length constants | done |
| 2 | `HashTreeRoot for ByteVector<N>` in `ssz` crate | done |
| 3 | Generic `list_hash_tree_root<T: HashTreeRoot>` | done |
| 4 | `Encode`/`Decode` for `Bytes32` in helper crate | skipped (orphan rules block this without a newtype wrapper, which would add more noise than it removes) |
| 5 | `impl_fixed_ssz_container!` macro | skipped (≈30 mechanical lines saved but obscures the wire layout that cross-client review depends on; the explicit form wins on grep-ability) |
| 6 | `Vote::from_ssz_bytes` via `read_fixed` | done |
| 7 | Shared test-fixtures module | done |
| 8 | `variable_tail_payloads` for `Block`/`SignedBlock` | skipped (single-tail containers gain no readability from the array-of-tails pattern) |

## Verification

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

All 310 workspace tests pass. No public API change beyond the new trait impl in `ssz` and the centralized constants (crate-private).
