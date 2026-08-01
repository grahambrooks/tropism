# tropism

A dependency analyzer for polyglot repositories. It finds import cycles, manifest problems, and
duplicated packages, and it enforces the architecture rules your team actually wrote down — across
Go, JavaScript/TypeScript, Rust, C#, Python, Ruby, Java, Swift, and C++, through one CLI and one
JSON contract.

**It never invokes a package manager and never executes the code it analyzes.** tropism works on a
fresh checkout with no toolchain, no network, and no installed dependencies, by reading manifests,
lockfiles, and source. That constraint is the reason it exists, and it is also the reason some
checks are unavailable in some ecosystems — which tropism says out loud rather than reporting a clean
result it cannot justify.

```
$ tropism analyze demo/dotnet

error[module-rule:.:8743cba7]: `api` must not depend on `data` (rule: api-goes-through-the-domain)
 --> Shop.Api/OrderController.cs:5:1
  |
5 | using Shop.Data;
  | ^^^^^^^^^^^^^^^^ imported Shop.Data
  |
note: confidence: high
note: tropism.toml: The API layer talks to the domain and nothing else. A controller that calls the
      data layer directly couples HTTP concerns to the storage schema, and the domain
      stops being the place where the rules live.
```

## The name

**`tropism` is an anagram of `imports`.**

```diff
@@ the same seven letters, rearranged @@

- i m p o r t s
- 1 2 3 4 5 6 7

+ 6 5 4 3 1 7 2
+ t r o p i s m
```

Nothing added, nothing left over. `6 5 4 3` is **port** read backwards, which is the whole trick:
`imports` is `i·m·port·s`, and `tropism` is `trop·i·s·m`.

A tropism is directed growth in response to a stimulus — a plant turning toward light is
phototropism, roots turning downward is geotropism. A dependency graph is directed growth too: every
import is an edge pointing somewhere, and the shape a codebase grows into is the sum of them.

