//! Team-authored dependency rules.
//!
//! See `design/11-dependency-rules.md`. The property that makes these worth having
//! is that a violation is the *presence* of an import or a declaration — a fact
//! about a line of source — rather than the absence of one. That is why rule
//! findings are `High` confidence while `unused-dep` is not.
//!
//! Implemented: `deny`, `independent`, `allow_only`, package denylists,
//! `allowed_in` scoping, and closed-world approved lists. Not yet implemented, and
//! rejected with a clear error rather than silently ignored: `layers`, `require`,
//! `transitive`, and version constraints.

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;

use crate::report::{CheckId, Confidence, Evidence, Finding, Severity};

pub const RULESET_FILE: &str = "tropism.toml";

/// Whether a dependency edge came from a manifest or from source.
///
/// A rule broken only at the declaration level is still broken: the coupling is
/// real and the import is one commit away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeLevel {
    Declared,
    Imported,
}

impl EdgeLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Imported => "imported",
        }
    }
}

/// One dependency between two places in the repository.
#[derive(Debug, Clone)]
pub struct DependencyEdge {
    pub from: Utf8PathBuf,
    pub to: Utf8PathBuf,
    pub line: Option<u32>,
    /// What was written — the import specifier or the dependency name.
    pub label: String,
    pub level: EdgeLevel,
    /// The module each end belongs to, project-qualified.
    ///
    /// Rules only need the paths; the repo-wide cycle graph needs the modules, and
    /// collecting them here avoids a second walk. `to_module` is `None` when the
    /// provider could not name the module inside the target project, in which case
    /// the cycle graph falls back to that project's root.
    pub from_module: Option<crate::graph::ModuleId>,
    pub to_module: Option<crate::graph::ModuleId>,
}

/// One use of an external package.
#[derive(Debug, Clone)]
pub struct PackageUse {
    pub package: String,
    pub at: Utf8PathBuf,
    pub line: Option<u32>,
    pub level: EdgeLevel,
}

