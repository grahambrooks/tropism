# Convenience targets for cutting a release.
#
# The release pipeline is the standard release-kit v2 workflow
# (.github/workflows/release.yml + scripts/release.py, configured by .release.env),
# which is **tag-driven**: the tag is the version. It stamps the tag's version into
# the build, and after publishing lands the bump (and the regenerated Homebrew
# formula) back on main through a pull request. A committed version is what
# `tropism --version`, the formula, and crates.io all read, and this file is what
# keeps bumping it — including the internal dependency versions the workflow does
# not touch — from being a chore anyone has to remember the steps for.
#
#   make release        cut the next CalVer release
#   make release-dry    print exactly what `make release` would do, and stop
#   make version        print the next version
#   make check          what CI runs, locally
#
# CalVer is YYYY.M.MICRO, where MICRO counts the releases already cut this month.
# The month has no leading zero: 2026.08.1 is not valid SemVer, and cargo
# requires SemVer.

SHELL := /usr/bin/env bash
.SHELLFLAGS := -eu -o pipefail -c
.DEFAULT_GOAL := help

YEAR    := $(shell date -u +%Y)
MONTH   := $(shell date -u +%-m)
MICRO    = $(shell git tag -l 'v$(YEAR).$(MONTH).*' | wc -l | tr -d ' ')
VERSION  = $(YEAR).$(MONTH).$(MICRO)
TAG      = v$(VERSION)

.PHONY: help
help:
	@echo 'make release      cut $(TAG) — bump, commit, tag, push'
	@echo 'make release-dry  show what that would do, without doing it'
	@echo 'make version      print the next version'
	@echo 'make check        fmt, clippy, tests, and tropism on itself'
	@echo 'make check-scripts  evaluation/ shell scripts, against real bash 3.2'
	@echo 'make plan         what the release workflow would build for the next tag'
	@echo 'make alerts       Dependabot alerts, split demo fixtures from real ones'

.PHONY: version
version:
	@echo '$(VERSION)'

.PHONY: check
check:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace --all-targets
	cargo test --workspace --doc
	cargo build -p tropism --no-default-features
	cargo run --quiet -p tropism -- check
	$(MAKE) check-scripts

# The evaluation harness has to run on a stock macOS, which has shipped bash
# 3.2.57 as /bin/bash since 2007 and is not going to stop. `;;&` reached a user
# past a green shellcheck run because `bash -n a.sh b.sh` checks only the *first*
# file — hence the loop, and hence /bin/bash explicitly rather than $$(which bash).
.PHONY: check-scripts
check-scripts:
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
plan:
	python3 scripts/release.py plan '$(TAG)'

# Dependabot alerts cannot be filtered by path, and every manifest under demo/ is
# a deliberately-broken fixture, so the real count is buried unless it is split
# out. Read-only; pass --apply to the script itself to dismiss.
.PHONY: alerts
alerts:
	@./scripts/dismiss-demo-alerts.sh

# Refuse to cut a release from a tree that would produce a surprise: the wrong
# branch, uncommitted work, a stale local main, or a tag that already exists.
# Each of these has a different bad outcome and none of them is obvious after the
# fact, which is the entire reason this target exists rather than a wiki page.
.PHONY: release-guard
release-guard:
	@test "$$(git rev-parse --abbrev-ref HEAD)" = main \
		|| { echo 'release: not on main'; exit 1; }
	@test -z "$$(git status --porcelain)" \
		|| { echo 'release: working tree is dirty'; exit 1; }
	@git fetch --quiet origin main
	@test "$$(git rev-parse HEAD)" = "$$(git rev-parse origin/main)" \
		|| { echo 'release: local main differs from origin/main'; exit 1; }
	@! git rev-parse -q --verify 'refs/tags/$(TAG)' >/dev/null \
		|| { echo 'release: tag $(TAG) already exists'; exit 1; }

.PHONY: release-dry
release-dry:
	@echo 'would release $(VERSION), tagged $(TAG)'
	@echo '  1. set version = "$(VERSION)" in Cargo.toml'
	@echo '  2. refresh Cargo.lock'
	@echo '  3. commit "Release $(VERSION)"'
	@echo '  4. tag $(TAG)'
	@echo '  5. push main and $(TAG) — the tag is what starts the release'
	@echo
	@echo "currently: $$(grep -m1 '^version = ' Cargo.toml)"

.PHONY: release
release: release-guard check
	@echo '--> releasing $(VERSION)'
	# Only the workspace version line, anchored, so no dependency version is
	# touched by a stray match.
	perl -pi -e 's/^version = "[^"]*"$$/version = "$(VERSION)"/ if $$. < 20' Cargo.toml
	@grep -q '^version = "$(VERSION)"$$' Cargo.toml \
		|| { echo 'release: failed to set the version'; exit 1; }
	# The internal deps carry a `version` as well as a `path`, because
	# `cargo publish` refuses a path dependency that has no version requirement
	# and strips the `path` from what it uploads. They are the same CalVer as
	# the workspace, so they move with it — left behind, publish-crate.yml
	# would upload tropism-lang pinned to a tropism-core that predates it.
	perl -pi -e 's/^(tropism-(?:core|lang) = \{ path = "[^"]*", version = )"[^"]*"/$$1"$(VERSION)"/' Cargo.toml
	@test "$$(grep -c '^tropism-\(core\|lang\) = {.*version = "$(VERSION)" }$$' Cargo.toml)" = 2 \
		|| { echo 'release: failed to set the internal dependency versions'; exit 1; }
	# Cargo.lock records the workspace members' own versions, so it moves too.
	cargo update --workspace --offline
	git add Cargo.toml Cargo.lock
	# Nothing to commit when re-cutting a version whose build failed: the bump
	# already landed and only the tag needs replacing. Committing nothing is not
	# an error, so do not let `git commit` make it one.
	@git diff --cached --quiet \
		&& echo 'version already at $(VERSION); tagging the existing commit' \
		|| git commit --quiet -m 'Release $(VERSION)'
	git tag -a '$(TAG)' -m 'tropism $(VERSION)'
	git push --quiet origin main
	git push --quiet origin '$(TAG)'
	@echo
	@echo 'pushed $(TAG). The release workflow builds six targets and publishes:'
	@echo '  https://github.com/grahambrooks/tropism/actions/workflows/release.yml'
	@echo '  https://github.com/grahambrooks/tropism/releases/tag/$(TAG)'
	@echo
	@echo 'When it finishes, it opens and merges a PR regenerating Formula/tropism.rb'
	@echo 'from the published checksums. Nothing here bumps the formula: its version'
	@echo 'and its checksums have to land together, and the checksums do not exist yet.'
	@echo 'Remember to `git pull` before the next release — that PR lands on main.'
