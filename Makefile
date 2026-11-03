# Developer targets.
#
#   make check          the pre-push gate: exactly the CI `check` job
#   make check-all      check, plus the other CI gates that run locally
#   make hooks          install the prek git hooks (prek.toml)
#   make release        tag origin/main with the next CalVer and push the tag
#   make release-dry    print what `make release` would do, and stop
#
# Releases: release.mk (release-kit) computes the next CalVer — today's UTC date,
# vYYYY.M.D, incremented while that tag exists — tags origin/main and pushes only
# the tag. The release-kit v2 workflow (.github/workflows/release.yml +
# scripts/release.py, configured by .release.env) stamps that version into
# [workspace.package], which every crate inherits, builds and publishes, and lands
# the bump (and the regenerated Homebrew formula) back on main through a PR.
# Nothing is bumped or committed locally.

SHELL := /usr/bin/env bash
.SHELLFLAGS := -eu -o pipefail -c
.DEFAULT_GOAL := help

.PHONY: help
help: ## List targets
	@grep -hE '^[a-zA-Z0-9_-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{ printf "  %-18s %s\n", $$1, $$2 }'

.PHONY: check
check: fmt-check lint test ## The pre-push gate: the CI check job (fmt, clippy, tests, doc-tests)

# What the old `make check` covered beyond the CI check job: the no-default-features
# build (CI's features job), tropism on itself (the dogfood job; the pre-push hook
# also runs `tropism analyze`), and the evaluation scripts.
.PHONY: check-all
check-all: check check-scripts ## check, plus the features build, dogfood and script checks
	cargo build -p tropism --no-default-features --locked
	cargo run --quiet -p tropism -- check

.PHONY: fmt fmt-check lint test hooks
fmt: ## Format the code
	cargo fmt --all

fmt-check: ## Fail if the code is not formatted
	cargo fmt --all --check

lint: ## Clippy with warnings denied, as CI runs it
	cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

# Doc-tests are excluded from --all-targets and would otherwise never run.
test: ## Tests and doc-tests, as CI runs them
	cargo test --workspace --all-targets --all-features --locked
	cargo test --workspace --doc --all-features --locked

hooks: ## Install the prek git hooks (pre-commit and pre-push)
	prek install

.PHONY: version
version: release-version ## Alias for release-version

.PHONY: check-scripts
check-scripts: ## evaluation/ shell scripts, against real bash 3.2
	@for f in evaluation/*.sh; do \
		/bin/bash -n "$$f" || { echo "FAILED under bash 3.2: $$f" >&2; exit 1; }; \
	done
	@if command -v shellcheck >/dev/null 2>&1; then \
		shellcheck -x -P evaluation evaluation/*.sh || exit 1; \
	else \
		echo 'shellcheck not installed; syntax checked only'; \
	fi
	@python3 -m py_compile evaluation/report.py && rm -rf evaluation/__pycache__
	@echo 'evaluation scripts OK'

.PHONY: plan
plan: ## What the release workflow would build for the next tag
	python3 scripts/release.py plan "v$$($(MAKE) --no-print-directory -s release-version)"

# Dependabot alerts cannot be filtered by path, and every manifest under demo/ is
# a deliberately-broken fixture, so the real count is buried unless it is split
# out. Read-only; pass --apply to the script itself to dismiss.
.PHONY: alerts
alerts: ## Dependabot alerts, split demo fixtures from real ones
	@./scripts/dismiss-demo-alerts.sh

.PHONY: watch-ci
watch-ci: ## After pushing, wait for every CI run on HEAD
	@command -v watch-ci >/dev/null || { echo "watch-ci not on PATH: ln -s ~/dev/projects/agentic-twin/scripts/watch-ci.sh ~/.local/bin/watch-ci" >&2; exit 1; }
	watch-ci

include release.mk
