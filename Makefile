# theway — workspace Makefile
#
# Targets mirror the .github/workflows/ci.yml jobs so `make ci` locally is a faithful
# proxy for what main-branch pushes will see. `make help` lists everything.

CARGO ?= cargo
BIN   ?= theway
OUTPUT_DIR ?= output
THEWAY_BINARY ?= $(OUTPUT_DIR)/debug/$(BIN)
THEWAY_RELEASE_BINARY ?= $(OUTPUT_DIR)/release/$(BIN)

.DEFAULT_GOAL := help

# --- discovery --------------------------------------------------------------

.PHONY: help
help: ## show this help
	@awk 'BEGIN {FS = ":.*?## "} /^[a-zA-Z_-]+:.*?## / {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

# --- build ------------------------------------------------------------------

.PHONY: build
build: ## cargo build --workspace --features tui,server (full CLI; SDK-only consumers need neither)
	$(CARGO) build --workspace --features tui,server --target-dir $(OUTPUT_DIR)
	cp $(THEWAY_BINARY) $(dir $(THEWAY_BINARY))tw$(EXE)

.PHONY: release
release: ## cargo build --release --features tui,server (optimized binary at (THEWAY_RELEASE_BINARY))
	$(CARGO) build --workspace --release --features tui,server --target-dir $(OUTPUT_DIR)

.PHONY: check
check: ## fast type-check without producing artifacts (full workspace incl. tui,server)
	$(CARGO) check --workspace --all-targets --features tui,server

# --- tests ------------------------------------------------------------------

.PHONY: test
test: ## run every workspace test (full CLI surface)
	$(CARGO) test --workspace --features tui,server

.PHONY: test-coding-agent
test-coding-agent: ## run only the harness crate's tests
	$(CARGO) test -p theway

.PHONY: test-agent
test-agent: ## run only the harness-core crate's tests
	$(CARGO) test -p theway-core

.PHONY: test-ai
test-ai: ## run only the theway-llm-provider crate's tests
	$(CARGO) test -p theway-llm-provider

.PHONY: test-mcp
test-mcp: ## run only the MCP client tests
	$(CARGO) test -p theway-mcp

# --- quality gates ----------------------------------------------------------

.PHONY: fmt
fmt: ## rewrite files via rustfmt
	$(CARGO) fmt --all

.PHONY: fmt-check
fmt-check: ## rustfmt --check (CI uses this)
	$(CARGO) fmt --all --check

.PHONY: lint
lint: ## clippy with -D warnings (matches CI)
	$(CARGO) clippy --workspace --all-targets --features tui,server -- -D warnings

.PHONY: ci
ci: fmt-check lint test ## run the full CI pipeline locally

# --- run / install ----------------------------------------------------------

.PHONY: run
run: ## build + run theway in dev mode (interactive)
	$(CARGO) run --workspace --features tui,server -p theway

.PHONY: install
install: ## install theway binary into ~/.cargo/bin
	$(CARGO) install --path crates/app --features tui,server --force
	cp ~/.cargo/bin/theway$(EXE) ~/.cargo/bin/tw$(EXE)

# --- docs / housekeeping ----------------------------------------------------

.PHONY: doc
doc: ## build rustdoc for the workspace (private items included)
	$(CARGO) doc --workspace --no-deps --document-private-items

.PHONY: doc-open
doc-open: doc ## build + open rustdoc in browser
	$(CARGO) doc --workspace --no-deps --document-private-items --open

.PHONY: clean
clean: ## cargo clean
	$(CARGO) clean

.PHONY: outdated
outdated: ## list outdated workspace deps (requires `cargo-outdated`)
	$(CARGO) outdated --workspace --root-deps-only

# --- release helpers --------------------------------------------------------

.PHONY: changelog
changelog: ## print the unreleased changelog section
	@awk '/^## \[Unreleased\]/,/^## \[/{ if (!/^## \[/||$$0 ~ /Unreleased/) print }' CHANGELOG.md

.PHONY: version
version: ## print the workspace version
	@grep -E '^version' Cargo.toml | head -1 | sed -E 's/version = "([^"]+)"/\1/'
