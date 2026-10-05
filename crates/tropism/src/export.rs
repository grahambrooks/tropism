//! `tropism export`: dependency and conformance records for software-analytics.
//!
//! Writes delivery-schema records (software-analytics `delivery-schema.md`,
//! "Changeability") as JSON Lines on stdout, for every git mirror under a
//! directory laid out `<owner>/<name>`, as insight-collect keeps them.
//! insight-collect runs it after each sync and streams the records into
//! insight (roadmap Phase 5).
//!
//! Per repository and checked-out commit:
//! - `conformance_snapshot`: what ran and what it found, by check;
//! - `conformance_finding`: each finding, with tropism's stable finding id, so
//!   a finding can be followed from the commit that introduced it to the one
//!   that resolved it.
//!
//! **Cursor.** As `symgraph export`: each exported commit gets a sequence
//! number (`source_seq`), kept in `<state>/export.json`. A repository is
//! exported again only when its commit changed, or when its last export is
//! newer than the `--since` cursor (sent, but not confirmed). Record ids name
//! the commit, so a re-sent export is idempotent.
//!
//! **git.** As with `--staged` and `--since` on `check`, `git` runs only in the
//! CLI, and only to read each mirror's commit; tropism-core still sees a plain
//! directory.

use std::collections::BTreeMap;
use std::io::Write;
use std::process::Command;

use anyhow::{Context, Result, bail};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tropism_core::pipeline;
use tropism_core::report::{CheckId, CheckStatus, ProjectReport, Severity};

/// Delivery-schema version these records follow.
pub const SCHEMA_VERSION: &str = "1.0";

