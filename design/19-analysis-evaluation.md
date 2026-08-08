# 19 — Analysis evaluation against real repositories

**Plan, not results.** A design for measuring what tropism's *analysis* actually gets right, across
all ten languages and both repository shapes, against public repositories with independently
obtainable ground truth.

[10-js-evaluation.md](10-js-evaluation.md) is the model and the precedent: point the tool at real
code, count what it says, **audit a sample by hand**, and let the number decide. That run measured
one ecosystem and produced the 63% false-positive figure that still shapes the roadmap. This one
covers the other nine, and a great deal has changed since — workspace boundaries, `tsconfig` paths,
yarn and pnpm lockfiles, and the D39 performance sweep all landed without any of them being measured
against real code beyond spot checks.

---

## The one rule that makes ground truth possible

**tropism must not invoke a package manager. The evaluation harness must.**

That distinction is the whole method. `cargo tree --duplicates`, `npm ls`, `madge --circular` and
`jdeps` are how a claim gets checked; they are oracles, not inputs. Nothing the harness learns may
feed back into a tropism run, or the result measures the harness.

---

## Corpus

Every entry below was **verified to exist with the manifests listed** on 2026-08-08 via the GitHub
API, rather than assumed. Star counts are a proxy for "code someone actually maintains".

| Repository | Languages | Shape | Verified artefacts | Why it is in the set |
| --- | --- | --- | --- | --- |
| kubernetes/kubernetes | Go | mono | `go.mod`, `go.sum`, **`go.work`** | The only verified `go.work` in the set — exercises workspace declaration parsing at scale |
| prometheus/prometheus | Go | poly | `go.mod`, `go.sum` | Regression anchor: the repo the Go test-file rules were derived from |
| grafana/grafana | Go + TS | mono | `go.mod`, `go.sum`, `package.json`, `yarn.lock` | Polyglot monorepo; the language-boundary case open question 1 was about |
| denoland/deno | Rust | mono | `Cargo.toml`, `Cargo.lock` | Large Cargo workspace with a real resolved tree |
| tokio-rs/tokio | Rust | mono | `Cargo.toml`, **no `Cargo.lock`** | A library, so the lock is gitignored — exercises D2's new "the tree is elsewhere" message |
| BurntSushi/ripgrep | Rust | poly | `Cargo.toml`, `Cargo.lock` | Small, hand-auditable end to end |
| astral-sh/ruff | Rust + Python | mono | `Cargo.toml`, `Cargo.lock`, `pyproject.toml`, `uv.lock` | Two ecosystems, both with lockfiles, in one tree |
| apache/airflow | Python | mono | `pyproject.toml`, `uv.lock` | Large Python monorepo; flat-environment diamond behaviour (S7) |
| pallets/flask | Python | poly | `pyproject.toml`, `uv.lock` | Small, well-understood, import→distribution mapping (S5) |
| microsoft/vscode | TS | mono | `package.json`, `package-lock.json` | The npm baseline, at ~188k stars of scale |
| vercel/next.js | TS | mono | `package.json`, `pnpm-lock.yaml`, `pnpm-workspace.yaml` | **pnpm** — the parser D14 added, plus a pnpm workspace declaration |
| facebook/react | JS | mono | `package.json`, `yarn.lock` | **yarn** — the other D14 parser, at scale |
| spring-projects/spring-boot | Java | mono | `build.gradle`, `settings.gradle` | Gradle multi-project; `include` parsing, no lockfile (S3) |
| google/guava | Java | mono | `pom.xml`, `guava/pom.xml` | Maven reactor; `<modules>`, and D32 parent-POM inheritance |
| elastic/elasticsearch | Java | mono | `build.gradle`, `settings.gradle` | Gradle at large scale; version catalogs (D30) |
| jellyfin/jellyfin | C# | mono | `Directory.Packages.props`, `Jellyfin.sln` | Central Package Management (D19) and `.sln` (D20) |
| AvaloniaUI/Avalonia | C# | mono | `Directory.Packages.props` | Second CPM sample; namespace-based module identity |
| microsoft/terminal | C++ | mono | `vcpkg.json` | One of the few large OSS C++ repos with a manifest at all |
| apache/arrow | C++ + Python | mono | `cpp/vcpkg.json`, `python/pyproject.toml` | C++ and Python in one tree, manifests in subdirectories |
| pointfreeco/swift-composable-architecture | Swift | poly | `Package.swift`, `Package.resolved` | The only verified Swift repo *with* a resolved file |
| apple/swift-nio | Swift | poly | `Package.swift`, **no `Package.resolved`** | A library, so no resolved file — the common Swift case |
| rails/rails | Ruby | mono | `Gemfile`, `Gemfile.lock`, per-gem `.gemspec` | Monorepo of gems; D29 (`.gemspec` unclaimed) shows up here |
| mastodon/mastodon | Ruby + JS | mono | `Gemfile`, `Gemfile.lock`, `package.json`, `yarn.lock` | Ruby + yarn in one tree |
| discourse/discourse | Ruby + JS | mono | `Gemfile`, `Gemfile.lock`, `package.json`, `pnpm-lock.yaml` | Ruby + pnpm in one tree |

