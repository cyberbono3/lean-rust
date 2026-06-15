# Application / Entry Layer

Crates: `node` (composition root), `lean-cli` (argument parsing + subcommands),
`bin/lean-rust` (binary entry point).

## Class diagram

![Application layer class diagram](../diagrams/application-class.svg)

Source: [`application-class.puml`](../diagrams/application-class.puml).

- **`bin/lean-rust`** — `main` (`#[tokio::main]`) and the `run` boot routine:
  `build_devnet_config`, `init_tracing`, `shutdown_signal`, plus default-address
  constants.
- **`lean-cli`** — the `Cli` flag struct, the `Command` subcommand enum
  (`DevnetConfig`, `GeneratePrivateKey`, `PeerId`), and the `genesis` / `keygen`
  helper functions.
- **`node`** — the node `Config` (wiring inputs), `new_devnet` (composition
  root), and the adapters that bridge ports to implementations:
  `PublisherAdapter` (duties `Publisher` → p2p), `RpcProviderAdapter` (p2p RPC →
  storage), `GossipIngestService` (p2p receivers → chain).

## Sequence — boot

![Boot sequence](../diagrams/application-seq-boot.svg)

Source: [`application-seq-boot.puml`](../diagrams/application-seq-boot.puml).

`main` parses the CLI, initializes tracing, and either runs a subcommand and
exits or builds the devnet config, calls `new_devnet` to wire all services, then
starts the node and waits for a shutdown signal before stopping it.
