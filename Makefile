.DEFAULT_GOAL := help
.PHONY: help build release run demo install install\:full uninstall fmt lint test check clean

help: ## Show available commands
	@awk '/^[a-z][a-z\\:-]*:.*## / {name = $$0; sub(/: .*/, "", name); gsub(/\\/, "", name); printf "  %-14s %s\n", name, substr($$0, index($$0, "## ") + 3)}' $(MAKEFILE_LIST)

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
	@# Replace any existing service (e.g. one from the curl installer) so it runs this build.
	@if [ "$(MODE)" = "service" ]; then \
		tha="$${CARGO_HOME:-$$HOME/.cargo}/bin/tha"; \
		"$$tha" service uninstall && "$$tha" service install && "$$tha" service start; \
	fi

install\:full: ## Install tha in service mode with the security module enabled
	$(MAKE) install MODE=service
	"$${CARGO_HOME:-$$HOME/.cargo}/bin/tha" module install security
	"$${CARGO_HOME:-$$HOME/.cargo}/bin/tha" module enable security

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
