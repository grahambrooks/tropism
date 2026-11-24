//! From checkouts to a portfolio report.
//!
//! 1. **Discover** the checkouts under each root: the root itself if it is a
//!    repository, otherwise each child directory that is one.
//! 2. **Identify** each by its remote. Only remotes under `owner` are own
//!    repositories, and only own repositories can be libraries; any checkout can
//!    be a user.
//! 3. **Scan** each for what it publishes (Cargo packages, Go modules, npm
//!    packages, binaries) and what it declares.
//! 4. **Link**: a declaration naming an own repository — by git URL, by a local
//!    path into another checkout, by Go module path, or by a published name — is
//!    an edge. Lockfile entries from own repositories that no manifest declares
//!    are transitive edges.
//! 5. **Facts** from each library's history: its newest stable tag, whether its
//!    default branch has commits its users would get, and how far behind each
//!    user is.
//! 6. **Findings** from the facts, at the severity a portfolio rule sets or at
//!    `info` when none does.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use camino::{Utf8Path, Utf8PathBuf};
use rayon::prelude::*;
use tropism_core::report::{CheckStatus, Confidence, Evidence, Severity};
use tropism_core::telemetry;

use crate::binary::{self, Patterns, Target};
use crate::history::{History, Tag, visible_to_users};
use crate::model::*;
use crate::remote::{self, RepoId};
use crate::rules::{PortfolioRules, RuleKind};
use crate::scan::{self, PortfolioExclude, RepoScan};
use crate::{cargo, npm, version};

pub struct Options {
    pub roots: Vec<Utf8PathBuf>,
    pub owner: String,
    /// Read tags with `git ls-remote` instead of the checkout's refs.
    pub fetch: bool,
    pub rules: Option<PortfolioRules>,
    pub tool_version: String,
}

/// Evidence lists are for a reader to verify a link, not an inventory of every
/// line that mentions it.
const MAX_EVIDENCE: usize = 5;

pub const LIMITATIONS: &[&str] = &[
    "duplicate-copy describes the lockfile, not the build: a lockfile records neither features nor target platforms, so two copies in it do not prove both are compiled (known limitation S8).",
    "Binary links come from text matching in Makefiles, hooks, workflows, scripts and tool manifests, not from import resolution. They are medium confidence, and count toward a rule only when the rule names kind \"binary\".",
    "A repository that is not checked out under a root contributes nothing. Libraries it would publish are listed as unavailable, never as having no users, and its own links are unknown.",
    "npm resolution is read from package-lock.json only; yarn.lock and pnpm-lock.yaml are not read for portfolio links, so npm edges there carry no resolved version.",
];

const LOCAL_TAGS_LIMITATION: &str = "Tags were read from each checkout's refs as last fetched. A tag pushed since is invisible without --fetch, and release-pending compares against the default branch as last fetched.";

struct RepoData {
    name: String,
    path: Utf8PathBuf,
    canonical: Option<PathBuf>,
    id: Option<RepoId>,
    own: bool,
    branch: Option<String>,
    head: Option<String>,
    scan: RepoScan,
    duplicates: Vec<Utf8PathBuf>,
}

