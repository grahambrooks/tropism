# evaluation/

The harness for [design/19-analysis-evaluation.md](../design/19-analysis-evaluation.md): run tropism
against 24 pinned public repositories covering all ten languages, establish ground truth with the
native tooling, and report where the analysis is right.

```sh
./pin-corpus.sh     # (optional) re-pin the corpus to current SHAs
./run.sh            # clone + analyze, in the hermetic container
./oracles.sh        # ground truth, in per-ecosystem containers
./report.py         # writes REPORT.md
./report.py --audit-sample 20 > audit.json
```

`REPORT.md` is **committed**, unlike `results/`. It is small and human-readable, so the next
evaluation lands as a reviewable diff in a pull request rather than as a directory of JSON nobody
opens. `--stdout` prints instead, `--output PATH` writes elsewhere.

Its header carries the tropism version, the report schema version and the corpus pin, and
**deliberately no generation timestamp** — a clock reading would make every regeneration a diff even
when no number moved. The per-repository timings are data and stay; a timestamp on top of them is
noise. Regenerating an unchanged report says so and leaves the bytes alone.

```sh
./clean.sh          # reclaim clones and scratch, keep results
./clean.sh --all    # results too
./clean.sh --images # and the docker images this harness built
```

Both scripts are resumable and failure-tolerant: anything already produced is skipped, and a
repository that cannot be cloned or analyzed is **recorded as failed** and the run continues. A
three-hour run must not die on repository nineteen. `FORCE=1` re-does completed work.

## Disk

**One repository is on disk at a time.** Each checkout is deleted as soon as it has been analyzed,
and the oracle pass deletes its copy along with whatever `node_modules` or `target` the oracle
installed into it. The corpus includes kubernetes, vscode, elasticsearch and dotnet/runtime; keeping
them all would cost tens of gigabytes to hold data that is fully reproducible from `corpus.tsv`.

The result JSON is the artefact. The clone is scaffolding.

Both scripts refuse to start a repository when free space is below `MIN_FREE_MIB` (8 GiB for
`run.sh`, 16 GiB for `oracles.sh`, which installs on top of the checkout). Running out half way
through leaves a truncated checkout that the resume logic would treat as complete, so it stops first
and says so.

`KEEP=1 ./run.sh` retains checkouts for debugging, at much greater cost.

## git-lfs is disabled, deliberately

`microsoft/vscode` keeps test fixtures such as
`extensions/copilot/test/simulation/cache/base.sqlite` in LFS, and on a machine without `git-lfs`
the checkout aborts the entire run:

```
git-lfs filter-process: git-lfs: command not found
fatal: extensions/copilot/.../base.sqlite: smudge filter lfs failed
```

`fetch_repo` overrides the filters (`filter.lfs.smudge=cat`, `filter.lfs.process=`,
`filter.lfs.required=false`), which leaves those paths as their pointer stubs. That is the right
outcome rather than a workaround: they are binary fixtures, tropism never reads them, and fetching
them would cost gigabytes across the corpus for nothing the analysis uses. Verified — vscode checks
out 17,597 files and analyzes as 147 projects with the stubs in place.

Installing `git-lfs` would also work and is not needed; the harness should not require a tool to
analyze a repository when tropism itself does not.

## Why there are two containers, not one

They have opposite requirements, and collapsing them would invalidate the result.

|  | `Dockerfile.tropism` | `oracles/Dockerfile.oracles` |
| --- | --- | --- |
| Toolchains | **none** — asserted at build time | exactly one, per ecosystem |
| Network at run time | `--network none` | on; resolving is the point |
| Repository mount | read-only | writable **throwaway copy** |
| Purpose | measure tropism | establish ground truth |

**The tropism image proves the claim rather than assuming it.** tropism's central promise is that it
works on a checkout with nothing installed, and the only way to test a negative claim is to make its
violation fail loudly. Run on a developer laptop, a bug where tropism shelled out to `npm` or read an
installed `node_modules` would be invisible — every one of those things happens to be available
there. In this image it breaks. The Dockerfile ends with a loop that fails the build if `cargo`,
`node`, `python3`, `go`, `dotnet`, `ruby` or `swift` can be found on `PATH`.

**The oracle images exist because ground truth is expensive and dangerous.** `cargo tree`, `npm ls`
and `madge` need dependencies actually resolved, and resolving executes arbitrary code from 24
repositories nobody here audited — npm lifecycle scripts, `build.rs`, `setup.py`, Gradle build logic.
That should not run on a developer machine.

