#!/usr/bin/env python3
"""Turns the evaluation's raw output into the report design/19 specifies.

The dimensions it can compute, it computes. The ones needing a human — the
hand audits behind D4, and every check in an ecosystem with no oracle — it
lists as outstanding work with the sample already drawn.

That split is the point. A report that quietly omitted the un-auditable half
would read as a clean bill of health for checks nobody looked at, which is the
failure mode `CheckStatus` exists to prevent, one level up.

    ./report.py                     # writes REPORT.md
    ./report.py --output x.md       # somewhere else
    ./report.py --stdout            # print instead, for piping
    ./report.py --json              # machine-readable
    ./report.py --audit-sample 20   # draw the D4 audit sample
    ./report.py --baseline results.before   # add a delta column against a prior run

REPORT.md is committed, unlike results/. It is small, human-readable, and the
thing worth reviewing — so the next evaluation shows up as a diff in a pull
request rather than as a directory of JSON nobody opens.

Nothing derived from the wall clock goes in the header for that reason: a
generation timestamp would make every regeneration a diff even when no number
moved. The per-repository timings are data and stay; a timestamp on top of them
is noise.
"""

from __future__ import annotations

import argparse
import collections
import json
import pathlib
import random
import sys

HERE = pathlib.Path(__file__).resolve().parent
RESULTS = HERE / "results"
ORACLES = HERE / "oracles" / "results"
CORPUS = HERE / "corpus.tsv"
DEFAULT_OUTPUT = HERE / "REPORT.md"

# Set by --baseline. A prior results/ directory to diff against, because
# design/19's argument for keeping raw results is that the *next* evaluation is a
# diff — a second column of absolute numbers is not one.
BASELINE: pathlib.Path | None = None

# Checks that gate, versus checks that inform. design/10 is why they are not
# reported together: an aggregate over both hides the split that matters.
INFERRED = ["cycle", "unused-dep", "missing-dep", "version-conflict", "diamond-dep"]


def corpus_pin() -> str:
    """The corpus's pin line, so a report always says which commits it describes."""
    for line in CORPUS.read_text().splitlines():
        if line.startswith("# Pinned"):
            return line.lstrip("# ").rstrip(".")
    return "unpinned"


def load_corpus() -> list[dict]:
    rows = []
    for line in CORPUS.read_text().splitlines():
        if line.startswith("#") or not line.strip():
            continue
        repo, sha, langs, shape, note = (line.split("\t") + [""] * 5)[:5]
        rows.append(
            {
                "repo": repo,
                "sha": sha,
                "languages": langs.split(","),
                "shape": shape,
                "note": note,
                "slug": repo.replace("/", "__"),
            }
        )
    return rows


def load_baseline(slug: str) -> dict | None:
    if BASELINE is None:
        return None
    path = BASELINE / f"{slug}.json"
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text())
    except json.JSONDecodeError:
        return None


def delta(now: int, before: int | None) -> str:
    """A signed change, or blank when there is nothing to compare against.

    Blank rather than zero: "no baseline" and "no change" are different claims,
    and this whole tool exists because those two get conflated.
    """
    if before is None:
        return ""
    if now == before:
        return " (=)"
    return f" ({now - before:+,})"


def load(slug: str) -> dict | None:
    path = RESULTS / f"{slug}.json"
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text())
    except json.JSONDecodeError as exc:
        return {"error": f"unparseable result: {exc}"}


