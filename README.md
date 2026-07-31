# gdep

A dependency analyzer for polyglot repositories. It finds import cycles, manifest problems, and
duplicated packages, and it enforces the architecture rules your team actually wrote down — across
Go, JavaScript/TypeScript, Rust, and C#, through one CLI and one JSON contract.

**It never invokes a package manager and never executes the code it analyzes.** gdep works on a
fresh checkout with no toolchain, no network, and no installed dependencies, by reading manifests,
lockfiles, and source. That constraint is the reason it exists, and it is also the reason some
checks are unavailable in some ecosystems — which gdep says out loud rather than reporting a clean
result it cannot justify.

```
$ gdep analyze demo/dotnet

error[module-rule:.:8743cba7]: `api` must not depend on `data` (rule: api-goes-through-the-domain)
 --> Shop.Api/OrderController.cs:5:1
  |
5 | using Shop.Data;
  | ^^^^^^^^^^^^^^^^ imported Shop.Data
  |
note: confidence: high
note: gdep.toml: The API layer talks to the domain and nothing else. A controller that calls the
      data layer directly couples HTTP concerns to the storage schema, and the domain
      stops being the place where the rules live.
```

## Quick start

```sh
cargo build --release
./target/release/gdep analyze /path/to/repo
```

There are no published binaries or crates yet; the release process is specified in
[design/13-build-and-release.md](design/13-build-and-release.md) but not built. Note that
`cargo install gdep` would install an **unrelated** crate of the same name — see that document.

Or take the guided tour, which runs against deliberately-broken sample projects in `demo/`:

```sh
./scripts/demo.sh              # every language, plus gdep analyzing itself
./scripts/demo.sh dotnet       # one language: go | javascript | rust | dotnet
./scripts/demo.sh --tui        # end in the interactive browser
```

## What it checks

| Check              | What it finds                             | Reliability                                                     |
| ------------------ | ----------------------------------------- | --------------------------------------------------------------- |
| `cycle`            | Import cycles between modules             | **Sound.** Reads only import syntax                             |
| `module-rule`      | Violations of your architecture rules     | **Sound.** A violation is a line of source                      |
| `package-rule`     | Banned, unapproved, or misplaced packages | **Sound.** Same                                                 |
| `version-conflict` | A package installed at several versions   | **Sound** where a real lockfile exists                          |
| `diamond-dep`      | Dependents that disagreed about a version | **Sound** where a real lockfile exists                          |
| `missing-dep`      | Imported but not declared                 | Good. Capped at Medium confidence                               |
| `unused-dep`       | Declared but never imported               | **Weak — 63% false positives on real JS.** Do not gate CI on it |
| `dependency-bloat` | —                                         | Not implemented; reports unavailable                            |

Those reliability ratings are measured, not asserted. The method and the numbers are in
[design/10-js-evaluation.md](design/10-js-evaluation.md), and the reason for the split is in
[design/12-known-limitations.md](design/12-known-limitations.md): a cycle or a rule violation is the
*presence* of an import, which is a fact about a line of source. An unused dependency is an
*absence*, and absence cannot be proven without an installed dependency tree.

## Language support

| Language                | Manifest       | Lockfile             | Resolved-tree checks                              |
| ----------------------- | -------------- | -------------------- | ------------------------------------------------- |
| Go                      | `go.mod`       | `go.sum`             | **No** — `go.sum` is hashes, not a resolved graph |
| JavaScript / TypeScript | `package.json` | `package-lock.json`  | Yes                                               |
| Rust                    | `Cargo.toml`   | `Cargo.lock`         | Yes                                               |
| C# / .NET               | `*.csproj`     | `packages.lock.json` | Yes, when present (it is opt-in)                  |

Python, Java, C++, Swift, and Ruby are specified but not built.

## Architecture rules

Put a `gdep.toml` at the root and gdep enforces it, in the manner of NDepend, JDepend, or ArchUnit —
but across every language in the repository, hermetically, from one ruleset.

```toml
[modules]
core = "crates/gdep-core/**"
cli  = "crates/gdep-cli/**"
mcp  = "crates/gdep-mcp/**"

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

That example is [this repository's own ruleset](gdep.toml), enforced on every run and asserted by
the test suite. Full specification: [design/11-dependency-rules.md](design/11-dependency-rules.md).

## Output

```sh
gdep analyze .                    # diagnostics on a terminal, JSON when piped
gdep analyze . --format json      # the machine contract
gdep analyze . --format tui       # interactive browser (terminal only)
gdep analyze . --fail-on error    # CI gate
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
which get reported rather than fixed — from those merely *deferred*. The most significant deferred
one is that cycle detection currently runs per project, so a cycle spanning two packages in a
monorepo is invisible to it.

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

Two of those are worth reading even if you never touch the code.
[09-product-review.md](design/09-product-review.md) concludes that gdep cannot compete with
`go mod tidy` on detection, because `go mod tidy` finds the same problems *and fixes them*.
[10-js-evaluation.md](design/10-js-evaluation.md) then measures which checks survive contact with
real repositories. The rules feature exists because those two documents said the generic checks
were the weak part.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets    # must stay clean
cargo run -p gdep-cli -- analyze .        # dogfood
```

gdep analyzes itself in CI: `crates/gdep-lang/tests/demos.rs` asserts it reports nothing in its own
source beyond genuine `Cargo.lock` duplicates, and that it satisfies its own ruleset. That test has
caught four false-positive classes that no fixture would have.

See [CLAUDE.md](CLAUDE.md) for layout and the language semantics that cost real debugging.