It also *mutates the checkout*. `oracles.sh` therefore copies the tree first and deletes the copy
afterwards, so the pristine clone `run.sh` analyzed can never acquire a `node_modules/` that a
re-run would then see. Without that, the second tropism run measures something different from the
first and nobody would notice.

One image per ecosystem rather than one with ten: a combined image is enormous, and a version skew in
one toolchain would silently change the oracle for another.

## Portability

**The scripts must stay bash 3.2 compatible.** macOS has shipped bash 3.2.57 as
`/bin/bash` since 2007 for licensing reasons, so `#!/usr/bin/env bash` finds 3.2 on a stock machine.
Requiring a Homebrew bash to run an evaluation harness is friction the harness does not need.

So: no `;;&`, no `declare -A`, no `mapfile`, no `${var,,}`, no `[[ -v ]]`.

`make check-scripts` verifies it against the real `/bin/bash`, **one file at a time** — because
`bash -n a.sh b.sh` checks only the first file, which is exactly how a `;;&` shipped past a green
lint and broke `oracles.sh` on first use.

## The rule this all serves

**tropism must not invoke a package manager. The harness must.**

`cargo tree`, `npm ls`, `madge` and `jdeps` are oracles, never inputs. Nothing the oracle pass learns
is allowed to reach a tropism run — which is exactly what the read-only mount and the throwaway copy
enforce mechanically, rather than by remembering.

## Layout

| Path | |
| --- | --- |
| `corpus.tsv` | 24 repositories pinned to SHAs, with languages and shape |
| `Dockerfile.tropism` | the hermetic runner |
| `oracles/Dockerfile.oracles` | one build target per ecosystem |
| `run.sh` | clone at the pinned SHA, analyze, record JSON + wall-clock |
| `oracles.sh` | ground truth, on throwaway copies |
| `report.py` | the report; `--json` and `--audit-sample` too |
| `results/`, `oracles/results/`, `.checkouts/` | gitignored, reproducible |

## What the report answers

`REPORT.md` opens with **Where tropism does well** and **Where it is deficient**, computed
mechanically from the numbers below them so a reader can argue with a threshold rather than with an
adjective. Every criterion is stated inline.

Beneath that, the sections that make the verdicts checkable:

| | |
| --- | --- |
| **D1 Discovery** | plus *fixture-shaped* and *empty* project counts — a manifest under `tests/` is a real project and inflates every number below it |
| **Per-language** | the split design/19 asks for: tropism is ten providers of different maturity, and a mean over them describes none of them |
| **D2 Resolution** | statement resolution *and* the raw rate, because they mean different things — see below — with the unresolved reasons ranked |
| **D3/D5 Findings** | normalised per 1,000 source files, since elasticsearch has 31,458 files and flask has 83 |
| **Confidence** | a High-confidence rule violation and a Low-confidence `unused-dep` are different claims and must not be summed |

**Read the statement column in D2, not the raw rate.** The raw rate counts bare path references as
failures, and Rust leaves an unrecognised path root unresolved *by design*. On this repository that
is the difference between 28% and 100%. The raw rate is still what caps hygiene confidence, which is
D41 — the report flags any project where the two diverge, because there every hygiene finding is
pinned to Low for a reason unrelated to whether tropism understood the code.

## What the report will not do

It computes what is computable and **lists the rest as outstanding**, with the sample already drawn.

The D4 hygiene audit cannot be automated — design/10's 63% figure exists because someone read 35
findings against the source. And C#, C++ and Swift have no oracle worth automating, so their numbers
are unverified counts rather than measured accuracy, and the report says so in those words.

A report that quietly omitted the un-auditable half would read as a clean bill of health for checks
nobody looked at. That is the failure mode `CheckStatus` exists to prevent, one level up.

## Already found, before the real corpus ran

Validating against four local repositories surfaced a contract defect on first contact: the JSON
contract said `java-script`, `type-script` and `c-sharp` while the text renderer, `tropism
workspaces` and `tropism explain` said `javascript`, `typescript` and `csharp`. Two spellings for one
language across two surfaces of one tool, in the contract the MCP server is meant to share with the
CLI.

Fixed — D40 in [design/12](../design/12-known-limitations.md). The wire format is now derived from
`Language::as_str`, so the two cannot drift again, and `report.py` needs no workaround.

One defect from four repositories run only to check the scripts worked. That is the argument for the
other 24.