/// A library: an own repository that something links to.
#[derive(Clone)]
struct LibInfo {
    repo: Option<usize>,
    remote: RepoId,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EdgeKey {
    from: usize,
    to: String,
    kind: LinkKind,
    package: String,
    via: Via,
    pinned: Option<Pin>,
}

struct EdgeAcc {
    through: BTreeSet<String>,
    resolved: BTreeSet<Resolved>,
    evidence: Vec<Evidence>,
    extra_evidence: usize,
    confidence: Confidence,
}

struct Duplicate {
    user: usize,
    library: String,
    lockfile: Utf8PathBuf,
    package: String,
    copies: Vec<serde_json::Value>,
    labels: Vec<String>,
}

struct Linker<'a> {
    owner: &'a str,
    repos: &'a [RepoData],
    by_id: HashMap<(String, String, String), usize>,
    cargo_packages: HashMap<&'a str, Vec<usize>>,
    go_modules: Vec<(&'a str, usize)>,
    npm_packages: HashMap<&'a str, Vec<usize>>,
    binaries: HashMap<&'a str, Vec<usize>>,
    libs: BTreeMap<String, LibInfo>,
    edges: BTreeMap<EdgeKey, EdgeAcc>,
    duplicates: Vec<Duplicate>,
    notes: Vec<Note>,
}

fn id_key(id: &RepoId) -> (String, String, String) {
    (
        id.host.clone(),
        id.owner.clone(),
        id.name.to_ascii_lowercase(),
    )
}

impl<'a> Linker<'a> {
    fn new(owner: &'a str, repos: &'a [RepoData]) -> Self {
        let mut linker = Self {
            owner,
            repos,
            by_id: HashMap::new(),
            cargo_packages: HashMap::new(),
            go_modules: Vec::new(),
            npm_packages: HashMap::new(),
            binaries: HashMap::new(),
            libs: BTreeMap::new(),
            edges: BTreeMap::new(),
            duplicates: Vec::new(),
            notes: Vec::new(),
        };
        for (index, repo) in repos.iter().enumerate() {
            if !repo.own {
                continue;
            }
            if let Some(id) = &repo.id {
                linker.by_id.insert(id_key(id), index);
            }
            for package in &repo.scan.packages {
                match package.ecosystem {
                    Ecosystem::Cargo => linker
                        .cargo_packages
                        .entry(package.name.as_str())
                        .or_default()
                        .push(index),
                    Ecosystem::Go => linker.go_modules.push((package.name.as_str(), index)),
                    Ecosystem::Npm => linker
                        .npm_packages
                        .entry(package.name.as_str())
                        .or_default()
                        .push(index),
                }
            }
            for binary in &repo.scan.binaries {
                linker
                    .binaries
                    .entry(binary.as_str())
                    .or_default()
                    .push(index);
            }
        }
        for list in linker
            .cargo_packages
            .values_mut()
            .chain(linker.npm_packages.values_mut())
            .chain(linker.binaries.values_mut())
        {
            list.sort_unstable();
            list.dedup();
        }
        // Longest module path first, so `x/sub` claims `x/sub/pkg` before `x` does.
        linker
            .go_modules
            .sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(b.0)));
        let mut ambiguous: Vec<_> = linker
            .binaries
            .iter()
            .filter(|(_, owners)| owners.len() > 1)
            .map(|(name, owners)| {
                let names: Vec<&str> = owners.iter().map(|&i| repos[i].name.as_str()).collect();
                format!(
                    "binary `{name}` is built by {}; uses of it are not attributed",
                    names.join(", ")
                )
            })
            .collect();
        ambiguous.sort();
        linker
            .notes
            .extend(ambiguous.into_iter().map(|message| Note {
                repo: None,
                file: None,
                message,
            }));
        linker
    }

    /// The library a remote names, if the owner owns it. Registers it.
    fn lib_for_id(&mut self, id: &RepoId) -> Option<String> {
        if !id.is_owned_by(self.owner) {
            return None;
        }
        let repo = self.by_id.get(&id_key(id)).copied();
        Some(self.register(repo, id.clone()))
    }

    fn lib_for_repo(&mut self, index: usize) -> Option<String> {
        let repo = &self.repos[index];
        let id = repo.id.clone().filter(|_| repo.own)?;
        Some(self.register(Some(index), id))
    }

    fn register(&mut self, repo: Option<usize>, remote: RepoId) -> String {
        let name = match repo {
            Some(index) => self.repos[index].name.clone(),
            None => remote.name.clone(),
        };
        self.libs
            .entry(name.clone())
            .or_insert(LibInfo { repo, remote });
        name
    }

    fn lib_for_path(&mut self, from: usize, path: &Utf8Path) -> Option<String> {
        let canonical = std::fs::canonicalize(path).ok()?;
        let index = self
            .repos
            .iter()
            .enumerate()
            .filter(|(i, r)| *i != from && r.own)
            .find(|(_, r)| {
                r.canonical
                    .as_ref()
                    .is_some_and(|c| canonical.starts_with(c))
            })?
            .0;
        self.lib_for_repo(index)
    }

    fn lib_for_go_module(&mut self, path: &str) -> Option<String> {
        let found = self
            .go_modules
            .iter()
            .find(|(module, _)| path == *module || path.starts_with(&format!("{module}/")))
            .map(|(_, index)| *index);
        match found {
            Some(index) => self.lib_for_repo(index),
            None => self.lib_for_id(&remote::parse(path)?),
        }
    }

    /// A repository as a tool manifest or a workflow spells it.
    fn lib_for_spelling(&mut self, spelling: &str) -> Option<String> {
        if let Some(id) = remote::parse(spelling) {
            return self.lib_for_id(&id);
        }
        let name = match spelling.split_once('/') {
            Some((owner, name)) if owner.eq_ignore_ascii_case(self.owner) => name,
            Some(_) => return None,
            None => spelling,
        };
        let index = self.repos.iter().position(|r| {
            r.own
                && r.id
                    .as_ref()
                    .is_some_and(|id| id.name.eq_ignore_ascii_case(name))
        })?;
        self.lib_for_repo(index)
    }

    fn lib_for_binary(&mut self, name: &str) -> Option<String> {
        match self.binaries.get(name).map(Vec::as_slice) {
            Some([index]) => self.lib_for_repo(*index),
            Some(_) => None, // ambiguous: noted once in `new`
            None => self.lib_for_spelling(name),
        }
    }

    fn is_self(&self, from: usize, lib: &str) -> bool {
        self.libs.get(lib).and_then(|l| l.repo) == Some(from)
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        key: EdgeKey,
        confidence: Confidence,
        evidence: Evidence,
        resolved: impl IntoIterator<Item = Resolved>,
        through: impl IntoIterator<Item = String>,
    ) {
        if self.is_self(key.from, &key.to) {
            return;
        }
        let acc = self.edges.entry(key).or_insert_with(|| EdgeAcc {
            through: BTreeSet::new(),
            resolved: BTreeSet::new(),
            evidence: Vec::new(),
            extra_evidence: 0,
            confidence,
        });
        acc.confidence = acc.confidence.max(confidence);
        acc.resolved.extend(resolved);
        acc.through.extend(through);
        if acc.evidence.contains(&evidence) {
            return;
        }
        if acc.evidence.len() < MAX_EVIDENCE {
            acc.evidence.push(evidence);
        } else {
            acc.extra_evidence += 1;
        }
    }

    fn link_cargo(&mut self, from: usize) {
        let repo = &self.repos[from];
        // Lockfile copies a manifest declaration accounts for, as
        // `(lockfile, name, source)`. Any other copy from an own repository is a
        // transitive edge — including a second copy of a library the repository
        // also declares directly, which is the two-copies case.
        let mut claimed: BTreeSet<(Utf8PathBuf, String, Option<String>)> = BTreeSet::new();
        for (manifest_rel, manifest) in &repo.scan.cargo_manifests {
            let dir = manifest_rel.parent().unwrap_or(Utf8Path::new(""));
            for dep in &manifest.deps {
                let lib = match (&dep.git, &dep.path) {
                    (Some(git), _) => remote::parse(git).and_then(|id| self.lib_for_id(&id)),
                    (None, Some(path)) => self.lib_for_path(from, &repo.path.join(dir).join(path)),
                    (None, None) => None,
                };
                let Some(lib) = lib else { continue };
                let lib_id = self.libs[&lib].remote.clone();
                let mut resolved = Vec::new();
                for (lock_rel, packages) in &repo.scan.cargo_locks {
                    for p in packages.iter().filter(|p| p.name == dep.package) {
                        let source = p.source.as_deref().and_then(cargo::git_source);
                        let matches = match (&dep.git, &source) {
                            // Same repository, and the copy this pin produces:
                            // `tag = "v1"` locks as `?tag=v1`, no ref as no query.
                            (Some(_), Some(source)) => {
                                remote::parse(&source.url).is_some_and(|id| id.same_repo(&lib_id))
                                    && match dep.pin.kind {
                                        PinKind::DefaultBranch => source.reference.is_none(),
                                        _ => source.reference.as_ref() == Some(&dep.pin),
                                    }
                            }
                            (None, None) => p.source.is_none(),
                            _ => false,
                        };
                        if !matches {
                            continue;
                        }
                        claimed.insert((lock_rel.clone(), p.name.clone(), p.source.clone()));
                        resolved.push(Resolved {
                            lockfile: repo.path.join(lock_rel),
                            version: Some(p.version.clone()),
                            reference: source.as_ref().and_then(|s| s.reference.clone()),
                            commit: source.and_then(|s| s.commit),
                        });
                    }
                }
                self.add(
                    EdgeKey {
                        from,
                        to: lib,
                        kind: LinkKind::Cargo,
                        package: dep.package.clone(),
                        via: Via::Direct,
                        pinned: Some(dep.pin.clone()),
                    },
                    Confidence::High,
                    Evidence::new(
                        repo.path.join(manifest_rel),
                        dep.line,
                        format!("[{}] {} = {}", dep.section, dep.package, dep.pin),
                    ),
                    resolved,
                    [],
                );
            }
        }

        for (lock_rel, packages) in &repo.scan.cargo_locks {
            let mut own_copies: BTreeMap<&str, Vec<(&cargo::LockPackage, String)>> =
                BTreeMap::new();
            for package in packages {
                let Some(source) = package.source.as_deref().and_then(cargo::git_source) else {
                    continue;
                };
                let Some(lib) = remote::parse(&source.url).and_then(|id| self.lib_for_id(&id))
                else {
                    continue;
                };
                own_copies
                    .entry(package.name.as_str())
                    .or_default()
                    .push((package, lib.clone()));
                let through: Vec<String> = packages
                    .iter()
                    .filter(|q| {
                        q.dependencies
                            .iter()
                            .any(|entry| cargo::names(entry, package))
                    })
                    .map(|q| q.name.clone())
                    .collect();
                if claimed.contains(&(
                    lock_rel.clone(),
                    package.name.clone(),
                    package.source.clone(),
                )) {
                    continue;
                }
                let note = if through.is_empty() {
                    format!("{} {} in the lockfile", package.name, package.version)
                } else {
                    format!(
                        "{} {} via {}",
                        package.name,
                        package.version,
                        through.join(", ")
                    )
                };
                self.add(
                    EdgeKey {
                        from,
                        to: lib,
                        kind: LinkKind::Cargo,
                        package: package.name.clone(),
                        via: Via::Transitive,
                        pinned: Some(
                            source
                                .reference
                                .clone()
                                .unwrap_or_else(|| Pin::new(PinKind::DefaultBranch, "HEAD")),
                        ),
                    },
                    Confidence::High,
                    Evidence::new(repo.path.join(lock_rel), None, note),
                    [Resolved {
                        lockfile: repo.path.join(lock_rel),
                        version: Some(package.version.clone()),
                        reference: source.reference.clone(),
                        commit: source.commit.clone(),
                    }],
                    through,
                );
            }
            for (name, copies) in own_copies {
                // A registry copy of the same name counts too: the lockfile then
                // holds the library twice, once from git and once from crates.io.
                let registry: Vec<&cargo::LockPackage> = packages
                    .iter()
                    .filter(|p| {
                        p.name == name
                            && p.source
                                .as_deref()
                                .is_some_and(|s| s.starts_with("registry+"))
                    })
                    .collect();
                if copies.len() + registry.len() < 2 {
                    continue;
                }
                let describe = |p: &cargo::LockPackage| {
                    let source = p.source.as_deref().and_then(cargo::git_source);
                    let label = match source.as_ref().and_then(|s| s.reference.as_ref()) {
                        Some(pin) => pin.to_string(),
                        None if source.is_some() => format!(
                            "{} @ {}",
                            p.version,
                            source
                                .as_ref()
                                .and_then(|s| s.commit.as_deref())
                                .map_or("?", |c| &c[..c.len().min(8)])
                        ),
                        None => format!("{} from the registry", p.version),
                    };
                    let through: Vec<String> = packages
                        .iter()
                        .filter(|q| q.dependencies.iter().any(|entry| cargo::names(entry, p)))
                        .map(|q| q.name.clone())
                        .collect();
                    let json = serde_json::json!({
                        "version": p.version,
                        "source": p.source,
                        "used_by": through,
                    });
                    (label, json)
                };
                let mut labels = Vec::new();
                let mut json = Vec::new();
                for p in copies.iter().map(|(p, _)| *p).chain(registry) {
                    let (label, value) = describe(p);
                    labels.push(label);
                    json.push(value);
                }
                self.duplicates.push(Duplicate {
                    user: from,
                    library: copies[0].1.clone(),
                    lockfile: repo.path.join(lock_rel),
                    package: name.to_owned(),
                    copies: json,
                    labels,
                });
            }
        }
    }

    fn link_go(&mut self, from: usize) {
        let repo = &self.repos[from];
        for (mod_rel, gomod) in &repo.scan.go_mods {
            let dir = mod_rel.parent().unwrap_or(Utf8Path::new(""));
            let sum = repo.scan.go_sums.get(&dir.join("go.sum"));
            for require in &gomod.requires {
                let Some(lib) = self.lib_for_go_module(&require.path) else {
                    continue;
                };
                let replace = gomod.replaces.iter().find(|r| r.path == require.path);
                let pinned = match replace {
                    Some(r) if r.is_local() => Pin::new(PinKind::Path, &r.target),
                    Some(r) => Pin::new(
                        PinKind::Version,
                        r.target_version
                            .clone()
                            .unwrap_or_else(|| require.version.clone()),
                    ),
                    None => Pin::new(PinKind::Version, &require.version),
                };
                let resolved = sum
                    .filter(|pairs| {
                        pairs.contains(&(require.path.clone(), require.version.clone()))
                    })
                    .map(|_| Resolved {
                        lockfile: repo.path.join(dir).join("go.sum"),
                        version: Some(require.version.clone()),
                        reference: None,
                        commit: version::go_pseudo_commit(&require.version).map(str::to_owned),
                    });
                self.add(
                    EdgeKey {
                        from,
                        to: lib,
                        kind: LinkKind::Go,
                        package: require.path.clone(),
                        via: if require.indirect {
                            Via::Indirect
                        } else {
                            Via::Direct
                        },
                        pinned: Some(pinned),
                    },
                    Confidence::High,
                    Evidence::new(
                        repo.path.join(mod_rel),
                        Some(require.line),
                        format!("require {} {}", require.path, require.version),
                    ),
                    resolved,
                    [],
                );
            }
        }
    }

    fn link_npm(&mut self, from: usize) {
        let repo = &self.repos[from];
        for (pkg_rel, package) in &repo.scan.npm_packages {
            let dir = pkg_rel.parent().unwrap_or(Utf8Path::new(""));
            let lock = repo
                .scan
                .npm_locks
                .iter()
                .find(|(lock_rel, _)| lock_rel.parent().unwrap_or(Utf8Path::new("")) == dir);
            for dep in &package.deps {
                let (lib, pin, confidence) = if let Some((url, pin)) = npm::git_spec(&dep.spec) {
                    let Some(lib) = remote::parse(&url).and_then(|id| self.lib_for_id(&id)) else {
                        continue;
                    };
                    (lib, pin, Confidence::High)
                } else {
                    let local = ["file:", "link:", "workspace:", "portal:", "npm:"]
                        .iter()
                        .any(|p| dep.spec.starts_with(p));
                    let owners = self
                        .npm_packages
                        .get(dep.name.as_str())
                        .cloned()
                        .unwrap_or_default();
                    let [index] = owners.as_slice() else { continue };
                    if local || *index == from {
                        continue;
                    }
                    let Some(lib) = self.lib_for_repo(*index) else {
                        continue;
                    };
                    (lib, Pin::new(PinKind::Range, &dep.spec), Confidence::Medium)
                };
                let resolved: Vec<Resolved> = lock
                    .into_iter()
                    .flat_map(|(lock_rel, entries)| {
                        entries
                            .iter()
                            .filter(|e| e.location == format!("node_modules/{}", dep.name))
                            .map(move |e| Resolved {
                                lockfile: repo.path.join(lock_rel),
                                version: Some(e.version.clone()).filter(|v| !v.is_empty()),
                                reference: None,
                                commit: e
                                    .resolved
                                    .as_deref()
                                    .and_then(|r| r.rsplit_once('#'))
                                    .map(|(_, c)| c.to_owned())
                                    .filter(|c| version::looks_like_commit(c)),
                            })
                    })
                    .collect();
                self.add(
                    EdgeKey {
                        from,
                        to: lib,
                        kind: LinkKind::Npm,
                        package: dep.name.clone(),
                        via: Via::Direct,
                        pinned: Some(pin),
                    },
                    confidence,
                    Evidence::new(
                        repo.path.join(pkg_rel),
                        dep.line,
                        format!("{}: \"{}\": \"{}\"", dep.section, dep.name, dep.spec),
                    ),
                    resolved,
                    [],
                );
            }
            // Two copies of an own package in one lockfile.
            if let Some((lock_rel, entries)) = lock {
                let mut by_name: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
                for entry in entries {
                    let owned_by_git = entry
                        .resolved
                        .as_deref()
                        .and_then(remote::parse)
                        .is_some_and(|id| id.is_owned_by(self.owner));
                    if owned_by_git || self.npm_packages.contains_key(entry.name.as_str()) {
                        by_name
                            .entry(entry.name.as_str())
                            .or_default()
                            .insert(entry.version.as_str());
                    }
                }
                for (name, versions) in by_name.into_iter().filter(|(_, v)| v.len() > 1) {
                    let Some(&index) = self.npm_packages.get(name).and_then(|o| o.first()) else {
                        continue;
                    };
                    let Some(library) = self.lib_for_repo(index) else {
                        continue;
                    };
                    self.duplicates.push(Duplicate {
                        user: from,
                        library,
                        lockfile: repo.path.join(lock_rel),
                        package: name.to_owned(),
                        copies: versions
                            .iter()
                            .map(|v| serde_json::json!({ "version": v }))
                            .collect(),
                        labels: versions.iter().map(|v| (*v).to_owned()).collect(),
                    });
                }
            }
        }
    }

    fn link_binaries(&mut self, from: usize) {
        let repo = &self.repos[from];
        for found in &repo.scan.binary_refs {
            let (lib, package) = match &found.target {
                Target::Repo(spelling) => {
                    let Some(lib) = self.lib_for_spelling(spelling) else {
                        continue;
                    };
                    (lib.clone(), lib)
                }
                Target::Binary(name) => {
                    let Some(lib) = self.lib_for_binary(name) else {
                        continue;
                    };
                    (lib, name.clone())
                }
            };
            self.add(
                EdgeKey {
                    from,
                    to: lib,
                    kind: LinkKind::Binary,
                    package,
                    via: Via::Direct,
                    pinned: found.pin.clone(),
                },
                Confidence::Medium,
                Evidence::new(repo.path.join(&found.file), found.line, found.how),
                [],
                [],
            );
        }
        for line in &repo.scan.command_lines {
            for word in binary::command_words(&line.text) {
                let Some(owners) = self.binaries.get(word) else {
                    continue;
                };
                let [index] = owners.as_slice() else { continue };
                let index = *index;
                if index == from {
                    continue;
                }
                let Some(lib) = self.lib_for_repo(index) else {
                    continue;
                };
                self.add(
                    EdgeKey {
                        from,
                        to: lib,
                        kind: LinkKind::Binary,
                        package: word.to_owned(),
                        via: Via::Direct,
                        pinned: None,
                    },
                    Confidence::Medium,
                    Evidence::new(
                        repo.path.join(&line.file),
                        Some(line.line),
                        format!("runs `{word}`"),
                    ),
                    [],
                    [],
                );
            }
        }
    }
}

