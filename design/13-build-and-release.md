# 13 — Build and release

GitHub Actions, continuous release, CalVer versioning, binary downloads, and crates.io publishing.

A dependency analyzer's own supply chain should model the practices it advocates, so this document
is more careful than the size of the project strictly warrants: reproducible builds, checksums,
build provenance, and an explicit statement of what is irreversible.

**Nothing here is implemented.** No `.github/workflows/` files exist, deliberately — adding them
makes them live on the next push, and two of the requirements below (continuous release, crates.io
publishing) take actions that cannot be undone. The blocking decision in §1 should be settled first.

---

## 1. Blocking: the crate name is taken

**`gdep` on crates.io is an unrelated live crate** — "Git-deploy — Easily deploy & auto-update
apps", version 0.1.4, ~3,400 downloads, published 2025-02-10. The name cannot be used.

Checked on 2026-07-31:

| Name            | crates.io                     |
| --------------- | ----------------------------- |
| `gdep`          | **taken** (unrelated project) |
| `gdep-cli`      | available                     |
| `gdep-core`     | available                     |
| `gdeps`         | available                     |
| `gdep-analyzer` | available                     |
| `archdep`       | available                     |

**Recommendation: publish the binary crate as `gdep-cli`, keep the binary named `gdep`.**
`cargo install gdep-cli` then installs a `gdep` executable, which is a common and accepted pattern.
It needs no rename of the project, the repository, or the command.

The cost is a real footgun: **`cargo install gdep` installs someone else's tool.** That must be
stated in the README's install section, not left for a user to discover. If that is unacceptable,
rename the project now — it is far cheaper before a release exists than after.

---

## 2. Versioning: CalVer

**Format: `YYYY.MM.MICRO`**, e.g. `2026.7.0`, `2026.7.1`, `2026.8.0`.

- `MICRO` counts releases within the calendar month and resets to `0` each month.
- **No leading zeros.** `2026.07.1` is not a valid semver version and crates.io rejects it, so July
  is `7`, not `07`.

### The interaction with Cargo's semver, which matters

Cargo parses `2026.7.1` as major `2026`, minor `7`, patch `1`. A dependency written
`gdep-cli = "2026.7.1"` therefore resolves to `>=2026.7.1, <2027.0.0`.

So Cargo treats **every release within a calendar year as compatible** — including genuinely
breaking ones — and treats the January release as breaking even when nothing changed. CalVer carries
no compatibility information, and layering it onto a field Cargo reads as compatibility is actively
misleading.

**This is why only the binary crate is published.** Nobody writes `gdep-cli = "…"` in a
`Cargo.toml`, so nobody is exposed to the mismatch. The libraries stay `publish = false` until there
is a real library consumer, at which point they get their own SemVer line independent of the CLI's
CalVer.

That also keeps a promise the project has not yet had to make: publishing `gdep-core` commits it to
a public API, and its API is still moving — four language slices forced four trait changes
([12-known-limitations.md](12-known-limitations.md)).

### What consumers should actually version against

The JSON `schema_version` in [05-interfaces.md](05-interfaces.md) is the compatibility contract for
anyone parsing gdep's output. It is independent of the release version, increments only on a
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
| `gdep` binaries, six targets    | GitHub Releases | every green `main` |
| `SHA256SUMS` + build provenance | GitHub Releases | every green `main` |
| `gdep-cli` crate                | crates.io       | guarded — see §6   |
| `gdep-core`, `gdep-lang`        | nowhere         | `publish = false`  |

---

## 4. CI workflow (`.github/workflows/ci.yml`)

Runs on pull requests and pushes to `main`. Release is gated on it passing.

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
      - run: cargo build -p gdep-cli --no-default-features

  msrv:
    steps:
      - uses: dtolnay/rust-toolchain@1.85     # matches rust-version
      - run: cargo check --workspace

  dogfood:
    steps:
      - run: cargo build -p gdep-cli
      # BLOCKED — see below. The intended gate is:
      #   ./target/debug/gdep analyze . --exclude 'demo/**' --exclude '**/tests/fixtures/**' --fail-on error
```

`clippy -D warnings` matches the standard the repository already holds itself to.

### The dogfood gate is blocked on a missing flag

`gdep.toml` declares that the CLI and MCP server must stay independent and that core is a leaf.
Running gdep against itself in CI is what would make those rules load-bearing rather than
decorative — and it is the most on-brand job in the whole workflow.

**It does not work today.** Verified:

```
gdep analyze .                       exit=1   demo/ and test fixtures are deliberately broken
gdep analyze crates                  exit=1   fixtures live under crates/gdep-lang/tests/
gdep analyze crates/a crates/b       exit=2   only one path argument is accepted
```

gdep has **no `--exclude`**, and `.gitignore` cannot help because those directories are tracked on
purpose. Scanning one crate at a time would pass, but it would defeat the rules entirely: a rule
like "the CLI must not depend on the MCP server" spans two crates, so narrowing the scan removes the
very edges it exists to check.

The test suite asserts the same property today (`gdep_satisfies_its_own_ruleset` in
`crates/gdep-lang/tests/demos.rs`) by filtering in Rust. So the capability is proven; only the CLI
cannot express it.

**This job stays commented out until `--exclude` exists** — registered as D23 in
[12-known-limitations.md](12-known-limitations.md). Shipping a green CI badge over a gate that was
quietly narrowed until it passed would be worse than having no gate.

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

The musl build matters more than usual here. gdep's proposition is analysis on a bare checkout with
nothing installed; a static binary that runs in a scratch container is the same claim at the
distribution layer.

### The constraint that shapes this: gdep is not a pure-Rust binary

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

- `gdep-<version>-<target>.tar.gz` for Unix, `.zip` for Windows, each containing the binary plus
  `README.md` and `LICENSE`.
- A single `SHA256SUMS` covering every archive.
- `actions/attest-build-provenance` for build provenance, so a downloaded binary can be traced to
  the workflow run and commit that produced it. A supply-chain tool should be verifiable itself.

### Install paths to document

```sh
cargo install gdep-cli                       # installs a `gdep` binary
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
        if cargo info "gdep-cli@${VERSION}" >/dev/null 2>&1; then
          echo "already published"; exit 0
        fi
    - run: cargo publish -p gdep-cli --allow-dirty
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
      - run: cargo build --release -p gdep-cli --target ${{ matrix.target }}
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

## 8. Open questions

1. **The crate name (§1).** Publish as `gdep-cli` and document the `cargo install gdep` footgun, or
   rename the project? This blocks any crates.io work and gets more expensive after the first
   release.
2. **crates.io cadence.** Fully continuous, or tag/dispatch-gated? The recommendation is gated; the
   requirement said continuous. Worth an explicit decision because it is not reversible.
3. **A `v` prefix on tags?** `v2026.7.0` reads as a tag; `2026.7.0` matches the crate version
   exactly. The sketch assumes `v`.
4. **Windows on ARM** (`aarch64-pc-windows-msvc`) is omitted. Add if anyone asks; it needs a cross C
   toolchain, per §5.
5. **Signing.** Build provenance covers "this came from that workflow". Sigstore/cosign signatures
   would go further. Probably premature.

---

## 9. What this does not cover

Container images, a Homebrew tap, Linux distribution packaging, and a documentation site. None are
needed before there is a user, and each adds a release surface that has to keep working.