**24 repositories, 10 languages, 6 of them polyglot.** Shape coverage is deliberate: mono and poly,
lockfile present and absent, workspace declared and not.

### The C++ finding, before the evaluation even starts

`facebook/folly` and `mongodb/mongo` were checked and have **no Conan or vcpkg manifest of any kind**.
This is not an accident of selection — the great majority of open-source C++ builds with CMake and
resolves dependencies through the system, a submodule, or `FetchContent`.

**So for C++, tropism discovers no project at all in most real repositories**, and the evaluation
should say so plainly rather than reporting a good score on the atypical two that do have a manifest.
This is a scope limitation of the *ecosystem convention*, not a defect, and it belongs in
[12-known-limitations.md](12-known-limitations.md) as a structural entry once confirmed.

---

## Dimensions

Ordered by weight. The brief is analysis capability, so D1–D5 are the evaluation and the rest are
guardrails.

### D1 — Discovery (gates everything)

Did tropism find the projects a human would say are there?

*Measure:* projects found vs a hand inventory of manifests in the repo. Report misses and spurious
roots separately — a missed project silently removes code from every other number, so this is
reported first and everything else is conditioned on it.

*Oracle:* `find` for manifest filenames, minus what `.gitignore` excludes.

### D2 — Resolution rate (caps confidence)

Share of imports resolving to `Internal`, `External` or `Stdlib` rather than `Unresolved`.

Already first-class in the design — it is the best available proxy for provider completeness and it
caps hygiene confidence. The interesting output is not the mean but the **distribution of unresolved
reasons**, which names the next provider gap directly. `tropism explain` is the instrument.

*Target:* ≥95% per language. Below 80% means the language's findings should not be trusted at all.

### D3 — Cycle soundness (the check that held up)

`cycle` was the one check that survived design/10 on every repository. It is the most valuable thing
to protect.

*Oracles, per language:*

| Language | Oracle | Note |
| --- | --- | --- |
| Go | **the compiler** | Go rejects import cycles, so any `module`-scope cycle on compiling code is *by definition* a false positive. A free, exact oracle — this is how the `_test.go` rules were found |
| JS/TS | `madge --circular` | Established tool, file-level like tropism's |
| Python | `pylint --disable=all --enable=cyclic-import` | |
| Java | `jdeps -cycles` | Package-level |
| Rust, C#, C++, Swift, Ruby | hand audit of a sample | No trusted tool; sample 10 per language |

*Measure:* precision and recall against the oracle. **Precision matters more** — a false cycle is
what gets a tool switched off.

### D4 — Manifest hygiene (the check that did not)

`unused-dep` measured 63% false positives on JS. The question here is whether that generalises or
whether JS is uniquely bad — JS has invisible-usage channels (HTML `<script src>`, config files,
framework strings) that Go and Rust largely lack.

*Measure:* **hand audit, 20 findings per language, stratified across repos**, classified into
true positive / invisible-usage false positive / provider-gap false positive. The third bucket is the
actionable one.

*Prediction to test:* Go and Rust land under 15%; Python and Ruby land near JS because both resolve
dynamically. If Go and Rust are genuinely low, `unused-dep` could gate for those languages — which
would be a real product change and the most valuable thing this evaluation could produce.

### D5 — Resolved-tree checks

`version-conflict` and `diamond-dep`, now that yarn and pnpm are parsed (D14).

