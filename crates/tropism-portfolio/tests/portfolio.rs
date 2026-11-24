//! `tropism portfolio` end to end, over real git repositories built in a
//! temporary directory.
//!
//! The portfolio is owned by `acme`, and every remote is a `github.com` URL that
//! is never contacted: nothing here fetches, and `origin/main` is set by hand as
//! though it had been. One user, `site`, carries every kind of link; the
//! libraries carry the histories that make each per-link fact come out a known
//! way.

use std::path::{Path, PathBuf};
use std::process::Command;

use camino::Utf8PathBuf;
use tropism_core::report::{Confidence, Severity};
use tropism_portfolio::model::*;
use tropism_portfolio::{Git, Options, PortfolioRules, run};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Dev")
        .env("GIT_AUTHOR_EMAIL", "dev@example.com")
        .env("GIT_COMMITTER_NAME", "Dev")
        .env("GIT_COMMITTER_EMAIL", "dev@example.com")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct Repo(PathBuf);

impl Repo {
    fn new(root: &Path, dir: &str, remote: &str, files: &[(&str, &str)]) -> Self {
        let path = root.join(dir);
        std::fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        git(&path, &["remote", "add", "origin", remote]);
        let repo = Self(path);
        repo.commit(files, "initial");
        repo
    }

    fn commit(&self, files: &[(&str, &str)], message: &str) -> String {
        self.commit_as(files, message, None)
    }

    fn commit_as(&self, files: &[(&str, &str)], message: &str, author: Option<&str>) -> String {
        for (name, text) in files {
            let file = self.0.join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
            git(&self.0, &["add", name]);
        }
        let mut args = vec!["commit", "-q", "--allow-empty", "-m", message];
        let author_arg;
        if let Some(author) = author {
            author_arg = format!("--author={author} <bot@example.com>");
            args.push(&author_arg);
        }
        git(&self.0, &args);
        git(&self.0, &["rev-parse", "HEAD"])
    }

    fn tag(&self, name: &str) {
        git(&self.0, &["tag", "-a", name, "-m", name]);
    }

