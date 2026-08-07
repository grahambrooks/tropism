//! Tests for `tropism check` — the scoped, rules-only run behind the pre-commit hook.
//!
//! Run against `demo/python`, which carries two rule violations in two different
//! files. That is the minimum shape needed to test a ratchet: one violation to
//! attribute to a change, and one to leave behind as pre-existing.
//!
//! See `design/14-incremental-checking.md`.

use camino::Utf8PathBuf;
use tropism_core::pipeline::{self, CheckOutcome, CheckScope, Options};
use tropism_core::report::CheckId;

fn demo(name: &str) -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("demo")
        .join(name)
}

fn check(name: &str, files: &[&str]) -> CheckOutcome {
    let providers = tropism_lang::registry();
    let scope = if files.is_empty() {
        CheckScope::Repository
    } else {
        CheckScope::Files(files.iter().map(Utf8PathBuf::from).collect())
    };
    pipeline::check(&demo(name), &providers, &Options::default(), &scope).expect("check failed")
}

fn violations(outcome: &CheckOutcome) -> Vec<String> {
    outcome
        .report
        .findings()
        .map(|finding| finding.message.clone())
        .collect()
}

/// The whole-repository scope is what CI runs, and it must agree with `analyze`
/// about the rules. If these two ever disagree, the hook is lying about what CI
/// will do.
#[test]
fn the_repository_scope_finds_every_violation() {
    let outcome = check("python", &[]);
    assert_eq!(violations(&outcome).len(), 2, "{:?}", violations(&outcome));
    assert_eq!(outcome.suppressed, Some(0));

    let providers = tropism_lang::registry();
    let full = pipeline::analyze(&demo("python"), &providers, &Options::default()).unwrap();
    let from_analyze: Vec<String> = full
        .findings()
        .filter(|f| matches!(f.check, CheckId::ModuleRule | CheckId::PackageRule))
        .map(|f| f.message.clone())
        .collect();
    assert_eq!(
        violations(&outcome),
        from_analyze,
        "check and analyze must not disagree about the rules"
    );
}

/// A violation belongs to the file at the *source* end of its edge.
#[test]
fn a_violation_is_attributed_to_the_file_that_introduces_it() {
    let outcome = check("python", &["src/app/main.py"]);
    let found = violations(&outcome);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].contains("`entry` may depend only on `api`"),
        "{found:?}"
    );
    // Not `Some(1)`. Counting the backlog means parsing every file, which is the
    // cost D36 removes, so a scoped run declines to count rather than reporting a
    // number it did not earn. See `CheckOutcome::suppressed`.
    assert_eq!(outcome.suppressed, None);
}

/// The ratchet, and the reason this feature exists: a repository that already
/// violates its rules still passes every commit that does not add a new violation.
/// No baseline file, nothing to regenerate after a refactor.
#[test]
fn a_clean_file_passes_while_the_repository_is_dirty() {
    let outcome = check("python", &["src/app/models.py"]);
    assert!(violations(&outcome).is_empty());
    assert_eq!(
        outcome.suppressed, None,
        "a scoped run must not claim a backlog figure it did not measure"
    );

    // The backlog is still countable — by the run that actually looks. This pairing
    // is the contract: fast and honest at commit time, exact on the whole
    // repository, and never a `0` that means "did not look".
    let whole = check("python", &[]);
    assert_eq!(whole.suppressed, Some(0));
    assert_eq!(violations(&whole).len(), 2);
}

/// The other end of the edge is not the commit's fault. `storage.py` is imported
/// by `main.py` in violation of a rule, but a change to `storage.py` did not
/// create that edge.
#[test]
fn the_target_end_of_an_edge_is_not_attributed() {
    let outcome = check("python", &["src/app/storage.py"]);
    let found = violations(&outcome);
    // Only the package rule, which storage.py itself violates by importing
    // requests — not the module rule, whose source end is main.py.
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("requests"), "{found:?}");
}

