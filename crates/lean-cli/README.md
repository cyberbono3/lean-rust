# lean-cli

Library surface for the `lean-rust` binary.

Carries the CLI parser, genesis builders, and identity keygen helpers so the
binary entry-point (`bin/lean-rust/src/main.rs`) stays a thin shell that
wires these into the runtime composition root (`node::new_devnet`). Kept as
a library so the pieces are unit-testable without spawning the binary.

## Scope

- [`cli`](./src/cli.rs) — the `clap` parser: `Cli`, the `Command` enum, and
  the flag → runtime-config mapping.
- [`genesis`](./src/genesis.rs) — genesis builders, including the loader
  that decodes the compact interop `genesis.ssz` via
  `protocol::State::from_ream_legacy_ssz_bytes`.
- [`keygen`](./src/keygen.rs) — libp2p identity key generation / loading.

## Tier and dependencies

Binary-support crate. Depends on `protocol`, `config`, `node`,
`lean-observability`, `lean-p2p-host`, and `clap`. The runtime services
themselves live in the `runtime/*` crates; this crate only assembles inputs
for them.