fn discover(roots: &[Utf8PathBuf], notes: &mut Vec<Note>) -> anyhow::Result<Vec<Utf8PathBuf>> {
    let mut found = Vec::new();
    for root in roots {
        if !root.is_dir() {
            anyhow::bail!("{root}: not a directory");
        }
        if root.join(".git").exists() {
            found.push(root.clone());
            continue;
        }
        let mut children: Vec<Utf8PathBuf> = std::fs::read_dir(root)?
            .flatten()
            .filter_map(|entry| Utf8PathBuf::from_path_buf(entry.path()).ok())
            .filter(|path| path.is_dir() && path.join(".git").exists())
            .collect();
        children.sort();
        if children.is_empty() {
            notes.push(Note {
                repo: None,
                file: Some(root.clone()),
                message: "no git repositories directly under this root".into(),
            });
        }
        found.extend(children);
    }
    Ok(found)
}

pub fn run(options: &Options, history: &dyn History) -> anyhow::Result<PortfolioReport> {
    let mut span = telemetry::span("portfolio");
    span.set_type("analyze");
    let mut notes = Vec::new();
    let paths = discover(&options.roots, &mut notes)?;

    // Identify every checkout first: names come from remotes, and the portfolio
    // exclusions are written against names.
    let identified: Vec<_> = paths
        .par_iter()
        .map(|path| {
            let remote = history.remote_url(path.as_std_path());
            let id = remote.as_deref().and_then(remote::parse);
            (
                path.clone(),
                id,
                history.branch(path.as_std_path()),
                history.head(path.as_std_path()),
            )
        })
        .collect();

    let mut seen: HashMap<(String, String, String), usize> = HashMap::new();
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let mut repos: Vec<RepoData> = Vec::new();
    for (path, id, branch, head) in identified {
        if let Some(id) = &id
            && let Some(&first) = seen.get(&id_key(id))
        {
            repos[first].duplicates.push(path);
            continue;
        }
        let own = id.as_ref().is_some_and(|id| id.is_owned_by(&options.owner));
        let dir_name = path
            .canonicalize_utf8()
            .ok()
            .and_then(|p| p.file_name().map(str::to_owned))
            .unwrap_or_else(|| path.to_string());
        let mut name = match &id {
            Some(id) if own => id.name.clone(),
            Some(id) => format!("{}/{}", id.owner, id.name),
            None => dir_name,
        };
        if taken.contains(&name) {
            let mut n = 2;
            while taken.contains(&format!("{name}~{n}")) {
                n += 1;
            }
            name = format!("{name}~{n}");
        }
        taken.insert(name.clone());
        if let Some(id) = &id {
            seen.insert(id_key(id), repos.len());
        }
        repos.push(RepoData {
            name,
            canonical: std::fs::canonicalize(&path).ok(),
            path,
            id,
            own,
            branch,
            head,
            scan: RepoScan::default(),
            duplicates: Vec::new(),
        });
    }

    let patterns = Patterns::new(&options.owner);
    let empty = Vec::new();
    let rules_source = options
        .rules
        .as_ref()
        .map(|r| r.source.clone())
        .unwrap_or_default();
    let portfolio_exclude = PortfolioExclude {
        patterns: options.rules.as_ref().map_or(&empty, |r| &r.exclude),
        source: &rules_source,
    };
    {
        let mut scan_span = telemetry::span("portfolio scan");
        scan_span.set_type("discover").attr("count", repos.len());
        let scans: Vec<RepoScan> = repos
            .par_iter()
            .map(|repo| scan::scan_repo(&repo.path, &repo.name, &patterns, &portfolio_exclude))
            .collect();
        for (repo, scan) in repos.iter_mut().zip(scans) {
            repo.scan = scan;
        }
    }

    for repo in &repos {
        for (file, message) in &repo.scan.notes {
            notes.push(Note {
                repo: Some(repo.name.clone()),
                file: file.as_ref().map(|f| repo.path.join(f)),
                message: message.clone(),
            });
        }
    }

    let mut linker = Linker::new(&options.owner, &repos);
    for index in 0..repos.len() {
        linker.link_cargo(index);
        linker.link_go(index);
        linker.link_npm(index);
        linker.link_binaries(index);
    }
    notes.append(&mut linker.notes);
    let Linker {
        libs,
        edges: edge_map,
        duplicates,
        ..
    } = linker;

    // Facts from each library's history, read once per library.
    let lib_names: Vec<&String> = libs.keys().collect();
    let facts: Vec<LibFacts> = lib_names
        .par_iter()
        .map(|name| lib_facts(&libs[*name], &repos, options.fetch, history))
        .collect();
    let facts: BTreeMap<&str, LibFacts> = lib_names.iter().map(|n| n.as_str()).zip(facts).collect();
    for (name, fact) in &facts {
        if let Some(message) = &fact.tag_error {
            notes.push(Note {
                repo: Some((*name).to_owned()),
                file: None,
                message: message.clone(),
            });
        }
    }

    let mut edges = Vec::new();
    for (key, acc) in edge_map {
        let fact = &facts[key.to.as_str()];
        let lib = &libs[&key.to];
        let resolved: Vec<Resolved> = acc.resolved.into_iter().collect();
        let (behind_by, behind_note) = behind(&key, &resolved, lib, fact, &repos, history);
        let mut evidence = acc.evidence;
        if acc.extra_evidence > 0
            && let Some(last) = evidence.last_mut()
        {
            last.note = format!("{} (and {} more)", last.note, acc.extra_evidence);
        }
        edges.push(Edge {
            from: repos[key.from].name.clone(),
            to: key.to.clone(),
            kind: key.kind,
            package: key.package,
            via: key.via,
            through: acc.through.into_iter().collect(),
            pinned: key.pinned,
            resolved,
            newest: fact.newest.clone(),
            behind_by,
            behind_note,
            confidence: acc.confidence,
            evidence,
        });
    }

    let mut libraries: Vec<Library> = libs
        .iter()
        .map(|(name, lib)| {
            let fact = &facts[name.as_str()];
            let mut users: Vec<String> = edges
                .iter()
                .filter(|e| &e.to == name)
                .map(|e| e.from.clone())
                .collect();
            users.sort();
            users.dedup();
            Library {
                repo: name.clone(),
                remote: lib.remote.display(),
                status: if lib.repo.is_some() {
                    LibraryStatus::Available
                } else {
                    LibraryStatus::Unavailable
                },
                packages: lib
                    .repo
                    .map(|i| repos[i].scan.packages.clone())
                    .unwrap_or_default(),
                newest_tag: fact.newest.clone(),
                release: fact.release.clone(),
                users,
            }
        })
        .collect();
    libraries.sort_by(|a, b| a.repo.cmp(&b.repo));

    let findings = findings(options, &repos, &libs, &libraries, &edges, &duplicates);
    let mut checks = BTreeMap::new();
    for check in PortfolioCheck::ALL {
        let status = if check == PortfolioCheck::StaleRule && options.rules.is_none() {
            CheckStatus::unavailable(format!(
                "no {} was given or found",
                crate::rules::RULES_FILE
            ))
        } else {
            CheckStatus::Ran {
                finding_count: findings.iter().filter(|f| f.check == check).count(),
            }
        };
        checks.insert(check, status);
    }

    let mut exclusions: BTreeMap<(String, Utf8PathBuf), usize> = BTreeMap::new();
    if let Some(rules) = &options.rules {
        for (pattern, _) in &rules.exclude {
            exclusions.insert((pattern.clone(), rules.source.clone()), 0);
        }
    }
    for repo in &repos {
        for (key, count) in &repo.scan.excluded {
            *exclusions.entry(key.clone()).or_default() += count;
        }
    }

    let mut limitations: Vec<&'static str> = LIMITATIONS.to_vec();
    if !options.fetch {
        limitations.push(LOCAL_TAGS_LIMITATION);
    }
    span.attr("repos", repos.len()).attr("edges", edges.len());

    Ok(PortfolioReport {
        schema: SCHEMA,
        schema_version: SCHEMA_VERSION,
        tool: Tool {
            name: "tropism",
            version: options.tool_version.clone(),
        },
        owner: options.owner.clone(),
        roots: options.roots.clone(),
        tags_from: if options.fetch {
            TagSource::Remote
        } else {
            TagSource::Local
        },
        rules: options.rules.as_ref().map(|r| r.source.clone()),
        repos: repos
            .iter()
            .map(|r| Repo {
                name: r.name.clone(),
                path: r.path.clone(),
                remote: r.id.as_ref().map(RepoId::display),
                own: r.own,
                branch: r.branch.clone(),
                head: r.head.clone(),
                packages: r.scan.packages.clone(),
                binaries: r.scan.binaries.iter().cloned().collect(),
                duplicate_checkouts: r.duplicates.clone(),
            })
            .collect(),
        libraries,
        edges,
        checks,
        findings,
        exclusions: exclusions
            .into_iter()
            .map(|((pattern, source), matched)| Exclusion {
                pattern,
                source,
                matched,
            })
            .collect(),
        notes,
        limitations,
    })
}

