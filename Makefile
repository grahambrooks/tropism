# Convenience targets for cutting a release.
#
# The release pipeline is `dist` (cargo-dist), which is **tag-driven**: it reads
# the version from Cargo.toml and expects a matching tag. That is the one thing
# this repository used to do differently — the version was permanently 0.0.0 and
# CalVer was computed at release time and injected, never committed, to avoid a
# push loop and release-bump noise in the history.
#
# Adoption won that argument. A committed version is what every installer, every
# package registry, and `tropism --version` all read, and it is what dist needs to
# match a tag against. So the version now lives in Cargo.toml, and this file is
# what keeps bumping it from being a chore anyone has to remember the steps for.
#
#   make release        cut the next CalVer release
#   make release-dry    print exactly what `make release` would do, and stop
#   make version        print the next version
#   make check          what CI runs, locally
#
# CalVer is YYYY.M.MICRO, where MICRO counts the releases already cut this month.
# The month has no leading zero: 2026.08.1 is not valid SemVer, and dist, cargo,
# and npm all require SemVer.

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
	@echo 'make plan         what dist would build for the current version'
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

.PHONY: plan
plan:
	dist plan

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
