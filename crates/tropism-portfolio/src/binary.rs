//! Installed-binary links: one repository running a tool another one builds.
//!
//! **Text matching, not import resolution.** Nothing imports a CLI; a Makefile
//! runs it, a hook pins it, a workflow installs it. So these links are found by
//! patterns over the files where tools are pinned and run, and are reported at
//! medium confidence. They are for visibility: bumping a pinned CLI is a release
//! decision, not a build that breaks, so there is no cascade for them and they
//! never gate by default.
//!
//! What is matched, most specific first:
//!
//! - a table with a `repo` key and a `ref`/`rev`/`tag`/`version` beside it, in a
//!   TOML or JSON file at the repository root (`prek.toml`, `tools.toml`,
//!   `tools.lock.json`);
//! - `repo:` / `rev:` pairs in `.pre-commit-config.yaml`;
//! - `owner/repo@ref` (a workflow `uses:`, `go install …@v1`);
//! - `github.com/owner/repo/releases/download/<tag>/…`;
//! - an `install` line naming the owner's repository URL (`cargo install --git …`);
//! - `brew "owner/tap/formula"` and `brew install owner/tap/formula`;
//! - a command whose first word is a binary the owner builds, in a Makefile
//!   recipe, a workflow, a script or a Dockerfile `RUN`.
//!
//! A URL that is merely mentioned — a README link, a comment — is not a link.

use camino::Utf8PathBuf;
use regex::Regex;

use crate::model::{Pin, PinKind};
use crate::text::line_of;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A repository, as the file spells it: a URL, `owner/name`, or a bare name.
    Repo(String),
    /// A binary or Homebrew formula name.
    Binary(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryRef {
    pub file: Utf8PathBuf,
    pub line: Option<u32>,
    pub target: Target,
    pub pin: Option<Pin>,
    pub how: &'static str,
}

/// A line where a command's first word could be one of the owner's binaries.
/// Matched later, once every repository's binaries are known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    pub file: Utf8PathBuf,
    pub line: u32,
    pub text: String,
}

/// How a file takes part in binary-link discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRole {
    /// A Makefile, justfile, script or workflow: patterns and commands.
    Commands,
    /// A Dockerfile: patterns, and commands on `RUN` lines.
    Dockerfile,
    /// A Brewfile.
    Brewfile,
    /// `.pre-commit-config.yaml`.
    PreCommit,
    /// A root-level TOML or JSON file that may pin tools by `repo` and `ref`.
    Structured,
}

pub struct Patterns {
    at_ref: Regex,
    download: Regex,
    url: Regex,
    formula: Regex,
    flag: Regex,
}

impl Patterns {
    pub fn new(owner: &str) -> Self {
        let owner = regex::escape(owner);
        let name = r"[A-Za-z0-9_.-]+";
        let compile = |pattern: String| Regex::new(&pattern).expect("static pattern compiles");
        Self {
            at_ref: compile(format!(
                r"(?i)(?:^|[\s'\x22=(])(?:github\.com/)?{owner}/({name})(?:/[A-Za-z0-9_./-]*)?@([A-Za-z0-9_./-]+)"
            )),
            download: compile(format!(
                r"(?i)github\.com/{owner}/({name})/releases/download/([^/\s'\x22]+)"
            )),
            url: compile(format!(r"(?i)github\.com[/:]{owner}/({name})")),
            formula: compile(format!(
                r#"(?i)(?:brew\s+"|brew\s+install\s+(?:--\S+\s+)*){owner}/[A-Za-z0-9_-]+/([A-Za-z0-9_@.+-]+)"#
            )),
            flag: compile(r"--(tag|rev|branch)[=\s]+([A-Za-z0-9_./-]+)".to_owned()),
        }
    }
}

