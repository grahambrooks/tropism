# Rust demo

A three-crate Cargo workspace: one shared `engine` and two surfaces, `app` and
`service` — the same shape as tropism's own core/cli/mcp split. `Cargo.lock` is a genuinely resolved graph, so
version-conflict and diamond-dep run — but only at the workspace root, because
that is where the lockfile lives.

## Dependency rules (`tropism.toml`)

- **Violated** — `surfaces-are-independent`: `app` imports `service` instead of
  going through `engine`. This is the motivating case for the whole feature, and
  it is caught **twice**: once at the `Cargo.toml` declaration and once at the
  `use` in `main.rs`. A rule broken only in a manifest is still broken.
- **Satisfied** — `engine-is-a-leaf`: the shared crate depends on neither surface.
- **Violated** — `regex-belongs-to-the-engine`: `regex` is scoped to `engine` but
  imported by `app`.

Each finding renders the team's `reason` verbatim, which is the part no inferred
finding can ever supply.

Planted problems:

- A module cycle between `engine`'s `parser` and `evaluator`. Rust *permits*
  module cycles, unlike Go, so nothing in the toolchain catches this.
- `once_cell` is declared by `app` but never used.
- `serde_json` is imported by `engine` but never declared.
- `libc` is resolved at two versions in `Cargo.lock`.

Planted traps, which tropism must **not** report:

- `anyhow::Result` is used with no `use` statement at all. Idiomatic Rust does
  this constantly; extracting only `use` would call `anyhow` unused.
- `#[derive(thiserror::Error)]` — a path inside an attribute token tree.
- `use super::*` inside `#[cfg(test)] mod tests` — containment, not a cycle.

## A limitation this demo makes visible

`missing-dep` fires for `serde_json` only because `parser.rs` writes
`use serde_json::from_str`. Had it written `serde_json::from_str(..)` inline with
no `use`, tropism would have counted the crate as *used* but would **not** have
reported it missing.

That asymmetry is deliberate. A bare path proves a crate is used, but it cannot
prove one is undeclared — `Palette::plain()` is a local type, and treating every
unrecognised path root as a missing dependency would invent findings on every
file. Proving usage is sound; proving absence is not. The same asymmetry is why
`unused-dep` is the least reliable check in the tool (design/10-js-evaluation.md).
