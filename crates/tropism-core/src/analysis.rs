//! Analyzers: pure functions from graphs to findings.
//!
//! No analyzer performs I/O. It receives a finished [`AnalysisContext`], which is
//! what makes every one of them testable against a hand-built context with no
//! fixture repository at all. See `design/04-analyzers.md`.

use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;

use crate::graph::{ModuleGraph, ModuleId};
use crate::model::{DeclaredDep, Project, ResolvedDep};
use crate::provider::{ImportForm, ImportTarget};
use crate::report::{CheckId, CheckStatus, Confidence, Evidence, Finding, Severity};

/// One import site, after resolution.
#[derive(Debug, Clone)]
pub struct ResolvedImport {
    pub file: Utf8PathBuf,
    /// The module this file belongs to. Carried explicitly rather than re-derived
    /// from the path: the provider decides the mapping, and matching by path prefix
    /// cites files from sibling directories.
    pub owner: ModuleId,
    pub line: u32,
    pub raw: String,
    /// Whether this was an import *statement* or a bare path reference.
    ///
    /// The distinction matters to the resolution rate. Rust deliberately leaves an
    /// unrecognised path root `Unresolved` — `Palette::plain()` is a local type, and
    /// treating it as external would invent a missing dependency in every file — so
    /// counting path references as failed resolutions measures a design decision
    /// rather than a provider gap.
    pub form: ImportForm,
    pub target: ImportTarget,
}

/// Everything the analyzers get to see.
pub struct AnalysisContext {
    pub project: Project,
    pub module_graph: ModuleGraph,
    pub declared: Vec<DeclaredDep>,
    pub imports: Vec<ResolvedImport>,
    /// `None` when no lockfile was present. Every resolved-tree check reports
    /// `Unavailable` in that case rather than guessing from version ranges.
    pub resolved_tree: Option<Vec<ResolvedDep>>,
    /// Set when the ecosystem has a lockfile that is not a resolved graph, which
    /// is a different situation from having no lockfile at all.
    pub resolved_tree_note: Option<String>,
    /// Packages published by other projects in the same scan.
    ///
    /// A monorepo sibling is resolvable at runtime through the workspace even when
    /// the importing package does not declare it, so reporting it missing is wrong.
    /// It stays visible to `unused-dep`, which still needs to see it used.
    pub sibling_packages: Vec<String>,
}

impl AnalysisContext {
    /// Share of imports the provider could classify. The best available proxy for
    /// provider completeness, and the thing that caps hygiene confidence.
    pub fn resolution_rate(&self) -> f64 {
        if self.imports.is_empty() {
            return 1.0;
        }
        let unresolved = self
            .imports
            .iter()
            .filter(|i| matches!(i.target, ImportTarget::Unresolved { .. }))
            .count();
        1.0 - (unresolved as f64 / self.imports.len() as f64)
    }

    /// The resolution picture, for the report. Same numerator and denominator as
    /// [`Self::resolution_rate`], so what a reader sees is what capped the
    /// confidence.
    pub fn resolution(&self) -> crate::report::Resolution {
        use crate::report::{Resolution, UnresolvedReason};
        let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
        let mut unresolved = 0;
        let mut statements = 0;
        let mut unresolved_statements = 0;
        for import in &self.imports {
            let is_statement = import.form == ImportForm::Statement;
            if is_statement {
                statements += 1;
            }
            if let ImportTarget::Unresolved { reason } = &import.target {
                unresolved += 1;
                if is_statement {
                    unresolved_statements += 1;
                }
                *reasons.entry(reason.as_str()).or_default() += 1;
            }
        }
        let mut reasons: Vec<UnresolvedReason> = reasons
            .into_iter()
            .map(|(reason, count)| UnresolvedReason {
                reason: reason.to_owned(),
                count,
            })
            .collect();
        // Most frequent first, then alphabetically, so the list is deterministic.
        reasons.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.reason.cmp(&b.reason)));
        reasons.truncate(10);

        Resolution {
            imports: self.imports.len(),
            resolved: self.imports.len() - unresolved,
            unresolved,
            rate: self.resolution_rate(),
            statements,
            statement_rate: if statements == 0 {
                1.0
            } else {
                1.0 - (unresolved_statements as f64 / statements as f64)
            },
            reasons,
        }
    }

    /// Confidence ceiling for checks that compare declarations against imports.
    ///
    /// Never `High`: an import tropism cannot see — reflection, code generation, a
    /// build tag it did not evaluate — is invisible by construction.
    fn hygiene_confidence(&self) -> Confidence {
        if self.resolution_rate() < 0.9 {
            Confidence::Low
        } else {
            Confidence::Medium
        }
    }

    fn external_imports(&self) -> impl Iterator<Item = (&ResolvedImport, &str)> {
        self.imports
            .iter()
            .filter_map(|import| match &import.target {
                ImportTarget::External(name) => Some((import, name.as_str())),
                _ => None,
            })
    }
}