    /// As though `git fetch` had last seen HEAD on origin/main.
    fn publish(&self) {
        git(&self.0, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(
            &self.0,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
    }
}

fn cargo_toml(name: &str, deps: &str) -> String {
    format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n{deps}"
    )
}

const SITE_LOCK: &str = r#"version = 4

[[package]]
name = "site"
version = "0.1.0"
dependencies = [
 "mmd",
 "scene 1.0.0",
]

[[package]]
name = "mmd"
version = "1.0.0"
source = "git+https://github.com/acme/mmd?tag=v1.0.0#1111111111111111111111111111111111111111"
dependencies = [
 "scene 1.2.0",
]

[[package]]
name = "scene"
version = "1.0.0"
source = "git+https://github.com/acme/scene?tag=v1.0.0#2222222222222222222222222222222222222222"

[[package]]
name = "scene"
version = "1.2.0"
source = "git+https://github.com/acme/scene?tag=v1.2.0#3333333333333333333333333333333333333333"

[[package]]
name = "x"
version = "9.9.9"
source = "git+https://github.com/other/x?tag=v9.9.9#4444444444444444444444444444444444444444"
"#;

/// The portfolio, built once per test: `(tempdir, root)`.
fn portfolio() -> (tempfile::TempDir, Utf8PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // scene: three releases, then a real change no tag has released.
    let scene = Repo::new(
        root,
        "scene",
        "git@github.com:acme/scene.git",
        &[("Cargo.toml", &cargo_toml("scene", "")), ("src/lib.rs", "")],
    );
    scene.tag("v1.0.0");
    scene.commit(&[("src/lib.rs", "// 1.1")], "feat: 1.1");
    scene.tag("v1.1.0");
    scene.commit(&[("src/lib.rs", "// 1.2")], "feat: 1.2");
    scene.tag("v1.2.0");
    scene.commit(&[("src/lib.rs", "// next")], "feat: unreleased");
    scene.publish();

    // mmd: after its newest tag, only a lockfile refresh and release-kit's own
    // commit — nothing its users would get.
    let mmd = Repo::new(
        root,
        "mmd",
        "https://github.com/acme/mmd",
        &[
            (
                "Cargo.toml",
                &cargo_toml(
                    "mmd",
                    "scene = { git = \"https://github.com/acme/scene\", tag = \"v1.2.0\" }\n",
                ),
            ),
            ("src/lib.rs", ""),
        ],
    );
    mmd.tag("v1.0.0");
    mmd.commit(&[("src/lib.rs", "// 1.1")], "feat: 1.1");
    mmd.tag("v1.1.0");
    mmd.commit(
        &[("Cargo.lock", "version = 4\n")],
        "chore(deps): cargo update",
    );
    mmd.commit_as(
        &[(
            "Cargo.toml",
            &cargo_toml(
                "mmd",
                "scene = { git = \"https://github.com/acme/scene\", tag = \"v1.2.0\" }\n# 1.1.0\n",
            ),
        )],
        "Release v1.1.0: update Homebrew formula and version",
        Some("github-actions[bot]"),
    );
    mmd.publish();

    let gotool = Repo::new(
        root,
        "gotool",
        "https://github.com/acme/gotool.git",
        &[
            ("go.mod", "module github.com/acme/gotool\n\ngo 1.25\n"),
            ("main.go", "package main\n"),
        ],
    );
    gotool.tag("v1.0.0");
    gotool.commit(&[("main.go", "package main\n// 1.1\n")], "feat");
    gotool.tag("v1.1.0");
    gotool.publish();

    let widget = Repo::new(
        root,
        "widget",
        "https://github.com/acme/widget",
        &[("package.json", r#"{"name": "widget", "version": "2.0.0"}"#)],
    );
    widget.tag("v1.0.0");
    widget.commit(&[("index.js", "")], "feat");
    widget.tag("v2.0.0");
    widget.publish();

    let tool = Repo::new(
        root,
        "tool",
        "https://github.com/acme/tool",
        &[
            ("Cargo.toml", &cargo_toml("toolbin", "")),
            ("src/main.rs", "fn main() {}"),
            (".release.env", "BINS=toolbin\n"),
            ("examples/demo/Cargo.toml", &cargo_toml("demo", "")),
            ("examples/demo/src/main.rs", "fn main() {}"),
        ],
    );
    tool.tag("v1.0.0");
    tool.commit(&[("src/main.rs", "fn main() { }")], "feat");
    tool.tag("v1.1.0");
    tool.publish();

    // A third party's repository, checked out beside the owner's.
    Repo::new(
        root,
        "x",
        "https://github.com/other/x",
        &[("Cargo.toml", &cargo_toml("x", ""))],
    );

    let site_cargo = cargo_toml(
        "site",
        "mmd = { git = \"https://github.com/acme/mmd\", tag = \"v1.0.0\" }\n\
         scene = { git = \"https://github.com/acme/scene\", tag = \"v1.0.0\" }\n\
         missing = { git = \"https://github.com/acme/missing\", tag = \"v1.0.0\" }\n\
         x = { git = \"https://github.com/other/x\", tag = \"v9.9.9\" }\n",
    );
    Repo::new(
        root,
        "site",
        "https://github.com/acme/site",
        &[
            ("Cargo.toml", &site_cargo),
            ("Cargo.lock", SITE_LOCK),
            ("src/main.rs", "fn main() {}"),
            (
                "go.mod",
                "module github.com/acme/site\n\ngo 1.25\n\nrequire (\n\tgithub.com/acme/gotool v1.0.0\n\tgithub.com/spf13/cobra v1.8.0\n)\n",
            ),
            (
                "go.sum",
                "github.com/acme/gotool v1.0.0 h1:abc=\ngithub.com/acme/gotool v1.0.0/go.mod h1:def=\n",
            ),
            (
                "package.json",
                r#"{"name": "site", "dependencies": {"widget": "github:acme/widget#v1.0.0", "left-pad": "^1.0.0"}}"#,
            ),
            (
                "prek.toml",
                "[[repos]]\nrepo = \"https://github.com/acme/tool\"\nrev = \"v1.0.0\"\nhooks = [{ id = \"toolbin\" }]\n\n[[repos]]\nrepo = \"local\"\n",
            ),
            ("Makefile", "check:\n\ttoolbin run .\n\tdemo --flag\n"),
            ("tropism.toml", "exclude = [\"fixtures/**\"]\n"),
            (
                "fixtures/broken/Cargo.toml",
                &cargo_toml(
                    "broken",
                    "scene = { git = \"https://github.com/acme/scene\", tag = \"v0.0.1\" }\n",
                ),
            ),
        ],
    );

    let root = Utf8PathBuf::from_path_buf(root.to_path_buf()).unwrap();
    (tmp, root)
}

fn analyze(root: &Utf8PathBuf, rules: Option<&str>) -> PortfolioReport {
    let options = Options {
        roots: vec![root.clone()],
        owner: "acme".into(),
        fetch: false,
        rules: rules
            .map(|text| PortfolioRules::parse("tropism-portfolio.toml".into(), text).unwrap()),
        tool_version: "test".into(),
    };
    run(&options, &Git).unwrap()
}

fn edges<'a>(report: &'a PortfolioReport, from: &str, to: &str) -> Vec<&'a Edge> {
    report
        .edges
        .iter()
        .filter(|e| e.from == from && e.to == to)
        .collect()
}

fn library<'a>(report: &'a PortfolioReport, name: &str) -> &'a Library {
    report
        .libraries
        .iter()
        .find(|l| l.repo == name)
        .unwrap_or_else(|| panic!("no library {name}"))
}

