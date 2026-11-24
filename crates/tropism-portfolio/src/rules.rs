//! `tropism-portfolio.toml`: cross-repository rules.
//!
//! A separate file from `tropism.toml`, because the directory it lives in — a
//! workspace such as `~/dev/projects` — is usually not a repository, and because
//! the two answer different questions: `tropism.toml` constrains what a
//! repository's modules may import; this constrains how its repositories track
//! each other. Putting both in one file would make every repository's ruleset
//! carry keys that mean nothing inside it.
//!
//! ```toml
//! schema_version = 1
//! owner = "grahambrooks"            # default for --owner
//! exclude = ["sustain/evals/**"]    # `<repo>/<path>` globs
//!
//! [[rules]]
//! id = "stay-current"
//! max_tags_behind = 2
//! libraries = ["mmd", "puml"]       # optional: repo or package names
//! users = ["ferrisite"]             # optional
//! kinds = ["cargo", "go"]           # optional; "binary" must be named to count
//! severity = "error"                # default error: the team asserted it
//! reason = "…"
//!
//! [[rules]]
//! id = "one-diagram-scene"
//! single_copy = ["diagram-scene"]
//! severity = "warning"              # default warning: see S8
//!
//! [[rules]]
//! id = "release-promptly"
//! release_pending = true
//! severity = "warning"              # default warning
//! ```
//!
//! As with `tropism.toml`, an unknown key is an error rather than ignored, so a
//! file never appears to enforce more than it does.

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use tropism_core::report::Severity;

use crate::model::LinkKind;