pub enum CheckResult {
    Ran(Vec<Finding>),
    Unavailable(String),
}

pub trait Analyzer {
    fn check_id(&self) -> CheckId;
    fn run(&self, ctx: &AnalysisContext) -> CheckResult;
}

/// Runs every analyzer, returning per-check status alongside the findings.
pub fn run_all(ctx: &AnalysisContext) -> (BTreeMap<CheckId, CheckStatus>, Vec<Finding>) {
    let analyzers: Vec<Box<dyn Analyzer>> = vec![
        Box::new(CycleAnalyzer),
        Box::new(UnusedDepAnalyzer),
        Box::new(MissingDepAnalyzer),
        Box::new(VersionConflictAnalyzer),
        Box::new(DiamondAnalyzer),
        Box::new(BloatAnalyzer),
    ];

    let mut statuses = BTreeMap::new();
    let mut findings = Vec::new();
    for analyzer in analyzers {
        match analyzer.run(ctx) {
            CheckResult::Ran(mut found) => {
                statuses.insert(
                    analyzer.check_id(),
                    CheckStatus::Ran {
                        finding_count: found.len(),
                    },
                );
                findings.append(&mut found);
            }
            CheckResult::Unavailable(reason) => {
                statuses.insert(analyzer.check_id(), CheckStatus::unavailable(reason));
            }
        }
    }
    (statuses, findings)
}

// ---------------------------------------------------------------------------

pub struct CycleAnalyzer;

impl Analyzer for CycleAnalyzer {
    fn check_id(&self) -> CheckId {
        CheckId::Cycle
    }

    fn run(&self, ctx: &AnalysisContext) -> CheckResult {
        let build_cycles = ctx.module_graph.cycles().into_iter().map(|members| {
            let count = members.len();
            (
                members,
                format!("import cycle among {count} modules"),
                "build",
            )
        });

        // A test-build cycle is a path from `pkg [test]` back to `pkg`, not an SCC.
        let test_cycles = ctx.module_graph.test_cycles().into_iter().map(|path| {
            let message = format!(
                "import cycle in the test build of `{}`, via {}",
                path[0].name,
                path[1..path.len() - 1]
                    .iter()
                    .map(ModuleId::to_string)
                    .collect::<Vec<_>>()
                    .join(" → ")
            );
            (path, message, "test")
        });

        let findings = build_cycles
            .chain(test_cycles)
            .map(|(members, message, kind)| {
                let labels: Vec<String> = members.iter().map(ModuleId::to_string).collect();
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();

                let evidence: Vec<Evidence> = members
                    .iter()
                    .filter_map(|member| {
                        // The first import from this member into another member.
                        // Matched on the owning module, not a path prefix — the
                        // latter cites `tsdb/agent/x.go` as evidence for `tsdb`.
                        ctx.imports.iter().find(|import| {
                            &import.owner == member
                                && matches!(&import.target, ImportTarget::Internal(target)
                                    if members.iter().any(|m| &m.name == target))
                        })
                    })
                    .map(|import| {
                        Evidence::new(
                            import.file.clone(),
                            Some(import.line),
                            format!("imports {}", import.raw),
                        )
                    })
                    .collect();

                Finding::new(
                    CheckId::Cycle,
                    &ctx.project.root,
                    &refs,
                    Severity::Warning,
                    Confidence::High,
                    message,
                )
                .with_evidence(evidence)
                .with_details(serde_json::json!({
                    "members": labels,
                    "kind": kind,
                    // Cycles are found at two scopes; see `pipeline::project_cycles`.
                    "scope": "module",
                }))
            })
            .collect();

        CheckResult::Ran(findings)
    }
}

