# 13 — Build and release

GitHub Actions, continuous release, CalVer versioning, binary downloads, and crates.io publishing.

A dependency analyzer's own supply chain should model the practices it advocates, so this document
is more careful than the size of the project strictly warrants: reproducible builds, checksums,
build provenance, and an explicit statement of what is irreversible.

**Implemented**, in `.github/workflows/ci.yml` and `.github/workflows/release.yml`. These are live:
CI runs on every push and pull request, and a green `main` cuts a release.

**The crates.io publish is deliberately not continuous.** It runs only on a manual
`workflow_dispatch` with `publish_crate: true`, because a published version can be yanked but never
deleted or reused. Binaries are replaceable; a registry version is permanent. §6 explains how to
flip it and what that costs.

Two things must be settled before the first successful release — see §10.

---

## 1. The name — resolved

The project was renamed from **gdep** to **tropism** on 2026-07-31, and the crate-name question that
blocked this document is closed.

`gdep` could not be published: crates.io already has an unrelated live crate of that name
("Git-deploy — Easily deploy & auto-update apps", ~3,400 downloads, published 2025-02-10). The
fallback would have been to publish as `gdep-cli` with the binary still called `gdep`, which leaves a
permanent footgun — `cargo install gdep` installing a stranger's tool.

Renaming removed the problem entirely rather than documenting around it. Availability verified on
2026-07-31:

| Name           | crates.io | npm       |
| -------------- | --------- | --------- |
| `tropism`      | available | available |
| `tropism-core` | available | —         |
| `tropism-lang` | available | —         |
| `tropism`      | available | —         |
| `tropism-mcp`  | available | —         |

So the crate, the binary, and the command are all simply `tropism`. No `-cli` suffix, no install
footgun, nothing to warn users about.

**Re-verify before the first publish.** These names were free on 2026-07-31 and nothing reserves
them; a placeholder crate can appear at any time, and the same search turned up three
architecture-linter placeholders registered within four months (`depguard`, `dowel`, `archward`).

Where the name comes from is recorded in the [README](../README.md); it matters here only in that
`tropism` is short enough to type, unambiguous when spoken, and free everywhere it needs to be.

## 2. Versioning: CalVer

**Format: `YYYY.MM.MICRO`**, e.g. `2026.7.0`, `2026.7.1`, `2026.8.0`.

- `MICRO` counts releases within the calendar month and resets to `0` each month.
- **No leading zeros.** `2026.07.1` is not a valid semver version and crates.io rejects it, so July
  is `7`, not `07`.

### The interaction with Cargo's semver, which matters

Cargo parses `2026.7.1` as major `2026`, minor `7`, patch `1`. A dependency written
`tropism = "2026.7.1"` therefore resolves to `>=2026.7.1, <2027.0.0`.

So Cargo treats **every release within a calendar year as compatible** — including genuinely
breaking ones — and treats the January release as breaking even when nothing changed. CalVer carries
no compatibility information, and layering it onto a field Cargo reads as compatibility is actively
misleading.

**This is why only the binary crate is published.** Nobody writes `tropism = "…"` in a
`Cargo.toml`, so nobody is exposed to the mismatch. The libraries stay `publish = false` until there
is a real library consumer, at which point they get their own SemVer line independent of the CLI's
CalVer.

That also keeps a promise the project has not yet had to make: publishing `tropism-core` commits it to
a public API, and its API is still moving — four language slices forced four trait changes
([12-known-limitations.md](12-known-limitations.md)).

### What consumers should actually version against

The JSON `schema_version` in [05-interfaces.md](05-interfaces.md) is the compatibility contract for
anyone parsing tropism's output. It is independent of the release version, increments only on a
breaking change to the report shape, and is the number a downstream tool should check. The README
and `--help` should say so.

### Where the version lives

The repository keeps `version = "0.0.0"` in `Cargo.toml`. The release workflow computes the CalVer
version and injects it before building and publishing; it is **not** committed back to `main`.

- No commit-back means no push loop and no release-bump noise in the history.
- `cargo publish` needs `--allow-dirty`, because `Cargo.toml` and `Cargo.lock` are modified in the
  working tree. That is expected, and the tag records exactly which commit was released.

---

## 3. What gets released

| Artifact                        | Where           | Cadence            |
| ------------------------------- | --------------- | ------------------ |
| `tropism` binaries, six targets | GitHub Releases | every green `main` |
| `SHA256SUMS` + build provenance | GitHub Releases | every green `main` |
| `tropism` crate                 | crates.io       | guarded — see §6   |
| `tropism-core`, `tropism-lang`  | nowhere         | `publish = false`  |

---

## 4. CI workflow (`.github/workflows/ci.yml`)