// ---------------------------------------------------------------------------
// File format

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRuleset {
    #[serde(default)]
    schema_version: Option<u32>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    workspaces: Vec<RawWorkspace>,
    #[serde(default)]
    modules: BTreeMap<String, RawModule>,
    #[serde(default)]
    module_rules: Vec<RawModuleRule>,
    #[serde(default)]
    packages: RawPackages,
    #[serde(default)]
    package_rules: Vec<RawPackageRule>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawModule {
    One(String),
    Many { paths: Vec<String> },
}

/// A workspace boundary stated by hand, overriding whatever the ecosystem's own
/// files say — or supplying one where the ecosystem states nothing.
///
/// `members` are globs over project roots, written relative to the scan root like
/// every other glob in this file. Two glob dialects in one file would be a trap, so
/// there is only one. Omitting `members` means "every project under `root`".
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorkspace {
    root: String,
    #[serde(default)]
    members: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPackages {
    #[serde(default)]
    unlisted: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModuleRule {
    id: String,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    deny: Option<RawFromTo>,
    #[serde(default)]
    independent: Option<Vec<String>>,
    #[serde(default)]
    allow_only: Option<RawFromTo>,
    /// Forbid any edge leaving its workspace.
    #[serde(default)]
    crosses_workspace: Option<bool>,
    // Specified but not implemented; present so the parser can reject them by name
    // rather than with a confusing unknown-field error.
    #[serde(default)]
    layers: Option<Vec<String>>,
    #[serde(default)]
    require: Option<toml::Value>,
    #[serde(default)]
    transitive: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFromTo {
    from: String,
    #[serde(default)]
    to: StringOrList,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPackageRule {
    id: String,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    replacement: Option<String>,
    #[serde(default)]
    deny: Option<StringOrList>,
    #[serde(default)]
    packages: Option<StringOrList>,
    #[serde(default)]
    allowed_in: Option<StringOrList>,
    #[serde(default)]
    allow: Option<StringOrList>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum StringOrList {
    #[default]
    Empty,
    One(String),
    Many(Vec<String>),
}

impl StringOrList {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::Empty => Vec::new(),
            Self::One(one) => vec![one],
            Self::Many(many) => many,
        }
    }
}

// ---------------------------------------------------------------------------
// Compiled form

struct ModuleGlob {
    name: String,
    pattern: String,
    matcher: globset::GlobMatcher,
}

enum ModuleRuleKind {
    /// `from` must not depend on any of `to`.
    Deny { from: String, to: Vec<String> },
    /// No member may depend on any other member.
    Independent(Vec<String>),
    /// `from` may depend only on `to`.
    AllowOnly { from: String, to: Vec<String> },
    /// No edge may leave its workspace.
    ///
    /// The one rule kind that names no module: it is about the *boundary*, which
    /// `crate::workspace` establishes from the ecosystem's own files rather than
    /// from a glob someone has to keep in step with the repository layout. An
    /// undeclared cross-workspace import resolves through hoisting today and breaks
    /// on publish, so a team that wants it to be an error can now say so instead of
    /// waiting for tropism to guess a severity for them.
    CrossesWorkspace,
}

struct ModuleRule {
    id: String,
    severity: Severity,
    reason: Option<String>,
    kind: ModuleRuleKind,
}

struct PackageRule {
    id: String,
    severity: Severity,
    reason: Option<String>,
    replacement: Option<String>,
    /// Packages this rule forbids outright.
    denied: Vec<String>,
    /// Packages restricted to a set of modules.
    scoped: Vec<String>,
    allowed_in: Vec<String>,
    /// Packages this rule explicitly permits, for a closed-world ruleset.
    allowed: Vec<String>,
}

/// Paths kept out of the analysis entirely.
///
/// Deliberate blind spots, so they are counted and reported: a repository that
/// excludes half of itself must not look like one that was fully analyzed. Same
/// reasoning as `CheckStatus` — silence never means clean.
#[derive(Default)]
pub struct ExcludeSet {
    patterns: Vec<(String, globset::GlobMatcher)>,
}

impl ExcludeSet {
    /// The pattern that excludes this path, if any.
    pub fn excluded_by(&self, path: &Utf8Path) -> Option<&str> {
        self.patterns
            .iter()
            .find(|(_, matcher)| matcher.is_match(path.as_std_path()))
            .map(|(pattern, _)| pattern.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn patterns(&self) -> impl Iterator<Item = &str> {
        self.patterns.iter().map(|(pattern, _)| pattern.as_str())
    }
}

pub struct Ruleset {
    exclude: ExcludeSet,
    workspaces: Vec<crate::workspace::WorkspaceSpec>,
    modules: Vec<ModuleGlob>,
    module_rules: Vec<ModuleRule>,
    package_rules: Vec<PackageRule>,
    closed_world: bool,
    source: Utf8PathBuf,
}

fn parse_severity(raw: Option<&str>) -> anyhow::Result<Severity> {
    // Rules default to `error`: the team asserted them, so breaking one is not a
    // suggestion.
    match raw {
        None => Ok(Severity::Error),
        Some(text) => text
            .parse::<Severity>()
            .map_err(|error| anyhow::anyhow!(error)),
    }
}

impl Ruleset {
    /// Loads `tropism.toml` from `scan_root`, if present.
    pub fn discover(scan_root: &Utf8Path) -> anyhow::Result<Option<Self>> {
        let path = scan_root.join(RULESET_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)?;
        Self::parse(Utf8PathBuf::from(RULESET_FILE), &text).map(Some)
    }

    pub fn parse(source: Utf8PathBuf, text: &str) -> anyhow::Result<Self> {
        let raw: RawRuleset =
            toml::from_str(text).map_err(|error| anyhow::anyhow!("{source}: {error}"))?;

        if let Some(version) = raw.schema_version
            && version != 1
        {
            anyhow::bail!("{source}: schema_version {version} is not supported (expected 1)");
        }

        let workspaces: Vec<crate::workspace::WorkspaceSpec> = raw
            .workspaces
            .into_iter()
            .map(|workspace| crate::workspace::WorkspaceSpec {
                root: Utf8PathBuf::from(workspace.root),
                members: workspace.members,
            })
            .collect();

        let mut exclude = ExcludeSet::default();
        for pattern in raw.exclude {
            let glob = globset::Glob::new(&pattern).map_err(|error| {
                anyhow::anyhow!("{source}: exclude pattern `{pattern}` is invalid: {error}")
            })?;
            exclude.patterns.push((pattern, glob.compile_matcher()));
        }

        let mut modules = Vec::new();
        for (name, module) in raw.modules {
            let patterns = match module {
                RawModule::One(pattern) => vec![pattern],
                RawModule::Many { paths } => paths,
            };
            for pattern in patterns {
                let glob = globset::Glob::new(&pattern).map_err(|error| {
                    anyhow::anyhow!("{source}: module `{name}` has an invalid glob: {error}")
                })?;
                modules.push(ModuleGlob {
                    name: name.clone(),
                    pattern,
                    matcher: glob.compile_matcher(),
                });
            }
        }

        let known: BTreeSet<&str> = modules.iter().map(|m| m.name.as_str()).collect();
        let mut module_rules = Vec::new();
        for rule in raw.module_rules {
            let severity = parse_severity(rule.severity.as_deref())?;

            for (field, present) in [
                ("layers", rule.layers.is_some()),
                ("require", rule.require.is_some()),
                ("transitive", rule.transitive.is_some()),
            ] {
                if present {
                    anyhow::bail!(
                        "{source}: rule `{}` uses `{field}`, which is specified in \
                         design/11-dependency-rules.md but not implemented yet",
                        rule.id
                    );
                }
            }

            // `crosses_workspace = false` is not a rule, it is the absence of one.
            // Accepting it silently would let a ruleset look like it enforced a
            // boundary it does not.
            if rule.crosses_workspace == Some(false) {
                anyhow::bail!(
                    "{source}: rule `{}` sets `crosses_workspace = false`, which enforces \
                     nothing; delete the rule instead",
                    rule.id
                );
            }
            let crosses = rule.crosses_workspace.unwrap_or(false).then_some(());

            let kind = match (rule.deny, rule.independent, rule.allow_only, crosses) {
                (Some(deny), None, None, None) => ModuleRuleKind::Deny {
                    from: deny.from,
                    to: deny.to.into_vec(),
                },
                (None, Some(members), None, None) => ModuleRuleKind::Independent(members),
                (None, None, Some(allow), None) => ModuleRuleKind::AllowOnly {
                    from: allow.from,
                    to: allow.to.into_vec(),
                },
                (None, None, None, Some(())) => ModuleRuleKind::CrossesWorkspace,
                (None, None, None, None) => anyhow::bail!(
                    "{source}: rule `{}` has no deny, independent, allow_only, or \
                     crosses_workspace",
                    rule.id
                ),
                _ => anyhow::bail!(
                    "{source}: rule `{}` sets more than one rule kind; split it in two",
                    rule.id
                ),
            };

            // A typo in a module name would otherwise make a rule silently do
            // nothing, which is the failure mode rulesets rot into.
            for name in kind_modules(&kind) {
                if !known.contains(name.as_str()) {
                    anyhow::bail!(
                        "{source}: rule `{}` names module `{name}`, which is not defined",
                        rule.id
                    );
                }
            }

            module_rules.push(ModuleRule {
                id: rule.id,
                severity,
                reason: rule.reason,
                kind,
            });
        }

        let mut package_rules = Vec::new();
        for rule in raw.package_rules {
            let severity = parse_severity(rule.severity.as_deref())?;
            let allowed_in = rule.allowed_in.unwrap_or_default().into_vec();
            for name in &allowed_in {
                if !known.contains(name.as_str()) {
                    anyhow::bail!(
                        "{source}: rule `{}` scopes to module `{name}`, which is not defined",
                        rule.id
                    );
                }
            }
            package_rules.push(PackageRule {
                id: rule.id,
                severity,
                reason: rule.reason,
                replacement: rule.replacement,
                denied: rule.deny.unwrap_or_default().into_vec(),
                scoped: rule.packages.unwrap_or_default().into_vec(),
                allowed_in,
                allowed: rule.allow.unwrap_or_default().into_vec(),
            });
        }

        let closed_world = match raw.packages.unlisted.as_deref() {
            None | Some("allow") => false,
            Some("deny") => true,
            Some(other) => {
                anyhow::bail!(
                    "{source}: packages.unlisted must be \"allow\" or \"deny\", got {other:?}"
                )
            }
        };

        Ok(Self {
            exclude,
            workspaces,
            modules,
            module_rules,
            package_rules,
            closed_world,
            source,
        })
    }

    pub fn exclude(&self) -> &ExcludeSet {
        &self.exclude
    }

    pub fn workspaces(&self) -> &[crate::workspace::WorkspaceSpec] {
        &self.workspaces
    }

    /// The two things the run needs *before* discovery walks anything: what to keep
    /// out, and where the workspace boundaries are.
    ///
    /// Both are read from the same file at the same time because reading it three
    /// times — once for exclusions, once for workspaces, once for rules — is how the
    /// three answers drift apart.
    pub fn into_prepass(self) -> (ExcludeSet, Vec<crate::workspace::WorkspaceSpec>) {
        (self.exclude, self.workspaces)
    }

    /// Loads the exclusions and workspace boundaries, for the pass that runs before
    /// discovery.
    pub fn discover_prepass(
        scan_root: &Utf8Path,
    ) -> anyhow::Result<(ExcludeSet, Vec<crate::workspace::WorkspaceSpec>)> {
        Ok(Self::discover(scan_root)?
            .map(Self::into_prepass)
            .unwrap_or_default())
    }

    /// Loads only the exclusions, for the pass that runs before discovery.
    ///
    /// Exclusions have to be known before any file is walked, while the rules
    /// themselves are evaluated at the end — so the same file is read for two
    /// different purposes at two different times.
    pub fn discover_excludes(scan_root: &Utf8Path) -> anyhow::Result<ExcludeSet> {
        Ok(Self::discover(scan_root)?
            .map(|ruleset| ruleset.exclude)
            .unwrap_or_default())
    }

    /// The module a path belongs to. Longest glob wins, so a more specific pattern
    /// claims files a broader one would otherwise take.
    pub fn module_of(&self, path: &Utf8Path) -> Option<&str> {
        self.modules
            .iter()
            .filter(|module| module.matcher.is_match(path.as_std_path()))
            .max_by_key(|module| module.pattern.len())
            .map(|module| module.name.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.module_rules.is_empty() && self.package_rules.is_empty()
    }

    /// How many rules this ruleset carries.
    ///
    /// Reported by `tropism check`: "checked 6 file(s) against 4 rule(s)" is the
    /// sentence that tells a developer the hook did something, and the same
    /// sentence reading "0 rule(s)" is the one that tells them it did not.
    pub fn rule_count(&self) -> usize {
        self.module_rules.len() + self.package_rules.len()
    }

    /// Evaluates every rule, returning findings plus the ids of rules that matched
    /// nothing.
    pub fn evaluate(
        &self,
        edges: &[DependencyEdge],
        uses: &[PackageUse],
        workspaces: &crate::workspace::WorkspaceMap,
        project_roots: &[Utf8PathBuf],
    ) -> (Vec<Finding>, Vec<String>) {
        let mut findings = Vec::new();

        // Which modules actually match something in this repository. A rule naming
        // a module that matches nothing is the way rulesets rot: someone renames a
        // directory and the rule protecting it silently stops doing anything.
        let seen: BTreeSet<&str> = edges
            .iter()
            .flat_map(|edge| [edge.from.as_path(), edge.to.as_path()])
            .chain(uses.iter().map(|use_site| use_site.at.as_path()))
            .filter_map(|path| self.module_of(path))
            .collect();

        for rule in &self.module_rules {
            // The boundary rule is about workspaces, not module globs, so it does
            // not need — and must not require — either end to be a named module.
            if matches!(rule.kind, ModuleRuleKind::CrossesWorkspace) {
                for edge in edges {
                    let (Some(from), Some(to)) = (
                        workspaces.of_path(&edge.from, project_roots),
                        workspaces.of_path(&edge.to, project_roots),
                    ) else {
                        continue;
                    };
                    if from.id == to.id {
                        continue;
                    }
                    let explanation = format!(
                        "workspace `{}` must not depend on workspace `{}`",
                        from.id, to.id
                    );
                    findings.push(self.module_finding(rule, &from.id, &to.id, edge, &explanation));
                }
                continue;
            }

            for edge in edges {
                let (Some(from), Some(to)) = (self.module_of(&edge.from), self.module_of(&edge.to))
                else {
                    continue;
                };
                if from == to {
                    continue;
                }

                if let Some(explanation) = rule.violated_by(from, to) {
                    findings.push(self.module_finding(rule, from, to, edge, &explanation));
                }
            }
        }

        for rule in &self.package_rules {
            for use_site in uses {
                if let Some(explanation) = rule.violated_by(use_site, self) {
                    findings.push(self.package_finding(rule, use_site, &explanation));
                }
            }
        }

        if self.closed_world {
            let permitted: BTreeSet<&str> = self
                .package_rules
                .iter()
                .flat_map(|rule| rule.allowed.iter().map(String::as_str))
                .collect();
            for use_site in uses.iter().filter(|u| u.level == EdgeLevel::Declared) {
                if !permitted.contains(use_site.package.as_str()) {
                    findings.push(self.closed_world_finding(use_site));
                }
            }
        }

        let mut stale: Vec<String> = self
            .module_rules
            .iter()
            .filter(|rule| match &rule.kind {
                // A boundary rule in a repository with one workspace cannot fire.
                // That is exactly as stale as a rule naming a renamed module, and
                // it is the likelier of the two: the rule is easy to write before
                // any boundary exists to enforce.
                ModuleRuleKind::CrossesWorkspace => workspaces.workspaces().len() < 2,
                kind => kind_modules(kind)
                    .iter()
                    .any(|m| !seen.contains(m.as_str())),
            })
            .map(|rule| rule.id.clone())
            .collect();
        stale.extend(
            self.package_rules
                .iter()
                .filter(|rule| rule.allowed_in.iter().any(|m| !seen.contains(m.as_str())))
                .map(|rule| rule.id.clone()),
        );
        stale.sort();
        stale.dedup();

        findings.sort_by(|a, b| a.id.cmp(&b.id));
        (findings, stale)
    }

    fn module_finding(
        &self,
        rule: &ModuleRule,
        from: &str,
        to: &str,
        edge: &DependencyEdge,
        explanation: &str,
    ) -> Finding {
        let mut evidence = vec![Evidence::new(
            edge.from.clone(),
            edge.line,
            format!("{} {}", edge.level.as_str(), edge.label),
        )];
        if let Some(reason) = &rule.reason {
            evidence.push(Evidence::new(
                self.source.clone(),
                None,
                reason.trim().to_owned(),
            ));
        }

        Finding::new(
            CheckId::ModuleRule,
            &Utf8PathBuf::new(),
            &[rule.id.as_str(), from, to, edge.from.as_str()],
            rule.severity,
            Confidence::High,
            format!("{explanation} (rule: {})", rule.id),
        )
        .with_evidence(evidence)
        .with_details(serde_json::json!({
            "rule_id": rule.id,
            "from": from,
            "to": to,
            "level": edge.level.as_str(),
        }))
    }

    fn package_finding(
        &self,
        rule: &PackageRule,
        use_site: &PackageUse,
        explanation: &str,
    ) -> Finding {
        let mut evidence = vec![Evidence::new(
            use_site.at.clone(),
            use_site.line,
            format!("{} here", use_site.level.as_str()),
        )];
        if let Some(replacement) = &rule.replacement {
            evidence.push(Evidence::new(
                self.source.clone(),
                None,
                format!("use `{replacement}` instead"),
            ));
        }
        if let Some(reason) = &rule.reason {
            evidence.push(Evidence::new(
                self.source.clone(),
                None,
                reason.trim().to_owned(),
            ));
        }

        Finding::new(
            CheckId::PackageRule,
            &Utf8PathBuf::new(),
            &[
                rule.id.as_str(),
                use_site.package.as_str(),
                use_site.at.as_str(),
            ],
            rule.severity,
            Confidence::High,
            format!("{explanation} (rule: {})", rule.id),
        )
        .with_evidence(evidence)
        .with_details(serde_json::json!({
            "rule_id": rule.id,
            "package": use_site.package,
            "level": use_site.level.as_str(),
            "replacement": rule.replacement,
        }))
    }

    fn closed_world_finding(&self, use_site: &PackageUse) -> Finding {
        Finding::new(
            CheckId::PackageRule,
            &Utf8PathBuf::new(),
            &["unlisted", use_site.package.as_str(), use_site.at.as_str()],
            Severity::Error,
            Confidence::High,
            format!(
                "`{}` is not on the approved list (packages.unlisted = \"deny\")",
                use_site.package
            ),
        )
        .with_evidence([Evidence::new(
            use_site.at.clone(),
            use_site.line,
            "declared here",
        )])
        .with_details(serde_json::json!({
            "rule_id": "unlisted",
            "package": use_site.package,
        }))
    }
}

fn kind_modules(kind: &ModuleRuleKind) -> Vec<String> {
    match kind {
        ModuleRuleKind::Deny { from, to } | ModuleRuleKind::AllowOnly { from, to } => {
            let mut all = vec![from.clone()];
            all.extend(to.iter().cloned());
            all
        }
        ModuleRuleKind::Independent(members) => members.clone(),
        // Names no module by construction, so nothing here can go stale by a
        // rename. Its staleness test is whether the repository has two workspaces
        // at all — see `Ruleset::evaluate`.
        ModuleRuleKind::CrossesWorkspace => Vec::new(),
    }
}

impl ModuleRule {
    fn violated_by(&self, from: &str, to: &str) -> Option<String> {
        match &self.kind {
            ModuleRuleKind::Deny {
                from: rule_from,
                to: rule_to,
            } => (rule_from == from && rule_to.iter().any(|t| t == to))
                .then(|| format!("`{from}` must not depend on `{to}`")),
            ModuleRuleKind::Independent(members) => (members.iter().any(|m| m == from)
                && members.iter().any(|m| m == to))
            .then(|| format!("`{from}` and `{to}` must be independent")),
            ModuleRuleKind::AllowOnly {
                from: rule_from,
                to: allowed,
            } => (rule_from == from && !allowed.iter().any(|t| t == to)).then(|| {
                if allowed.is_empty() {
                    format!("`{from}` must not depend on anything, but depends on `{to}`")
                } else {
                    format!(
                        "`{from}` may depend only on {}, but depends on `{to}`",
                        allowed
                            .iter()
                            .map(|a| format!("`{a}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }),
            // Evaluated against the workspace map, not module names, so it is
            // handled by the caller and never reached here.
            ModuleRuleKind::CrossesWorkspace => None,
        }
    }
}

impl PackageRule {
    fn violated_by(&self, use_site: &PackageUse, ruleset: &Ruleset) -> Option<String> {
        if self.denied.iter().any(|denied| denied == &use_site.package) {
            return Some(format!("`{}` is not allowed", use_site.package));
        }

        // Scoping is about *where the code lives*, so it only applies to import
        // sites. A manifest declaration has no module location — reporting one
        // would say a package is "used in an unassigned path", which is not a
        // statement about the architecture at all.
        if use_site.level == EdgeLevel::Imported
            && self.scoped.iter().any(|scoped| scoped == &use_site.package)
        {
            let module = ruleset.module_of(&use_site.at);
            let permitted = module.is_some_and(|m| self.allowed_in.iter().any(|a| a == m));
            if !permitted {
                return Some(format!(
                    "`{}` is restricted to {} but is used in `{}`",
                    use_site.package,
                    self.allowed_in
                        .iter()
                        .map(|a| format!("`{a}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    module.unwrap_or("an unassigned path")
                ));
            }
        }

        None
    }
}

#[cfg(test)]
impl Ruleset {
    /// Test shim: evaluate with no workspace information.
    ///
    /// Every rule kind except `crosses_workspace` is independent of the workspace
    /// map, so the tests that predate it pass an empty one and stay unchanged.
    fn evaluate_bare(
        &self,
        edges: &[DependencyEdge],
        uses: &[PackageUse],
    ) -> (Vec<Finding>, Vec<String>) {
        self.evaluate(edges, uses, &crate::workspace::WorkspaceMap::default(), &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &str = r#"
schema_version = 1

[modules]
core = "crates/core/**"
cli = "crates/cli/**"
mcp = "crates/mcp/**"

[[module_rules]]
id = "surfaces-are-independent"
independent = ["cli", "mcp"]
reason = "Both are adapters over core."

[[module_rules]]
id = "core-is-a-leaf"
allow_only = { from = "core", to = [] }

[[package_rules]]
id = "no-archived-yaml"
deny = ["serde_yaml"]
replacement = "saphyr"

[[package_rules]]
id = "tui-stays-in-the-cli"
packages = ["ratatui"]
allowed_in = ["cli"]
"#;

    fn ruleset() -> Ruleset {
        Ruleset::parse(Utf8PathBuf::from("tropism.toml"), RULES).unwrap()
    }

    fn edge(from: &str, to: &str) -> DependencyEdge {
        DependencyEdge {
            from: Utf8PathBuf::from(from),
            to: Utf8PathBuf::from(to),
            line: Some(1),
            label: "x".to_owned(),
            level: EdgeLevel::Imported,
            // Rules match on paths; module identity is only used by the cycle graph.
            from_module: None,
            to_module: None,
        }
    }

    fn use_of(package: &str, at: &str) -> PackageUse {
        PackageUse {
            package: package.to_owned(),
            at: Utf8PathBuf::from(at),
            line: Some(1),
            level: EdgeLevel::Declared,
        }
    }

    fn import_of(package: &str, at: &str) -> PackageUse {
        PackageUse {
            level: EdgeLevel::Imported,
            ..use_of(package, at)
        }
    }

    #[test]
    fn matches_paths_to_modules() {
        let rules = ruleset();
        assert_eq!(
            rules.module_of(Utf8Path::new("crates/cli/src/main.rs")),
            Some("cli")
        );
        assert_eq!(rules.module_of(Utf8Path::new("README.md")), None);
    }

    /// The motivating case: two adapters that must not know about each other.
    #[test]
    fn independent_modules_may_not_depend_on_each_other() {
        let rules = ruleset();
        let (findings, _) = rules.evaluate_bare(
            &[edge("crates/cli/src/main.rs", "crates/mcp/src/main.rs")],
            &[],
        );
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("must be independent"));
        assert_eq!(
            findings[0].severity,
            Severity::Error,
            "rules default to error"
        );
        assert_eq!(findings[0].confidence, Confidence::High);
    }

    #[test]
    fn independence_is_symmetric() {
        let rules = ruleset();
        let (findings, _) = rules.evaluate_bare(
            &[edge("crates/mcp/src/main.rs", "crates/cli/src/main.rs")],
            &[],
        );
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn a_permitted_dependency_produces_nothing() {
        let rules = ruleset();
        let (findings, _) = rules.evaluate_bare(
            &[edge("crates/cli/src/main.rs", "crates/core/src/lib.rs")],
            &[],
        );
        assert!(findings.is_empty(), "cli may depend on core");
    }

    #[test]
    fn allow_only_with_an_empty_list_forbids_everything() {
        let rules = ruleset();
        let (findings, _) = rules.evaluate_bare(
            &[edge("crates/core/src/lib.rs", "crates/cli/src/main.rs")],
            &[],
        );
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("must not depend on anything"));
    }

    #[test]
    fn a_finding_carries_the_teams_reason() {
        let rules = ruleset();
        let (findings, _) = rules.evaluate_bare(
            &[edge("crates/cli/src/main.rs", "crates/mcp/src/main.rs")],
            &[],
        );
        let notes: Vec<&str> = findings[0]
            .evidence
            .iter()
            .map(|e| e.note.as_str())
            .collect();
        assert!(
            notes.iter().any(|n| n.contains("adapters over core")),
            "got {notes:?}"
        );
    }

    #[test]
    fn a_denied_package_is_reported_with_its_replacement() {
        let rules = ruleset();
        let (findings, _) =
            rules.evaluate_bare(&[], &[use_of("serde_yaml", "crates/core/Cargo.toml")]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].details["replacement"], "saphyr");
    }

    #[test]
    fn a_scoped_package_is_allowed_inside_its_module() {
        let rules = ruleset();
        let (findings, _) =
            rules.evaluate_bare(&[], &[import_of("ratatui", "crates/cli/src/tui.rs")]);
        assert!(findings.is_empty());
    }

    #[test]
    fn a_scoped_package_is_reported_outside_its_module() {
        let rules = ruleset();
        let (findings, _) =
            rules.evaluate_bare(&[], &[import_of("ratatui", "crates/core/src/lib.rs")]);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("restricted to"));
    }

    /// Scoping is about where code lives, so a manifest declaration — which has no
    /// module location — is not a violation of it.
    #[test]
    fn a_scoped_package_is_not_reported_from_a_manifest_declaration() {
        let rules = ruleset();
        let (findings, _) = rules.evaluate_bare(&[], &[use_of("ratatui", "crates/cli/Cargo.toml")]);
        assert!(findings.is_empty());
    }

    /// Rulesets rot: a module gets renamed and the rule protecting it silently
    /// stops doing anything. Staleness keys on whether the named modules match any
    /// path at all — not on whether the rule happened to fire.
    #[test]
    fn a_rule_naming_a_module_that_matches_nothing_is_stale() {
        let rules = ruleset();
        // Only `cli` and `core` exist in this repository; `mcp` matches nothing.
        let (_, stale) = rules.evaluate_bare(
            &[edge("crates/cli/src/main.rs", "crates/core/src/lib.rs")],
            &[],
        );
        assert!(
            stale.contains(&"surfaces-are-independent".to_owned()),
            "got {stale:?}"
        );
        assert!(
            !stale.contains(&"core-is-a-leaf".to_owned()),
            "both its modules exist"
        );
    }

    /// A rule whose modules all exist is satisfied, not stale. Reporting satisfied
    /// rules as rot would make the check unusable.
    #[test]
    fn a_satisfied_rule_is_not_stale() {
        let rules = ruleset();
        let (findings, stale) = rules.evaluate_bare(
            &[
                edge("crates/cli/src/main.rs", "crates/core/src/lib.rs"),
                edge("crates/mcp/src/main.rs", "crates/core/src/lib.rs"),
            ],
            &[],
        );
        assert!(findings.is_empty());
        assert!(
            !stale.contains(&"surfaces-are-independent".to_owned()),
            "got {stale:?}"
        );
    }

    #[test]
    fn an_approved_list_rejects_anything_unlisted() {
        let rules = Ruleset::parse(
            Utf8PathBuf::from("tropism.toml"),
            "[modules]\nall = \"**\"\n\n[packages]\nunlisted = \"deny\"\n\n\
             [[package_rules]]\nid = \"approved\"\nallow = [\"serde\"]\n",
        )
        .unwrap();
        let (findings, _) = rules.evaluate_bare(
            &[],
            &[
                use_of("serde", "Cargo.toml"),
                use_of("sketchy", "Cargo.toml"),
            ],
        );
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("sketchy"));
    }

    // --- exclusions -------------------------------------------------------

    fn excludes(patterns: &str) -> ExcludeSet {
        Ruleset::parse(Utf8PathBuf::from("tropism.toml"), patterns)
            .unwrap()
            .exclude
    }

    #[test]
    fn excludes_a_directory_tree() {
        let set = excludes("exclude = [\"demo/**\"]\n");
        assert_eq!(
            set.excluded_by(Utf8Path::new("demo/go/go.mod")),
            Some("demo/**")
        );
        assert_eq!(
            set.excluded_by(Utf8Path::new("crates/core/src/lib.rs")),
            None
        );
    }

    /// The pattern that makes this repository's own CI gate possible.
    #[test]
    fn excludes_a_nested_fixture_directory() {
        let set = excludes("exclude = [\"**/tests/fixtures/**\"]\n");
        assert!(
            set.excluded_by(Utf8Path::new(
                "crates/tropism-lang/tests/fixtures/go-cycle/go.mod"
            ))
            .is_some()
        );
        assert!(
            set.excluded_by(Utf8Path::new("crates/tropism-lang/src/go.rs"))
                .is_none()
        );
    }

    #[test]
    fn no_exclude_key_means_nothing_is_excluded() {
        assert!(excludes("[modules]\na = \"a/**\"\n").is_empty());
    }

    #[test]
    fn an_invalid_exclude_glob_is_an_error() {
        let error = parse_error("exclude = [\"a/**/[\"]\n");
        assert!(error.contains("exclude pattern"), "{error}");
    }

    // --- ruleset errors ---------------------------------------------------

    fn parse_error(text: &str) -> String {
        match Ruleset::parse(Utf8PathBuf::from("tropism.toml"), text) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("expected a ruleset error"),
        }
    }

    /// A typo would otherwise make a rule silently protect nothing.
    #[test]
    fn an_unknown_module_name_is_an_error() {
        let error = parse_error(
            "[modules]\na = \"a/**\"\n[[module_rules]]\nid = \"r\"\ndeny = { from = \"a\", to = [\"typo\"] }\n",
        );
        assert!(error.contains("typo"), "{error}");
        assert!(error.contains("not defined"), "{error}");
    }

    #[test]
    fn a_rule_with_no_kind_is_an_error() {
        let error = parse_error("[modules]\na = \"a/**\"\n[[module_rules]]\nid = \"r\"\n");
        assert!(
            error.contains("no deny, independent, allow_only, or crosses_workspace"),
            "{error}"
        );
    }

    /// Specified but unimplemented features are rejected by name rather than
    /// silently ignored, so a ruleset never appears to enforce more than it does.
    #[test]
    fn unimplemented_rule_kinds_are_rejected_explicitly() {
        for (field, text) in [
            (
                "layers",
                "[modules]\na = \"a/**\"\n[[module_rules]]\nid = \"r\"\nlayers = [\"a\"]\n",
            ),
            (
                "transitive",
                "[modules]\na = \"a/**\"\n[[module_rules]]\nid = \"r\"\ndeny = { from = \"a\", to = [\"a\"] }\ntransitive = true\n",
            ),
        ] {
            let error = parse_error(text);
            assert!(error.contains(field), "{field}: {error}");
            assert!(error.contains("not implemented"), "{field}: {error}");
        }
    }

    #[test]
    fn an_unsupported_schema_version_is_an_error() {
        let error = parse_error("schema_version = 99\n");
        assert!(error.contains("schema_version 99"), "{error}");
    }

    #[test]
    fn an_invalid_glob_is_an_error() {
        let error = parse_error("[modules]\na = \"a/**/[\"\n");
        assert!(error.contains("invalid glob"), "{error}");
    }
}
