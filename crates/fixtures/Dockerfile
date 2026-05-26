FROM rust:1.85-bookworm AS builder

WORKDIR /workspace
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY bin ./bin

RUN cargo build -p lean-beacon --release

FROM debian:bookworm-slim AS runtime

ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 lean \
    && useradd --system --uid 10001 --gid lean --home-dir /home/lean --create-home lean

COPY --from=builder --chown=lean:lean /workspace/target/release/lean-beacon /usr/local/bin/lean-beacon

# Pre-create writable mount points so volume-backed dirs land with lean
# ownership (the compose mounts ./logs:/var/lean-logs and ./config / ./genesis
# read-only, but /data is volume-backed and needs to be writable).
RUN mkdir -p /var/lean-logs /data && chown lean:lean /var/lean-logs /data

USER lean:lean

EXPOSE 9000/udp
EXPOSE 5052/tcp
EXPOSE 8080/tcp

ENTRYPOINT ["/usr/local/bin/lean-beacon"]
