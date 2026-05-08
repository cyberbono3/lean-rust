.PHONY: verify build test lint fmt fmt-check clean help

CARGO ?= cargo
WORKSPACE_FLAGS := --workspace --all-targets

help:
	@echo "lean-rust Makefile targets:"
	@echo "  make verify  - fmt --check + clippy + test (the canonical CI gate)"
	@echo "  make build   - cargo build --workspace"
	@echo "  make test    - cargo test --workspace"
	@echo "  make lint    - cargo clippy --workspace --all-targets -- -D warnings"
	@echo "  make fmt     - cargo fmt (apply)"
	@echo "  make fmt-check - cargo fmt --check"
	@echo "  make clean   - cargo clean"

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
