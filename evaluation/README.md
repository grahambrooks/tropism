# evaluation/

The harness for [design/19-analysis-evaluation.md](../design/19-analysis-evaluation.md): run tropism
against 24 pinned public repositories covering all ten languages, establish ground truth with the
native tooling, and report where the analysis is right.

```sh
./pin-corpus.sh     # (optional) re-pin the corpus to current SHAs
./run.sh            # clone + analyze, in the hermetic container
./oracles.sh        # ground truth, in per-ecosystem containers
./report.py         # markdown report to stdout
./report.py --audit-sample 20 > audit.json
```

Both scripts are resumable: anything already produced is skipped, so an interrupted run costs
nothing to restart. `FORCE=1` re-does it.

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

## What the report will not do

It computes what is computable and **lists the rest as outstanding**, with the sample already drawn.

The D4 hygiene audit cannot be automated — design/10's 63% figure exists because someone read 35
findings against the source. And C#, C++ and Swift have no oracle worth automating, so their numbers
are unverified counts rather than measured accuracy, and the report says so in those words.

A report that quietly omitted the un-auditable half would read as a clean bill of health for checks
nobody looked at. That is the failure mode `CheckStatus` exists to prevent, one level up.

## Already found, before the real corpus ran

Validating the harness against four local repositories surfaced a contract defect: `Language` derives
`serde(rename_all = "kebab-case")`, so the **JSON contract says `java-script`, `type-script` and
`c-sharp`** while `Language::as_str()` — the text renderer, `tropism workspaces`, `tropism explain` —
says `javascript`, `typescript` and `csharp`. Two spellings for one language across two surfaces of
one tool, in the contract the MCP server is meant to share with the CLI.

`report.py` normalises around it so the evaluation stays honest about a defect that is not the
evaluation's. The normalisation should be deleted once the contract has one spelling.