pub struct ExportOptions {
    /// Directory of mirrors, `<owner>/<name>` each.
    pub mirrors: Utf8PathBuf,
    /// Directory for the export's own state (sequence numbers per commit).
    pub state: Utf8PathBuf,
    /// The caller's cursor: the highest `source_seq` it has stored.
    pub since: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ExportSummary {
    pub repositories: usize,
    pub exported: usize,
    pub unchanged: usize,
    pub failed: usize,
    pub records: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    next_seq: u64,
    repositories: BTreeMap<String, Exported>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Exported {
    commit: String,
    seq: u64,
}

/// Export every mirror's records to `out`. A repository that fails is
/// reported on stderr and left for the next run; the others still export.
pub fn export(opts: &ExportOptions, out: &mut impl Write) -> Result<ExportSummary> {
    let state_path = opts.state.join("export.json");
    let mut state: State = match std::fs::read_to_string(&state_path) {
        Ok(text) => serde_json::from_str(&text).with_context(|| format!("reading {state_path}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
        Err(e) => return Err(e).with_context(|| format!("reading {state_path}")),
    };

    let providers = tropism_lang::registry();
    let mut summary = ExportSummary::default();
    for (repo, path) in discover(&opts.mirrors)? {
        summary.repositories += 1;
        let commit = match git(&path, &["rev-parse", "HEAD"]) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("tropism export: {repo}: {e:#}");
                summary.failed += 1;
                continue;
            }
        };
        let seq = match state.repositories.get(&repo) {
            Some(prev) if prev.commit == commit && prev.seq <= opts.since => {
                summary.unchanged += 1;
                continue;
            }
            // Sent before but not confirmed: the same records again.
            Some(prev) if prev.commit == commit => prev.seq,
            _ => {
                state.next_seq += 1;
                state.next_seq
            }
        };
        match repository_records(&repo, &path, &commit, seq, &providers) {
            Ok(records) => {
                for r in &records {
                    serde_json::to_writer(&mut *out, r)?;
                    out.write_all(b"\n")?;
                }
                summary.records += records.len();
                summary.exported += 1;
                state.repositories.insert(repo, Exported { commit, seq });
            }
            Err(e) => {
                eprintln!("tropism export: {repo}: {e:#}");
                summary.failed += 1;
            }
        }
    }
    out.flush()?;

    std::fs::create_dir_all(&opts.state).with_context(|| format!("creating {}", opts.state))?;
    let tmp = opts.state.join("export.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&state)?)?;
    std::fs::rename(&tmp, &state_path)?;
    Ok(summary)
}

/// Git checkouts under `mirrors`, as (`owner/name`, path), in name order.
fn discover(mirrors: &Utf8Path) -> Result<Vec<(String, Utf8PathBuf)>> {
    let dirs = |dir: &Utf8Path| -> Result<Vec<Utf8PathBuf>> {
        let mut v: Vec<Utf8PathBuf> = dir
            .read_dir_utf8()
            .with_context(|| format!("reading {dir}"))?
            .filter_map(|e| e.ok())
            .map(|e| e.into_path())
            .filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.starts_with('.')))
            .collect();
        v.sort();
        Ok(v)
    };
    let mut found = Vec::new();
    for owner in dirs(mirrors)? {
        for repo in dirs(&owner)? {
            if repo.join(".git").exists() {
                let name = |p: &Utf8Path| p.file_name().unwrap_or_default().to_string();
                found.push((format!("{}/{}", name(&owner), name(&repo)), repo));
            }
        }
    }
    Ok(found)
}

fn git(path: &Utf8Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Analyze the checkout (with its tropism.toml, if any) and build its records.
fn repository_records(
    repo: &str,
    path: &Utf8Path,
    commit: &str,
    seq: u64,
    providers: &[&dyn tropism_core::provider::LanguageProvider],
) -> Result<Vec<Value>> {
    let options = pipeline::Options {
        respect_ignore: true,
        rules_path: None,
        use_rules: true,
        rules_only: false,
    };
    let report = pipeline::analyze(path, providers, &options)?;
    let common = Common {
        repo,
        commit,
        seq,
        changed_at: &git(path, &["log", "-1", "--format=%cI", "HEAD"])?,
        observed_at: &rfc3339_now(),
    };

    let mut records = vec![snapshot(
        &common,
        &report.projects,
        path.join("tropism.toml").exists(),
    )];
    for project in &report.projects {
        for f in &project.findings {
            let first = f.evidence.first();
            records.push(common.record(
                "conformance_finding",
                &format!("finding:{}", f.id),
                json!({
                    // The same finding at every commit: follow it from the one
                    // that introduced it to the one that resolved it.
                    "finding_key": format!("{repo}:{}", f.id),
                    "finding_id": f.id,
                    "check": f.check.as_str(),
                    "severity": f.severity.as_str(),
                    "confidence": f.confidence.as_str(),
                    "message": f.message,
                    "project": match project.project.root.as_str() { "" => ".", root => root },
                    "language": project.project.language.as_str(),
                    "file": first.map(|e| e.file.as_str()),
                    "line": first.and_then(|e| e.line),
                    "dependency": f.details.get("dependency").cloned(),
                }),
            ));
        }
    }
    Ok(records)
}

fn snapshot(common: &Common, projects: &[ProjectReport], rules_file: bool) -> Value {
    let count = |check: CheckId| -> usize {
        projects
            .iter()
            .map(|p| p.findings.iter().filter(|f| f.check == check).count())
            .sum()
    };
    let severity = |s: Severity| -> usize {
        projects
            .iter()
            .flat_map(|p| &p.findings)
            .filter(|f| f.severity == s)
            .count()
    };
    // Checks that couldn't run in some project, by check: honest gaps.
    let mut unavailable: BTreeMap<&str, usize> = BTreeMap::new();
    for p in projects {
        for (check, status) in &p.checks {
            if !matches!(status, CheckStatus::Ran { .. }) {
                *unavailable.entry(check.as_str()).or_default() += 1;
            }
        }
    }
    let ran = |check: CheckId| {
        projects
            .iter()
            .any(|p| matches!(p.checks.get(&check), Some(CheckStatus::Ran { .. })))
    };
    let mut languages: Vec<&str> = projects
        .iter()
        .map(|p| p.project.language.as_str())
        .collect();
    languages.sort_unstable();
    languages.dedup();
    let rule_violations = count(CheckId::ModuleRule) + count(CheckId::PackageRule);
    common.record(
        "conformance_snapshot",
        "snapshot",
        json!({
            "projects": projects.len(),
            "languages": languages,
            "rules": rules_file,
            "rules_ran": ran(CheckId::ModuleRule) || ran(CheckId::PackageRule),
            "violations": rule_violations,
            "cycles": count(CheckId::Cycle),
            "manifest_problems": count(CheckId::UnusedDep) + count(CheckId::MissingDep),
            "unused_deps": count(CheckId::UnusedDep),
            "missing_deps": count(CheckId::MissingDep),
            "version_conflicts": count(CheckId::VersionConflict),
            "diamond_deps": count(CheckId::DiamondDep),
            "findings": projects.iter().map(|p| p.findings.len()).sum::<usize>(),
            "errors": severity(Severity::Error),
            "warnings": severity(Severity::Warning),
            "unavailable_checks": unavailable.values().sum::<usize>(),
            "unavailable_by_check": unavailable,
        }),
    )
}

/// Fields every record of one repository's export shares.
struct Common<'a> {
    repo: &'a str,
    commit: &'a str,
    seq: u64,
    changed_at: &'a str,
    observed_at: &'a str,
}

impl Common<'_> {
    fn record(&self, record_type: &str, key: &str, fields: Value) -> Value {
        let mut r = Map::new();
        r.insert("record_type".into(), json!(record_type));
        r.insert(
            "id".into(),
            json!(format!("tropism:{}@{}:{key}", self.repo, self.commit)),
        );
        r.insert("schema_version".into(), json!(SCHEMA_VERSION));
        r.insert("source".into(), json!("tropism"));
        r.insert("repo".into(), json!(self.repo));
        r.insert("commit".into(), json!(self.commit));
        r.insert("changed_at".into(), json!(self.changed_at));
        r.insert("observed_at".into(), json!(self.observed_at));
        r.insert("collected_at".into(), json!(self.observed_at));
        r.insert("source_seq".into(), json!(self.seq));
        if let Value::Object(f) = fields {
            r.extend(f);
        }
        Value::Object(r)
    }
}