fn classify(reference: &str) -> Pin {
    let reference = reference.trim_end_matches(['.', ',']);
    if crate::version::stable_tag(reference).is_some() {
        Pin::new(PinKind::Tag, reference)
    } else if crate::version::looks_like_commit(reference) {
        Pin::new(PinKind::Rev, reference)
    } else {
        Pin::new(PinKind::Branch, reference)
    }
}

pub fn scan(
    file: &Utf8PathBuf,
    text: &str,
    role: FileRole,
    patterns: &Patterns,
) -> (Vec<BinaryRef>, Vec<CommandLine>) {
    let mut refs = Vec::new();
    let mut commands = Vec::new();
    match role {
        FileRole::Structured => structured(file, text, &mut refs),
        FileRole::PreCommit => pre_commit(file, text, &mut refs),
        FileRole::Commands | FileRole::Dockerfile | FileRole::Brewfile => {}
    }
    if matches!(role, FileRole::Structured | FileRole::PreCommit) {
        return (refs, commands);
    }
    let make = file.file_name().is_some_and(|n| {
        n.eq_ignore_ascii_case("makefile") || n == "GNUmakefile" || n.ends_with(".mk")
    });
    for (index, line) in text.lines().enumerate() {
        let line_no = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        let mut push = |target: Target, pin: Option<Pin>, how| {
            refs.push(BinaryRef {
                file: file.clone(),
                line: Some(line_no),
                target,
                pin,
                how,
            });
        };
        let mut seen_repo = false;
        for caps in patterns.at_ref.captures_iter(line) {
            push(
                Target::Repo(caps[1].to_owned()),
                Some(classify(&caps[2])),
                "owner/repo@ref",
            );
            seen_repo = true;
        }
        for caps in patterns.download.captures_iter(line) {
            push(
                Target::Repo(caps[1].to_owned()),
                Some(Pin::new(PinKind::Tag, &caps[2])),
                "release download",
            );
            seen_repo = true;
        }
        if !seen_repo
            && line.contains("install")
            && let Some(caps) = patterns.url.captures(line)
        {
            let pin = patterns
                .flag
                .captures(line)
                .map(|flag| match &flag[1] {
                    "tag" => Pin::new(PinKind::Tag, &flag[2]),
                    "rev" => Pin::new(PinKind::Rev, &flag[2]),
                    _ => Pin::new(PinKind::Branch, &flag[2]),
                })
                .unwrap_or_else(|| Pin::new(PinKind::DefaultBranch, "HEAD"));
            push(
                Target::Repo(caps[1].trim_end_matches(".git").to_owned()),
                Some(pin),
                "install from URL",
            );
        }
        for caps in patterns.formula.captures_iter(line) {
            push(Target::Binary(caps[1].to_owned()), None, "homebrew formula");
        }

        let command = match role {
            FileRole::Commands if make => line.starts_with('\t'),
            FileRole::Commands => true,
            FileRole::Dockerfile => trimmed.starts_with("RUN "),
            _ => false,
        };
        if command && !trimmed.is_empty() {
            // A recipe's `@` (silent), `-` (ignore errors) and `+` prefixes belong
            // to make, not to the command. Elsewhere a leading `-` is a flag.
            let text = if make {
                trimmed.trim_start_matches(['@', '-', '+'])
            } else {
                line
            };
            commands.push(CommandLine {
                file: file.clone(),
                line: line_no,
                text: text.to_owned(),
            });
        }
    }
    (refs, commands)
}

