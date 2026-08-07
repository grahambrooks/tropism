//! What bounds a set of mutually-importable projects.
//!
//! Every test here pins a behaviour that was wrong before `tropism-core::workspace`
//! existed. The sibling set used to be *every project in the scan root*, regardless
//! of language or workspace, so two undeclared imports that no package manager
//! would resolve produced no finding at all — while the rule engine, which is
//! repo-wide by design, reported the very same edges as violations. One analysis,
//! two answers about one import.
//!
//! See `design/07-open-questions.md`, question 1.

use camino::Utf8PathBuf;
use tropism_core::pipeline::{self, Options};
use tropism_core::report::{CheckId, Report};
use tropism_core::workspace::WorkspaceOrigin;

fn fixture(name: &str) -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn analyze(name: &str) -> Report {
    let providers = tropism_lang::registry();
    pipeline::analyze(&fixture(name), &providers, &Options::default()).expect("analysis failed")
}

/// The packages one project reports as imported-but-not-declared.
fn missing_in(report: &Report, root: &str) -> Vec<String> {
    report
        .projects
        .iter()
        .find(|project| project.project.root == root)
        .unwrap_or_else(|| panic!("no project at `{root}`"))
        .findings
        .iter()
        .filter(|finding| finding.check == CheckId::MissingDep)
        .map(|finding| finding.details["dependency"].as_str().unwrap().to_owned())
        .collect()
}

// ---------------------------------------------------------------------------
// The two leaks

/// A Rust crate named `mylib` must never satisfy a JavaScript `import 'mylib'`.
///
/// The sibling set is bounded by language *unconditionally* — not by the workspace
/// first and language second — because no workspace declaration, however
/// authoritative, can make a `.rlib` importable from Node.
#[test]
fn a_crate_in_another_language_never_satisfies_an_import() {
    let report = analyze("workspaces");
    assert_eq!(
        missing_in(&report, "jsapp"),
        vec!["mylib"],
        "a Rust crate published in the repository made a JS import look declared"
    );
}

/// Two npm workspaces in one repository are two workspaces. `@a/web` importing
/// `@b/kit` does not resolve — npm would fail — and tropism used to say nothing.
#[test]
fn a_sibling_in_a_different_workspace_is_still_undeclared() {
    let report = analyze("workspaces");
    let missing = missing_in(&report, "serviceA/packages/web");
    assert!(
        missing.contains(&"@b/kit".to_owned()),
        "a package from a different workspace was silently exempted: {missing:?}"
    );
}

// ---------------------------------------------------------------------------
// What must keep working

/// The exemption that earned its place. `10-js-evaluation.md` measured this
/// removing 107 findings across ten real repositories; narrowing the boundary must
/// not take those back.
#[test]
fn a_sibling_in_the_same_workspace_is_still_exempt() {
    let report = analyze("workspaces");
    let missing = missing_in(&report, "serviceA/packages/web");
    assert!(
        !missing.contains(&"@a/ui".to_owned()),
        "a genuine workspace sibling was reported as missing: {missing:?}"
    );
    // The control: a package published nowhere in the repository is still missing,
    // so the test above cannot pass by the check having stopped running.
    assert!(missing.contains(&"lodash".to_owned()), "{missing:?}");
}

/// An exemption is a deliberate blind spot, and a silent blind spot reads exactly
/// like a clean project — the failure mode `CheckStatus` exists to prevent.
#[test]
fn an_exemption_is_disclosed_with_its_provenance() {
    let report = analyze("workspaces");
    let project = report
        .projects
        .iter()
        .find(|project| project.project.root == "serviceA/packages/web")
        .expect("web project");

    let exemption = project
        .sibling_exemptions
        .iter()
        .find(|exemption| exemption.package == "@a/ui")
        .expect("the exemption that suppressed a missing-dep must be disclosed");

    assert_eq!(
        exemption.provided_by.as_deref(),
        Some(Utf8PathBuf::from("serviceA/packages/ui").as_path()),
        "an exemption nobody can trace to a project is one nobody can check"
    );
    assert_eq!(exemption.imports, 1);

    // `@b/kit` was reported, not exempted, so it must not appear here.
    assert!(
        !project
            .sibling_exemptions
            .iter()
            .any(|exemption| exemption.package == "@b/kit")
    );
}

