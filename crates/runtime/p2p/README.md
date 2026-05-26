# runtime-p2p

libp2p QUIC-v1 host construction for the runtime shell (Tier 6).

Assembles the libp2p identity, transport, and composite
[`NetworkBehaviour`] into a single [`P2pService`] driven by one
swarm-poll task. The public [`Host`] handle is a cheap clone that
reaches the swarm through an `mpsc::Sender<HostCommand>` — the only
ownership shape that scales to gossip publish and req/resp send in
later additions without `Arc<Mutex<Swarm>>` contention.

## Scope

- [`DevnetHost::build`] — front door. Wires identity (load or
  generate), bootnodes (YAML), transport, and behaviour into a
  [`P2pService`] without starting the swarm.
- [`P2pService`] — implements [`runtime_core::Service`]. `start`
  binds the listener (fail-fast under a 2-second deadline), dials
  any configured bootnodes, and spawns the swarm-poll task.
  `stop` signals shutdown via the command channel + cancellation
  token and joins the task.
- [`Host`] — clone-friendly handle. `peer_id()` +
  `publish_block` / `publish_vote` (gossipsub) +
  `send_blocks_by_root` (req/resp).
- [`gossip`] — typed [`Topic`] wrapper, [`MessageId`] / [`PublishError`]
  re-exports, and one-shot [`BlockReceiver`] / [`VoteReceiver`] handles
  for inbound `SignedBlock` / `SignedVote` payloads.
- [`rpc`] — [`RpcProvider`] trait (composing binary implements over
  `storage::Store`), [`NoOpRpcProvider`] convenience, typed
  [`RpcRequest`] / [`RpcResponse`] enums, [`RpcError`] failure surface.
- [`HostOptions`] + newtypes — `ListenAddr`, `AgentVersion`,
  `IdentityPath`, `BootnodesPath`. Always-valid: construction goes
  through `HostOptions::new` (typed) or `HostOptions::try_new`
  (loose).
- [`HostError`] — typed failure surface covering option
  validation, identity IO + decode, bootnode parse, bind, and
  transport errors. `AlreadyStarted` for lifecycle misuse.

[`DevnetHost::build`]: ./src/devnet.rs
[`P2pService`]: ./src/service.rs
[`Host`]: ./src/host/mod.rs
[`HostCommand`]: ./src/host/mod.rs
[`HostOptions`]: ./src/options.rs
[`HostError`]: ./src/error.rs
[`gossip`]: ./src/gossip/mod.rs
[`Topic`]: ./src/gossip/mod.rs
[`MessageId`]: ./src/gossip/publisher.rs
[`PublishError`]: ./src/gossip/publisher.rs
[`BlockReceiver`]: ./src/gossip/handler.rs
[`VoteReceiver`]: ./src/gossip/handler.rs
[`rpc`]: ./src/rpc/mod.rs
[`RpcProvider`]: ./src/rpc/mod.rs
[`NoOpRpcProvider`]: ./src/rpc/mod.rs
[`RpcRequest`]: ./src/host/behaviour/codec.rs
[`RpcResponse`]: ./src/host/behaviour/codec.rs
[`RpcError`]: ./src/rpc/mod.rs
[`runtime_core::Service`]: ../core/src/service.rs
[`NetworkBehaviour`]: https://docs.rs/libp2p/0.55/libp2p/swarm/trait.NetworkBehaviour.html

## Composite behaviour

[`DevnetBehaviour`] (in [`src/host/behaviour.rs`]) combines:

- `gossipsub::Behaviour` with the deterministic 20-byte
  message-id function from
  [`networking::compute_gossipsub_message_id`]. Snappy domain
  (`MESSAGE_DOMAIN_VALID_SNAPPY` / `MESSAGE_DOMAIN_INVALID_SNAPPY`)
  is resolved per-message via a thread-local `snap::raw::Decoder`
  + reusable scratch buffer (alloc-free on the hot path). Frames
  claiming a decompressed size larger than 16 MiB are rejected as
  invalid without allocating (DOS cap).
  Authenticity: `ValidationMode::Anonymous` (devnet0 is unsigned).
- `request_response::Behaviour<SszSnappyCodec>` advertising
  `STATUS_PROTOCOL_V1` and `BLOCKS_BY_ROOT_PROTOCOL_V1`. The
  codec uses SSZ payloads inside Ream-compatible req/resp Snappy frames.