def summarise(report: dict) -> dict:
    """Per-repository facts, computed straight from the JSON contract."""
    projects = report.get("projects", [])
    findings = [f for p in projects for f in p.get("findings", [])]
    by_check = collections.Counter(f["check"] for f in findings)

    status = collections.Counter()
    unavailable_reasons = []
    for project in projects:
        for check, state in project.get("checks", {}).items():
            status[(check, state.get("status"))] += 1
            if state.get("status") == "unavailable":
                unavailable_reasons.append((check, state.get("reason", "")))

    exemptions = [e for p in projects for e in p.get("sibling_exemptions", [])]

    # Resolution: the share of imports tropism understood. Absent from results
    # produced before it was added to the contract, and absent (not zero) on a
    # rules-only run — "did not look" must not read as "understood nothing".
    res = [p["resolution"] for p in projects if p.get("resolution")]
    imports = sum(r["imports"] for r in res)
    unresolved = sum(r["unresolved"] for r in res)
    statements = sum(r.get("statements", 0) for r in res)
    unresolved_statements = sum(
        round(r.get("statements", 0) * (1 - r.get("statement_rate", 1.0))) for r in res
    )
    reasons = collections.Counter()
    for r in res:
        for entry in r.get("reasons", []):
            reasons[entry["reason"]] += entry["count"]

    # A project with no source files is a manifest with nothing under it — a test
    # fixture, a package stub, a vendored descriptor. They inflate every count
    # below D1 without being code anyone wants findings about.
    empty = sum(1 for p in projects if p.get("source_file_count", 0) == 0)
    fixtures = sum(
        1 for p in projects
        if any(seg in p["root"].lower().split("/")
               for seg in ("test", "tests", "testdata", "fixture", "fixtures",
                           "spec", "specs", "example", "examples", "registry"))
    )

    confidence = collections.Counter(f["confidence"] for f in findings)
    severity = collections.Counter(f["severity"] for f in findings)

    return {
        "projects": len(projects),
        "languages": sorted({p["language"] for p in projects}),
        "source_files": sum(p.get("source_file_count", 0) for p in projects),
        "findings": len(findings),
        "by_check": dict(by_check),
        "skipped_files": len(report.get("skipped", [])),
        "seconds": report.get("evaluation", {}).get("seconds"),
        "exit_code": report.get("evaluation", {}).get("exit_code"),
        "ran": sum(v for (_, s), v in status.items() if s == "ran"),
        "unavailable": sum(v for (_, s), v in status.items() if s == "unavailable"),
        "failed": sum(v for (_, s), v in status.items() if s == "failed"),
        "unavailable_reasons": unavailable_reasons,
        "sibling_exemptions": len(exemptions),
        "exempted_imports": sum(e.get("imports", 0) for e in exemptions),
        "has_resolution": bool(res),
        "imports": imports,
        "unresolved": unresolved,
        "resolution_rate": (imports - unresolved) / imports if imports else None,
        "statements": statements,
        "statement_rate": (statements - unresolved_statements) / statements if statements else None,
        "unresolved_reasons": reasons,
        "empty_projects": empty,
        "fixture_projects": fixtures,
        "confidence": confidence,
        "severity": severity,
        "per_project": [
            {"language": p["language"],
             "files": p.get("source_file_count", 0),
             "findings": len(p.get("findings", [])),
             "by_check": collections.Counter(f["check"] for f in p.get("findings", [])),
             "resolution": p.get("resolution")}
            for p in projects
        ],
    }


def oracle_status(slug: str) -> dict[str, bool]:
    found = {}
    for path in ORACLES.glob(f"{slug}.*.json"):
        try:
            data = json.loads(path.read_text())
        except json.JSONDecodeError:
            continue
        found[data.get("oracle", path.stem)] = bool(data.get("ok"))
    return found


def draw_audit_sample(corpus, size: int, seed: int = 20260808) -> list[dict]:
    """D4's sample: hygiene findings, stratified by language.

    Seeded, so the sample is reproducible and a second auditor grades the same
    findings — otherwise two audits are not comparable.
    """
    rng = random.Random(seed)
    pool: dict[str, list] = collections.defaultdict(list)
    for row in corpus:
        report = load(row["slug"])
        if not report:
            continue
        for project in report.get("projects", []):
            for finding in project.get("findings", []):
                if finding["check"] in ("unused-dep", "missing-dep"):
                    pool[project["language"]].append(
                        {
                            "repo": row["repo"],
                            "language": project["language"],
                            "check": finding["check"],
                            "id": finding["id"],
                            "message": finding["message"],
                            "evidence": finding.get("evidence", [])[:2],
                        }
                    )
    sample = []
    for language, findings in sorted(pool.items()):
        rng.shuffle(findings)
        sample.extend(findings[:size])
    return sample