// ---------------------------------------------------------------------------
// Boundary inference

#[test]
fn npm_workspaces_globs_are_read_from_the_manifest() {
    let providers = tropism_lang::registry();
    let report = pipeline::workspaces(&fixture("workspaces"), &providers, &Options::default())
        .expect("workspace resolution failed");

    let service = report
        .workspaces
        .iter()
        .find(|workspace| workspace.id == "serviceA")
        .expect("serviceA is declared by its own package.json");

    assert_eq!(service.origin, WorkspaceOrigin::Declared);
    assert_eq!(
        service.declared_by.as_deref(),
        Some(Utf8PathBuf::from("serviceA/package.json").as_path()),
        "a boundary nobody can trace back to a file is one nobody can correct"
    );
    assert_eq!(
        service.members,
        vec![
            Utf8PathBuf::from("serviceA"),
            Utf8PathBuf::from("serviceA/packages/ui"),
            Utf8PathBuf::from("serviceA/packages/web"),
        ]
    );
}

/// A project in an ecosystem that declares nothing falls back to language
/// grouping — and says so, because that is an inference rather than a fact.
#[test]
fn an_undeclared_project_falls_back_to_language_grouping() {
    let providers = tropism_lang::registry();
    let report = pipeline::workspaces(&fixture("workspaces"), &providers, &Options::default())
        .expect("workspace resolution failed");

    let rust = report
        .workspaces
        .iter()
        .find(|workspace| workspace.members.contains(&Utf8PathBuf::from("rustlib")))
        .expect("rustlib is in some workspace");

    assert_eq!(rust.origin, WorkspaceOrigin::Language);
    assert!(rust.declared_by.is_none());
    assert!(
        !rust.members.contains(&Utf8PathBuf::from("jsapp")),
        "the language fallback must not merge two languages"
    );
}

#[test]
fn crossings_name_both_ends_and_the_line() {
    let providers = tropism_lang::registry();
    let report = pipeline::workspaces(&fixture("workspaces"), &providers, &Options::default())
        .expect("workspace resolution failed");

    let crossing = report
        .crossings
        .iter()
        .find(|crossing| crossing.label == "@b/kit")
        .expect("the cross-workspace import must be listed");

    assert_eq!(crossing.from, "serviceA/packages/web/src/index.js");
    assert_eq!(crossing.to_workspace, "toolsB");
    assert_eq!(crossing.line, Some(1));
}

// ---------------------------------------------------------------------------
// The configured override

/// Python declares no workspace anywhere, so without `[[workspaces]]` every Python
/// project in a repository is one group and a cross-service import is invisible.
/// This is the case option E exists for.
#[test]
fn a_configured_boundary_splits_an_ecosystem_that_declares_none() {
    let report = analyze("workspaces-configured");
    let missing = missing_in(&report, "svc/alpha");

    assert!(
        missing.contains(&"gamma".to_owned()),
        "a project outside the configured workspace was exempted anyway: {missing:?}"
    );
    assert!(
        !missing.contains(&"beta".to_owned()),
        "a member of the configured workspace must stay exempt: {missing:?}"
    );
}

#[test]
fn a_configured_boundary_outranks_the_language_fallback() {
    let providers = tropism_lang::registry();
    let report = pipeline::workspaces(
        &fixture("workspaces-configured"),
        &providers,
        &Options::default(),
    )
    .expect("workspace resolution failed");

    let svc = report
        .workspaces
        .iter()
        .find(|workspace| workspace.id == "svc")
        .expect("the configured workspace");
    assert_eq!(svc.origin, WorkspaceOrigin::Configured);
    assert!(!svc.members.contains(&Utf8PathBuf::from("other/gamma")));
}