- `identify::Behaviour` advertising `lean/0.1.0` as the protocol
  version plus the configured [`AgentVersion`].
- `ping::Behaviour` (default config).

[`DevnetBehaviour`]: ./src/host/behaviour.rs
[`src/host/behaviour.rs`]: ./src/host/behaviour.rs
[`networking::compute_gossipsub_message_id`]: ../../networking/src/gossipsub.rs

## Gossip

`Service::start` subscribes the swarm's gossipsub behaviour to every
[`Topic`] variant using the Ream-compatible local-pq topics in
[`networking::BLOCK_TOPIC_V1`] / [`networking::VOTE_TOPIC_V1`]. Failure
surfaces as [`HostError::GossipSubscribe`] and rolls the lifecycle back
to `Idle`.

Outbound: [`Host::publish_block`] / [`Host::publish_vote`] SSZ-encode +
Snappy-block-compress the payload via [`networking::encode_gossip`],
dispatch a [`HostCommand::Publish`] to the swarm-poll task, and await
the libp2p [`MessageId`] over a `oneshot` reply. The single-task
ownership invariant on the `Swarm` is preserved — there is no
`Arc<Mutex<Swarm>>` on the publish path.

Inbound: `gossipsub::Event::Message` is routed inside the swarm task
through [`gossip::handler::route_gossipsub_message`]. The payload is
[`networking::decode_gossip`]-ed into a typed `SignedBlock` /
`SignedVote` and forwarded over per-topic `mpsc::Sender`s. The
receivers live on [`P2pService`] behind a one-shot guard:

```rust
let mut blocks = service.take_block_receiver().expect("one-shot");
let mut votes  = service.take_vote_receiver().expect("one-shot");
while let Some(block) = blocks.recv().await { /* ... */ }
```

Backpressure: the handler uses `try_send` and drops on a full receiver
(logged at `warn`). Gossipsub mesh replay covers transient loss.

[`Host::publish_block`]: ./src/gossip/publisher.rs
[`Host::publish_vote`]: ./src/gossip/publisher.rs
[`HostCommand::Publish`]: ./src/host/mod.rs
[`HostError::GossipSubscribe`]: ./src/error.rs
[`gossip::handler::route_gossipsub_message`]: ./src/gossip/handler.rs
[`networking::BLOCK_TOPIC_V1`]: ../../networking/src/topics.rs
[`networking::VOTE_TOPIC_V1`]: ../../networking/src/topics.rs
[`networking::encode_gossip`]: ../../networking/src/codecs.rs
[`networking::decode_gossip`]: ../../networking/src/codecs.rs

## Req/Resp

Two protocols on `request_response::Behaviour<SszSnappyCodec>`:

- [`networking::STATUS_PROTOCOL_V1`] — handshake.
  Each side sends a `Status` request on `ConnectionEstablished`. The
  peer's reply is validated against the local Status from
  [`RpcProvider::local_status`]; mismatched peers are disconnected via
  `Swarm::disconnect_peer_id`. Devnet0-permissive predicate: same
  finalized slot ⇒ roots must agree; otherwise one party is ahead and
  the other can sync.
- [`networking::BLOCKS_BY_ROOT_PROTOCOL_V1`]
  — request a list of blocks by tree-root. Inbound requests are
  answered by looking up each root via
  [`RpcProvider::get_block_by_root`]; unknown roots are silently
  dropped (response is empty when every root is missing). Outbound
  requests go through [`Host::send_blocks_by_root`]; the swarm task
  parks the reply oneshot in an `OutboundRequestId`-keyed correlation
  table until the matching libp2p response or failure event fires.

The codec ([`SszSnappyCodec`]) bridges sync-encoded SSZ + Snappy frames
(from [`networking::encode_req_resp`] / [`networking::decode_req_resp`])
to libp2p's async substream API via a read-to-end + sync-decode
pattern.

Storage decoupling: [`RpcProvider`] is the trait the composing binary
(`node`) implements over `storage::Store`. `runtime-p2p` accepts
`Arc<dyn RpcProvider>` at [`DevnetHost::build_with_provider`].
Lifecycle tests use [`DevnetHost::build`], which wires a
[`NoOpRpcProvider`] internally.

