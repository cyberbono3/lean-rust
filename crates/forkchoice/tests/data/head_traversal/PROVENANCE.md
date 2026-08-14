# head_traversal — provenance

LMD-GHOST head-traversal vectors backing `crates/forkchoice/tests/parity.rs`.

The upstream canonical trajectory fixture is a full-store replay (genesis → advance time → import block → process attestation → …). Replaying it requires the block-import path, which lands in a later forkchoice change.

Until then, the vectors exercised by `parity.rs` are **hand-derived**:

- Linear chain, no votes → head defaults to the deepest reachable block.
- Two-fork supermajority → head follows weight.
- Tie-break by root-bytes lex order on equal weight; slot is not part of the key.
- `min_score` threshold filters under-supported subtrees.
- Empty inputs and missing roots surface the typed `ForkchoiceError` variants.

When the block-import path lands, the trajectory replay can be added as an extra parity test against that fixture; the hand-derived cases stay in place as deterministic regression coverage for `get_fork_choice_head` in isolation.