pub const RULES_FILE: &str = "tropism-portfolio.toml";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRules {
    schema_version: Option<u32>,
    owner: Option<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    rules: Vec<RawRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: String,
    max_tags_behind: Option<u32>,
    single_copy: Option<Vec<String>>,
    release_pending: Option<bool>,
    #[serde(default)]
    libraries: Vec<String>,
    #[serde(default)]
    users: Vec<String>,
    kinds: Option<Vec<String>>,
    severity: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleKind {
    MaxTagsBehind(u32),
    SingleCopy(Vec<String>),
    ReleasePending,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub kind: RuleKind,
    /// Empty means every library.
    pub libraries: Vec<String>,
    /// Empty means every user.
    pub users: Vec<String>,
    pub kinds: Vec<LinkKind>,
    pub severity: Severity,
    pub reason: Option<String>,
}

impl Rule {
    pub fn covers_library(&self, repo: &str, package: &str) -> bool {
        self.libraries.is_empty() || self.libraries.iter().any(|l| l == repo || l == package)
    }

    pub fn covers_user(&self, user: &str) -> bool {
        self.users.is_empty() || self.users.iter().any(|u| u == user)
    }
}

pub struct PortfolioRules {
    pub source: Utf8PathBuf,
    pub owner: Option<String>,
    pub exclude: Vec<(String, globset::GlobMatcher)>,
    pub rules: Vec<Rule>,
}

impl PortfolioRules {
    pub fn load(path: &Utf8Path) -> anyhow::Result<Self> {
        let text =
            std::fs::read_to_string(path).map_err(|error| anyhow::anyhow!("{path}: {error}"))?;
        Self::parse(path.to_owned(), &text)
    }

    pub fn parse(source: Utf8PathBuf, text: &str) -> anyhow::Result<Self> {
        let raw: RawRules =
            toml::from_str(text).map_err(|error| anyhow::anyhow!("{source}: {error}"))?;
        if let Some(version) = raw.schema_version
            && version != 1
        {
            anyhow::bail!("{source}: schema_version {version} is not supported (expected 1)");
        }
        let mut exclude = Vec::new();
        for pattern in raw.exclude {
            let glob = globset::Glob::new(&pattern).map_err(|error| {
                anyhow::anyhow!("{source}: exclude pattern `{pattern}` is invalid: {error}")
            })?;
            exclude.push((pattern, glob.compile_matcher()));
        }
        let mut rules = Vec::new();
        let mut ids = std::collections::BTreeSet::new();
        for rule in raw.rules {
            if !ids.insert(rule.id.clone()) {
                anyhow::bail!("{source}: rule id `{}` is used twice", rule.id);
            }
            let kinds_given = [
                rule.max_tags_behind.is_some(),
                rule.single_copy.is_some(),
                rule.release_pending.is_some(),
            ]
            .iter()
            .filter(|given| **given)
            .count();
            if kinds_given != 1 {
                anyhow::bail!(
                    "{source}: rule `{}` must set exactly one of `max_tags_behind`, `single_copy`, `release_pending`",
                    rule.id
                );
            }
            let (kind, default_severity) = if let Some(n) = rule.max_tags_behind {
                (RuleKind::MaxTagsBehind(n), Severity::Error)
            } else if let Some(packages) = rule.single_copy {
                if packages.is_empty() {
                    anyhow::bail!("{source}: rule `{}`: `single_copy` names nothing", rule.id);
                }
                // S8: the lockfile resolves two copies; the build may not link
                // both. Warn until there is evidence on the false-positive rate.
                (RuleKind::SingleCopy(packages), Severity::Warning)
            } else if rule.release_pending == Some(true) {
                (RuleKind::ReleasePending, Severity::Warning)
            } else {
                anyhow::bail!(
                    "{source}: rule `{}`: `release_pending = false` asserts nothing; remove the rule",
                    rule.id
                );
            };
            let severity = match rule.severity.as_deref() {
                None => default_severity,
                Some(text) => text
                    .parse::<Severity>()
                    .map_err(|error| anyhow::anyhow!("{source}: rule `{}`: {error}", rule.id))?,
            };
            let kinds = match rule.kinds {
                // An installed-binary link is text matching; it counts only when a
                // rule asks for it by name.
                None => vec![LinkKind::Cargo, LinkKind::Go, LinkKind::Npm],
                Some(names) => names
                    .iter()
                    .map(|name| {
                        LinkKind::parse(name).ok_or_else(|| {
                            anyhow::anyhow!(
                                "{source}: rule `{}`: unknown kind `{name}` (cargo, go, npm, binary)",
                                rule.id
                            )
                        })
                    })
                    .collect::<anyhow::Result<_>>()?,
            };
            rules.push(Rule {
                id: rule.id,
                kind,
                libraries: rule.libraries,
                users: rule.users,
                kinds,
                severity,
                reason: rule.reason,
            });
        }
        Ok(Self {
            source,
            owner: raw.owner,
            exclude,
            rules,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> anyhow::Result<PortfolioRules> {
        PortfolioRules::parse("tropism-portfolio.toml".into(), text)
    }

    #[test]
    fn defaults_follow_what_each_rule_can_prove() {
        let rules = parse(
            "owner = \"grahambrooks\"\n[[rules]]\nid = \"a\"\nmax_tags_behind = 2\n[[rules]]\nid = \"b\"\nsingle_copy = [\"diagram-scene\"]\n[[rules]]\nid = \"c\"\nrelease_pending = true\n",
        )
        .unwrap();
        assert_eq!(rules.owner.as_deref(), Some("grahambrooks"));
        assert_eq!(rules.rules[0].severity, Severity::Error);
        assert_eq!(rules.rules[1].severity, Severity::Warning);
        assert_eq!(rules.rules[2].severity, Severity::Warning);
        assert!(!rules.rules[0].kinds.contains(&LinkKind::Binary));
    }

    #[test]
    fn a_rule_that_asserts_nothing_or_too_much_is_rejected() {
        for text in [
            "[[rules]]\nid = \"a\"\n",
            "[[rules]]\nid = \"a\"\nmax_tags_behind = 1\nrelease_pending = true\n",
            "[[rules]]\nid = \"a\"\nrelease_pending = false\n",
            "[[rules]]\nid = \"a\"\nsingle_copy = []\n",
            "[[rules]]\nid = \"a\"\nmax_tags_behind = 1\nkinds = [\"pip\"]\n",
            "[[rules]]\nid = \"a\"\nmax_tags_behind = 1\nversion_range = \"^1\"\n",
            "[[rules]]\nid = \"a\"\nmax_tags_behind = 1\n[[rules]]\nid = \"a\"\nmax_tags_behind = 2\n",
            "schema_version = 2\n",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }
}
