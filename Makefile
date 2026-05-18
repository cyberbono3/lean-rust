.PHONY: verify build test lint fmt fmt-check clean help \
	devnet-build devnet-genesis devnet-up devnet-down devnet-stop \
	devnet-clean devnet-clean-check devnet-status devnet-start

CARGO ?= cargo
WORKSPACE_FLAGS := --workspace --all-targets
PQ_DEVNET_ROOT := crates/pq-devnet-0
PQ_DEVNET_CORE := $(PQ_DEVNET_ROOT)/scripts/core
PQ_DEVNET_COMPOSE_FILE := $(PQ_DEVNET_CORE)/docker-compose.yml
PQ_DEVNET_COMPOSE := docker compose -f $(PQ_DEVNET_COMPOSE_FILE) --project-directory $(PQ_DEVNET_ROOT)
PQ_DEVNET_DOWN := $(PQ_DEVNET_COMPOSE) down --remove-orphans

REAM_HEAD_URL ?= http://127.0.0.1:5052/lean/v0/head
LEAN_RUST_HEAD_URL ?= http://127.0.0.1:5053/lean/v0/head

help:
	@echo "lean-rust Makefile targets:"
	@echo "  make verify  - fmt --check + clippy + test (the canonical CI gate)"
	@echo "  make build   - cargo build --workspace"
	@echo "  make test    - cargo test --workspace"
	@echo "  make lint    - cargo clippy --workspace --all-targets -- -D warnings"
	@echo "  make fmt     - cargo fmt (apply)"
	@echo "  make fmt-check - cargo fmt --check"
	@echo "  make clean   - cargo clean"
	@echo ""
	@echo "local-pq devnet targets:"
	@echo "  make devnet-build   - build lean-rust:local"
	@echo "  make devnet-genesis - generate local-pq keys and genesis"
	@echo "  make devnet-up      - start ream + lean-rust containers"
	@echo "  make devnet-down    - stop containers, keep generated state"
	@echo "  make devnet-stop    - safe stop alias for devnet-down"
	@echo "  make devnet-clean   - remove containers, volumes, generated state, and logs"
	@echo "  make devnet-clean-check - verify devnet-clean removes generated state only"
	@echo "  make devnet-status  - probe both /lean/v0/head endpoints"
	@echo "  make devnet-start   - build + genesis + up"

verify: fmt-check lint test

build:
	$(CARGO) build --workspace

test:
	$(CARGO) test --workspace

lint:
	$(CARGO) clippy $(WORKSPACE_FLAGS) -- -D warnings

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clean:
	$(CARGO) clean

devnet-build:
	$(PQ_DEVNET_CORE)/build-lean-rust.sh

devnet-genesis:
	$(PQ_DEVNET_CORE)/setup-genesis.sh

devnet-up:
	$(PQ_DEVNET_COMPOSE) up -d

devnet-down:
	$(PQ_DEVNET_DOWN)

devnet-stop:
	@$(PQ_DEVNET_DOWN) 2>/dev/null || true

devnet-clean:
	$(PQ_DEVNET_CORE)/cleanup.sh

devnet-clean-check:
	+MAKE="$(MAKE)" $(PQ_DEVNET_CORE)/check-cleanup.sh

devnet-status:
	@REAM_HEAD_URL="$(REAM_HEAD_URL)" LEAN_RUST_HEAD_URL="$(LEAN_RUST_HEAD_URL)" $(PQ_DEVNET_CORE)/status.sh

devnet-start:
	$(MAKE) devnet-build
	$(MAKE) devnet-genesis
	$(MAKE) devnet-up
