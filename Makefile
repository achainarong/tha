.DEFAULT_GOAL := help
.PHONY: help build release run demo install uninstall fmt lint test check clean

help: ## Show available commands
	@awk 'BEGIN {FS = ":.*## "} /^[a-z-]+:.*## / {printf "  %-14s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

build: ## Build a debug binary
	cargo build --locked

release: ## Build an optimized binary
	cargo build --release --locked

run: ## Run Tha (use ARGS="..." for CLI options)
	cargo run --locked -- $(ARGS)

demo: ## Explore fixture workloads without a running engine
	cargo run --locked -- --demo

MODE ?= standalone

install: ## Install tha into Cargo's bin directory (MODE=standalone|service, no sudo)
	@case "$(MODE)" in standalone|service) ;; *) echo "MODE must be standalone or service" >&2; exit 1 ;; esac
	cargo install --path . --locked
	@if [ "$(MODE)" = "service" ]; then "$${CARGO_HOME:-$$HOME/.cargo}/bin/tha" service install && "$${CARGO_HOME:-$$HOME/.cargo}/bin/tha" service start; fi

uninstall: ## Uninstall tha using Cargo
	cargo uninstall tha

fmt: ## Format Rust source
	cargo fmt --all

lint: ## Run Clippy with warnings denied
	cargo clippy --locked --all-targets -- -D warnings

test: ## Run all tests
	cargo test --locked --all-targets

check: ## Check formatting, lint, and tests
	cargo fmt --all -- --check
	$(MAKE) lint
	$(MAKE) test

clean: ## Remove Cargo build artifacts
	cargo clean