struct LibFacts {
    tags: Vec<Tag>,
    newest: Option<String>,
    release: Release,
    tag_error: Option<String>,
}

fn lib_facts(lib: &LibInfo, repos: &[RepoData], fetch: bool, history: &dyn History) -> LibFacts {
    let Some(index) = lib.repo else {
        return LibFacts {
            tags: Vec::new(),
            newest: None,
            release: Release {
                status: ReleaseStatus::Unavailable,
                branch: None,
                commits: Vec::new(),
                reason: Some("not checked out under any root".into()),
            },
            tag_error: None,
        };
    };
    let path = repos[index].path.as_std_path();
    let (tags, tag_error) = match if fetch {
        history.remote_tags(path)
    } else {
        history.local_tags(path)
    } {
        Ok(tags) => (tags, None),
        Err(error) => (
            Vec::new(),
            Some(format!(
                "tags could not be read{}: {error}",
                if fetch { " from origin" } else { "" }
            )),
        ),
    };
    let newest = version::newest(tags.iter().map(|t| t.name.as_str())).map(|(t, _)| t.to_owned());
    let not_comparable = |branch: Option<String>, reason: String| Release {
        status: ReleaseStatus::NotComparable,
        branch,
        commits: Vec::new(),
        reason: Some(reason),
    };
    let release = match &newest {
        None if tag_error.is_some() => not_comparable(None, "its tags could not be read".into()),
        None => Release {
            status: ReleaseStatus::NoTag,
            branch: None,
            commits: Vec::new(),
            reason: Some("no stable version tag yet".into()),
        },
        Some(tag) => {
            let commit = tags
                .iter()
                .find(|t| &t.name == tag)
                .map(|t| t.commit.clone())
                .unwrap_or_default();
            match history.default_branch(path) {
                None => not_comparable(
                    None,
                    "no remote-tracking default branch (origin/HEAD, origin/main)".into(),
                ),
                Some(branch) => match history.commits_between(path, &commit, &branch) {
                    None => not_comparable(
                        Some(branch),
                        format!("{tag} is not in this checkout's history; fetch it to compare"),
                    ),
                    Some(commits) => {
                        let visible: Vec<String> = visible_to_users(&commits)
                            .into_iter()
                            .map(|c| {
                                let subject: String = c.subject.chars().take(72).collect();
                                format!("{} {subject}", &c.sha[..c.sha.len().min(8)])
                            })
                            .collect();
                        Release {
                            status: if visible.is_empty() {
                                ReleaseStatus::Current
                            } else {
                                ReleaseStatus::Pending
                            },
                            branch: Some(branch),
                            commits: visible,
                            reason: None,
                        }
                    }
                },
            }
        }
    };
    LibFacts {
        tags,
        newest,
        release,
        tag_error,
    }
}