// ---------------------------------------------------------------------------

pub struct UnusedDepAnalyzer;

impl Analyzer for UnusedDepAnalyzer {
    fn check_id(&self) -> CheckId {
        CheckId::UnusedDep
    }

    fn run(&self, ctx: &AnalysisContext) -> CheckResult {
        let used: BTreeSet<&str> = ctx.external_imports().map(|(_, name)| name).collect();
        let confidence = ctx.hygiene_confidence();

        let findings = ctx
            .declared
            .iter()
            .filter(|dep| dep.kind.expects_direct_import())
            .filter(|dep| !used.contains(dep.name.as_str()))
            .map(|dep| {
                Finding::new(
                    CheckId::UnusedDep,
                    &ctx.project.root,
                    &[dep.name.as_str()],
                    Severity::Warning,
                    confidence,
                    format!("`{}` is declared but never imported", dep.name),
                )
                .with_evidence([Evidence::new(
                    dep.declared_at.file.clone(),
                    dep.declared_at.line,
                    "declared here",
                )])
                .with_details(serde_json::json!({
                    "dependency": dep.name,
                    "requirement": dep.requirement,
                }))
            })
            .collect();

        CheckResult::Ran(findings)
    }
}

// ---------------------------------------------------------------------------

pub struct MissingDepAnalyzer;

impl Analyzer for MissingDepAnalyzer {
    fn check_id(&self) -> CheckId {
        CheckId::MissingDep
    }

    fn run(&self, ctx: &AnalysisContext) -> CheckResult {
        let declared: BTreeSet<&str> = ctx
            .declared
            .iter()
            .map(|d| d.name.as_str())
            .chain(ctx.sibling_packages.iter().map(String::as_str))
            .collect();
        let confidence = ctx.hygiene_confidence();

        // Group by package so one undeclared dependency imported in nine files is
        // one finding with nine pieces of evidence, not nine findings.
        let mut by_package: BTreeMap<&str, Vec<&ResolvedImport>> = BTreeMap::new();
        for (import, name) in ctx.external_imports() {
            if !declared.contains(name) {
                by_package.entry(name).or_default().push(import);
            }
        }

        let findings = by_package
            .into_iter()
            .map(|(name, sites)| {
                let evidence = sites.iter().take(5).map(|import| {
                    Evidence::new(
                        import.file.clone(),
                        Some(import.line),
                        format!("imports {}", import.raw),
                    )
                });
                Finding::new(
                    CheckId::MissingDep,
                    &ctx.project.root,
                    &[name],
                    Severity::Error,
                    confidence,
                    format!("`{name}` is imported but not declared"),
                )
                .with_evidence(evidence)
                .with_details(serde_json::json!({
                    "dependency": name,
                    "import_sites": sites.len(),
                }))
            })
            .collect();

        CheckResult::Ran(findings)
    }
}

// ---------------------------------------------------------------------------

/// Why a resolved-tree check could not run.
fn unavailable_reason(ctx: &AnalysisContext) -> String {
    ctx.resolved_tree_note
        .clone()
        .unwrap_or_else(|| "no lockfile found; the resolved dependency tree is unknown".to_owned())
}

pub struct VersionConflictAnalyzer;

impl Analyzer for VersionConflictAnalyzer {
    fn check_id(&self) -> CheckId {
        CheckId::VersionConflict
    }

    fn run(&self, ctx: &AnalysisContext) -> CheckResult {
        let Some(resolved) = &ctx.resolved_tree else {
            return CheckResult::Unavailable(unavailable_reason(ctx));
        };

        // Group by package name; more than one version means the tree carries
        // duplicates. In npm this is legal and common (nesting is the mechanism
        // that permits it), so it is Info — a bundle-size and
        // instanceof-mismatch concern, not a build failure.
        let mut versions: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for dep in resolved {
            versions
                .entry(dep.name.as_str())
                .or_default()
                .insert(dep.version.as_str());
        }

        let findings = versions
            .into_iter()
            .filter(|(_, found)| found.len() > 1)
            .map(|(name, found)| {
                let list: Vec<&str> = found.iter().copied().collect();
                Finding::new(
                    CheckId::VersionConflict,
                    &ctx.project.root,
                    &[name],
                    Severity::Info,
                    Confidence::High,
                    format!(
                        "`{name}` is resolved at {} versions: {}",
                        list.len(),
                        list.join(", ")
                    ),
                )
                .with_evidence([Evidence::new(
                    ctx.project.lockfile.clone().unwrap_or_default(),
                    None,
                    format!("{} copies in the resolved tree", list.len()),
                )])
                .with_details(serde_json::json!({ "dependency": name, "versions": list }))
            })
            .collect();

        CheckResult::Ran(findings)
    }
}

