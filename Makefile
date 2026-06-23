.DEFAULT_GOAL := help
.PHONY: help build release run test fmt fmt-check lint audit deny check ci install clean

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

build: ## Debug build
	cargo build

release: ## Optimized release build
	cargo build --release

run: ## Run the bridge (default binary)
	cargo run --release

test: ## Run the spec + native test suite
	cargo test

fmt: ## Format the code
	cargo fmt --all

fmt-check: ## Check formatting (CI gate)
	cargo fmt --all -- --check

lint: ## Clippy with warnings denied
	cargo clippy --all-targets --all-features -- -D warnings

audit: ## Advisory scan
	cargo audit

deny: ## cargo-deny: advisories, bans, licenses, sources
	cargo deny check advisories bans licenses sources

check: fmt-check lint test ## Fast pre-commit gate (format, lint, test)

ci: fmt-check lint test release audit deny ## Everything CI runs

install: ## Install the binaries from this checkout
	cargo install --path .

clean: ## Remove build artifacts
	cargo clean