/// The current UTC time as RFC 3339, without a date crate.
fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    rfc3339(secs)
}

fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc_timestamps() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_791_158_400), "2026-10-05T00:00:00Z");
    }

    fn temp(name: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .unwrap()
            .join(format!("tropism-export-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A Rust crate under `<dir>/acme/app` declaring a dependency it never uses.
    fn mirror(dir: &Utf8Path) -> Utf8PathBuf {
        let path = dir.join("acme").join("app");
        std::fs::create_dir_all(path.join("src")).unwrap();
        std::fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = \"1\"\n",
        )
        .unwrap();
        std::fs::write(path.join("src/main.rs"), "fn main() {}\n").unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-qm",
                "init",
            ],
        ] {
            git(&path, &args).unwrap();
        }
        path
    }

    fn run(mirrors: &Utf8Path, state: &Utf8Path, since: u64) -> (ExportSummary, Vec<Value>) {
        let opts = ExportOptions {
            mirrors: mirrors.to_owned(),
            state: state.to_owned(),
            since,
        };
        let mut out = Vec::new();
        let summary = export(&opts, &mut out).unwrap();
        let records = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        (summary, records)
    }

    #[test]
    fn exports_findings_once_the_caller_confirms_them() {
        let dir = temp("cursor");
        let mirrors = dir.join("workspace");
        let state = dir.join("state");
        let repo = mirror(&mirrors);
        let commit = git(&repo, &["rev-parse", "HEAD"]).unwrap();

        let (first, records) = run(&mirrors, &state, 0);
        assert_eq!((first.exported, first.unchanged), (1, 0));
        let snapshot = records
            .iter()
            .find(|r| r["record_type"] == "conformance_snapshot")
            .expect("a snapshot");
        assert_eq!(snapshot["repo"], "acme/app");
        assert_eq!(snapshot["source"], "tropism");
        assert_eq!(snapshot["commit"], commit.as_str());
        assert_eq!(snapshot["rules"], false, "no tropism.toml");
        assert_eq!(
            snapshot["unused_deps"], 1,
            "serde is declared but never used"
        );
        let finding = records
            .iter()
            .find(|r| r["record_type"] == "conformance_finding")
            .expect("a finding");
        assert_eq!(finding["check"], "unused-dep");
        assert!(
            finding["finding_key"]
                .as_str()
                .unwrap()
                .starts_with("acme/app:unused-dep")
        );
        assert_eq!(finding["source_seq"], 1);

        // Not confirmed (cursor 0): the same ids again. Confirmed: nothing.
        let (_, again) = run(&mirrors, &state, 0);
        assert_eq!(
            again.iter().map(|r| r["id"].clone()).collect::<Vec<_>>(),
            records.iter().map(|r| r["id"].clone()).collect::<Vec<_>>()
        );
        let (confirmed, none) = run(&mirrors, &state, 1);
        assert_eq!((confirmed.exported, confirmed.unchanged), (0, 1));
        assert!(none.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