#[test]
fn repositories_are_identified_by_remote_and_owner() {
    let (_tmp, root) = portfolio();
    let report = analyze(&root, None);
    let names: Vec<(&str, bool)> = report
        .repos
        .iter()
        .map(|r| (r.name.as_str(), r.own))
        .collect();
    assert_eq!(
        names,
        vec![
            ("gotool", true),
            ("mmd", true),
            ("scene", true),
            ("site", true),
            ("tool", true),
            ("widget", true),
            ("other/x", false),
        ]
    );
    // SSH and HTTPS spellings reduce to one.
    let scene = report.repos.iter().find(|r| r.name == "scene").unwrap();
    assert_eq!(scene.remote.as_deref(), Some("github.com/acme/scene"));
    // release-kit's BINS wins over inference, and examples ship nothing.
    let tool = report.repos.iter().find(|r| r.name == "tool").unwrap();
    assert_eq!(tool.binaries, vec!["toolbin"]);
}

#[test]
fn libraries_are_own_repositories_something_links_to() {
    let (_tmp, root) = portfolio();
    let report = analyze(&root, None);
    let names: Vec<&str> = report.libraries.iter().map(|l| l.repo.as_str()).collect();
    assert_eq!(
        names,
        vec!["gotool", "missing", "mmd", "scene", "tool", "widget"]
    );
    // A third party's repository is never a library, however it is linked.
    assert!(!report.edges.iter().any(|e| e.to.contains('x')));

    let missing = library(&report, "missing");
    assert_eq!(missing.status, LibraryStatus::Unavailable);
    assert_eq!(missing.release.status, ReleaseStatus::Unavailable);
    let edge = edges(&report, "site", "missing")[0];
    assert_eq!(edge.behind_by, None);
    assert!(
        edge.behind_note
            .as_deref()
            .unwrap()
            .contains("not checked out")
    );
}

#[test]
fn release_pending_ignores_lockfile_and_release_bot_commits() {
    let (_tmp, root) = portfolio();
    let report = analyze(&root, None);

    let scene = library(&report, "scene");
    assert_eq!(scene.newest_tag.as_deref(), Some("v1.2.0"));
    assert_eq!(scene.release.status, ReleaseStatus::Pending);
    assert_eq!(scene.release.commits.len(), 1);
    assert!(scene.release.commits[0].ends_with("feat: unreleased"));
    assert_eq!(scene.users, vec!["mmd", "site"]);

    let mmd = library(&report, "mmd");
    assert_eq!(
        mmd.release.status,
        ReleaseStatus::Current,
        "{:?}",
        mmd.release
    );

    let pending: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.check == PortfolioCheck::ReleasePending)
        .map(|f| f.repo.as_str())
        .collect();
    assert_eq!(pending, vec!["scene"]);
}

