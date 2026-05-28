# lean-wire

Wire codecs + protocol IDs for the consensus networking layer (Tier 4).

Tier 4: depends on `types`, `protocol`, `ssz`. No `runtime` or `storage`
imports — this crate defines the *wire shapes*, not the transport (the
libp2p driver lives in `lean-p2p-host`).

## Scope

- [`Status`], [`BlocksByRootRequest`], [`BlocksByRootResponse`] — typed
  req/resp payloads with SSZ codec.
- [`STATUS_PROTOCOL_V1`], [`BLOCKS_BY_ROOT_PROTOCOL_V1`], [`ProtocolId`] —
  libp2p protocol-ID constants.
- [`MAX_REQUEST_BLOCKS`] — `BlocksByRoot` list-length cap.
- [`BLOCK_TOPIC_V1`], [`VOTE_TOPIC_V1`] — gossipsub topic names.
- [`compute_gossipsub_message_id`](./src/gossipsub.rs) — deterministic
  20-byte gossipsub message-id primitive.
- [`encode_req_resp`] / [`decode_req_resp`] / [`encode_gossip`] /
  [`decode_gossip`](./src/codecs.rs) and the framing helpers
  [`read_req_resp_frame`] / [`write_req_resp_frame`] — SSZ+snappy
  encode/decode for req/resp and gossip payloads.
- [`NetworkingError`] — crate error type.

[`Status`]: ./src/messages.rs
[`BlocksByRootRequest`]: ./src/messages.rs
[`BlocksByRootResponse`]: ./src/messages.rs
[`STATUS_PROTOCOL_V1`]: ./src/protocol_ids.rs
[`BLOCKS_BY_ROOT_PROTOCOL_V1`]: ./src/protocol_ids.rs
[`ProtocolId`]: ./src/protocol_ids.rs
[`MAX_REQUEST_BLOCKS`]: ./src/config.rs
[`BLOCK_TOPIC_V1`]: ./src/topics.rs
[`VOTE_TOPIC_V1`]: ./src/topics.rs
[`encode_req_resp`]: ./src/codecs.rs
[`decode_req_resp`]: ./src/codecs.rs
[`encode_gossip`]: ./src/codecs.rs
[`decode_gossip`]: ./src/codecs.rs
[`read_req_resp_frame`]: ./src/frames.rs
[`write_req_resp_frame`]: ./src/frames.rs
[`NetworkingError`]: ./src/error.rs

## Tier and dependencies

Tier 4. Depends on `types`, `protocol`, `ssz`. No transport (`libp2p`)
dependency — the host driver in `runtime/p2p` consumes these wire shapes.
