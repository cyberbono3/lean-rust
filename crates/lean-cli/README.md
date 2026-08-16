# lean-cli

Library surface for the `lean-rust` binary.

Everything the binary does beyond `main` and `run` lives here, so the entry
point (`bin/lean-rust/src/main.rs`) is a genuinely thin shell that wires
these pieces into the runtime composition root (`node::new_devnet`). Kept as
a library so each piece is unit-testable without spawning the binary.

This file is also the crate documentation (`#![doc = include_str!]` in
`src/lib.rs`), so the module inventory below has exactly one home.

## Scope

- [`cli`](./src/cli.rs) — the `clap` parser: `Cli`, the `Command` enum, and
  the flag → runtime-config mapping.
- [`genesis`](./src/genesis.rs) — genesis builders, including the loader
  that decodes the compact interop `genesis.ssz` via
  `protocol::State::from_ream_legacy_ssz_bytes`.
- [`keygen`](./src/keygen.rs) — libp2p identity key generation / loading.
- [`validator_keygen`](./src/validator_keygen.rs) — offline XMSS validator
  attestation-key generation and the coordinator-canonical `genesis_validators`
  pubkey manifest (the `generate-validator-keys` subcommand). Distinct from
  `keygen` (libp2p Ed25519 peer identity).
- [`startup`](./src/startup.rs) — tracing installation, the one-shot
  `startup configuration` log line, and warnings for flags that are
  accepted for CLI compatibility but not wired.
- [`commands`](./src/commands.rs) — subcommand dispatch (`devnet-config`,
  keygen, peer id) behind `dispatch(&Cli) -> anyhow::Result<Dispatch>`.
- [`node_config`](./src/node_config.rs) — CLI-to-node wiring: address
  defaults, identity-path precedence, storage-backend mapping, genesis
  synthesis, and validator-group selection, assembled into `node::Config`.
- [`shutdown`](./src/shutdown.rs) — SIGINT/SIGTERM handling for the binary.

## Tier and dependencies

Binary-support crate. Depends on `runtime`, `node` (for `node::Config`,
which it assembles), `config`, `protocol`, `ssz`, `crypto` (the validator
keygen port), `clap`, `libp2p`, `tokio` (signal handling only), `rand`, and
`hex`. The runtime services themselves live in `runtime`; this crate only
assembles inputs for them and hands the result to `node::new_devnet`.
