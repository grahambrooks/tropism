//! What a repository's git history says: its remote, its tags, and what its
//! default branch has that its newest tag does not.
//!
//! **Read-only, and never a package manager.** Every call here reads refs and
//! objects the checkout already has. Nothing fetches, so a checkout's refs are
//! exactly as the developer left them; `--fetch` adds `git ls-remote`, which lists
//! the remote's tags without writing anything locally. The constraint in
//! CLAUDE.md is about never resolving dependencies with a native tool and never
//! executing the analyzed repository's code; listing refs does neither.
//!
//! It is a trait so the analysis can be tested against a fixed history, and so the
//! git subprocess stays out of `tropism-core`, which knows nothing of version
//! control.

use std::path::Path;
use std::process::Command;

use tropism_core::telemetry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// The commit the tag points at, peeled through an annotated tag.
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub author: String,
    pub subject: String,
    pub files: Vec<String>,
}

pub trait History: Sync {
    /// `origin`'s URL, or the first remote's when there is no `origin`.
    fn remote_url(&self, repo: &Path) -> Option<String>;
    /// The current branch, `None` when detached.
    fn branch(&self, repo: &Path) -> Option<String>;
    fn head(&self, repo: &Path) -> Option<String>;
    /// Tags as this checkout knows them.
    fn local_tags(&self, repo: &Path) -> Result<Vec<Tag>, String>;
    /// Tags as `origin` has them now (`git ls-remote`).
    fn remote_tags(&self, repo: &Path) -> Result<Vec<Tag>, String>;
    /// The remote-tracking ref for the default branch, e.g. `origin/main`.
    fn default_branch(&self, repo: &Path) -> Option<String>;
    /// Commits reachable from `to` but not from `from`, newest first. `None` when
    /// either end is not in this checkout's history.
    fn commits_between(&self, repo: &Path, from: &str, to: &str) -> Option<Vec<Commit>>;
    /// Tags whose commit is not an ancestor of `rev`. `None` when `rev` is not in
    /// this checkout's history.
    fn tags_not_merged_into(&self, repo: &Path, rev: &str) -> Option<Vec<String>>;
}

/// The real thing: the `git` binary.
pub struct Git;

impl Git {
    fn run(repo: &Path, args: &[&str]) -> Result<String, String> {
        let mut span = telemetry::span(&format!(
            "run git {}",
            args.first().copied().unwrap_or_default()
        ));
        span.set_type("exec")
            .set_kind(telemetry::Kind::Client)
            .attr("process.executable.name", "git");
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            // A remote that wants credentials fails at once rather than waiting
            // on a terminal nobody is watching.
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| {
                span.fail("spawn");
                format!("running `git` failed: {error}")
            })?;
        let code = output.status.code().unwrap_or(-1);
        span.attr("process.exit.code", code);
        if !output.status.success() {
            span.fail(&format!("exit_{code}"));
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn line(repo: &Path, args: &[&str]) -> Option<String> {
        Self::run(repo, args)
            .ok()
            .map(|out| out.trim().to_owned())
            .filter(|out| !out.is_empty())
    }
}

impl History for Git {
    fn remote_url(&self, repo: &Path) -> Option<String> {
        Self::line(repo, &["remote", "get-url", "origin"]).or_else(|| {
            let first = Self::line(repo, &["remote"])?;
            let name = first.lines().next()?.to_owned();
            Self::line(repo, &["remote", "get-url", &name])
        })
    }

    fn branch(&self, repo: &Path) -> Option<String> {
        Self::line(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])
    }

    fn head(&self, repo: &Path) -> Option<String> {
        Self::line(repo, &["rev-parse", "--verify", "--quiet", "HEAD"])
    }

    fn local_tags(&self, repo: &Path) -> Result<Vec<Tag>, String> {
        let out = Self::run(
            repo,
            &[
                "for-each-ref",
                "--format=%(refname:short)%09%(objectname)%09%(*objectname)",
                "refs/tags",
            ],
        )?;
        Ok(out
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('\t');
                let name = fields.next()?.to_owned();
                let object = fields.next()?;
                let peeled = fields.next().filter(|p| !p.is_empty()).unwrap_or(object);
                Some(Tag {
                    name,
                    commit: peeled.to_owned(),
                })
            })
            .collect())
    }

    fn remote_tags(&self, repo: &Path) -> Result<Vec<Tag>, String> {
        let out = Self::run(
            repo,
            &[
                "-c",
                "http.lowSpeedLimit=1000",
                "-c",
                "http.lowSpeedTime=20",
                "ls-remote",
                "--tags",
                "origin",
            ],
        )?;
        Ok(parse_ls_remote(&out))
    }

    fn default_branch(&self, repo: &Path) -> Option<String> {
        Self::line(
            repo,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--verify",
                "--quiet",
                "origin/HEAD",
            ],
        )
        .or_else(|| {
            ["origin/main", "origin/master"].into_iter().find_map(|r| {
                Self::line(repo, &["rev-parse", "--verify", "--quiet", r]).map(|_| r.to_owned())
            })
        })
    }

    fn commits_between(&self, repo: &Path, from: &str, to: &str) -> Option<Vec<Commit>> {
        let range = format!("{from}..{to}");
        let out = Self::run(
            repo,
            &[
                "log",
                "--name-only",
                "--format=%x1e%H%x1f%an%x1f%s",
                &range,
                "--",
            ],
        )
        .ok()?;
        Some(parse_log(&out))
    }

    fn tags_not_merged_into(&self, repo: &Path, rev: &str) -> Option<Vec<String>> {
        Self::run(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{rev}^{{commit}}"),
            ],
        )
        .ok()?;
        let out = Self::run(repo, &["tag", "--no-merged", rev]).ok()?;
        Some(out.lines().map(str::to_owned).collect())
    }
}

