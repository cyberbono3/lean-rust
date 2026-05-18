## Summary

- Added a repo-local Docker build path for `lean-rust:local`.
- Added `crates/pq-devnet-0/Dockerfile` to build `lean-beacon` in a Rust builder image and package it into a slim runtime image.
- Added `.dockerignore` coverage so Docker build context excludes `target/`, generated pq-devnet genesis artifacts, generated keys, logs, local env files, and VCS/agent state.
- Added `crates/pq-devnet-0/scripts/core/build-lean-rust.sh` with `.env` loading, `LEAN_RUST_IMAGE` override support, image-exists skip behavior, and `FORCE=1` rebuild support.
- Exposed the devnet ports required by the local-pq scaffold: `9000/udp`, `5052/tcp`, and `8080/tcp`.

## Verification

- `./crates/pq-devnet-0/scripts/core/build-lean-rust.sh`
- `./crates/pq-devnet-0/scripts/core/build-lean-rust.sh` skips when `lean-rust:local` already exists
- `FORCE=1 DOCKER_CHECK_TIMEOUT_SECONDS=30 ./crates/pq-devnet-0/scripts/core/build-lean-rust.sh`
- `docker run --rm lean-rust:local --help`
- `docker image inspect lean-rust:local --format '{{json .Config.ExposedPorts}}'`
- `bash -n crates/pq-devnet-0/scripts/core/build-lean-rust.sh`
- `git diff --check`
- `cargo test -p pq-devnet-0`
