# lean-rust

Implementation of Lean Ethereum in Rust.

## local-pq Devnet

The repo contains a crate-local Docker devnet for running one `ream` node and
one `lean-rust` node against generated local-pq genesis state.

```sh
cp crates/pq-devnet-0/.env.example crates/pq-devnet-0/.env
make devnet-start
make devnet-status
docker compose -f crates/pq-devnet-0/scripts/core/docker-compose.yml \
  --project-directory crates/pq-devnet-0 logs -f
make devnet-stop
```

There is no `make devnet-logs` target currently; use the `docker compose logs`
command above to follow container logs.

Use `make devnet-clean` when generated keys, genesis artifacts, logs,
containers, and Docker volumes should be removed. See
[`docs/local-pq-devnet0.md`](docs/local-pq-devnet0.md) for the full operator
guide and troubleshooting notes.
