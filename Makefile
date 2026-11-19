# Chores for this workspace. `make help` lists every target.
#
# Each target is a thin call to cargo, `cargo xtask` or a binary built here. The logic lives in
# Rust (xtask/), so this file stays a menu. Written for the GNU make 3.81 that macOS ships.

SHELL := /bin/bash
.DEFAULT_GOAL := help

R := ./target/release

# Variables a target reads (set them on the command line, e.g. `make run FILE=notes.md`).
FILE   ?=
TARGET ?=
TIME   ?=

.PHONY: help build doctor run fmt lint test verify audit fuzz clean

## Getting started

help: ## List every target with a one-sentence description.
	@awk 'BEGIN {FS = ":.*## "} /^## / {printf "\n\033[1m%s\033[0m\n", substr($$0, 4)} /^[a-z_]+:.*## / {printf "  \033[36m%-8s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)
	@printf '\nVariables: FILE (for run), TARGET and TIME (for fuzz).\n'

build: ## Build the release binaries (launcher and window) into target/release.
	cargo build --release

doctor: ## Check the tools the build and the checks need are installed, showing versions to compare with CONTRIBUTING.md.
	@ok=1; for t in cargo rustc rustup; do \
	  if command -v $$t >/dev/null; then printf '  ok       %s\n' "$$t"; else printf '  MISSING  %s\n' "$$t"; ok=0; fi; done; \
	for t in cargo-deny cargo-fuzz cargo-llvm-cov typos zizmor reuse docker; do \
	  if command -v $$t >/dev/null; then printf '  ok       %-14s %s\n' "$$t" "$$($$t --version 2>/dev/null | head -1)"; \
	  else printf '  optional %s (not installed)\n' "$$t"; fi; done; \
	printf '  rustc    %s\n' "$$(rustc --version)"; \
	nightly="$$(sed -n 's/^channel = "\(.*\)"/\1/p' fuzz/rust-toolchain.toml)"; \
	if rustup toolchain list | grep -q "^$$nightly"; then printf '  ok       %s (for fuzz)\n' "$$nightly"; \
	else printf '  optional %s (for fuzz, not installed)\n' "$$nightly"; fi; \
	test $$ok = 1

run: build ## Run the release build from target/release without installing it, e.g. `make run FILE=notes.md`.
	$(R)/launcher $(FILE)

## Quality

fmt: ## Format every crate with rustfmt.
	cargo fmt --all

lint: ## Run clippy over the workspace with warnings as errors, as `cargo xtask ci` does.
	cargo clippy --workspace --all-targets -- -D warnings

test: ## Run the workspace tests.
	cargo test --workspace

verify: ## Run the full gate CI runs: fmt check, clippy, tests and the xtask lints (`cargo xtask ci`).
	cargo xtask ci

audit: ## Check dependencies for advisories, licences and sources with cargo deny, as CI does.
	cargo deny check

fuzz: ## Fuzz one target from its seeds, e.g. `make fuzz TARGET=ex_command TIME=60` (pinned nightly and cargo-fuzz).
	@test -n "$(TARGET)" || { echo "make fuzz: give TARGET=<name>, one of: $$(ls fuzz/fuzz_targets | sed 's/\.rs$$//' | tr '\n' ' ')"; exit 2; }
	@test -f "fuzz/fuzz_targets/$(TARGET).rs" || { echo "make fuzz: no target $(TARGET) in fuzz/fuzz_targets"; exit 2; }
	@# New inputs go to a scratch corpus first, so the committed seeds stay as they are.
	scratch="$${TMPDIR:-/tmp}/corpus-$(TARGET)"; mkdir -p "$$scratch"; \
	max=$$(test "$(TARGET)" = editor_keys && echo 4096 || echo 65536); \
	cd fuzz && cargo fuzz run $(TARGET) "$$scratch" corpus/$(TARGET) -- \
	  -max_len=$$max -timeout=10 $(if $(TIME),-max_total_time=$(TIME))

## Housekeeping

clean: ## Remove cargo's build output (target/).
	cargo clean