Runs on pull requests and pushes to `main`. Release is gated on it passing.

Action versions were verified against the GitHub API on 2026-07-31 and are current:
`actions/checkout@v7`, `actions/upload-artifact@v7`, `actions/download-artifact@v8`,
`actions/attest-build-provenance@v4`, `Swatinem/rust-cache@v2`, `softprops/action-gh-release@v3`.
`dtolnay/rust-toolchain` is pinned by channel (`@stable`, `@1.97`) rather than by version, which is
how that action is meant to be used — its only tag is a `v1` from 2022.

```yaml
jobs:
  check:
    strategy:
      matrix:
        os: [ubuntu-24.04, macos-14, windows-2022]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
      # The TUI must stay optional; this is the only thing that proves it.
      - run: cargo build -p tropism --no-default-features

  msrv:
    steps:
      - uses: dtolnay/rust-toolchain@1.97     # matches rust-version
      - run: cargo check --workspace

  dogfood:
    steps:
      - run: cargo build -p tropism
      # tropism gates its own merges on its own ruleset.
      - run: ./target/debug/tropism analyze . --fail-on error
```

`clippy -D warnings` matches the standard the repository already holds itself to.

### The dogfood gate

`tropism.toml` declares that the CLI and MCP server must stay independent, that core is a leaf, and
that the tree-sitter grammars stay inside `tropism-lang`. Running tropism against itself in CI is what
makes those rules load-bearing rather than decorative, and it is the most on-brand job in the
workflow.

It works because `tropism.toml` also carries `exclude` patterns for `demo/**` and
`**/tests/fixtures/**` — directories full of deliberately-broken sample projects, which would
otherwise keep the exit code permanently non-zero. Verified: `tropism analyze . --fail-on error` exits
0, with 58 excluded paths disclosed in the report and 17 findings remaining, all of them genuine
`Cargo.lock` duplicates below error severity.

The gate must never be narrowed to make it pass. Scanning one crate at a time would exit 0 and mean
nothing: a rule like "the CLI must not depend on the MCP server" spans two crates, so narrowing the
scan removes the very edges it exists to check. Two tests guard this —
`tropism_passes_its_own_ci_gate` and `tropism_excludes_its_demos_and_fixtures` in
`crates/tropism-lang/tests/demos.rs`, the second asserting that no exclusion is stale.

---

## 5. Binary downloads

### Targets

| Target                      | Runner             | Notes                      |
| --------------------------- | ------------------ | -------------------------- |
| `x86_64-unknown-linux-gnu`  | `ubuntu-24.04`     |                            |
| `x86_64-unknown-linux-musl` | `ubuntu-24.04`     | static; needs `musl-tools` |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | native ARM runner          |
| `x86_64-apple-darwin`       | `macos-13`         | last x86 macOS runner      |
| `aarch64-apple-darwin`      | `macos-14`         |                            |
| `x86_64-pc-windows-msvc`    | `windows-2022`     |                            |

The musl build matters more than usual here. tropism's proposition is analysis on a bare checkout with
nothing installed; a static binary that runs in a scratch container is the same claim at the
distribution layer.

### The constraint that shapes this: tropism is not a pure-Rust binary

**Every tree-sitter grammar compiles C.** `tree-sitter`, `tree-sitter-go`, `-javascript`,
`-typescript`, `-rust`, and `-c-sharp` all build C sources through `cc` at compile time.

So cross-compilation needs a cross **C** toolchain, not merely a Rust target — `cargo build --target
aarch64-unknown-linux-gnu` on an x86 runner fails at the C stage without one. Two consequences:

- **Build each target on a native runner** where one exists. GitHub now offers `ubuntu-24.04-arm`
  and both macOS architectures, so only musl needs special handling.
- **musl needs `musl-tools`** and `CC_x86_64_unknown_linux_musl=musl-gcc` in the environment.

`cargo-dist` would generate most of this workflow automatically and is worth considering, but it
assumes SemVer-shaped versions and would fight the CalVer scheme. Hand-rolling a six-entry matrix is
roughly sixty lines and keeps version computation under our control.

### Packaging

- `tropism-<version>-<target>.tar.gz` for Unix, `.zip` for Windows, each containing the binary plus
  `README.md` and `LICENSE`.
- A single `SHA256SUMS` covering every archive.
- `actions/attest-build-provenance` for build provenance, so a downloaded binary can be traced to
  the workflow run and commit that produced it. A supply-chain tool should be verifiable itself.

### Install paths to document

```sh
cargo install tropism                       # installs a `tropism` binary
```

A `curl | sh` installer is deliberately **not** recommended here. It is convenient and it asks users
to pipe a network response into a shell, which is exactly the posture this tool exists to avoid
requiring. If one is added later it should verify `SHA256SUMS` before executing anything.