/// The words in command position on a line: the first word of each command in a
/// pipeline or list, after the prefixes that run another command.
pub fn command_words(line: &str) -> Vec<&str> {
    let mut words = Vec::new();
    for segment in line.split(['|', ';', '&', '`', '(']) {
        let mut tokens = segment.split_whitespace().peekable();
        while let Some(&token) = tokens.peek() {
            let skip = matches!(
                token,
                "-" | "run:"
                    | "RUN"
                    | "exec"
                    | "sudo"
                    | "time"
                    | "env"
                    | "command"
                    | "then"
                    | "do"
                    | "else"
                    | "$"
                    | "!"
            ) || (token.contains('=') && !token.starts_with('-'));
            if !skip {
                break;
            }
            tokens.next();
        }
        let Some(word) = tokens.next() else { continue };
        let word = word.trim_matches(['"', '\'']);
        // A flag is never a command: `-m "…"` on a continuation line, or
        // `args+=(--baseline …)`.
        if word.starts_with('-') || word.starts_with('$') {
            continue;
        }
        // `~/.cargo/bin/tropism` is still tropism; `./target/debug/tropism` is the
        // repository running its own build, which is not an installed binary.
        if word.starts_with("./") || word.starts_with("target/") {
            continue;
        }
        let word = word.rsplit('/').next().unwrap_or(word);
        if !word.is_empty() {
            words.push(word);
        }
    }
    words
}

fn structured(file: &Utf8PathBuf, text: &str, refs: &mut Vec<BinaryRef>) {
    let value: Option<serde_json::Value> = if file.extension() == Some("json") {
        serde_json::from_str(text).ok()
    } else {
        toml::from_str::<toml::Value>(text)
            .ok()
            .and_then(|v| serde_json::to_value(v).ok())
    };
    let Some(value) = value else { return };
    let mut found = Vec::new();
    walk(&value, &mut found);
    for (repo, pin) in found {
        refs.push(BinaryRef {
            file: file.clone(),
            line: line_of(text, &repo),
            target: Target::Repo(repo),
            pin,
            how: "repo and ref in a tool manifest",
        });
    }
}

fn walk(value: &serde_json::Value, found: &mut Vec<(String, Option<Pin>)>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(repo) = map.get("repo").and_then(serde_json::Value::as_str) {
                let pin = ["ref", "rev", "tag", "version"].iter().find_map(|key| {
                    map.get(*key)
                        .and_then(serde_json::Value::as_str)
                        .map(classify)
                });
                if pin.is_some() {
                    found.push((repo.to_owned(), pin));
                }
            }
            for child in map.values() {
                walk(child, found);
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|item| walk(item, found)),
        _ => {}
    }
}