pub struct DiamondAnalyzer;

impl Analyzer for DiamondAnalyzer {
    fn check_id(&self) -> CheckId {
        CheckId::DiamondDep
    }

    fn run(&self, ctx: &AnalysisContext) -> CheckResult {
        let Some(resolved) = &ctx.resolved_tree else {
            return CheckResult::Unavailable(unavailable_reason(ctx));
        };

        let by_key: BTreeMap<&str, &ResolvedDep> =
            resolved.iter().map(|dep| (dep.key.as_str(), dep)).collect();

        // Dependents are counted per package *name*, not per copy. When a resolver
        // duplicates a package to satisfy disagreeing dependents, each copy ends up
        // with exactly one dependent — so counting per copy finds nothing, which is
        // precisely backwards.
        let mut arms: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
        for dep in resolved {
            for edge in &dep.dependencies {
                if let Some(target) = by_key.get(edge.as_str()) {
                    arms.entry(target.name.as_str())
                        .or_default()
                        .insert(dep.name.as_str(), target.version.as_str());
                }
            }
        }

        let mut versions: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for dep in resolved {
            versions
                .entry(dep.name.as_str())
                .or_default()
                .insert(dep.version.as_str());
        }

        // Only diamonds with a consequence are reported. A shared dependency whose
        // arms agreed on one version is the normal shape of every tree, and
        // reporting those would bury the ones that actually cost something.
        let findings = arms
            .into_iter()
            .filter(|(name, dependents)| {
                dependents.len() > 1 && versions.get(name).is_some_and(|v| v.len() > 1)
            })
            .map(|(name, dependents)| {
                let split: Vec<String> = dependents
                    .iter()
                    .map(|(dependent, version)| format!("{dependent} → {version}"))
                    .collect();

                Finding::new(
                    CheckId::DiamondDep,
                    &ctx.project.root,
                    &[name],
                    Severity::Warning,
                    Confidence::High,
                    format!(
                        "`{name}` is pulled in by {} dependents that disagree on the version ({})",
                        dependents.len(),
                        split.join(", ")
                    ),
                )
                .with_evidence([Evidence::new(
                    ctx.project.lockfile.clone().unwrap_or_default(),
                    None,
                    format!("duplicated to satisfy {}", split.join(" and ")),
                )])
                .with_details(serde_json::json!({
                    "dependency": name,
                    "dependents": dependents,
                }))
            })
            .collect();

        CheckResult::Ran(findings)
    }
}

pub struct BloatAnalyzer;

impl Analyzer for BloatAnalyzer {
    fn check_id(&self) -> CheckId {
        CheckId::DependencyBloat
    }

    fn run(&self, _ctx: &AnalysisContext) -> CheckResult {
        CheckResult::Unavailable(
            "deferred: no crisp definition yet, see design/07-open-questions.md".to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DepKind, Language, Provenance};

    fn context(declared: Vec<DeclaredDep>, imports: Vec<ResolvedImport>) -> AnalysisContext {
        AnalysisContext {
            project: Project {
                root: Utf8PathBuf::from("svc"),
                language: Language::Go,
                manifests: vec!["svc/go.mod".into()],
                lockfile: None,
            },
            module_graph: ModuleGraph::new(),
            declared,
            imports,
            resolved_tree: None,
            resolved_tree_note: None,
            sibling_packages: Vec::new(),
        }
    }

    fn dep(name: &str, kind: DepKind) -> DeclaredDep {
        DeclaredDep {
            name: name.to_owned(),
            requirement: "v1.0.0".to_owned(),
            kind,
            declared_at: Provenance::new("svc/go.mod", Some(4)),
        }
    }

    fn import(file: &str, line: u32, raw: &str, target: ImportTarget) -> ResolvedImport {
        ResolvedImport {
            form: ImportForm::Statement,
            file: file.into(),
            owner: ModuleId::module("."),
            line,
            raw: raw.to_owned(),
            target,
        }
    }

    fn findings(result: CheckResult) -> Vec<Finding> {
        match result {
            CheckResult::Ran(findings) => findings,
            CheckResult::Unavailable(reason) => panic!("expected the check to run: {reason}"),
        }
    }

    #[test]
    fn unused_dep_flags_a_declared_but_unimported_package() {
        let ctx = context(vec![dep("github.com/a/b", DepKind::Runtime)], vec![]);
        let found = findings(UnusedDepAnalyzer.run(&ctx));
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("github.com/a/b"));
    }