The naming is borrowed from [Hamcrest](https://hamcrest.org/), which is an anagram of *matchers*.
`imports` is the right word to rearrange here: every language provider's hardest work is
`extract_imports` and `resolve_import`, and every check in the tool is ultimately a question about
what imports what.

Pronounced *TROH-pizm*.

## Quick start

Download a binary from [releases](https://github.com/grahambrooks/tropism/releases) — Linux (gnu and
musl, x86-64 and arm64), macOS (Intel and Apple silicon), and Windows. Every release carries
`SHA256SUMS` and build provenance attestation, so a downloaded binary traces back to the workflow run
and commit that produced it.

```sh
tropism check                        # the rules, over everything
tropism check src/api/user.ts        # the rules, scoped to a change
tropism analyze /path/to/repo        # every check
```

Or build it:

```sh
cargo build --release
./target/release/tropism analyze /path/to/repo
```

Not on crates.io yet — publishing is wired up but deliberately manual, since a version there can be
yanked but never reused. When it happens, `cargo install tropism` is the whole story: the crate, the
binary, and the command are all the same name.

Or take the guided tour, which runs against deliberately-broken sample projects in `demo/`:

```sh
./scripts/demo.sh              # every language, plus tropism analyzing itself
./scripts/demo.sh dotnet       # one language: go | javascript | rust | dotnet
                               #               python | ruby | java | swift | cpp
./scripts/demo.sh --tui        # end in the interactive browser
```


## Pre-commit hook

The differentiator, and the reason the hermetic design matters: **the check needs no build, no
install of your project, and no network.** ArchUnit needs a compiled classpath, NDepend a built
solution, `dependency-cruiser` a `node_modules`. tropism needs a directory, so it can run in the
second before a commit.

```yaml
# .pre-commit-config.yaml, for pre-commit or prek
repos:
  - repo: https://github.com/grahambrooks/tropism
    rev: v2026.8.2   # the first release with `check`
    hooks:
      - id: tropism          # rules, on changed files, at commit time
      - id: tropism-all      # every check, whole repository, at push time
```

`tropism check` reports only violations that **the changed files introduce** — a violation is an edge
between two modules, and it belongs to the file at the source end. So a repository with two hundred
existing violations passes every commit that does not add a two-hundred-and-first. That is a ratchet
with no baseline file to maintain and nothing to regenerate after a refactor, and it is what makes
the rules feature adoptable on a codebase that already breaks them.

The backlog is counted, never hidden:

```
checked 6 changed file(s) against 4 rule(s) — 1 violation(s)
  12 pre-existing violation(s) elsewhere are not shown; run `tropism check` for the whole repository
```

## What it checks

| Check              | What it finds                             | Reliability                                                     |
| ------------------ | ----------------------------------------- | --------------------------------------------------------------- |
| `cycle`            | Import cycles between modules             | **Sound.** Reads only import syntax                             |
| `module-rule`      | Violations of your architecture rules     | **Sound.** A violation is a line of source                      |
| `package-rule`     | Banned, unapproved, or misplaced packages | **Sound.** Same                                                 |
| `version-conflict` | A package installed at several versions   | Sound about the **lockfile**; a lockfile is feature-agnostic    |
| `diamond-dep`      | Dependents that disagreed about a version | Same. Cannot fire at all in a flat ecosystem                    |
| `missing-dep`      | Imported but not declared                 | Good. Capped at Medium confidence                               |
| `unused-dep`       | Declared but never imported               | **Weak — 63% false positives on real JS.** Do not gate CI on it |
| `dependency-bloat` | —                                         | Not implemented; reports unavailable                            |

Those reliability ratings are measured, not asserted. The method and the numbers are in
[design/10-js-evaluation.md](design/10-js-evaluation.md), and the reason for the split is in
[design/12-known-limitations.md](design/12-known-limitations.md): a cycle or a rule violation is the
*presence* of an import, which is a fact about a line of source. An unused dependency is an
*absence*, and absence cannot be proven without an installed dependency tree.

**Read `version-conflict` and `diamond-dep` as statements about the lockfile, not about your build.**
Dogfooding measured the gap: tropism reports 17 of them against this repository, every one a correct
reading of `Cargo.lock`, while `cargo tree --duplicates` finds only three duplicate sets in the graph
that actually compiles. A lockfile is resolved once for every feature combination and every target
platform and records neither, so it contains copies no build ever links — here, an optional terminal
backend that is never enabled and a UEFI-only crate. Deciding otherwise needs the feature resolution
of each dependency's own manifest, which is not in the repository. See S8 in
[design/12-known-limitations.md](design/12-known-limitations.md).

## Language support

| Language                | Manifest                                     | Lockfile                    | Resolved-tree checks                                    |
| ----------------------- | -------------------------------------------- | --------------------------- | ------------------------------------------------------- |
| Go                      | `go.mod`                                     | `go.sum`                    | **No** — hashes, not a resolved graph                   |
| JavaScript / TypeScript | `package.json`                               | `package-lock.json`         | Yes                                                     |
| Rust                    | `Cargo.toml`                                 | `Cargo.lock`                | Yes                                                     |
| C# / .NET               | `*.csproj`                                   | `packages.lock.json`        | Yes, when present (it is opt-in)                        |
| Python                  | `pyproject.toml`, `requirements.txt`         | `uv.lock`, `poetry.lock`    | Yes — but the environment is flat, so no diamonds exist |
| Ruby                    | `Gemfile`                                    | `Gemfile.lock`              | Yes — Bundler resolves flat, so a conflict cannot occur |
| Java                    | `pom.xml`, `build.gradle[.kts]`              | `gradle.lockfile`           | **No** — Maven has none; Gradle's carries no edges      |
| Swift                   | `Package.swift`                              | `Package.resolved`          | **No** — a flat pin list with no edges                  |
| C++                     | `conanfile.txt/.py`, `vcpkg.json`            | `conan.lock`                | **No** — flat pinned references, no edges               |

All ten target languages are built. Four of the manifests are *programs* rather than data —
`Gemfile`, `Package.swift`, `conanfile.py`, and `build.gradle` — and tropism parses the declarative
subset of each with a grammar rather than executing it. Anything dynamic contributes nothing:
`gem "rails-#{variant}"` names no gem that can be known without running the file, and a package that
does not exist is worse in a report than one that is missing from it.

## Architecture rules

Put a `tropism.toml` at the root and tropism enforces it, in the manner of NDepend, JDepend, or ArchUnit —
but across every language in the repository, hermetically, from one ruleset.

```toml
[modules]
core = "crates/tropism-core/**"
cli  = "crates/tropism/**"
mcp  = "crates/tropism-mcp/**"

[[module_rules]]
id = "surfaces-are-independent"
independent = ["cli", "mcp"]
reason = """
The CLI and MCP server are independent adapters over one analysis core. Shared
behaviour belongs in core, not in a dependency between the two surfaces.
"""

[[package_rules]]
id = "tui-stays-in-the-cli"
packages = ["ratatui"]
allowed_in = ["cli"]
reason = "The interactive browser is a CLI concern; core stays renderer-agnostic."
```

Violations are caught at **both levels** — the manifest declaration and the import — because a rule
broken in a manifest is still broken. Each finding renders your `reason` verbatim, which is the part
no inferred finding can ever supply.

That example is [this repository's own ruleset](tropism.toml), enforced on every run and asserted by
the test suite. Full specification: [design/11-dependency-rules.md](design/11-dependency-rules.md).

## Output

```sh
tropism analyze .                    # diagnostics on a terminal, JSON when piped
tropism analyze . --format json      # the machine contract
tropism analyze . --format tui       # interactive browser (terminal only)
tropism analyze . --fail-on error    # CI gate

tropism check                        # rules only, whole repository
tropism check src/a.rs src/b.ts      # rules only, scoped to these files
tropism check --staged               # ...to what is staged
tropism check --since origin/main    # ...to what a branch introduced
```

Exit codes are the CI contract: `0` ran clean, `1` findings at or above `--fail-on`, `2` could not
run. A broken invocation never looks like a passing build.

**Zero findings is not the same as "checked and clean."** Every check reports `ran`, `unavailable`,
or `failed`, with a reason:

```
unavailable version-conflict — go.sum records hashes for the whole module graph, not the
                               versions MVS selected, and carries no edges; a resolved tree
                               needs the Go resolver
```

No consumer — human or agent — can distinguish a clean result from a check that never ran unless the
tool says so.

## Limitations

Documented deliberately and in one place:
**[design/12-known-limitations.md](design/12-known-limitations.md)**.

It separates limitations that are *structural* — consequences of never running a package manager,
which get reported rather than fixed — from those merely *deferred*.

The one to read is **S8**: a lockfile is resolved once for every feature combination and every
target platform and records neither, so `version-conflict` and `diamond-dep` describe the lockfile
rather than the build. The most significant *deferred* gap is that `tropism check` does not exist,
so the rules can only be enforced over a whole repository and not over a change.

## Design

The `design/` directory is the specification, written before the code and corrected by it wherever
building contradicted it.

| Document                                                    | Answers                                                 |
| ----------------------------------------------------------- | ------------------------------------------------------- |
| [01-architecture.md](design/01-architecture.md)             | How the system is layered                               |
| [02-data-model.md](design/02-data-model.md)                 | The core types every layer passes around                |
| [03-language-providers.md](design/03-language-providers.md) | How a language is added, and the import→package problem |
| [04-analyzers.md](design/04-analyzers.md)                   | Each check: algorithm, inputs, failure modes            |
| [05-interfaces.md](design/05-interfaces.md)                 | CLI, MCP, and the JSON contract                         |
| [06-testing.md](design/06-testing.md)                       | Establishing correctness for a tool with no oracle      |
| [08-crates.md](design/08-crates.md)                         | Verified dependency choices                             |
| [09-product-review.md](design/09-product-review.md)         | Is this worth building? Evidence from the Go slice      |
| [10-js-evaluation.md](design/10-js-evaluation.md)           | Ten real JS repositories, and what they proved          |
| [11-dependency-rules.md](design/11-dependency-rules.md)     | The ruleset                                             |
| [12-known-limitations.md](design/12-known-limitations.md)   | Everything that does not work, and why                  |
| [13-build-and-release.md](design/13-build-and-release.md)   | CI, CalVer, binaries, crates.io                         |
| [14-incremental-checking.md](design/14-incremental-checking.md) | **The product**, and the next thing to build        |

Three of those are worth reading even if you never touch the code.
[09-product-review.md](design/09-product-review.md) concludes that tropism cannot compete with
`go mod tidy` on detection, because `go mod tidy` finds the same problems *and fixes them*.
[10-js-evaluation.md](design/10-js-evaluation.md) then measures which checks survive contact with
real repositories — 63% false positives for manifest hygiene. The rules feature exists because those
two documents said the generic checks were the weak part, and
[14-incremental-checking.md](design/14-incremental-checking.md) is where that lands: one ruleset,
enforced at commit time and over the whole repository, across ten languages, with no build and no
install.

That framing is what is left after three claims were eliminated on evidence — "finds dependency
problems" (the incumbents do it better and fix it), "manifest hygiene is the value" (63% false
positives), and "the MCP server is the product" (its flagship query needs a resolved tree, and four
of the ten ecosystems have none). The reversal worth knowing: the hermetic constraint that makes the
weakest check unreliable is exactly what makes the strongest one deployable. ArchUnit needs a
compiled classpath, NDepend a built solution, `dependency-cruiser` a `node_modules`. tropism needs a
directory, which is why it can run in the second before a commit.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets    # must stay clean
cargo run -p tropism -- analyze .        # dogfood
```

tropism analyzes itself in CI: `crates/tropism-lang/tests/demos.rs` asserts it reports nothing in its own
source beyond genuine `Cargo.lock` duplicates, and that it satisfies its own ruleset. That test has
caught four false-positive classes that no fixture would have.

### The pre-commit hook

It also analyzes itself at commit time, through [prek](https://github.com/j178/prek) — a
pre-commit-compatible hook runner that is a single Rust binary with no Python runtime.

```sh
brew install prek     # or: cargo install prek
prek install          # write the git hook shim
prek run tropism -a   # run just this hook over everything, without committing
```

The hook is the same gate CI applies, `--fail-on error`, so a commit that would fail there fails
here first. It blocks on rule violations and missing dependencies and lets warnings through, and it
**never** gates on `unused-dep` — a check with 63% false positives would get the hook disabled
permanently by the first developer it blocked wrongly.

This is not yet the hook the design wants. `tropism check <files>` — rules only, scoped to what
changed, which gives ratcheting on an already-violating codebase for free — is specified in
[design/14-incremental-checking.md](design/14-incremental-checking.md) and not built. Until it is,
the hook runs the whole repository, which costs ~0.6s here and would not stay affordable elsewhere.

See [CLAUDE.md](CLAUDE.md) for layout and the language semantics that cost real debugging.