/// `--no-rules` turns the override off, which is what makes it possible to see
/// what the ecosystems declare on their own.
#[test]
fn no_rules_ignores_a_configured_boundary() {
    let providers = tropism_lang::registry();
    let options = Options {
        use_rules: false,
        ..Options::default()
    };
    let report = pipeline::workspaces(&fixture("workspaces-configured"), &providers, &options)
        .expect("workspace resolution failed");

    assert!(
        report
            .workspaces
            .iter()
            .all(|workspace| workspace.origin == WorkspaceOrigin::Language),
        "with the ruleset off, nothing but the language fallback should remain"
    );
}

// ---------------------------------------------------------------------------
// The rule

/// The boundary as something a team asserts, rather than something tropism infers a
/// severity for. A cross-workspace import resolves today through hoisting and
/// breaks when the package is built on its own, so whether that is an error is the
/// team's call — which is exactly the argument for rules over inferred checks.
#[test]
fn crosses_workspace_reports_an_edge_that_leaves_its_workspace() {
    let providers = tropism_lang::registry();
    let options = Options {
        rules_path: Some(fixture("workspaces").join("boundary.toml")),
        ..Options::default()
    };
    let report =
        pipeline::analyze(&fixture("workspaces"), &providers, &options).expect("analysis failed");

    let violations: Vec<&str> = report
        .findings()
        .filter(|finding| finding.check == CheckId::ModuleRule)
        .filter_map(|finding| finding.details["rule_id"].as_str())
        .collect();

    assert!(
        violations.contains(&"no-cross-workspace"),
        "the boundary rule did not fire: {violations:?}"
    );

    // Both ends are named, so the finding says which boundary was crossed rather
    // than only that one was.
    let finding = report
        .findings()
        .find(|finding| finding.details["rule_id"] == "no-cross-workspace")
        .unwrap();
    assert!(finding.message.contains("workspace"), "{}", finding.message);
}

/// A rule that cannot fire protects nothing, and this one is easy to write before
/// any boundary exists to enforce. Same reasoning as a rule naming a renamed
/// module — it is reported rather than silently satisfied.
#[test]
fn crosses_workspace_is_stale_in_a_single_workspace_repository() {
    let providers = tropism_lang::registry();
    let options = Options {
        rules_path: Some(fixture("workspaces").join("boundary.toml")),
        ..Options::default()
    };
    let report =
        pipeline::analyze(&fixture("go-service"), &providers, &options).expect("analysis failed");

    let stale = report
        .findings()
        .any(|finding| finding.details["stale"] == true);
    assert!(
        stale,
        "a boundary rule in a one-workspace repository must be reported as stale"
    );
}

/// D2: a workspace member says *where* the resolved tree is, rather than "no
/// lockfile found".
///
/// The registered fix was to walk upward and adopt the ancestor's lockfile. That
/// would report one workspace-wide resolution once per member — 17 findings times
/// five crates on tropism's own repository — and attribute a shared resolution to
/// crates that do not own it. The misleading part was the message.
#[test]
fn a_workspace_member_names_the_lockfile_that_covers_it() {
    let providers = tropism_lang::registry();
    let root = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let report = pipeline::analyze(&root, &providers, &Options::default()).expect("analysis");

    let member = report
        .projects
        .iter()
        .find(|p| p.project.root == "crates/tropism-core")
        .expect("tropism-core is a workspace member");

    let reason = match member.checks.get(&CheckId::VersionConflict) {
        Some(tropism_core::report::CheckStatus::Unavailable { reason }) => reason.clone(),
        other => panic!("expected unavailable, got {other:?}"),
    };
    assert!(
        reason.contains("Cargo.lock"),
        "the reason must name where the resolved tree actually is: {reason}"
    );

    // And the checks must still run exactly once, on the project that owns it.
    let owners: Vec<&str> = report
        .projects
        .iter()
        .filter(|p| {
            p.findings
                .iter()
                .any(|f| f.check == CheckId::VersionConflict)
        })
        .map(|p| p.project.root.as_str())
        .collect();
    assert_eq!(
        owners,
        vec![""],
        "resolved-tree findings must not duplicate"
    );
}