*Oracles:* `cargo tree --duplicates`, `npm ls --all`, `pnpm list --depth Infinity`, `yarn info`.

**The comparison must be against the lockfile, not the build.** S8 is explicit that a lockfile is
resolved for every feature combination and platform and records neither, so tropism will legitimately
report duplicates that no build compiles. The metric is therefore *"does tropism agree with the
lockfile"*, and the S8 gap — how far lockfile truth is from build truth — is reported **separately**
as a magnitude, because that number is what a user actually experiences and it has only ever been
measured on this repository (17 findings, 3 real duplicates).

*Extra check unlocked by D14:* yarn and pnpm parsers must agree with `npm ls` on repos that have
been migrated, and the three parsers must not disagree about the same dependency tree.

### D6 — Workspace boundaries (new, unmeasured)

The whole of open question 1 shipped on synthetic fixtures.

*Measure:* for each repo, `tropism workspaces` output vs the repo's own declaration. Two failure
modes to count separately:

- **Wrong boundary** — projects grouped that the ecosystem separates, or vice versa.
- **`origin: language` where a declaration exists** — tropism failed to read something it could have.
  This is the strictly-worse case and should be zero for Cargo, npm, pnpm, go.work, Maven and Gradle.

Also report **sibling-exemption counts** per repo. That number is a blind spot by construction, and
nobody has yet seen how large it gets on real code.

### D7 — Robustness (guardrail)

Panics, non-zero exit for reasons other than findings, skipped files, and `Unavailable` reasons that
are wrong rather than merely unhelpful. Any panic is a P1 regardless of everything else.

### D8 — Scale (guardrail, post-D39)

Wall-clock for `analyze`, and for `check` scoped to one changed file, on the largest repos.
Kubernetes and vscode are the stress cases. Confirms the D39 sweep holds outside synthetic fixtures —
the scaling tests guard per-import cost, not whole-run behaviour.

### Out of scope

**Rule expressiveness.** Whether a repo's real architecture can be written as `tropism.toml` is a
product question, not an analysis one, and it needs the repo's maintainers to say what the intended
architecture *is*. Worth its own evaluation; not this one.

---

## Method

1. **Pin the corpus.** Shallow clone at a recorded commit SHA (`--depth 1 --filter=blob:none`), so
   the run is reproducible and re-runnable after a change. Expect 10–15 GB.
2. **Run** `tropism analyze --format json` per repo, recording version and wall-clock.
3. **Commit the raw JSON** under `evaluation/` keyed by repo and SHA. This is what makes the second
   run cheap: the next evaluation is a *diff*, and a regression becomes visible without re-auditing.
4. **Collect oracles** with the native tooling, in a separate pass.
5. **Audit by hand** where no oracle exists — D3 for five languages, all of D4.
6. **Write up per dimension**, with the per-language split visible. An aggregate number across ten
   languages would hide exactly the variation that matters.

---

## What each result would change

Stated in advance, so the analysis cannot be fitted to the outcome afterwards.

| Result | Consequence |
| --- | --- |
| `unused-dep` FP < 15% for Go/Rust | Promote it to gating **for those languages only**. The largest product change available |
| `unused-dep` FP > 50% everywhere | Consider removing the check rather than shipping a known-wrong one |
| Any `cycle` false positive on Go | A bug, by construction. Highest-priority fix |
| `cycle` precision < 90% anywhere | Cycle stops being the check the product leans on; roadmap changes |
| Resolution rate < 80% for a language | That language ships as advisory-only until the gap closes |
| `origin: language` where a declaration exists | Straight bug in the D14/workspace work |
| C++ discovers nothing on most repos | Record as structural; consider whether CMake support changes the scope question |
| Any panic | P1 |

---

## Cost and sequencing

Roughly a day of compute and cloning, plus the hand audits, which dominate: ~200 findings for D4 and
~50 for D3, at a few minutes each with the source open. That is two to three days of careful work and
it is the part that cannot be automated — the 63% figure exists precisely because someone read 35
findings against the source.

Sequence it **after the MCP server and D8 baseline are decided**, since both change what a user does
with the output, but **before promoting any check to gating**. Nothing in the current roadmap depends
on it; everything about what tropism should *claim* does.