fn pre_commit(file: &Utf8PathBuf, text: &str, refs: &mut Vec<BinaryRef>) {
    let mut current: Option<(String, u32)> = None;
    let mut flush = |current: &mut Option<(String, u32)>, pin: Option<Pin>| {
        if let Some((repo, line)) = current.take() {
            refs.push(BinaryRef {
                file: file.clone(),
                line: Some(line),
                target: Target::Repo(repo),
                pin,
                how: "pre-commit repo and rev",
            });
        }
    };
    for (index, line) in text.lines().enumerate() {
        let line_no = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let trimmed = line.trim().trim_start_matches("- ").trim();
        if let Some(repo) = trimmed.strip_prefix("repo:") {
            flush(&mut current, None);
            current = Some((repo.trim().trim_matches(['"', '\'']).to_owned(), line_no));
        } else if let Some(rev) = trimmed.strip_prefix("rev:") {
            let rev = rev.trim().trim_matches(['"', '\'']);
            flush(&mut current, Some(classify(rev)));
        }
    }
    flush(&mut current, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(name: &str, text: &str, role: FileRole) -> Vec<(Target, Option<String>)> {
        let patterns = Patterns::new("grahambrooks");
        scan(&Utf8PathBuf::from(name), text, role, &patterns)
            .0
            .into_iter()
            .map(|r| (r.target, r.pin.map(|p| p.value)))
            .collect()
    }

    #[test]
    fn a_tool_manifest_pins_by_repo_and_ref() {
        let text = "[[plugin]]\nname = \"architecture\"\n  [[plugin.tool]]\n  repo = \"tropism\"\n  ref = \"v2026.11.2\"\n";
        assert_eq!(
            refs("tools.toml", text, FileRole::Structured),
            vec![(Target::Repo("tropism".into()), Some("v2026.11.2".into()))]
        );
        let prek = "[[repos]]\nrepo = \"https://github.com/grahambrooks/tropism\"\nrev = \"v2026.11.3\"\n[[repos]]\nrepo = \"local\"\n";
        assert_eq!(
            refs("prek.toml", prek, FileRole::Structured),
            vec![(
                Target::Repo("https://github.com/grahambrooks/tropism".into()),
                Some("v2026.11.3".into())
            )]
        );
    }

    #[test]
    fn pre_commit_yaml_pairs_repo_with_rev() {
        let text = "repos:\n  - repo: https://github.com/grahambrooks/tropism\n    rev: v2026.11.3\n    hooks:\n      - id: tropism\n  - repo: local\n    hooks: []\n";
        let found = refs(".pre-commit-config.yaml", text, FileRole::PreCommit);
        assert_eq!(
            found[0],
            (
                Target::Repo("https://github.com/grahambrooks/tropism".into()),
                Some("v2026.11.3".into())
            )
        );
        assert_eq!(found[1], (Target::Repo("local".into()), None));
    }

    #[test]
    fn workflow_and_script_patterns() {
        let text = "      - uses: grahambrooks/release-kit/.github/actions/setup@v2\n      - run: go install github.com/grahambrooks/imprint/cmd/imprint@v1.4.0\n      - run: curl -L https://github.com/grahambrooks/tropism/releases/download/v2026.11.3/tropism.tar.gz\n      - run: cargo install --git https://github.com/grahambrooks/sustain --tag v2026.11.5\n      - run: brew install grahambrooks/tap/symgraph\n# see https://github.com/grahambrooks/forge for docs\n";
        let found = refs(".github/workflows/ci.yml", text, FileRole::Commands);
        assert_eq!(
            found,
            vec![
                (Target::Repo("release-kit".into()), Some("v2".into())),
                (Target::Repo("imprint".into()), Some("v1.4.0".into())),
                (Target::Repo("tropism".into()), Some("v2026.11.3".into())),
                (Target::Repo("sustain".into()), Some("v2026.11.5".into())),
                (Target::Binary("symgraph".into()), None),
            ]
        );
    }

    #[test]
    fn a_mention_is_not_a_link() {
        let text = "Docs live at https://github.com/grahambrooks/forge.\nemail: someone@grahambrooks.com\n";
        assert!(refs("Makefile", text, FileRole::Commands).is_empty());
    }

    #[test]
    fn command_position_only() {
        assert_eq!(
            command_words("\ttropism analyze . --format text"),
            vec!["tropism"]
        );
        assert_eq!(
            command_words("      - run: symgraph index && colab find x | mmd render"),
            vec!["symgraph", "colab", "mmd"]
        );
        assert_eq!(
            command_words("  FOO=1 ~/.cargo/bin/sustain --dry-run"),
            vec!["sustain"]
        );
        assert_eq!(
            command_words("\t./target/debug/tropism analyze ."),
            Vec::<&str>::new()
        );
        assert_eq!(command_words("      - name: Run tropism"), vec!["name:"]);
        // Found on real repositories: a flag on a continuation line, and an array
        // append, each once read as a binary named after the flag.
        assert_eq!(
            command_words("  -m \"Zero transfers\" | tee /dev/stderr"),
            vec!["tee"]
        );
        assert_eq!(
            command_words("            args+=(--baseline \"$baseline\")"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn only_recipe_lines_of_a_makefile_are_commands() {
        let patterns = Patterns::new("grahambrooks");
        let text = "TROPISM ?= tropism\ncheck:\n\t-@tropism analyze .\n";
        let (_, commands) = scan(
            &Utf8PathBuf::from("Makefile"),
            text,
            FileRole::Commands,
            &patterns,
        );
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].line, 3);
        assert_eq!(command_words(&commands[0].text), vec!["tropism"]);
    }
}
