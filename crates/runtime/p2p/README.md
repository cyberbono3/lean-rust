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
- [`Host`] — clone-friendly handle. `peer_id()` and a
  `pub(crate)` command channel today; gossip publish / req/resp
  send variants extend [`HostCommand`] in later milestones.
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
  codec is a stub — every read/write returns
  `io::ErrorKind::Unsupported` until the handler logic lands.
- `identify::Behaviour` advertising `lean/0.1.0` as the protocol
  version plus the configured [`AgentVersion`].
- `ping::Behaviour` (default config).

[`DevnetBehaviour`]: ./src/host/behaviour.rs
[`src/host/behaviour.rs`]: ./src/host/behaviour.rs
[`networking::compute_gossipsub_message_id`]: ../../networking/src/gossipsub.rs

## Identity persistence

`<identity_path>` holds the libp2p protobuf-encoded keypair. Missing
file → generate Ed25519, persist, chmod `0600` (POSIX). Corrupt file
→ [`HostError::InvalidIdentity`] — never silently overwritten.

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

- Topic subscribe / publish — follow-up work.
- `Status` / `BlocksByRoot` handler logic — follow-up work
  (codec is a stub in this crate).
- Two-node loopback interop — follow-up work.
- `runtime-chain::Publisher` adapter wiring — `node` crate.

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