/// `sha\trefs/tags/name` lines, with `^{}` lines carrying the peeled commit of an
/// annotated tag.
pub fn parse_ls_remote(out: &str) -> Vec<Tag> {
    let mut tags: Vec<Tag> = Vec::new();
    for line in out.lines() {
        let Some((sha, reference)) = line.split_once('\t') else {
            continue;
        };
        let Some(name) = reference.strip_prefix("refs/tags/") else {
            continue;
        };
        if let Some(name) = name.strip_suffix("^{}") {
            if let Some(tag) = tags.iter_mut().find(|t| t.name == name) {
                tag.commit = sha.to_owned();
            }
            continue;
        }
        tags.push(Tag {
            name: name.to_owned(),
            commit: sha.to_owned(),
        });
    }
    tags
}

fn parse_log(out: &str) -> Vec<Commit> {
    out.split('\x1e')
        .filter(|record| !record.trim().is_empty())
        .filter_map(|record| {
            let mut lines = record.lines();
            let header = lines.next()?;
            let mut fields = header.split('\x1f');
            Some(Commit {
                sha: fields.next()?.to_owned(),
                author: fields.next()?.to_owned(),
                subject: fields.next().unwrap_or_default().to_owned(),
                files: lines
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect(),
            })
        })
        .collect()
}

/// Lockfiles. A library's users never read the library's own lockfile, so a
/// commit touching nothing else gives them nothing.
const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "go.sum",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
];

/// The commits a library's users would get from a release — sustain's
/// `visible_to_users`: lock-only commits and release-kit's own bump commit do not
/// count. A merge commit lists no files and so does not count either; the commits
/// it merges are counted on their own.
pub fn visible_to_users(commits: &[Commit]) -> Vec<&Commit> {
    commits
        .iter()
        .filter(|c| !(c.author.ends_with("[bot]") && c.subject.starts_with("Release v")))
        .filter(|c| {
            c.files.iter().any(|file| {
                let base = file.rsplit('/').next().unwrap_or(file);
                !LOCKFILES.contains(&base)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(author: &str, subject: &str, files: &[&str]) -> Commit {
        Commit {
            sha: "a".repeat(40),
            author: author.into(),
            subject: subject.into(),
            files: files.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    #[test]
    fn lock_only_and_release_bot_commits_are_not_unreleased_work() {
        let commits = [
            commit("Graham", "chore(deps): cargo update", &["Cargo.lock"]),
            commit("Graham", "nested lock", &["crates/x/Cargo.lock"]),
            commit(
                "github-actions[bot]",
                "Release v2026.11.3: update Homebrew formula and version",
                &["Cargo.toml", "Formula/x.rb"],
            ),
            commit("Graham", "merge", &[]),
            commit(
                "Graham",
                "feat: a real change",
                &["src/lib.rs", "Cargo.lock"],
            ),
        ];
        let visible = visible_to_users(&commits);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].subject, "feat: a real change");
    }

    #[test]
    fn a_human_commit_titled_release_still_counts() {
        let commits = [commit(
            "Graham",
            "Release v2026.11.3: notes",
            &["README.md"],
        )];
        assert_eq!(visible_to_users(&commits).len(), 1);
    }

    #[test]
    fn ls_remote_peels_annotated_tags() {
        let out = "aaa\trefs/tags/v1.0.0\nbbb\trefs/tags/v1.0.0^{}\nccc\trefs/tags/v1.1.0\n";
        let tags = parse_ls_remote(out);
        assert_eq!(
            tags,
            vec![
                Tag {
                    name: "v1.0.0".into(),
                    commit: "bbb".into()
                },
                Tag {
                    name: "v1.1.0".into(),
                    commit: "ccc".into()
                },
            ]
        );
    }

    #[test]
    fn log_records_carry_their_files() {
        let out = "\x1eabc\x1fGraham\x1ffeat: x\n\nsrc/a.rs\nCargo.lock\n\x1edef\x1fBot\x1fmerge\n";
        let commits = parse_log(out);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].files, vec!["src/a.rs", "Cargo.lock"]);
        assert!(commits[1].files.is_empty());
    }
}
