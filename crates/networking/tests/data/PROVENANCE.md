# networking testdata — provenance

Verbatim copies of fixtures from the lean-go reference implementation.

## `messages.json`

Carries the canonical `protocol_id` strings, parsed payloads, and the `max_request_blocks` constant for `Status` / `BlocksByRootRequest` / `BlocksByRootResponse`. It does **not** carry encoded SSZ byte sequences.

The parity test in [tests/messages.rs](../messages.rs) validates:

1. Protocol-ID strings match byte-for-byte against `STATUS_PROTOCOL_V1` / `BLOCKS_BY_ROOT_PROTOCOL_V1`.
2. `MAX_REQUEST_BLOCKS` equals the fixture's `max_request_blocks`.
3. The Rust codec round-trips the typed values built from the parsed payloads (`encode → decode → equal`).

Byte-parity against lean-go's SSZ encoder output lands when either the lean-go fixture grows an `encoded_hex` field or a separate dedicated parity-vector dump is added.

### Signature representation note

The fixture's `SignedBlock` signature is 32 bytes; the Rust protocol crate models the same field as `Bytes4000` (4000-byte XMSS placeholder). The parity helper pads the JSON's leading bytes with zeros to fill the Rust type. The round-trip assertion is therefore structural ("the Rust codec preserves whatever it was handed") rather than a byte-for-byte cross-implementation match.

## `gossipsub.json`

Carries 4 cases covering the gossipsub message-id input/output triplet:

- `topic_hex`, `raw_data_hex`, `snappy_mode`, optional `decompressed_hex`.
- `domain_hex`, `hash_input_hex`, `message_id_hex`.

Cases cover the three snappy-mode branches (`none`, `success`, `failure`) plus a binary-payload case. Every field is verified byte-for-byte by the co-located parity test in [src/gossipsub.rs](../../src/gossipsub.rs):

1. `build_message_id_hash_input(domain, topic, payload)` matches `hash_input_hex`.
2. `compute_gossipsub_message_id(domain, topic, payload)` matches `message_id_hex`.

This is the strongest cross-implementation parity guarantee we can land before the libp2p adapter exists — the layout and the SHA-256 truncation are byte-identical to lean-go's reference for every documented case.

## `wire-parity/*.ssz`

Verbatim SSZ-encoded container blobs (paired with their `.root.hex` HashTreeRoot files in the source repository; the roots are not copied because the SSZ HashTreeRoot helper hasn't landed yet).

| Fixture                              | Container       |
| ------------------------------------ | --------------- |
| `empty.blockbody.ssz`                | `BlockBody`     |
| `two-votes.blockbody.ssz`            | `BlockBody`     |
| `genesis-4val.state.ssz`             | `State`         |
| `genesis-anchor.checkpoint.ssz`      | `Checkpoint`    |
| `slot12-justified.checkpoint.ssz`    | `Checkpoint`    |
| `slot1.blockheader.ssz`              | `BlockHeader`   |
| `slot1-empty.block.ssz`              | `Block`         |
| `slot1-empty.signedblock.ssz`        | `SignedBlock`   |
| `slot7-vote.vote.ssz`                | `Vote`          |
| `validator3-vote.signedvote.ssz`     | `SignedVote`    |

The parity test in [tests/parity.rs](../parity.rs) validates, for every fixture:

1. **SSZ byte-parity** — `decode::<T>(fixture)` then `encode(&value)` reproduces the fixture bytes exactly.
2. **Snappy framed wire round-trip** — `encode_req_resp_wire(fixture)` then `decode_req_resp_wire(wire)` returns the original SSZ.
3. **Length-prefixed stream round-trip** — `write_req_resp_frame` then `read_req_resp_frame` returns the original SSZ and leaves the cursor at clean EOF.

HashTreeRoot validation is out of scope until the SSZ HTR helper exists; the `.root.hex` files in the source corpus are intentionally not copied.
