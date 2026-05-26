# lean-rust

Implementation of Lean Ethereum in Rust.

## local-pq Devnet

The repo contains a crate-local Docker devnet for running one `ream` node and
one `lean-rust` node against generated local-pq genesis state.

```sh
cp crates/pq-devnet-0/.env.example crates/pq-devnet-0/.env
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
