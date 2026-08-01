# 15 — Evaluating `dist` (cargo-dist) against the hand-written release pipeline

**Status: evaluated on a branch, not adopted.** Everything here was run, not read about.
`dist` 0.32.0 (released 2026-05-21), evaluated 2026-08-01 against the pipeline in
[13-build-and-release.md](13-build-and-release.md), which has cut three releases.

The question is not "is dist good" — it plainly is. It is whether adopting it is worth what it
costs *this* project, whose release pipeline already works.

---

## What it does that the hand-written pipeline does not

**Installers, which is the whole reason this was worth evaluating.** The distribution problem is
corporate Windows with no admin rights, and the generated `tropism-installer.ps1` is exactly right
for it — verified by reading the generated script, not by trusting the docs:

- **Zero elevation.** No `RunAs`, no `#Requires -RunAsAdministrator`, no UAC path. Confirmed by
  grep: the string count for `RunAs|elevat|Administrator` in the generated script is `0`.
- **Per-user PATH.** It writes `registry::HKEY_CURRENT_USER\Environment`, never `HKLM`, and
  broadcasts the change so a new shell picks it up.
- **User-writable install prefix**, `CARGO_HOME` by configuration, with `TROPISM_INSTALL_DIR` to
  override.

Writing and maintaining that by hand is not a good use of anyone's time.

**Smaller artifacts, for free.** `.tar.xz` plus the `[profile.dist]` thin-LTO profile it adds:

| | aarch64-apple-darwin archive |
| --- | --- |
| current pipeline (`.tar.gz`, `--release`) | 3,939,627 bytes |
| dist (`.tar.xz`, `[profile.dist]`) | 2,519,004 bytes |

36% smaller, and the extracted binary passes the same checks — `--version`, and `check` exiting 1 on
a violating file.

**Four more channels from one config.** `installers = ["shell", "powershell", "npm", "homebrew"]`
produces `tropism-installer.sh`, `tropism-installer.ps1`, `tropism.rb`, and a complete npm package
with `binary-install.js`, `npm-shrinkwrap.json`, and a `run-tropism.js` shim. The npm one matters
here: it is a second no-admin route for teams that have Node but not Rust.

**A machine-readable plan.** `dist plan --output-format=json` describes every artifact before
anything is built, which is what its CI uses to compute the matrix. The hand-written workflow
hard-codes that matrix in YAML.

---

## What it costs

### 1. It owns `.github/workflows/release.yml`, exclusively

Not a preference — an enforced one. `dist plan` refuses to run at all while that file differs from
what it would generate:

```
× .github/workflows/release.yml has out of date contents and needs to be regenerated
```

The escape hatch is `allow-dirty = ["ci"]`, which this branch sets so the two can sit side by side
for comparison. Adopting dist properly means deleting the hand-written pipeline and dropping that
key. There is no "use dist for the installers and keep our release job" arrangement.

### 2. It is tag-driven; this project is not

The deeper conflict, and the one that decides the answer.

`dist` reads the version from the manifest and expects a matching tag. This repository's version is
permanently `0.0.0` and the CalVer is **computed at release time from the tag count and injected,
never committed** — deliberately, so there is no push loop and no release-bump noise in the history
([13-build-and-release.md](13-build-and-release.md)).

The two models are incompatible as they stand. dist announces `v0.0.0`, which is what every command
above reported. Reconciling them means one of:

- **Commit the version.** Give up the injection, accept a version-bump commit per release, and
  release by tagging. This is what dist expects and what most projects do.
- **Inject before dist runs.** Keep CalVer, have CI compute and write the version, then invoke
  `dist build --tag=vX.Y.Z` on a dirty tree. Workable, but it fights the tool in the one place the
  tool is most opinionated, and `dist plan` on a PR would still see `0.0.0`.

Neither is a small change, and the choice is about release philosophy rather than about dist.

### 3. It brings its own opinions about the workflow

The generated workflow is ~300 lines against the hand-written ~190, uses `ubuntu-22.04` runners
where the current one pins `ubuntu-24.04`, and installs dist at run time by piping curl to sh. All
defensible; none chosen by us. Notably it does **not** include the build-provenance attestation the
current pipeline emits via `actions/attest-build-provenance`.

### 4. Cross-compilation is new and this project is the awkward case

0.32.0 added Linux and Windows cross-compilation via `cargo-zigbuild` and `cargo-xwin`. tropism
compiles a C toolchain per tree-sitter grammar, which is precisely the case the current pipeline
avoids by building every target on a native runner. Untested here, and worth testing before relying
on it.

---

## What this branch contains

- `dist-workspace.toml` — the config, with `dispatch-releases = true` so an evaluation run cannot
  race the real pipeline, and `allow-dirty = ["ci"]` so both workflows can coexist.
- `.github/workflows/release-dist.yml` — dist's generated workflow, renamed from `release.yml` so
  the working pipeline survives. **Renaming it means dist will not update it**; regenerating writes
  `release.yml` again.
- `[profile.dist]` in the root manifest, and `dist = false` on the three non-shipping crates.
- `repository` and `homepage` in `[workspace.package]`, inherited by all four crates. Required by
  dist, and wanted by crates.io regardless — this part is worth keeping either way.

Nothing on `main` changed. The workspace still builds, tests, and passes its own ruleset.

---

## Recommendation

**Not yet, and not for the reason the evaluation started with.**

The installers are genuinely excellent and solve the corporate-Windows problem better than anything
hand-written would. But they are not the binding constraint:

1. **The binding constraint is signing.** An unsigned `.exe` trips SmartScreen and cannot be
   allowlisted by publisher in AppLocker or WDAC. dist does not solve that; a certificate does.
   [SignPath Foundation](https://signpath.io/solutions/open-source-community) is free for projects
   that qualify, and tropism does. Start that first — it has lead time and dist does not shorten it.
2. **The best channel for this tool is PyPI, which dist does not generate.** The adoption path is
   the pre-commit hook, and `language: python` puts the binary inside a venv the team's already-
   approved Python creates — no admin, no PATH, no Mark of the Web, and it inherits an AppLocker
   exception that already exists. That is a maturin job, not a dist one.
3. **The release pipeline is not the problem.** It works, it has cut three releases, it attests
   provenance, and its CalVer model is a deliberate decision that dist would force us to revisit.
   Replacing working infrastructure to gain installers we can also get another way is the wrong
   trade this month.

**Revisit dist when** the version model is being reconsidered anyway, or when shell/PowerShell
installers become the primary channel rather than a supplement. At that point the ~300 lines it
generates are strictly better than maintaining them by hand, and `allow-dirty` comes back out.

**Keep from this branch regardless:** `repository`/`homepage` in `[workspace.package]`, and the
`[profile.dist]`-style thin-LTO idea — a 36% size reduction is worth having whoever builds the
archive.