def by_language(done) -> dict:
    """Rolls per-project facts up by language.

    design/19 asks for the per-language split for a reason: an aggregate across ten
    languages hides exactly the variation that matters. tropism is not one tool with
    one accuracy — it is ten providers of visibly different maturity, and a mean
    over them describes none of them.
    """
    langs: dict[str, dict] = collections.defaultdict(
        lambda: {"repos": set(), "projects": 0, "files": 0, "findings": 0,
                 "by_check": collections.Counter(), "imports": 0, "unresolved": 0,
                 "statements": 0, "unresolved_statements": 0}
    )
    for row, data in done:
        for project in summarise(data)["per_project"]:
            entry = langs[project["language"]]
            entry["repos"].add(row["repo"])
            entry["projects"] += 1
            entry["files"] += project["files"]
            entry["findings"] += project["findings"]
            entry["by_check"].update(project["by_check"])
            if project["resolution"]:
                r = project["resolution"]
                entry["imports"] += r["imports"]
                entry["unresolved"] += r["unresolved"]
                entry["statements"] += r.get("statements", 0)
                entry["unresolved_statements"] += round(
                    r.get("statements", 0) * (1 - r.get("statement_rate", 1.0))
                )
    return langs


def per_thousand(count: int, files: int) -> str:
    """Findings per 1,000 source files.

    Raw counts across repositories of wildly different size are not comparable:
    elasticsearch has 31,458 files and flask has 83. Density is what says whether a
    check is quiet or noisy.
    """
    return f"{1000 * count / files:.1f}" if files else "—"


def verdicts(done) -> tuple[list[str], list[str]]:
    """What the numbers support saying, and what they do not.

    Deliberately mechanical. The criteria are stated in the report so a reader can
    disagree with the threshold rather than with an unexplained adjective, and so
    the same input always produces the same verdict.
    """
    good: list[str] = []
    bad: list[str] = []
    langs = by_language(done)

    failures = [row["repo"] for row, data in done if data.get("error")]
    if not failures:
        good.append(f"**No run failures.** {len(done)} repositories analyzed, none errored.")

    skipped = sum(summarise(d)["skipped_files"] for _, d in done)
    total_files = sum(summarise(d)["source_files"] for _, d in done)
    if total_files and skipped / total_files < 0.001:
        good.append(
            f"**Parsing is near-total.** {skipped} of {total_files:,} files unreadable "
            f"({100 * skipped / total_files:.3f}%)."
        )

    slowest = max(((summarise(d)["seconds"] or 0, summarise(d)["source_files"], row["repo"])
                   for row, d in done), default=(0, 0, ""))
    if slowest[0] and slowest[1]:
        good.append(
            f"**Scale holds.** Largest run {slowest[2]} — {slowest[1]:,} files in "
            f"{slowest[0]}s ({slowest[1] // max(slowest[0], 1):,} files/sec)."
        )

    # Resolution, per language, is the clearest maturity signal available.
    # Judged on *statements*, which is what provider completeness means. The raw
    # rate is reported too, because it is what caps confidence — but it counts
    # deliberately-unresolved path references as failures and would otherwise
    # condemn a provider that resolved everything it was asked to.
    measured = {k: v for k, v in langs.items() if v["statements"]}
    for language, entry in sorted(measured.items()):
        srate = (entry["statements"] - entry["unresolved_statements"]) / entry["statements"]
        if srate >= 0.95:
            good.append(
                f"**{language}: {100 * srate:.1f}% of import statements resolved** "
                f"({entry['statements']:,} statements)."
            )
        elif srate < 0.80:
            bad.append(
                f"**{language}: only {100 * srate:.1f}% of import statements resolved.** "
                f"Below 80%, design/19 says that language's findings should not be "
                f"trusted. The unresolved reasons below name the gap."
            )
        else:
            bad.append(
                f"**{language}: {100 * srate:.1f}% of import statements resolved.** Under "
                f"the 95% target; the unresolved reasons below name the gap."
            )
    if not measured:
        bad.append(
            "**Resolution rate not measurable.** These results predate the `resolution` "
            "field in the report contract. Re-run with `FORCE=1 ./run.sh` to measure "
            "D2 at all."
        )

    # The confidence cap is driven by `rate`, which counts path references as
    # failures. Where the two measures diverge, the cap is measuring a design
    # decision rather than provider completeness — and it pins every hygiene
    # finding in that project to Low for the wrong reason.
    for row, data in done:
        s = summarise(data)
        rate, srate = s.get("resolution_rate"), s.get("statement_rate")
        if rate is None or srate is None or not s["imports"]:
            continue
        if srate - rate > 0.25 and rate < 0.9:
            bad.append(
                f"**{row['repo']}: {100 * rate:.0f}% of imports resolve but "
                f"{100 * srate:.0f}% of import *statements* do.** The gap is bare path "
                f"references, which are deliberately left unresolved. Because the "
                f"confidence cap keys on the first number, every hygiene finding here "
                f"is pinned to Low for a reason unrelated to whether tropism understood "
                f"the code."
            )

    # Discovery inflation gates every number below it.
    for row, data in done:
        s = summarise(data)
        if s["projects"] >= 50 and s["fixture_projects"] / s["projects"] > 0.5:
            bad.append(
                f"**{row['repo']}: {s['fixture_projects']} of {s['projects']} projects "
                f"are under test or fixture paths.** Discovery gates every other number, "
                f"and a first run here is dominated by packages nobody wants findings "
                f"about. This is what `exclude` in tropism.toml is for."
            )

    # A check that never runs anywhere is a claim tropism cannot make.
    #
    # Counted **per repository, not per project**: a lockfile resolves a whole
    # workspace, so a monorepo of 700 members reports `version-conflict` unavailable
    # 699 times and available once, and that is correct behaviour rather than a gap
    # (D2). The honest question is whether tropism got a resolved tree for the
    # repository at all.
    reached = collections.Counter()
    for _, data in done:
        for check in ("cycle", "unused-dep", "missing-dep",
                      "version-conflict", "diamond-dep"):
            if any(p.get("checks", {}).get(check, {}).get("status") == "ran"
                   for p in data.get("projects", [])):
                reached[check] += 1
    for check in ("cycle", "unused-dep", "missing-dep", "version-conflict", "diamond-dep"):
        missing_in = len(done) - reached[check]
        if missing_in > len(done) / 2:
            bad.append(
                f"**`{check}` never ran in {missing_in} of {len(done)} repositories.** "
                f"Honest where no resolved tree exists (S3), but it caps what tropism "
                f"can claim in those ecosystems."
            )
        elif missing_in == 0:
            good.append(f"**`{check}` ran in every repository.**")

    return good, bad


