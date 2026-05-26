FROM rust:1.85-bookworm AS builder

WORKDIR /workspace
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build -p beacon --release

FROM debian:bookworm-slim AS runtime

ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /workspace/target/release/lean-beacon /usr/local/bin/lean-beacon

EXPOSE 9000/udp
EXPOSE 5052/tcp
EXPOSE 8080/tcp

ENTRYPOINT ["/usr/local/bin/lean-beacon"]
