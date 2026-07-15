# lean-rust

This is the implementation of Lean Ethereum in Rust.

> ⚠️ **Warning:** This code is work in progress. It is not clean and has not
> been reviewed yet. Use at your own risk.

**[Download the Lean Consensus spec (PDF)](docs/lean_consensus.pdf?raw=true)**
| [Architecture](docs/architecture/README.md)
| [Devnet Guide](docs/local-pq-devnet0.md)
| [Metrics](docs/metrics/README.md)

## What is lean-rust

lean-rust aims to be a modular, contributor-friendly, and fast implementation
of the Lean Consensus specification. The goal is an ecosystem where developers
can build on lean-rust without reinventing the wheel.

## What is the Lean Consensus

The Lean Consensus is beacon chain 2.0, the next generation of Ethereum:
hardened for ultimate security and decentralization, plus finality in seconds.
Its goal is to transition quickly and safely from the Beacon Chain to a
consensus layer design much closer to the final design of Ethereum.

[Download the Lean Consensus spec (PDF)](docs/lean_consensus.pdf?raw=true)
for the full specification and background.

## Goals

- Modular
- Contributor friendly
- Fast
- Extendible

## Workspace

lean-rust is a Cargo workspace. See the [architecture docs](docs/architecture/README.md)
for layer maps and UML class/sequence diagrams. Core crates:

| Crate | Purpose |
| ----- | ------- |
| `types` | Core domain types |
| `ssz` | SSZ serialization and hash-tree-root |
| `config` | Network and runtime configuration |
| `protocol` | State-transition function and protocol logic |
| `forkchoice` | Fork-choice rule |
| `storage` | Persistence layer |
| `networking` | P2P networking primitives |
| `runtime` | Node runtime — one crate with modules `core`, `chain`, `sync`, `duties`, `p2p`, `api`, `observability` |
| `node` | Node assembly |
| `lean-cli` | Command-line interface |
| `fixtures` | Test fixtures and devnet assets |
| `bin/lean-rust` | Binary entry point |

> Post-quantum signatures in the local-pq devnet are placeholders. Signature
> verification and aggregation are future work and are not yet implemented.

## local-pq Devnet

The repo contains a crate-local Docker devnet for running one `ream` node and
one `lean-rust` node against generated local-pq genesis state. See the
[pq-devnet-0 high-level plan](https://github.com/leanEthereum/pm/blob/main/breakout-rooms/leanConsensus/pq-interop/pq-devnet-0.md)
for the cross-client interop goals.

**Prerequisite:** [Docker](https://docs.docker.com/get-docker/) (with the
Compose plugin) must be installed and running.

```sh
make devnet-start
make devnet-status
make devnet-logs
make devnet-stop
```

Use `make devnet-logs-lean` or `make devnet-logs-ream` to follow a single
node, and `make devnet-debug-summary` to print high-signal log markers.

Use `make devnet-clean` when generated keys, genesis artifacts, logs,
containers, and Docker volumes should be removed. See
[`docs/local-pq-devnet0.md`](docs/local-pq-devnet0.md) for the full operator
guide and troubleshooting notes.

## Interop parameters (pq-devnet-1)

These are contract values, not version preferences. Every client on the network
must agree on them: a different signature-scheme revision, or a different
container size, means signed blocks and attestations do not verify across
clients. Changing any row is an interop break, not a routine bump.

| Parameter | Value | Source |
| --------- | ----- | ------ |
| leanSpec revision | `050fa4a18881d54d7dc07601fe59e34eb20b9630` | [leanEthereum/leanSpec](https://github.com/leanEthereum/leanSpec) |
| leanSig revision | `f10dcbefac2502d356d93f686e8b4ecd8dc8840a` | [leanEthereum/leanSig](https://github.com/leanEthereum/leanSig) — pinned in `Cargo.toml` |
| leanSig scheme alias | `SIGTopLevelTargetSumLifetime32Dim64Base8` | `signature::generalized_xmss::instantiations_poseidon_top_level::lifetime_2_to_the_32::hashing_optimized` |
| Scheme parameters | `LIFETIME = 2^32`, `DIM = 64`, `BASE = 8`, `TARGET_SUM = 375` | leanSpec `xmss` `PROD_CONFIG` |
| leanMetrics revision | `e077ac2a2190a4946e01737b27eb9a5636e6884e` | [leanEthereum/leanMetrics](https://github.com/leanEthereum/leanMetrics) |
| Validator registry limit | `2^12` = `4096` | `config::DEVNET_CONFIG.validator_registry_limit` |
| `Signature` | 3116 bytes | leanSpec `Signature` container |
| `PublicKey` | 52 bytes | leanSpec `Validator.pubkey` |

Revisions are recorded as full 40-character hashes rather than short prefixes: a
prefix can become ambiguous as an upstream repository grows, and this table is
the only place these values are written down.

The leanSig revision is additionally pinned to an exact commit in `Cargo.toml`
rather than a branch or tag, because a moving reference would rebuild against a
different scheme and invalidate keys already generated against this one.
`scripts/check-leansig-pin.sh` fails if that pin ever floats.

The signature and public-key sizes above are the devnet-1 values confirmed
against leanSpec. They are recorded, not frozen: confirmation against live
cross-client traffic is still outstanding.

## Building and Testing

```sh
cargo build
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

## Getting Help

Open an issue with [a bug report](https://github.com/cyberbono3/lean-rust/issues/new).

## License

Licensed under either of MIT or Apache-2.0 at your option.