fn behind(
    key: &EdgeKey,
    resolved: &[Resolved],
    lib: &LibInfo,
    fact: &LibFacts,
    repos: &[RepoData],
    history: &dyn History,
) -> (Option<u32>, Option<String>) {
    let Some(index) = lib.repo else {
        return (
            None,
            Some("the library is not checked out under any root".into()),
        );
    };
    if fact.newest.is_none() {
        return (None, Some("the library has no stable version tag".into()));
    }
    let names = || fact.tags.iter().map(|t| t.name.as_str());
    let by_commit = |commit: &str| -> (Option<u32>, Option<String>) {
        match history.tags_not_merged_into(repos[index].path.as_std_path(), commit) {
            Some(tags) => {
                let mut versions: Vec<_> =
                    tags.iter().filter_map(|t| version::stable_tag(t)).collect();
                versions.sort();
                versions.dedup();
                (
                    Some(u32::try_from(versions.len()).unwrap_or(u32::MAX)),
                    None,
                )
            }
            None => (
                None,
                Some(format!(
                    "commit {} is not in the library checkout's history",
                    &commit[..commit.len().min(12)]
                )),
            ),
        }
    };
    let by_version = |text: &str| version::parse(text).map(|v| version::newer_than(names(), v));
    let resolved_tag = resolved
        .iter()
        .find_map(|r| r.reference.as_ref().filter(|p| p.kind == PinKind::Tag));
    let resolved_commit = resolved.iter().find_map(|r| r.commit.as_deref());
    let resolved_version = resolved.iter().find_map(|r| r.version.as_deref());
    let Some(pin) = resolved_tag.or(key.pinned.as_ref()) else {
        return (
            None,
            Some("no version is pinned; the link is a use, not a pin".into()),
        );
    };
    match pin.kind {
        PinKind::Tag => match version::stable_tag(&pin.value) {
            Some(v) => (Some(version::newer_than(names(), v)), None),
            None => (
                None,
                Some(format!("`{}` is not a stable version tag", pin.value)),
            ),
        },
        PinKind::Version => match by_version(&pin.value) {
            Some(n) => (Some(n), None),
            None => match version::go_pseudo_commit(&pin.value) {
                Some(commit) => by_commit(commit),
                None => (
                    None,
                    Some(format!("`{}` is not a stable version", pin.value)),
                ),
            },
        },
        PinKind::Rev => by_commit(resolved_commit.unwrap_or(&pin.value)),
        PinKind::Branch | PinKind::DefaultBranch => match resolved_commit {
            Some(commit) => by_commit(commit),
            None => (
                None,
                Some("follows a branch, and no lockfile records which commit".into()),
            ),
        },
        PinKind::Range => match resolved_version.and_then(by_version) {
            Some(n) => (Some(n), None),
            None => (
                None,
                Some("a version range with no lockfile resolution".into()),
            ),
        },
        PinKind::Path => (
            None,
            Some("a local path: always the library's working tree, never a tag".into()),
        ),
    }
}