#[test]
fn cargo_links_direct_and_through_another_library() {
    let (_tmp, root) = portfolio();
    let report = analyze(&root, None);

    let mmd = edges(&report, "site", "mmd");
    assert_eq!(mmd.len(), 1);
    assert_eq!(mmd[0].pinned, Some(Pin::new(PinKind::Tag, "v1.0.0")));
    assert_eq!(mmd[0].behind_by, Some(1));
    assert_eq!(mmd[0].resolved[0].version.as_deref(), Some("1.0.0"));
    assert_eq!(mmd[0].confidence, Confidence::High);
    assert!(mmd[0].evidence[0].file.ends_with("site/Cargo.toml"));
    assert_eq!(mmd[0].evidence[0].line, Some(7));

    let scene = edges(&report, "site", "scene");
    assert_eq!(scene.len(), 2, "{scene:#?}");
    let direct = scene.iter().find(|e| e.via == Via::Direct).unwrap();
    assert_eq!(direct.behind_by, Some(2));
    // The second copy is reached through mmd, at mmd's pin.
    let through = scene.iter().find(|e| e.via == Via::Transitive).unwrap();
    assert_eq!(through.through, vec!["mmd"]);
    assert_eq!(through.pinned, Some(Pin::new(PinKind::Tag, "v1.2.0")));
    assert_eq!(through.behind_by, Some(0));

    // Excluded by site's own tropism.toml, as `tropism analyze` would.
    assert!(!report.edges.iter().any(|e| {
        e.evidence
            .iter()
            .any(|ev| ev.file.as_str().contains("fixtures"))
    }));
    let exclusion = &report.exclusions[0];
    assert_eq!(
        (exclusion.pattern.as_str(), exclusion.matched),
        ("fixtures/**", 1)
    );
}

#[test]
fn two_copies_in_one_lockfile_are_reported_as_the_lockfile_not_the_build() {
    let (_tmp, root) = portfolio();
    let report = analyze(&root, None);
    let duplicates: Vec<&PortfolioFinding> = report
        .findings
        .iter()
        .filter(|f| f.check == PortfolioCheck::DuplicateCopy)
        .collect();
    assert_eq!(duplicates.len(), 1);
    let finding = duplicates[0];
    assert_eq!(finding.repo, "site");
    assert_eq!(finding.library.as_deref(), Some("scene"));
    assert_eq!(finding.confidence, Confidence::Medium);
    assert_eq!(finding.severity, Severity::Info);
    assert!(
        finding
            .message
            .starts_with("the lockfile resolves 2 copies of scene"),
        "{}",
        finding.message
    );
    assert!(report.limitations.iter().any(|l| l.contains("S8")));
}

#[test]
fn go_npm_and_binary_links() {
    let (_tmp, root) = portfolio();
    let report = analyze(&root, None);

    let go = edges(&report, "site", "gotool");
    assert_eq!(go.len(), 1);
    assert_eq!(go[0].kind, LinkKind::Go);
    assert_eq!(go[0].package, "github.com/acme/gotool");
    assert_eq!(go[0].behind_by, Some(1));
    assert!(go[0].resolved[0].lockfile.ends_with("go.sum"));

    let npm = edges(&report, "site", "widget");
    assert_eq!(npm.len(), 1);
    assert_eq!(npm[0].kind, LinkKind::Npm);
    assert_eq!(npm[0].behind_by, Some(1));

    let tool = edges(&report, "site", "tool");
    assert_eq!(tool.len(), 2, "{tool:#?}");
    assert!(
        tool.iter()
            .all(|e| e.kind == LinkKind::Binary && e.confidence == Confidence::Medium)
    );
    let pinned = tool.iter().find(|e| e.pinned.is_some()).unwrap();
    assert_eq!(pinned.behind_by, Some(1));
    assert!(pinned.evidence[0].file.ends_with("prek.toml"));
    let used = tool.iter().find(|e| e.pinned.is_none()).unwrap();
    assert_eq!(used.package, "toolbin");
    assert!(used.evidence[0].file.ends_with("Makefile"));
    // `demo` is an example binary of tool's, not something it ships.
    assert!(!report.edges.iter().any(|e| e.package == "demo"));
}