/// Editing the ruleset can invalidate anything, so an incremental scope cannot
/// honestly narrow it. Open question 3 in design/14, decided.
#[test]
fn changing_the_ruleset_widens_the_scope_to_the_repository() {
    let outcome = check("python", &["tropism.toml"]);
    assert!(outcome.widened_by_ruleset_change);
    assert_eq!(violations(&outcome).len(), 2);
    assert_eq!(
        outcome.suppressed,
        Some(0),
        "nothing was suppressed; all was checked"
    );
}

/// D36: a scoped run parses only the files it can report on.
///
/// The behavioural proof that extraction is actually skipped rather than merely
/// filtered afterwards. `storage.py` violates a package rule at its own source end,
/// so a run that parsed it would find that violation and then suppress it — and
/// `suppressed` would be `Some(1)`. `None` is only reachable if the file was never
/// parsed at all.
#[test]
fn a_scoped_run_does_not_parse_the_files_it_was_not_given() {
    let scoped = check("python", &["src/app/models.py"]);
    assert!(violations(&scoped).is_empty());
    assert_eq!(scoped.suppressed, None);

    // Same repository, same rules, whole-repository scope: now it does look, and
    // finds exactly the two violations `analyze` reports.
    let whole = check("python", &[]);
    assert_eq!(violations(&whole).len(), 2);
    assert_eq!(whole.suppressed, Some(0));
}

/// The widened path must not inherit the scoped path's parse narrowing.
///
/// A ruleset change reports on the whole repository, so it has to *parse* the whole
/// repository. Reporting from a partial parse here would be a silent clean result
/// in the one case where the blast radius is largest.
#[test]
fn a_widened_run_parses_everything_not_just_the_named_files() {
    let outcome = check("python", &["tropism.toml"]);
    assert_eq!(
        violations(&outcome).len(),
        2,
        "a widened run that only parsed tropism.toml would report nothing"
    );
}

/// A file tropism has never heard of is not an error and not a pass-by-accident:
/// it simply attributes nothing.
#[test]
fn an_unknown_file_attributes_nothing() {
    let outcome = check("python", &["README.md"]);
    assert!(violations(&outcome).is_empty());
    assert_eq!(outcome.suppressed, None);
}

/// `check` runs the rules and nothing else. The inferred checks are not merely
/// absent from the findings — they are reported as not having run, so a consumer
/// cannot read silence as a clean bill of health.
#[test]
fn the_inferred_checks_are_reported_as_not_run_rather_than_omitted() {
    let outcome = check("python", &[]);
    let project = &outcome.report.projects[0];

    for check in CheckId::ANALYSIS {
        match &project.checks[&check] {
            tropism_core::report::CheckStatus::Unavailable { reason } => {
                assert!(reason.contains("rules only"), "{check}: {reason}")
            }
            other => panic!("{check}: expected unavailable, got {other:?}"),
        }
    }
    for check in CheckId::RULES {
        assert!(
            matches!(
                project.checks[&check],
                tropism_core::report::CheckStatus::Ran { .. }
            ),
            "{check} must have run"
        );
    }
}

/// The per-check counts have to describe the scoped run, not the repository-wide
/// one they were computed from.
#[test]
fn check_counts_describe_the_scope_that_was_actually_checked() {
    let outcome = check("python", &["src/app/models.py"]);
    let project = &outcome.report.projects[0];
    for check in CheckId::RULES {
        assert_eq!(
            project.checks[&check],
            tropism_core::report::CheckStatus::Ran { finding_count: 0 },
            "{check}"
        );
    }
}

/// tropism's own repository must pass its own hook, at both scopes.
#[test]
fn tropism_passes_its_own_check() {
    let root = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let providers = tropism_lang::registry();
    let outcome = pipeline::check(
        &root,
        &providers,
        &Options::default(),
        &CheckScope::Repository,
    )
    .unwrap();

    assert!(
        outcome.report.findings().next().is_none(),
        "tropism violates its own ruleset: {:?}",
        violations(&outcome)
    );
    assert!(
        outcome.rules_evaluated > 0,
        "a hook with no rules blocks nothing"
    );
}
