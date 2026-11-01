.DEFAULT_GOAL := help
.PHONY: help build release run demo install install\:full install\:agent uninstall fmt lint test check clean

# Locally, tests and lint leave the rest of the machine free. Override with: make check CPUS=8
# CI runners set CI, and there the job can use the whole machine.
ifeq ($(CI),)
CPUS ?= 4
# Pinning covers everything the tests start, but needs Linux and those cores to be available.
PIN := $(shell taskset -c 0-$$(($(CPUS) - 1)) true 2>/dev/null && echo taskset -c 0-$$(($(CPUS) - 1)))
LIMIT := CARGO_BUILD_JOBS=$(CPUS) RUST_TEST_THREADS=$(CPUS) $(PIN)
endif

help: ## Show available commands
	@awk '/^[a-z][a-z\\:-]*:.*## / {name = $$0; sub(/: .*/, "", name); gsub(/\\/, "", name); printf "  %-14s %s\n", name, substr($$0, index($$0, "## ") + 3)}' $(MAKEFILE_LIST)

build: ## Build a debug binary
	cargo build --locked

RELEASE_TAG := $(filter v%,$(MAKECMDGOALS))
ifneq ($(filter release,$(MAKECMDGOALS)),)
ifneq ($(RELEASE_TAG),)
ifneq ($(words $(MAKECMDGOALS)),2)
$(error Use exactly: make release vX.Y.Z)
endif
.PHONY: $(RELEASE_TAG)
$(RELEASE_TAG):
	@:
endif
endif

release: ## Build locally; add vX.Y.Z to commit all changes and publish a release
ifneq ($(RELEASE_TAG),)
	python3 scripts/release.py "$(RELEASE_TAG)"
else
	cargo build --release --locked
endif

run: ## Run Tha (use ARGS="..." for CLI options)
	cargo run --locked -- $(ARGS)

demo: ## Explore fixture workloads without a running engine
	cargo run --locked -- --demo

MODE ?= standalone

install: ## Install tha into Cargo's bin directory (MODE=standalone|service, no sudo)
	@case "$(MODE)" in standalone|service) ;; *) echo "MODE must be standalone or service" >&2; exit 1 ;; esac
	cargo install --path . --locked --bin tha
	@# Replace any existing service (e.g. one from the curl installer) so it runs this build.
	@if [ "$(MODE)" = "service" ]; then \
		tha="$${CARGO_HOME:-$$HOME/.cargo}/bin/tha"; \
		"$$tha" service uninstall && "$$tha" service install && "$$tha" service start; \
	fi

install\:full: ## Install tha in service mode with the security module enabled
	$(MAKE) install MODE=service
	"$${CARGO_HOME:-$$HOME/.cargo}/bin/tha" module install security
	"$${CARGO_HOME:-$$HOME/.cargo}/bin/tha" module enable security

install\:agent: ## Install only tha-agent into /usr/local/bin for a server (uses sudo)
	cargo build --release --locked --bin tha-agent
	sudo install -m 755 -o root target/release/tha-agent /usr/local/bin/tha-agent

uninstall: ## Uninstall tha using Cargo
	cargo uninstall tha

fmt: ## Format Rust source
	cargo fmt --all

lint: ## Run Clippy with warnings denied
	$(LIMIT) cargo clippy --locked --all-targets -- -D warnings

test: ## Run all tests (locally on at most CPUS cores, default 4)
	$(LIMIT) cargo test --locked --all-targets

check: ## Check formatting, lint, and tests
	cargo fmt --all -- --check
	$(MAKE) lint
	$(MAKE) test

clean: ## Remove Cargo build artifacts
	cargo clean