    #[test]
    fn unused_dep_ignores_indirect_dependencies() {
        let ctx = context(vec![dep("github.com/a/b", DepKind::Indirect)], vec![]);
        assert!(
            findings(UnusedDepAnalyzer.run(&ctx)).is_empty(),
            "indirect deps are expected to have no import"
        );
    }

    #[test]
    fn unused_dep_is_silent_when_the_package_is_imported() {
        let ctx = context(
            vec![dep("github.com/a/b", DepKind::Runtime)],
            vec![import(
                "svc/main.go",
                3,
                "github.com/a/b/sub",
                ImportTarget::External("github.com/a/b".to_owned()),
            )],
        );
        assert!(findings(UnusedDepAnalyzer.run(&ctx)).is_empty());
    }

    #[test]
    fn missing_dep_flags_an_undeclared_import() {
        let ctx = context(
            vec![],
            vec![import(
                "svc/main.go",
                3,
                "github.com/x/y",
                ImportTarget::External("github.com/x/y".to_owned()),
            )],
        );
        let found = findings(MissingDepAnalyzer.run(&ctx));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].severity, Severity::Error);
    }

    #[test]
    fn missing_dep_groups_import_sites_into_one_finding() {
        let target = ImportTarget::External("github.com/x/y".to_owned());
        let ctx = context(
            vec![],
            vec![
                import("svc/a.go", 3, "github.com/x/y", target.clone()),
                import("svc/b.go", 4, "github.com/x/y", target.clone()),
                import("svc/c.go", 5, "github.com/x/y", target),
            ],
        );
        let found = findings(MissingDepAnalyzer.run(&ctx));
        assert_eq!(found.len(), 1, "one dependency, one finding");
        assert_eq!(found[0].evidence.len(), 3);
    }

    #[test]
    fn stdlib_and_internal_imports_never_produce_findings() {
        let ctx = context(
            vec![],
            vec![
                import("svc/main.go", 1, "fmt", ImportTarget::Stdlib),
                import(
                    "svc/main.go",
                    2,
                    "svc/db",
                    ImportTarget::Internal("db".to_owned()),
                ),
            ],
        );
        assert!(findings(MissingDepAnalyzer.run(&ctx)).is_empty());
    }

    #[test]
    fn unresolved_imports_never_produce_a_missing_finding() {
        let ctx = context(
            vec![],
            vec![import(
                "svc/main.go",
                3,
                "mystery",
                ImportTarget::Unresolved {
                    reason: "no rule matched".to_owned(),
                },
            )],
        );
        assert!(
            findings(MissingDepAnalyzer.run(&ctx)).is_empty(),
            "a guess must never become a confident finding"
        );
    }

    #[test]
    fn a_low_resolution_rate_downgrades_hygiene_confidence() {
        let unresolved = |n: u32| {
            import(
                "svc/main.go",
                n,
                "mystery",
                ImportTarget::Unresolved {
                    reason: "unknown".to_owned(),
                },
            )
        };
        let ctx = context(
            vec![dep("github.com/a/b", DepKind::Runtime)],
            vec![
                unresolved(1),
                unresolved(2),
                import("svc/main.go", 3, "fmt", ImportTarget::Stdlib),
            ],
        );
        assert!(ctx.resolution_rate() < 0.9);
        assert_eq!(
            findings(UnusedDepAnalyzer.run(&ctx))[0].confidence,
            Confidence::Low
        );
    }

    /// Hygiene findings are capped at Medium even with perfect resolution: an
    /// import tropism cannot see is invisible by construction.
    /// The reported number must be the one that capped the confidence, or a reader
    /// sees a Low-confidence finding and a resolution figure that does not explain
    /// it.
    #[test]
    fn reported_resolution_matches_the_rate_that_caps_confidence() {
        let ctx = context(
            vec![dep("github.com/a/b", DepKind::Runtime)],
            vec![
                import(
                    "svc/a.go",
                    1,
                    "github.com/a/b",
                    ImportTarget::External("github.com/a/b".to_owned()),
                ),
                import(
                    "svc/b.go",
                    1,
                    "mystery",
                    ImportTarget::Unresolved {
                        reason: "no idea".to_owned(),
                    },
                ),
                import(
                    "svc/c.go",
                    1,
                    "mystery",
                    ImportTarget::Unresolved {
                        reason: "no idea".to_owned(),
                    },
                ),
            ],
        );

        let resolution = ctx.resolution();
        assert_eq!(resolution.imports, 3);
        assert_eq!(resolution.resolved, 1);
        assert_eq!(resolution.unresolved, 2);
        assert!((resolution.rate - ctx.resolution_rate()).abs() < f64::EPSILON);
        // The reasons are what name the next provider gap.
        assert_eq!(resolution.reasons.len(), 1);
        assert_eq!(resolution.reasons[0].count, 2);
    }

    /// Ordering is by frequency then alphabetically, so two runs over the same
    /// input produce the same bytes.
    #[test]
    fn unresolved_reasons_are_ordered_deterministically() {
        let ctx = context(
            vec![],
            vec![
                import(
                    "svc/a.go",
                    1,
                    "z",
                    ImportTarget::Unresolved {
                        reason: "zebra".to_owned(),
                    },
                ),
                import(
                    "svc/b.go",
                    1,
                    "a",
                    ImportTarget::Unresolved {
                        reason: "apple".to_owned(),
                    },
                ),
                import(
                    "svc/c.go",
                    1,
                    "a",
                    ImportTarget::Unresolved {
                        reason: "apple".to_owned(),
                    },
                ),
            ],
        );
        let resolution = ctx.resolution();
        let reasons: Vec<&str> = resolution
            .reasons
            .iter()
            .map(|r| r.reason.as_str())
            .collect();
        assert_eq!(reasons, vec!["apple", "zebra"]);
    }

    #[test]
    fn hygiene_confidence_is_never_high() {
        let ctx = context(vec![dep("github.com/a/b", DepKind::Runtime)], vec![]);
        assert_eq!(ctx.resolution_rate(), 1.0);
        assert_eq!(
            findings(UnusedDepAnalyzer.run(&ctx))[0].confidence,
            Confidence::Medium
        );
    }

    #[test]
    fn resolved_tree_checks_are_unavailable_without_a_lockfile() {
        let ctx = context(vec![], vec![]);
        assert!(matches!(
            VersionConflictAnalyzer.run(&ctx),
            CheckResult::Unavailable(_)
        ));
        assert!(matches!(
            DiamondAnalyzer.run(&ctx),
            CheckResult::Unavailable(_)
        ));
    }

    #[test]
    fn an_ecosystem_specific_reason_is_reported_when_present() {
        let mut ctx = context(vec![], vec![]);
        ctx.resolved_tree_note = Some("go.sum is not a resolved graph".to_owned());
        match VersionConflictAnalyzer.run(&ctx) {
            CheckResult::Unavailable(reason) => assert!(reason.contains("go.sum")),
            CheckResult::Ran(_) => panic!("expected unavailable"),
        }
    }

    /// The analyzers cover exactly the analysis checks. Rule checks are filled in
    /// by the pipeline, since the ruleset is evaluated repo-wide.
    #[test]
    fn run_all_reports_status_for_every_analysis_check() {
        let ctx = context(vec![], vec![]);
        let (statuses, _) = run_all(&ctx);
        let covered: Vec<CheckId> = statuses.keys().copied().collect();
        assert_eq!(
            covered,
            CheckId::ANALYSIS.to_vec(),
            "no check may be silently absent"
        );
    }
}