[`networking::STATUS_PROTOCOL_V1`]: ../../networking/src/protocol_ids.rs
[`networking::BLOCKS_BY_ROOT_PROTOCOL_V1`]: ../../networking/src/protocol_ids.rs
[`Host::send_blocks_by_root`]: ./src/rpc/client.rs
[`SszSnappyCodec`]: ./src/host/behaviour/codec.rs
[`networking::encode_req_resp`]: ../../networking/src/codecs.rs
[`networking::decode_req_resp`]: ../../networking/src/codecs.rs
[`RpcProvider::local_status`]: ./src/rpc/mod.rs
[`RpcProvider::get_block_by_root`]: ./src/rpc/mod.rs
[`DevnetHost::build_with_provider`]: ./src/devnet.rs

## Identity persistence

`<identity_path>` holds either a libp2p protobuf-encoded keypair or a
local-pq raw hex secp256k1 private key. Missing file → generate
Ed25519, persist protobuf, chmod `0600` (POSIX). Existing files are
loaded without mutation; invalid raw hex, wrong-length raw keys, and
invalid secp256k1 key material surface typed [`HostError`] variants.

## Bootnodes

Flat YAML list of multiaddr strings whose terminal component is
`/p2p/<peer-id>`:

```yaml
- /ip4/192.0.2.10/udp/9000/quic-v1/p2p/12D3KooW...
- /ip4/192.0.2.11/udp/9000/quic-v1/p2p/12D3KooW...
```

Each entry parses into a `(Multiaddr, PeerId)` pair: the swarm dials
the multiaddr; the peer id is required for outbound identification
before the libp2p handshake completes. Malformed entries surface as
[`HostError::InvalidBootnode`] carrying the offending raw string.

For local-pq devnet0, ream still consumes generated ENR `nodes.yaml`.
Rust consumes a temporary `bootnodes.rust.yaml` adapter in this flat
multiaddr shape. The adapter should include remote bootnodes only; the
2-node `leanrust_1` file contains `ream_0` and avoids self-dialing by
construction.

## Bind fail-fast

`Service::start` calls `Swarm::listen_on` and races the swarm's
first listener event against a 2-second deadline:

- `NewListenAddr` for our listener → OK.
- `ListenerClosed { reason: Err(_) }` or `ListenerError` for our
  listener → [`HostError::Bind`].
- Deadline elapsed → [`HostError::Bind`] with a deadline message.

The swarm-poll task does **not** spawn until bind confirms. On the
failure path, state rolls back to `Idle` so the service is
re-startable after the operator fixes the config.

## Dependency boundaries

`runtime-p2p` does **not** depend on `runtime-chain`,
`runtime-sync`, `runtime-duties`, `engine`, `storage`, `forkchoice`,
or `statetransition` — verified by `cargo metadata`. The
`Publisher` adapter wiring through `runtime-chain::Publisher` lives
in the `node` composition root, per Decision 7.

```bash
cargo metadata --format-version=1 \
  | jq -r '.packages[] | select(.name=="runtime-p2p").dependencies[].name' \
  | grep -E '^(runtime-chain|runtime-sync|runtime-duties|engine|storage|forkchoice|statetransition)$' \
  && exit 1 || exit 0
```

## Out of scope

- Two-node loopback interop smoke test — follow-up work.
- `runtime-chain::Publisher` adapter wiring — `node` crate.
- Per-block streaming `BlocksByRoot` wire format (current shape is one
  SSZ container per response).
- Topic scoring / peer scoring / mesh tuning.
- Backpressure-aware ingestion (current handler drops on full
  receiver; gossipsub mesh replay covers transient loss).

## Tier and dependencies

Tier 6. Depends on `runtime-core`, `networking`, `protocol`,
`config`, plus `libp2p` (QUIC-v1, gossipsub, request_response,
identify, ping, noise, yamux), `snap` for gossipsub message-id
domain resolution, and the standard async stack (`tokio`,
`tokio-util`, `async-trait`, `tracing`, `parking_lot`).

## Verification

```bash
cargo fmt --check
cargo clippy -p runtime-p2p --all-targets -- -D warnings
cargo test -p runtime-p2p
cargo test -p runtime-p2p --test host_build
```
