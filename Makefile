# theway — workspace Makefile
#
# Targets mirror the .github/workflows/ci.yml jobs so `make ci` locally is a faithful
# proxy for what main-branch pushes will see. `make help` lists everything.

CARGO ?= cargo
PYTHON ?= python3
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
build: ## cargo build --workspace (full CLI; daemon-only consumers use -p theway-daemon)
	$(CARGO) build --workspace --target-dir $(OUTPUT_DIR)
	cp $(THEWAY_BINARY) $(dir $(THEWAY_BINARY))tw$(EXE)

.PHONY: release
release: ## cargo build --release (optimized binary at (THEWAY_RELEASE_BINARY))
	$(CARGO) build --workspace --release --target-dir $(OUTPUT_DIR)

.PHONY: check
check: ## fast type-check without producing artifacts (full workspace incl. TUI crate)
	$(CARGO) check --workspace --all-targets

# --- tests ------------------------------------------------------------------

.PHONY: test
test: ## run every workspace test (full CLI surface)
	$(CARGO) test --workspace

.PHONY: test-coding-agent
test-coding-agent: ## run only the daemon crate's tests
	$(CARGO) test -p theway-daemon

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
	$(CARGO) clippy --workspace --all-targets -- -D warnings

.PHONY: feature-gate
feature-gate: ## feature-gate checks: thin core/provider builds + daemon backend matrix
	$(CARGO) check -p theway-core --no-default-features
	$(CARGO) check -p theway-core --no-default-features --features harness
	$(CARGO) check -p theway-llm-provider --no-default-features
	$(CARGO) check -p theway-llm-provider --no-default-features --features amazon-bedrock
	$(CARGO) check -p theway-llm-provider --all-features
	$(CARGO) check -p theway-daemon --all-targets
	$(CARGO) check -p theway-daemon --no-default-features --features sandbox --all-targets
	$(CARGO) check -p theway-daemon --all-features --all-targets
	$(CARGO) test -p theway-daemon --no-default-features --features sandbox --test sandbox_tool_gate

.PHONY: file-size-check
file-size-check: ## enforce the 800-line limit for all theway-* Rust files
	scripts/check-rust-file-size.sh

.PHONY: layering-check
layering-check: ## enforce workspace crate dependency boundaries
	scripts/check-workspace-layering.py

.PHONY: package-check
package-check: ## verify the probe's standalone crates.io package builds
	$(CARGO) publish --registry crates-io -p theway-probe --dry-run --allow-dirty

.PHONY: i18n-check
i18n-check: ## verify English/Chinese documentation pairs and recorded hashes
	scripts/verify-doc-i18n.py

.PHONY: i18n-test
i18n-test: ## test the bilingual documentation verifier
	$(PYTHON) -B -m unittest discover -s scripts/tests -p 'test_*.py'

.PHONY: doc-sync
doc-sync: i18n-test i18n-check ## verify bilingual documentation synchronization

.PHONY: ci
ci: fmt-check file-size-check layering-check package-check doc-sync sdks-check lint feature-gate test ## run the full CI pipeline locally

# --- run / install ----------------------------------------------------------

.PHONY: run
run: ## build + run theway in dev mode (interactive)
	$(CARGO) run --workspace -p theway-tui --bin theway

.PHONY: install
install: ## install theway + tw into $(CARGO_HOME)/bin (via scripts/install.sh)
	scripts/install.sh

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

# --- SDKs -------------------------------------------------------------------

.PHONY: hooks
hooks: ## enable the repo git hooks (transport proto change -> auto client SDK regen)
	git config core.hooksPath .githooks

.PHONY: sdk-sync
sdk-sync: ## sync transport proto -> sdks/client/proto + regenerate the TypeScript client
	bash scripts/sdk-sync.sh

.PHONY: sdk-publish
sdk-publish: ## publish the client SDK (BUMP=patch|minor|major, ISSUE="#n")
	bash scripts/sdk-publish.sh $(BUMP) $(ISSUE)

.PHONY: plugin-sdk-check
plugin-sdk-check: ## build, typecheck examples, and test the plugin development SDK
	@test -d sdks/plugin/node_modules || npm --prefix sdks/plugin install --no-audit --no-fund --loglevel=error
	npm --prefix sdks/plugin run check

.PHONY: sdks-check
sdks-check: sdk-sync ## sync/build the client SDK and check the plugin development SDK
	npm --prefix sdks/client run build
	$(MAKE) plugin-sdk-check

# --- release helpers --------------------------------------------------------

.PHONY: changelog
changelog: ## print the unreleased changelog section
	@awk '/^## \[Unreleased\]/,/^## \[/{ if (!/^## \[/||$$0 ~ /Unreleased/) print }' CHANGELOG.md

.PHONY: version
version: ## print the workspace version
	@grep -E '^version' Cargo.toml | head -1 | sed -E 's/version = "([^"]+)"/\1/'