def markdown(corpus) -> str:
    out: list[str] = []
    w = out.append

    loaded = [(row, load(row["slug"])) for row in corpus]
    done = [(r, d) for r, d in loaded if d and "error" not in d]
    missing = [r for r, d in loaded if d is None]
    errored = [(r, d) for r, d in loaded if d and "error" in d]

    w("# Analysis evaluation — results\n")
    w(f"Corpus: **{len(done)}/{len(corpus)} analyzed**"
      + (f", {len(missing)} not yet run" if missing else "")
      + (f", **{len(errored)} failed**" if errored else "")
      + ".\n")

    versions = sorted({d.get("tropism_version", "?") for _, d in done})
    schemas = sorted({str(d.get("schema_version", "?")) for _, d in done})
    w(f"| | |\n| --- | --- |")
    w(f"| tropism version | {', '.join(versions) or '—'} |")
    w(f"| report schema | {', '.join(schemas) or '—'} |")
    w(f"| corpus | {corpus_pin()} |")
    w("")
    if len(versions) > 1:
        w("> **Mixed tropism versions in this corpus.** The numbers below do not describe "
          "one build, and a difference between repositories may be a difference between "
          "versions. Re-run with `FORCE=1 ./run.sh`.\n")
    w("Generated by `evaluation/report.py`; regenerate after any run. Dimensions follow "
      "[design/19-analysis-evaluation.md](../design/19-analysis-evaluation.md).\n")

    good, bad = verdicts(done)
    w("## Summary\n")
    w("Mechanical, from the numbers below, so a reader can argue with a threshold "
      "rather than with an adjective. Criteria are named inline.\n")
    w("### Where tropism does well\n")
    for line in good or ["_Nothing met the criteria._"]:
        w(f"- {line}")
    w("")
    w("### Where it is deficient\n")
    for line in bad or ["_Nothing met the criteria._"]:
        w(f"- {line}")
    w("")
    w("**Not answered here.** Accuracy. Every count below is what tropism *said*, not "
      "what was true — that needs the oracle pass and the D4 hand audit, both of which "
      "this report marks as outstanding rather than quietly omitting.\n")

    if errored:
        w("## D7 — Robustness: failures\n")
        w("Any non-finding failure is a P1 regardless of every other number.\n")
        for row, data in errored:
            w(f"- **{row['repo']}** — {data.get('error')}")
        w("")

    # ---------------------------------------------------------------- D1
    w("## D1 — Discovery\n")
    w("Gates everything below: a missed project silently removes code from every "
      "other number.\n")
    w("| Repository | Shape | Projects | Fixture-shaped | Empty | Languages found | Source files |")
    w("| --- | --- | --- | --- | --- | --- | --- |")
    for row, data in done:
        s = summarise(data)
        found = ", ".join(s["languages"]) or "**none**"
        flag = "" if set(s["languages"]) >= set(row["languages"]) else " ⚠"
        share = f"{s['fixture_projects']} ({100 * s['fixture_projects'] // max(s['projects'], 1)}%)"
        heavy = " ⚠" if s["projects"] >= 50 and s["fixture_projects"] / s["projects"] > 0.5 else ""
        w(f"| {row['repo']} | {row['shape']} | {s['projects']} | {share}{heavy} "
          f"| {s['empty_projects']} | {found}{flag} | {s['source_files']:,} |")
    w("\n**Fixture-shaped** counts projects whose path contains a `test`, `spec`, "
      "`fixture`, `example` or `registry` segment; **Empty** counts manifests with no "
      "source file under them at all. Neither is a discovery bug — they are real "
      "manifests — but both inflate every number below, and a ⚠ marks a repository "
      "where most of what tropism found is not the code anyone wants reported on. "
      "`exclude` in `tropism.toml` is the answer, and none of these repositories has "
      "one.\n")
    w("A ⚠ beside a language means the corpus expected it and discovery did not find "
      "it. For C++ that is a property of the ecosystem rather than a defect — most "
      "open-source C++ ships no Conan or vcpkg manifest at all (design/19).\n")

    # ---------------------------------------------------------------- D5/D3
    w("## D3/D5 — Findings by check\n")
    w("Counts only. Soundness needs the oracle columns and the hand audit below; "
      "a count is not a verdict.\n")
    w("| Repository | " + " | ".join(INFERRED) + " | per 1k | Oracles available |")
    w("| --- | " + " | ".join("---" for _ in INFERRED) + " | --- | --- |")
    for row, data in done:
        s = summarise(data)
        before = load_baseline(row["slug"])
        prior = summarise(before) if before and "error" not in before else None
        cells = [
            str(s["by_check"].get(check, 0))
            + delta(
                s["by_check"].get(check, 0),
                prior["by_check"].get(check, 0) if prior else None,
            )
            for check in INFERRED
        ]
        oracles = oracle_status(row["slug"])
        got = ", ".join(k for k, ok in sorted(oracles.items()) if ok) or "—"
        density = per_thousand(s["findings"], s["source_files"])
        w(f"| {row['repo']} | " + " | ".join(cells) + f" | {density} | {got} |")
    w("")
    w("**per 1k** normalises by source files, because raw counts across repositories "
      "of wildly different size are not comparable — elasticsearch has 31,458 files "
      "and flask has 83. Density is what says whether a check is quiet or noisy.\n")

    confidence = collections.Counter()
    severity = collections.Counter()
    for _, data in done:
        s = summarise(data)
        confidence.update(s["confidence"])
        severity.update(s["severity"])
    if confidence:
        w("**Confidence, across every finding.** A High-confidence rule violation and a "
          "Low-confidence `unused-dep` are different claims and should never be counted "
          "together.\n")
        total = sum(confidence.values())
        w("| Confidence | Findings | Share |")
        w("| --- | --- | --- |")
        for level in ("high", "medium", "low"):
            n = confidence.get(level, 0)
            w(f"| {level} | {n:,} | {100 * n // max(total, 1)}% |")
        w("")

    # ---------------------------------------------------------------- D2
    w("## Per-language\n")
    w("The split design/19 asks for. tropism is not one tool with one accuracy — it is "
      "ten providers of visibly different maturity, and a mean over them describes "
      "none of them.\n")
    langs = by_language(done)
    w("| Language | Repos | Projects | Files | Statements resolved | All imports | "
      "Findings | per 1k files |")
    w("| --- | --- | --- | --- | --- | --- | --- | --- |")
    for language, entry in sorted(langs.items()):
        if entry["statements"]:
            srate = (entry["statements"] - entry["unresolved_statements"]) / entry["statements"]
            stmt = f"{100 * srate:.1f}%" + (" ⚠" if srate < 0.95 else "")
        else:
            stmt = "—"
        if entry["imports"]:
            rate = (entry["imports"] - entry["unresolved"]) / entry["imports"]
            allc = f"{100 * rate:.1f}%"
        else:
            allc = "—"
        w(f"| {language} | {len(entry['repos'])} | {entry['projects']} | "
          f"{entry['files']:,} | {stmt} | {allc} | {entry['findings']:,} | "
          f"{per_thousand(entry['findings'], entry['files'])} |")
    w("")

    w("## D2 — Resolution\n")
    w("The share of imports tropism understood, and **the number that caps every "
      "hygiene finding's confidence**: below 90% `unused-dep` and `missing-dep` drop "
      "to Low. design/19 targets ≥95% and treats below 80% as meaning that language's "
      "findings should not be trusted.\n")
    measured = [(row, summarise(d)) for row, d in done if summarise(d)["has_resolution"]]
    if measured:
        w("| Repository | Imports | Resolved (all) | Statements | Resolved (statements) |")
        w("| --- | --- | --- | --- | --- |")
        for row, s in measured:
            rate, srate = s["resolution_rate"], s["statement_rate"]
            all_cell = f"{100 * rate:.1f}%" if rate is not None else "—"
            st_cell = f"{100 * srate:.1f}%" if srate is not None else "—"
            if srate is not None and srate < 0.95:
                st_cell += " ⚠"
            w(f"| {row['repo']} | {s['imports']:,} | {all_cell} | {s['statements']:,} "
              f"| {st_cell} |")
        w("")
        w("**Read the statement column, not the first one.** *Resolved (all)* counts "
          "bare path references as failures, and Rust leaves an unrecognised path root "
          "unresolved *by design* — `Palette::plain()` is a local type, and calling it "
          "external would invent a missing dependency in every file. So on Rust the "
          "first column largely measures a deliberate decision. *Resolved (statements)* "
          "counts only imports tropism was asked to resolve and could not, which is what "
          "provider completeness means.\n")
        w("**`rate` — the first column — is still what caps hygiene confidence.** Where "
          "the two diverge sharply, every hygiene finding in that project is pinned to "
          "Low for a reason unrelated to whether tropism understood the code.\n")
        reasons = collections.Counter()
        for _, s in measured:
            reasons.update(s["unresolved_reasons"])
        if reasons:
            w("**Why imports did not resolve.** This is the actionable list: each line "
              "names a provider gap in the ecosystem's own vocabulary, ordered by how "
              "much it costs.\n")
            w("| Count | Reason |")
            w("| --- | --- |")
            for reason, count in reasons.most_common(15):
                w(f"| {count:,} | {reason[:110]} |")
            w("")
    else:
        w("> **Not measurable from these results.** They predate the `resolution` field "
          "in the report contract. Re-run with `FORCE=1 ./run.sh` — until then D2, a "
          "primary dimension, is simply unmeasured, and the confidence attached to "
          "every hygiene finding below is unexplained.\n")

    w("## D2b — Check availability\n")
    w("`unavailable` is not a failure — it is the honest answer where no resolved "
      "tree exists (S3). What matters is whether the *reason* is right.\n")
    reasons = collections.Counter()
    for _, data in done:
        for check, reason in summarise(data)["unavailable_reasons"]:
            reasons[(check, reason[:90])] += 1
    w("| Check | Reason | Projects |")
    w("| --- | --- | --- |")
    for (check, reason), count in reasons.most_common(15):
        w(f"| {check} | {reason}… | {count} |")
    w("")

    # ---------------------------------------------------------------- D6
    w("## D6 — Workspace boundaries and sibling exemptions\n")
    w("An exemption is a deliberate blind spot. This is the first time its size "
      "has been seen on real code.\n")
    w("| Repository | Packages exempted | Imports covered |")
    w("| --- | --- | --- |")
    for row, data in done:
        s = summarise(data)
        if s["sibling_exemptions"]:
            w(f"| {row['repo']} | {s['sibling_exemptions']} | {s['exempted_imports']} |")
    w("\nRun `tropism workspaces <repo>` to check origins; any `language` origin "
      "where the ecosystem *does* declare a workspace is a bug in the D14 work.\n")

    # ---------------------------------------------------------------- D7/D8
    w("## D7/D8 — Robustness and scale\n")
    w("| Repository | Seconds | Source files | Files/sec | Skipped files |")
    w("| --- | --- | --- | --- | --- |")
    for row, data in done:
        s = summarise(data)
        secs = s["seconds"]
        rate = f"{s['source_files'] / secs:,.0f}" if secs else "—"
        flag = " ⚠" if s["skipped_files"] else ""
        w(f"| {row['repo']} | {secs if secs is not None else '—'} "
          f"| {s['source_files']:,} | {rate} | {s['skipped_files']}{flag} |")
    w("")

    # ---------------------------------------------------------------- D4
    w("## D4 — Manifest hygiene: outstanding hand audit\n")
    w("**Not computable.** design/10 measured 63% false positives on JavaScript by "
      "reading 35 findings against the source; nothing automated can replace that, "
      "and the question here is whether that number is a JavaScript number or a "
      "tropism number.\n")
    pool = collections.Counter()
    for row, data in done:
        for project in data.get("projects", []):
            for finding in project.get("findings", []):
                if finding["check"] in ("unused-dep", "missing-dep"):
                    pool[project["language"]] += 1
    if pool:
        w("| Language | Hygiene findings available to sample |")
        w("| --- | --- |")
        for language, count in sorted(pool.items()):
            w(f"| {language} | {count} |")
        w("\nDraw the sample with `./report.py --audit-sample 20` — seeded, so a "
          "second auditor grades the same findings and the two audits are "
          "comparable.\n")
    else:
        w("No hygiene findings in the corpus yet.\n")

    w("## Not covered by any oracle\n")
    w("C#, C++ and Swift have no automated ground truth (see "
      "`oracles/Dockerfile.oracles`). Their numbers above are **unverified counts**, "
      "not measured accuracy, and must not be reported as though they were.\n")
    return "\n".join(out)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("-o", "--output", metavar="PATH",
                        help=f"where to write the report (default {DEFAULT_OUTPUT.name})")
    parser.add_argument("--stdout", action="store_true",
                        help="print the report instead of writing it")
    parser.add_argument("--baseline", metavar="DIR",
                        help="a prior results/ directory; adds a delta to each count")
    parser.add_argument("--json", action="store_true", help="machine-readable summary")
    parser.add_argument("--audit-sample", type=int, metavar="N",
                        help="draw N hygiene findings per language for the D4 audit")
    args = parser.parse_args()

    global BASELINE
    if args.baseline:
        BASELINE = pathlib.Path(args.baseline)
        if not BASELINE.is_dir():
            print(f"no such baseline directory: {BASELINE}", file=sys.stderr)
            return 1

    corpus = load_corpus()
    if not RESULTS.exists() or not any(RESULTS.glob("*.json")):
        print("no results yet — run ./run.sh first", file=sys.stderr)
        return 1

    if args.audit_sample:
        print(json.dumps(draw_audit_sample(corpus, args.audit_sample), indent=2))
        return 0
    if args.json:
        payload = {
            row["repo"]: summarise(data)
            for row in corpus
            if (data := load(row["slug"])) and "error" not in data
        }
        print(json.dumps(payload, indent=2, default=str))
        return 0

    # Exactly one trailing newline: the end-of-file-fixer pre-commit hook
    # rewrites anything else, so a report regenerated and committed would
    # otherwise be modified by the hook on every single run.
    text = markdown(corpus).rstrip("\n") + "\n"
    if args.stdout:
        print(text, end="")
        return 0

    output = pathlib.Path(args.output) if args.output else DEFAULT_OUTPUT
    output.parent.mkdir(parents=True, exist_ok=True)
    unchanged = output.exists() and output.read_text() == text
    output.write_text(text)
    analyzed = sum(1 for row in corpus if (d := load(row["slug"])) and "error" not in d)
    print(
        f"wrote {output} — {analyzed}/{len(corpus)} repositories"
        + (" (unchanged)" if unchanged else ""),
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
