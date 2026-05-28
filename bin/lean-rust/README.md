# lean-rust

The `lean-rust` binary entry point.

A thin shell: parse the CLI, initialize tracing, build genesis + identity,
wire everything into the runtime composition root (`node::new_devnet`), and
run the node until shutdown. All reusable logic lives in `lean-cli` and the
`runtime/*` crates so this stays a wiring layer.

## What it does

1. Parses [`lean_cli::cli::Cli`] (`clap`).
2. Installs tracing via [`lean_observability::init_tracing`] (verbosity,
   optional rolling file sink).
3. Builds the genesis state/block and the libp2p identity
   (`lean_cli::genesis`, `lean_cli::keygen`).
4. Assembles the devnet `Node` with `node::new_devnet` and runs its
   lifecycle (`chain → p2p → sync → duties → http → metrics`), draining on
   the shutdown signal.

## Endpoints (when running)

| Purpose | Default URL |
|---------|-------------|
| HTTP head | `http://127.0.0.1:<http-port>/lean/v0/head` |
| Prometheus metrics | `http://<metrics-addr>/metrics` |

## Run

```bash
cargo run -p lean-rust -- --help
```

For the full cross-client devnet (lean-rust + ream), see the `make devnet-*`
targets and `crates/fixtures/`.

## Dependencies

Depends on `lean-cli`, `node`, `lean-core`, `lean-observability`,
`lean-p2p-host`, and `clap`. No consensus logic of its own.
