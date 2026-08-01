//! The two git-backed conveniences over `check`'s file list.
//!
//! **This lives in the CLI, not in the core.** `tropism-core` takes a plain list of
//! paths and knows nothing about version control, which is what lets it analyze a
//! directory that is not a repository at all — an extracted tarball, a worktree
//! fragment, a container layer. Teaching it about git would spend that property on
//! a convenience.
//!
//! `design/14-incremental-checking.md` proposes reading the index directly with
//! `gix` rather than invoking `git`, so a checkout with no git binary still works.
//! That is not the trade taken here, and the reason is worth recording: `gix` is a
//! large dependency tree to add for two flags, and a checkout with no git binary
//! also has no staged files and no refs to diff against — the case the dependency
//! would buy does not exist. Invoking `git` is confined to these two functions and
//! happens only when the corresponding flag is passed, so the default path stays
//! exactly as hermetic as before.
//!
//! Note also what is *not* being run: `git` is not a package manager and reads no
//! manifest. The constraint in CLAUDE.md is about never resolving dependencies with
//! a native tool, and about never executing the analyzed repository's own code.
//! Neither is happening here.

use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};

/// Files staged for commit, relative to the repository root.
///
/// Includes added, copied, modified, and renamed entries; excludes deletions,
/// because a file that no longer exists cannot be the source end of an import.
pub fn staged_files(repo: &Utf8Path) -> anyhow::Result<Vec<Utf8PathBuf>> {
    run(
        repo,
        &[
            "diff",
            "--name-only",
            "--cached",
            "--diff-filter=ACMR",
            "-z",
        ],
    )
}

/// Files changed since `base`, relative to the repository root.
///
/// Uses the three-dot form, so the comparison is against the merge base rather
/// than the tip of `base`. Without it, a long-lived branch reports every file that
/// changed on `main` in the meantime as part of the change under review — which is
/// the "huge changed set" failure `design/14` warns about in open question 4.
pub fn files_since(repo: &Utf8Path, base: &str) -> anyhow::Result<Vec<Utf8PathBuf>> {
    run(
        repo,
        &[
            "diff",
            "--name-only",
            "--diff-filter=ACMR",
            "-z",
            &format!("{base}..."),
        ],
    )
}

fn run(repo: &Utf8Path, args: &[&str]) -> anyhow::Result<Vec<Utf8PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo.as_std_path())
        .args(args)
        .output()
        .map_err(|error| {
            anyhow::anyhow!("running `git` failed: {error}. Pass file paths directly instead")
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git {}: {}", args.join(" "), stderr.trim());
    }

    // `-z` because a filename may contain anything but NUL, including a newline.
    // Splitting on newlines silently truncates such a path into two wrong ones.
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| anyhow::anyhow!("git returned a path that is not valid UTF-8"))?;

    Ok(stdout
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(Utf8PathBuf::from)
        .collect())
}
