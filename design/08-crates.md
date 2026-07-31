# 08 — Crate selection

Verified against the crates.io API on **2026-07-30**, and adopted crates additionally proven by compiling and running. Re-verify before adding anything new; the
point of this document is that nobody has to take an earlier guess on faith.

## Adopted

Declared in the workspace `Cargo.toml`. Only crates actually used are declared — a dependency
analyzer shipping unused dependencies would be a poor advertisement.

| Crate               | Version | Last release | Used for                                      |
| ------------------- | ------- | ------------ | --------------------------------------------- |
| `serde`             | 1.0.229 | 2026-07-18   | The report contract                           |
| `serde_json`        | 1.0.151 | 2026-07-20   | JSON output, `details` payloads               |
| `clap`              | 4.6.4   | 2026-07-21   | CLI, derive API                               |
| `ignore`            | 0.4.31  | 2026-07-20   | Tree walking that understands `.gitignore`    |
| `camino`            | 1.2.5   | 2026-07-28   | UTF-8 paths that serialize without a dance    |
| `anyhow`            | 1.0.104 | 2026-07-18   | Errors in binaries and provider impls         |
| `thiserror`         | 2.0.19  | 2026-07-18   | Typed errors in the library                   |
| `blake3`            | 1.8.5   | 2026-04-25   | Content-derived, stable finding IDs           |
| `annotate-snippets` | 0.12.16 | 2026-05-06   | rustc/clippy-style diagnostic rendering       |
| `anstream`          | 1.0.0   | 2026-02-11   | Colour that degrades when piped               |
| `anstyle`           | 1.0.14  | 2026-03-13   | Style definitions for report chrome           |
| `ratatui`           | 0.30.2  | 2026-06-19   | `--format tui` browser (feature-gated)        |
| `insta`             | 1.48.0  | 2026-06-11   | Snapshot tests for the renderers              |
| `petgraph`          | 0.8.3   | 2025-09-30   | Tarjan SCC for cycle detection                |
| `tree-sitter`       | 0.26.11 | 2026-07-12   | Import extraction, with per-language grammars |
| `toml`              | 1.1.4   | 2026-07-28   | Cargo.toml, Cargo.lock, and the ruleset       |
| `globset`           | 0.4.19  | 2026-07-15   | Module path globs in the ruleset              |
| `quick-xml`         | 0.41.0  | 2026-06-29   | `.csproj` parsing                             |

Queued for the stages that need them, verified but not yet declared:

| Crate         | Version | Last release | Needed at                               |
| ------------- | ------- | ------------ | --------------------------------------- |
| `rayon`       | 1.12.0  | 2026-04-14   | Whenever parsing becomes the bottleneck |
| `quick-xml`   | 0.41.0  | 2026-06-29   | `pom.xml` when Java lands               |
| `semver`      | 1.0.28  | 2026-04-04   | Cargo-flavoured `VersionOps`            |
| `rmcp`        | 3.0.1   | 2026-07-29   | The MCP server                          |
| `serde-sarif` | 0.8.0   | 2025-05-09   | `--format sarif` for CI annotations     |

`cargo-lock` was on this list and is no longer needed: `Cargo.lock` is small, line-oriented TOML and
the `toml` crate already in the tree parses it directly, so the extra dependency bought nothing.

`rmcp` is the official Rust MCP SDK and is actively released, so the MCP surface in
[05-interfaces.md](05-interfaces.md) rests on maintained ground.

## Terminal UI

Two distinct surfaces, and keeping them distinct is what makes both safe.

**The default is styled stdout, not a TUI.** Worth stating plainly because it is easy to get
backwards: clippy is not a terminal UI. It is styled, structured stdout — no alternate screen, no
event loop, no cursor control. That is precisely why it works everywhere: it pipes to a file,
renders in CI logs, and survives being read by a machine. `--format auto` therefore resolves to
diagnostics or JSON, never to the browser.

**`ratatui` (0.30.2) provides the opt-in browser** behind `--format tui`, specified in
[05-interfaces.md](05-interfaces.md). The constraint that makes it safe is that it can only ever be
requested explicitly: redirecting it is an error rather than a stream of escape codes, so no
pipeline or CI job can end up inside an alternate screen. It is feature-gated
(`default = ["tui"]`), so `--no-default-features` drops it and its dependency tree entirely.

`ratatui` re-exports `crossterm`, so the backend needs no separate dependency, and its `TestBackend`
makes the layout snapshot-testable without a terminal — which is the difference between a second
maintained renderer and a second untested one.

The non-interactive stack is two more crates:

- **`annotate-snippets`** renders findings. It is maintained by rust-lang and is the crate the
  diagnostic format comes from, so gdep's output looks like the compiler output developers already
  parse without effort. It takes a snippet plus annotations and returns a string — a pure renderer,
  which keeps `gdep-core` free of any rendering concern.
- **`anstream`** handles colour. It strips ANSI automatically when stdout is not a terminal and
  honours `NO_COLOR`, so there is no second, uncoloured code path to keep in sync.

Rejected, with reasons:

- **`miette`** (7.6.0) is excellent, but it is an error-*reporting* framework: you derive
  `Diagnostic` on your error types. gdep's findings are data, not errors, and adopting it would pull
  a rendering concern into the core data model.
- **`codespan-reporting`** (0.13.1) and **`ariadne`** (0.6.0) are both capable and maintained. They
  lose only on the specific goal of looking exactly like rustc.

Optional additions when there is a reason: `indicatif` (0.18.6) for progress on large scans,
`comfy-table` (7.2.2) for summary tables.

## Unresolved

**YAML has no good option.** Needed for `pnpm-lock.yaml`, so it blocks JavaScript/TypeScript, which
is language four. The state of the ecosystem as of the verification date:

| Crate           | Version           | Last release | Note                               |
| --------------- | ----------------- | ------------ | ---------------------------------- |
| `serde_yaml`    | 0.9.34+deprecated | 2024-03-25   | Deprecated by its author           |
| `serde_yaml_ng` | 0.10.0            | 2024-05-26   | Fork; no release in over two years |
| `serde_norway`  | 0.9.42            | 2024-12-21   | Fork; no release in over a year    |
| `saphyr`        | 0.0.11            | 2026-07-11   | Actively developed, but pre-0.1    |

`saphyr` is the only actively maintained option and is the current best bet, at the cost of a
pre-0.1 API. Re-check when JS/TS work starts; the decision does not need to be made before then.

**Python version parsing may need vendoring.** `pep440_rs` (0.7.3, 2024-12) and `pep508_rs` (0.9.2,
2025-01) are the right crates but have not been released in over a year — development moved into
uv's tree. Re-check at step 7; be prepared to vendor or reimplement the subset gdep needs.

**tree-sitter grammar ABI compatibility — resolved.** Verified at runtime: the 0.26 core loads
`tree-sitter-go` and `tree-sitter-javascript` (ABI 15) and `tree-sitter-typescript` and
`tree-sitter-rust` (ABI 14) without complaint. tree-sitter supports a range of ABI versions, so the
lag between core and grammar releases is not the problem it appeared to be.

The original concern, kept for the record: grammar crates version independently of the core, and the
lag was large — `tree-sitter-typescript` at 0.23.2 against a 0.26.11 core — which looked like the
riskiest unverified assumption in the adopted stack.

## Toolchain

Rust edition 2024, `rust-version = "1.85"` (the edition's minimum). Developed against 1.97.1.
`unsafe_code = "forbid"` and `clippy::all = "warn"` are set workspace-wide.