fn current_label(edge: &Edge) -> String {
    edge.resolved
        .iter()
        .find_map(|r| r.reference.as_ref().map(ToString::to_string))
        .or_else(|| edge.pinned.as_ref().map(ToString::to_string))
        .unwrap_or_else(|| "an unpinned version".into())
}

fn findings(
    options: &Options,
    repos: &[RepoData],
    libs: &BTreeMap<String, LibInfo>,
    libraries: &[Library],
    edges: &[Edge],
    duplicates: &[Duplicate],
) -> Vec<PortfolioFinding> {
    let rules = options
        .rules
        .as_ref()
        .map(|r| r.rules.as_slice())
        .unwrap_or_default();
    let mut out = Vec::new();

    for library in libraries {
        if library.release.status != ReleaseStatus::Pending {
            continue;
        }
        let rule = rules
            .iter()
            .filter(|r| r.kind == RuleKind::ReleasePending)
            .filter(|r| {
                r.covers_library(&library.repo, "")
                    || library
                        .packages
                        .iter()
                        .any(|p| r.covers_library(&library.repo, &p.name))
            })
            .max_by_key(|r| r.severity);
        let tag = library.newest_tag.as_deref().unwrap_or_default();
        let branch = library
            .release
            .branch
            .as_deref()
            .unwrap_or("the default branch");
        let count = library.release.commits.len();
        let path = libs[&library.repo]
            .repo
            .map(|i| repos[i].path.clone())
            .unwrap_or_default();
        out.push(PortfolioFinding {
            id: PortfolioFinding::id(PortfolioCheck::ReleasePending, &library.repo, &[tag]),
            check: PortfolioCheck::ReleasePending,
            severity: rule.map_or(Severity::Info, |r| r.severity),
            confidence: Confidence::High,
            repo: library.repo.clone(),
            library: Some(library.repo.clone()),
            rule: rule.map(|r| r.id.clone()),
            message: format!(
                "{}: {count} commit(s) on {branch} since {tag} that its users would get; {} user(s) wait on a release",
                library.repo,
                library.users.len()
            ),
            evidence: vec![Evidence::new(
                path,
                None,
                format!("{branch} is {count} user-visible commit(s) past {tag}"),
            )],
            details: serde_json::json!({
                "tag": tag,
                "branch": branch,
                "commits": library.release.commits,
                "users": library.users,
            }),
        });
    }

    for edge in edges {
        let Some(behind_by) = edge.behind_by.filter(|n| *n > 0) else {
            continue;
        };
        let rule = rules
            .iter()
            .filter(|r| match r.kind {
                RuleKind::MaxTagsBehind(max) => behind_by > max,
                _ => false,
            })
            .filter(|r| r.kinds.contains(&edge.kind))
            .filter(|r| r.covers_library(&edge.to, &edge.package) && r.covers_user(&edge.from))
            .max_by_key(|r| r.severity);
        let current = current_label(edge);
        let newest = edge.newest.as_deref().unwrap_or_default();
        let via = match (edge.via, edge.through.is_empty()) {
            (Via::Transitive, false) => format!(" (through {})", edge.through.join(", ")),
            (Via::Transitive, true) => " (transitively)".into(),
            (Via::Indirect, _) => " (indirect)".into(),
            _ => String::new(),
        };
        let pinned = edge
            .pinned
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        out.push(PortfolioFinding {
            id: PortfolioFinding::id(
                PortfolioCheck::TagsBehind,
                &edge.from,
                &[&edge.to, &edge.package, edge.kind.as_str(), &pinned],
            ),
            check: PortfolioCheck::TagsBehind,
            severity: rule.map_or(Severity::Info, |r| r.severity),
            confidence: edge.confidence,
            repo: edge.from.clone(),
            library: Some(edge.to.clone()),
            rule: rule.map(|r| r.id.clone()),
            message: format!(
                "{} is on {} {current}{via}, {behind_by} tag(s) behind {newest}",
                edge.from, edge.package
            ),
            evidence: edge.evidence.clone(),
            details: serde_json::json!({
                "package": edge.package,
                "kind": edge.kind,
                "via": edge.via,
                "pinned": edge.pinned,
                "newest": newest,
                "behind_by": behind_by,
            }),
        });
    }

    for duplicate in duplicates {
        let user = &repos[duplicate.user].name;
        let rule = rules
            .iter()
            .filter(|r| match &r.kind {
                RuleKind::SingleCopy(names) => names
                    .iter()
                    .any(|n| *n == duplicate.package || *n == duplicate.library),
                _ => false,
            })
            .filter(|r| r.covers_user(user))
            .max_by_key(|r| r.severity);
        out.push(PortfolioFinding {
            id: PortfolioFinding::id(
                PortfolioCheck::DuplicateCopy,
                user,
                &[duplicate.lockfile.as_str(), &duplicate.package],
            ),
            check: PortfolioCheck::DuplicateCopy,
            severity: rule.map_or(Severity::Info, |r| r.severity),
            // S8: the lockfile resolves two copies; whether the build links both
            // depends on features and targets the lockfile does not record.
            confidence: Confidence::Medium,
            repo: user.clone(),
            library: Some(duplicate.library.clone()),
            rule: rule.map(|r| r.id.clone()),
            message: format!(
                "the lockfile resolves {} copies of {}: {}",
                duplicate.labels.len(),
                duplicate.package,
                duplicate.labels.join(", ")
            ),
            evidence: vec![Evidence::new(
                duplicate.lockfile.clone(),
                None,
                format!(
                    "{} entries for {}",
                    duplicate.labels.len(),
                    duplicate.package
                ),
            )],
            details: serde_json::json!({
                "package": duplicate.package,
                "copies": duplicate.copies,
            }),
        });
    }

    if let Some(rule_file) = &options.rules {
        let known_library = |name: &str| {
            libs.contains_key(name)
                || repos.iter().any(|r| {
                    r.own && (r.name == name || r.scan.packages.iter().any(|p| p.name == name))
                })
        };
        let known_user = |name: &str| repos.iter().any(|r| r.name == name);
        for rule in &rule_file.rules {
            let mut stale: Vec<(String, &str)> = Vec::new();
            let mut library_names: Vec<&String> = rule.libraries.iter().collect();
            if let RuleKind::SingleCopy(names) = &rule.kind {
                library_names.extend(names);
            }
            for name in library_names {
                if !known_library(name) {
                    stale.push((name.clone(), "library"));
                }
            }
            for name in &rule.users {
                if !known_user(name) {
                    stale.push((name.clone(), "repository"));
                }
            }
            for (name, what) in stale {
                out.push(PortfolioFinding {
                    id: PortfolioFinding::id(PortfolioCheck::StaleRule, rule_file.source.as_str(), &[&rule.id, &name]),
                    check: PortfolioCheck::StaleRule,
                    severity: Severity::Warning,
                    confidence: Confidence::High,
                    repo: rule_file.source.to_string(),
                    library: None,
                    rule: Some(rule.id.clone()),
                    message: format!(
                        "rule `{}` names {what} `{name}`, which no repository under the roots matches; it can never fire for it",
                        rule.id
                    ),
                    evidence: vec![Evidence::new(rule_file.source.clone(), None, format!("`{name}`"))],
                    details: serde_json::Value::Null,
                });
            }
        }
    }

    out.sort_by(|a, b| (a.check, &a.id).cmp(&(b.check, &b.id)));
    out
}
