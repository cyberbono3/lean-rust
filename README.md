# lean-rust

This is the implementation of Lean Ethereum in Rust.

> ⚠️ **Warning:** This code is work in progress. It is not clean and has not
> been reviewed yet. Use at your own risk.

**[Download the Lean Consensus spec (PDF)](docs/lean_consensus.pdf?raw=true)**
| [Devnet Guide](docs/local-pq-devnet0.md)

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

lean-rust is a Cargo workspace. Core crates:

| Crate | Purpose |
| ----- | ------- |
| `types` | Core domain types |
| `ssz` | SSZ serialization and hash-tree-root |
| `config` | Network and runtime configuration |
| `protocol` | State-transition function and protocol logic |
| `forkchoice` | Fork-choice rule |
| `storage` | Persistence layer |
| `networking` | P2P networking primitives |
| `observability` | Metrics and logging |
| `runtime/*` | Node runtime (core, chain, sync, duties, p2p, p2p-rpc, api) |
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