---

## 6. Continuous release, and the one place it is dangerous

**Binaries: fully continuous.** Every green `main` computes the next CalVer version, builds the
matrix, and publishes a GitHub Release. Cheap, and a bad release can be deleted and re-cut.

**crates.io: continuous but guarded.** A crates.io publish is **irreversible** — a version can be
yanked but never deleted, and a version number can never be reused. Publishing on every commit
permanently burns a version per commit and fills the registry with noise nobody asked for.

The guard:

```yaml
publish:
  needs: [release]
  if: github.ref == 'refs/heads/main'
  steps:
    # Idempotent: never fails a run because the version already exists.
    - name: Skip if already published
      run: |
        if cargo info "tropism@${VERSION}" >/dev/null 2>&1; then
          echo "already published"; exit 0
        fi
    - run: cargo publish -p tropism --allow-dirty
```

**Recommended default: publish to crates.io on a tag or a manual `workflow_dispatch`, not on every
commit.** Binaries stay continuous. The asymmetry is deliberate and is the whole reason to separate
the jobs: one artifact is replaceable and the other is permanent. Changing this is one `if:`
condition, and the cost of getting it wrong is a registry you cannot clean up.

### Credentials

Prefer crates.io **Trusted Publishing** (OIDC from GitHub Actions) if it is available for this
account — it removes the long-lived token entirely. Confirm current support before relying on it;
otherwise use a `CARGO_REGISTRY_TOKEN` secret scoped to publish-only, in a GitHub Environment with
required reviewers so a publish cannot happen without a human.

---

## 7. Release workflow sketch (`.github/workflows/release.yml`)

```yaml
on:
  workflow_run:
    workflows: [ci]
    types: [completed]
    branches: [main]

jobs:
  version:
    if: github.event.workflow_run.conclusion == 'success'
    outputs: { version: "${{ steps.calver.outputs.version }}" }
    steps:
      - uses: actions/checkout@v4
        with: { fetch-depth: 0 }        # tags are needed to count the micro
      - id: calver
        run: |
          YEAR=$(date -u +%Y); MONTH=$(date -u +%-m)   # %-m strips the leading zero
          MICRO=$(git tag -l "v${YEAR}.${MONTH}.*" | wc -l | tr -d ' ')
          echo "version=${YEAR}.${MONTH}.${MICRO}" >> "$GITHUB_OUTPUT"

  build:
    needs: version
    strategy: { matrix: { include: [ ... six targets ... ] } }
    steps:
      - run: |
          # Inject the version; never committed back to main.
          sed -i.bak 's/^version = "0.0.0"/version = "${{ needs.version.outputs.version }}"/' Cargo.toml
      - run: cargo build --release -p tropism --target ${{ matrix.target }}
      - # package, checksum, upload artifact

  release:
    needs: [version, build]
    steps:
      - uses: actions/attest-build-provenance@v1
      - # create tag v${version}, create GitHub Release, attach archives + SHA256SUMS
```

Counting tags for the micro makes the version a pure function of the release history, so a re-run
cannot silently produce a duplicate.

---

## 10. Blockers before the first release

**No `LICENSE` file.** `Cargo.toml` declares `license = "MIT OR Apache-2.0"` but neither text is in
the repository. `cargo publish` accepts that, and the packaging step tolerates it, but shipping
binaries and a crate under a licence whose text is absent is not a thing to do by accident. Add
`LICENSE-MIT` and `LICENSE-APACHE`, or change the declaration.

**The workspace root is a virtual manifest.** Verified: `cargo install --bins --locked --path .`
fails with *"found a virtual manifest … instead of a package manifest"*. That does not affect the
release workflow, which builds with `-p tropism`, but it does block one route to the pre-commit hook
— see [14-incremental-checking.md](14-incremental-checking.md). Registered as D25 in
[12-known-limitations.md](12-known-limitations.md).

## 8. Open questions

1. **crates.io cadence.** Fully continuous, or tag/dispatch-gated? The recommendation is gated; the
   requirement said continuous. Worth an explicit decision because it is not reversible.
2. **A `v` prefix on tags?** `v2026.7.0` reads as a tag; `2026.7.0` matches the crate version
   exactly. The sketch assumes `v`.
3. **Windows on ARM** (`aarch64-pc-windows-msvc`) is omitted. Add if anyone asks; it needs a cross C
   toolchain, per §5.
4. **Signing.** Build provenance covers "this came from that workflow". Sigstore/cosign signatures
   would go further. Probably premature.

---

## 9. What this does not cover

Container images, a Homebrew tap, Linux distribution packaging, and a documentation site. None are
needed before there is a user, and each adds a release surface that has to keep working.