#[test]
fn findings_without_rules_are_info_and_never_gate() {
    let (_tmp, root) = portfolio();
    let report = analyze(&root, None);
    assert_eq!(report.max_severity(), Some(Severity::Info));
    assert!(matches!(
        report.checks[&PortfolioCheck::StaleRule],
        tropism_core::report::CheckStatus::Unavailable { .. }
    ));
}

#[test]
fn rules_set_severity_and_binary_links_count_only_when_named() {
    let (_tmp, root) = portfolio();
    let rules = r#"
[[rules]]
id = "scene-current"
max_tags_behind = 1
libraries = ["scene"]
reason = "scene's users take each release"

[[rules]]
id = "everything-current"
max_tags_behind = 0
severity = "warning"

[[rules]]
id = "one-scene"
single_copy = ["scene"]

[[rules]]
id = "release-promptly"
release_pending = true
libraries = ["scene", "ghost"]
"#;
    let report = analyze(&root, Some(rules));
    let find = |check: PortfolioCheck, to: &str| {
        report
            .findings
            .iter()
            .filter(|f| f.check == check && f.library.as_deref() == Some(to))
            .collect::<Vec<_>>()
    };

    let scene = find(PortfolioCheck::TagsBehind, "scene");
    assert_eq!(scene.len(), 1, "only the direct edge is behind");
    assert_eq!(scene[0].severity, Severity::Error);
    assert_eq!(scene[0].rule.as_deref(), Some("scene-current"));

    let mmd = find(PortfolioCheck::TagsBehind, "mmd");
    assert_eq!(mmd[0].severity, Severity::Warning);
    assert_eq!(mmd[0].rule.as_deref(), Some("everything-current"));

    // Text matching never gates unless a rule names kind "binary".
    let tool = find(PortfolioCheck::TagsBehind, "tool");
    assert_eq!(tool.len(), 1);
    assert_eq!(tool[0].severity, Severity::Info);
    assert_eq!(tool[0].rule, None);

    let duplicate = find(PortfolioCheck::DuplicateCopy, "scene");
    assert_eq!(duplicate[0].severity, Severity::Warning);

    let pending = find(PortfolioCheck::ReleasePending, "scene");
    assert_eq!(pending[0].severity, Severity::Warning);

    let stale: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.check == PortfolioCheck::StaleRule)
        .collect();
    assert_eq!(stale.len(), 1);
    assert!(stale[0].message.contains("`ghost`"));

    assert_eq!(report.max_severity(), Some(Severity::Error));
}

#[test]
fn the_contract_is_versioned_and_stable() {
    let (_tmp, root) = portfolio();
    let first = analyze(&root, None);
    let json: serde_json::Value = serde_json::from_str(&first.to_json_pretty().unwrap()).unwrap();
    assert_eq!(json["schema"], "tropism-portfolio");
    assert_eq!(json["schema_version"], 1);
    for key in [
        "repos",
        "libraries",
        "edges",
        "findings",
        "checks",
        "notes",
        "limitations",
    ] {
        assert!(json.get(key).is_some(), "missing {key}");
    }
    let edge = &json["edges"][0];
    for key in [
        "from",
        "to",
        "kind",
        "package",
        "via",
        "confidence",
        "evidence",
    ] {
        assert!(edge.get(key).is_some(), "edge missing {key}");
    }
    // Same input, same report: ordering never depends on discovery or threads.
    let second = analyze(&root, None);
    assert_eq!(
        first.to_json_pretty().unwrap(),
        second.to_json_pretty().unwrap()
    );
}
