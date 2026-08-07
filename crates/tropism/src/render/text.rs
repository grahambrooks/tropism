//! Human-readable rendering, in the shape rustc and clippy use.
//!
//! `annotate-snippets` is the rust-lang crate behind that format, so findings get
//! the source-snippet treatment developers already read fluently. Colour goes
//! through `anstream`, which strips ANSI when the stream is not a terminal, so
//! piped output stays clean without a separate code path.

use std::fmt::Write as _;
use std::ops::Range;

use annotate_snippets::{AnnotationKind, Group, Level, Renderer, Snippet};
use anstyle::{AnsiColor, Style};
use camino::Utf8Path;
use tropism_core::pipeline::{CheckOutcome, CheckScope, ExplainReport, WorkspaceReport};
use tropism_core::report::{CheckStatus, Finding, Report, Severity};
use tropism_core::workspace::WorkspaceOrigin;

/// Styles for the report chrome. Held in a struct rather than as constants so tests
/// can snapshot plain output; in production `anstream` also strips ANSI when the
/// stream is not a terminal.
struct Palette {
    bold: Style,
    dim: Style,
    good: Style,
    warn: Style,
}

impl Palette {
    const fn styled() -> Self {
        Self {
            bold: Style::new().bold(),
            dim: Style::new().dimmed(),
            good: Style::new()
                .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Green)))
                .bold(),
            warn: Style::new()
                .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Yellow)))
                .bold(),
        }
    }

    const fn plain() -> Self {
        Self {
            bold: Style::new(),
            dim: Style::new(),
            good: Style::new(),
            warn: Style::new(),
        }
    }
}

/// Renders a whole report with colour.
pub fn render(report: &Report) -> String {
    render_with(report, true)
}

/// Renders the workspace boundaries and what crosses them.
pub fn render_workspaces(report: &WorkspaceReport) -> String {
    render_workspaces_with(report, true)
}

fn render_workspaces_with(report: &WorkspaceReport, colour: bool) -> String {
    let palette = if colour {
        Palette::styled()
    } else {
        Palette::plain()
    };
    let (bold, dim, warn) = (palette.bold, palette.dim, palette.warn);
    let mut out = String::new();

    if report.workspaces.is_empty() {
        return "no projects found\n".to_owned();
    }

    let _ = writeln!(
        out,
        "{bold}{} workspace(s){bold:#}\n",
        report.workspaces.len()
    );

    for workspace in &report.workspaces {
        let languages = workspace
            .languages
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "{bold}{}{bold:#}  {dim}({languages}){dim:#}",
            workspace.id
        );

        // How the boundary was established is the part a reader has to judge. A
        // `language` grouping is an inference tropism made because the ecosystem
        // stated nothing, and it is the one most likely to be wrong.
        let origin = match workspace.declared_by.as_ref() {
            Some(file) => format!("{} — declared by {file}", workspace.origin.as_str()),
            None => format!(
                "{} — inferred, because this ecosystem declares no workspace",
                workspace.origin.as_str()
            ),
        };
        let style = if workspace.origin == WorkspaceOrigin::Language {
            warn
        } else {
            dim
        };
        let _ = writeln!(out, "  {style}{origin}{style:#}");

        for member in &workspace.members {
            let member = if member.as_str().is_empty() {
                "."
            } else {
                member.as_str()
            };
            let _ = writeln!(out, "  {dim}• {member}{dim:#}");
        }
        out.push('\n');
    }

    if report.crossings.is_empty() {
        let _ = writeln!(
            out,
            "{dim}no dependency crosses a workspace boundary{dim:#}"
        );
        return out;
    }

    let _ = writeln!(
        out,
        "{warn}{} dependenc(ies) cross a workspace boundary{warn:#}",
        report.crossings.len()
    );
    let _ = writeln!(
        out,
        "{dim}each resolves today through hoisting and breaks when the package is built \
         alone; enforce with a `crosses_workspace` rule{dim:#}"
    );
    for crossing in &report.crossings {
        let line = crossing
            .line
            .map_or_else(String::new, |line| format!(":{line}"));
        let _ = writeln!(
            out,
            "  {dim}{}{line} — {} `{}` → workspace `{}`{dim:#}",
            crossing.from, crossing.level, crossing.label, crossing.to_workspace
        );
    }
    out
}

/// Renders one file's import classifications.
pub fn render_explain(report: &ExplainReport) -> String {
    render_explain_with(report, true)
}

fn render_explain_with(report: &ExplainReport, colour: bool) -> String {
    let palette = if colour {
        Palette::styled()
    } else {
        Palette::plain()
    };
    let (bold, dim, good, warn) = (palette.bold, palette.dim, palette.good, palette.warn);
    let mut out = String::new();

    let project = if report.project.as_str().is_empty() {
        "."
    } else {
        report.project.as_str()
    };
    let _ = writeln!(out, "{bold}{}{bold:#}", report.file);
    let _ = writeln!(
        out,
        "  {dim}project {project} ({}), module `{}`{dim:#}",
        report.language, report.module
    );
    if let Some(workspace) = &report.workspace {
        let _ = writeln!(
            out,
            "  {dim}workspace `{}` ({}){dim:#}",
            workspace.id,
            workspace.origin.as_str()
        );
    }
    out.push('\n');

    if report.imports.is_empty() {
        let _ = writeln!(out, "{dim}no imports{dim:#}");
        return out;
    }

    for import in &report.imports {
        // `unresolved` caps the confidence of every hygiene finding in the project,
        // so it is the outcome worth making visible rather than the clean ones.
        let style = match import.target.as_str() {
            "unresolved" => warn,
            "internal" | "stdlib" => good,
            _ => bold,
        };
        let _ = writeln!(
            out,
            "{style}{}:{}{style:#}  {} {dim}({}){dim:#}",
            import.line, import.raw, import.target, import.form
        );
        let _ = writeln!(out, "  {dim}{}{dim:#}", import.reason);
    }
    out
}

/// Renders a `check` run.
///
/// Deliberately terser than [`render`]: the reader is being interrupted mid-commit,
/// so the diagnostics come first and everything else is one line. The check-status
/// block is omitted because a rules-only run has nothing interesting to say there —
/// what it must not omit is the *scope*, since a run that examined six files must
/// never read like one that examined the repository.
pub fn render_check(outcome: &CheckOutcome) -> String {
    render_check_with(outcome, true)
}

pub fn render_check_with(outcome: &CheckOutcome, styled: bool) -> String {
    let renderer = if styled {
        Renderer::styled()
    } else {
        Renderer::plain()
    };
    let palette = if styled {
        Palette::styled()
    } else {
        Palette::plain()
    };
    let mut out = String::new();

    for finding in outcome.report.findings() {
        out.push_str(&render_finding(
            finding,
            &outcome.report.scan_root,
            &renderer,
        ));
        out.push('\n');
    }

    let violations = outcome.report.findings().count();
    let (bold, dim, good, warn) = (palette.bold, palette.dim, palette.good, palette.warn);
    let style = if violations == 0 { good } else { warn };

    // A widened run examined everything, whatever the scope asked for, and saying
    // "changed" there would misreport what was looked at.
    let scope = match &outcome.scope {
        CheckScope::Files(_) if !outcome.widened_by_ruleset_change => {
            format!("{} changed file(s)", outcome.checked_files)
        }
        _ => format!("{} file(s)", outcome.checked_files),
    };
    let _ = writeln!(
        out,
        "{bold}checked{bold:#} {scope} against {} rule(s) — {style}{violations} violation(s){style:#}",
        outcome.rules_evaluated
    );

    // A ruleset with no rules blocks nothing, and a hook that blocks nothing is
    // worse than no hook: it is a hook everyone believes is working.
    if outcome.rules_evaluated == 0 {
        let _ = writeln!(
            out,
            "  {warn}no rules were evaluated — nothing here can fail{warn:#}"
        );
    }

    if outcome.widened_by_ruleset_change {
        let _ = writeln!(
            out,
            "  {dim}the ruleset itself changed, so the whole repository was checked{dim:#}"
        );
    }

    // Counted, never hidden. A ratchet that conceals the backlog is how a codebase
    // ends up with two hundred violations nobody remembers agreeing to.
    if outcome.suppressed > 0 {
        let _ = writeln!(
            out,
            "  {dim}{} pre-existing violation(s) elsewhere are not shown; \
             run `tropism check` for the whole repository{dim:#}",
            outcome.suppressed
        );
    }

    out
}

/// Renders a whole report, optionally without any ANSI styling.
///
/// Check status comes *before* findings, and unavailable checks are always shown:
/// an empty finding list means nothing until you know which checks actually ran.
pub fn render_with(report: &Report, styled: bool) -> String {
    let renderer = if styled {
        Renderer::styled()
    } else {
        Renderer::plain()
    };
    let palette = if styled {
        Palette::styled()
    } else {
        Palette::plain()
    };
    let mut out = String::new();

    for project in &report.projects {
        let root = project.project.root.as_str();
        let (bold, dim) = (palette.bold, palette.dim);
        let _ = writeln!(
            out,
            "{bold}{}{bold:#} {dim}({}){dim:#}",
            if root.is_empty() { "." } else { root },
            project.project.language
        );

        for (check, status) in &project.checks {
            let _ = writeln!(out, "{}", status_line(*check, status, &palette));
        }
        out.push('\n');
    }

    for finding in report.findings() {
        out.push_str(&render_finding(finding, &report.scan_root, &renderer));
        out.push('\n');
    }

    if !report.excluded.is_empty() {
        let (dim, warn) = (palette.dim, palette.warn);
        let total: usize = report.excluded.iter().map(|e| e.matched).sum();
        let _ = writeln!(out, "{dim}{total} path(s) excluded by tropism.toml{dim:#}");
        for exclusion in &report.excluded {
            // A pattern matching nothing has stopped protecting anything.
            let style = if exclusion.matched == 0 { warn } else { dim };
            let note = if exclusion.matched == 0 {
                " (matches nothing)"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "  {style}{} — {} path(s){note}{style:#}",
                exclusion.pattern, exclusion.matched
            );
        }
        out.push('\n');
    }

    // Disclosed for the same reason exclusions are: an exemption is a deliberate
    // blind spot, and a silent one reads exactly like a clean project.
    let exemptions: Vec<(
        &camino::Utf8PathBuf,
        &tropism_core::report::SiblingExemption,
    )> = report
        .projects
        .iter()
        .flat_map(|project| {
            project
                .sibling_exemptions
                .iter()
                .map(move |exemption| (&project.project.root, exemption))
        })
        .collect();
    if !exemptions.is_empty() {
        let dim = palette.dim;
        let total: usize = exemptions.iter().map(|(_, e)| e.imports).sum();
        let _ = writeln!(
            out,
            "{dim}{total} import(s) needed no declaration ({} package(s) supplied by the \
             workspace){dim:#}",
            exemptions.len()
        );
        for (root, exemption) in &exemptions {
            let root = if root.as_str().is_empty() {
                "."
            } else {
                root.as_str()
            };
            let from = exemption
                .provided_by
                .as_ref()
                .map_or_else(String::new, |path| {
                    format!(
                        " from {}",
                        if path.as_str().is_empty() {
                            "."
                        } else {
                            path.as_str()
                        }
                    )
                });
            let _ = writeln!(
                out,
                "  {dim}{root}: {} — {}{from}, {} import(s){dim:#}",
                exemption.package,
                exemption.via.as_str(),
                exemption.imports
            );
        }
        out.push('\n');
    }

    if !report.skipped.is_empty() {
        let (warn, dim) = (palette.warn, palette.dim);
        let _ = writeln!(
            out,
            "{warn}{} file(s) skipped{warn:#}",
            report.skipped.len()
        );
        for skipped in &report.skipped {
            let _ = writeln!(out, "  {dim}{} — {}{dim:#}", skipped.file, skipped.reason);
        }
        out.push('\n');
    }

    out.push_str(&summary(report, &palette));
    out
}

fn status_line(
    check: tropism_core::report::CheckId,
    status: &CheckStatus,
    palette: &Palette,
) -> String {
    let (dim, good, warn) = (palette.dim, palette.good, palette.warn);
    match status {
        CheckStatus::Ran { finding_count: 0 } => format!("  {good}ok{good:#}          {check}"),
        CheckStatus::Ran { finding_count } => {
            format!("  {warn}{finding_count} found{warn:#}     {check}")
        }
        CheckStatus::Unavailable { reason } => {
            format!("  {dim}unavailable{dim:#} {check} {dim}— {reason}{dim:#}")
        }
        CheckStatus::Failed { error } => {
            format!("  {warn}failed{warn:#}      {check} {dim}— {error}{dim:#}")
        }
    }
}

fn summary(report: &Report, palette: &Palette) -> String {
    let findings = report.findings().count();
    let unavailable = report.unavailable().count();
    let projects = report.projects.len();
    let (dim, good, warn) = (palette.dim, palette.good, palette.warn);

    if projects == 0 {
        return format!("{dim}no projects found{dim:#}\n");
    }

    let mut line = if findings == 0 {
        format!("{good}no findings{good:#} across {projects} project(s)")
    } else {
        format!("{warn}{findings} finding(s){warn:#} across {projects} project(s)")
    };
    if unavailable > 0 {
        let _ = write!(line, " {dim}({unavailable} check(s) did not run){dim:#}");
    }
    line.push('\n');
    line
}

/// One finding, with source snippets where the evidence points at readable files.
fn render_finding(finding: &Finding, scan_root: &Utf8Path, renderer: &Renderer) -> String {
    let level = match finding.severity {
        Severity::Error => Level::ERROR,
        Severity::Warning => Level::WARNING,
        Severity::Info => Level::NOTE,
    };

    // Read each distinct evidence file once; snippets borrow from these.
    let mut sources: Vec<(String, String)> = Vec::new();
    for evidence in &finding.evidence {
        let key = evidence.file.as_str().to_owned();
        if sources.iter().any(|(path, _)| path == &key) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(scan_root.join(&evidence.file)) {
            sources.push((key, text));
        }
    }

    let mut group = Group::with_title(
        level
            .primary_title(finding.message.as_str())
            .id(finding.id.as_str()),
    );
    let mut is_first_annotation = true;

    for (path, text) in &sources {
        let mut snippet = Snippet::source(text.as_str()).path(path.as_str());
        let mut annotated = false;

        for evidence in finding.evidence.iter().filter(|e| e.file.as_str() == path) {
            let Some(span) = line_span(text, evidence.line) else {
                continue;
            };
            let kind = if is_first_annotation {
                AnnotationKind::Primary
            } else {
                AnnotationKind::Context
            };
            snippet = snippet.annotation(kind.span(span).label(evidence.note.as_str()));
            is_first_annotation = false;
            annotated = true;
        }

        if annotated {
            group = group.element(snippet);
        }
    }

    let confidence = format!("confidence: {}", finding.confidence.as_str());
    let mut groups = vec![
        group,
        Group::with_title(Level::NOTE.secondary_title(&confidence)),
    ];

    // Evidence pointing at unreadable or line-less locations still has to be shown,
    // or the finding loses the thing that makes it checkable.
    let orphans: Vec<String> = finding
        .evidence
        .iter()
        .filter(|e| {
            !sources
                .iter()
                .any(|(path, text)| path == e.file.as_str() && line_span(text, e.line).is_some())
        })
        .map(|e| match e.line {
            Some(line) => format!("{}:{}: {}", e.file, line, e.note),
            None => format!("{}: {}", e.file, e.note),
        })
        .collect();
    let orphan_note = orphans.join("\n");
    if !orphan_note.is_empty() {
        groups.push(Group::with_title(Level::NOTE.secondary_title(&orphan_note)));
    }

    format!("{}\n", renderer.render(&groups))
}

/// Byte range of the trimmed content of a 1-based line.
fn line_span(text: &str, line: Option<u32>) -> Option<Range<usize>> {
    let target = line? as usize;
    if target == 0 {
        return None;
    }

    let mut offset = 0usize;
    for (index, raw) in text.split_inclusive('\n').enumerate() {
        if index + 1 == target {
            let leading = raw.len() - raw.trim_start().len();
            let trimmed = raw.trim();
            return Some(offset + leading..offset + leading + trimmed.len());
        }
        offset += raw.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::testdata::sample_report;

    #[test]
    fn renders_findings_in_the_rustc_diagnostic_style() {
        insta::assert_snapshot!(render_with(&sample_report(), false));
    }

    fn outcome(scope: CheckScope, suppressed: usize, rules: usize) -> CheckOutcome {
        let mut report = sample_report();
        // The shared fixture carries a cycle, which a rules-only run never
        // produces. Emptied so these assertions are about the summary line.
        for project in &mut report.projects {
            project.findings.clear();
        }
        CheckOutcome {
            report,
            scope,
            checked_files: 6,
            rules_evaluated: rules,
            suppressed,
            widened_by_ruleset_change: false,
        }
    }

    /// A run that examined six files must never read like one that examined the
    /// repository — the distinction is the whole basis of the ratchet.
    #[test]
    fn the_summary_states_the_scope_that_was_checked() {
        let scoped = render_check_with(&outcome(CheckScope::Files(vec![]), 0, 4), false);
        assert!(
            scoped.contains("checked 6 changed file(s) against 4 rule(s)"),
            "{scoped}"
        );

        let whole = render_check_with(&outcome(CheckScope::Repository, 0, 4), false);
        assert!(
            whole.contains("checked 6 file(s) against 4 rule(s)"),
            "{whole}"
        );
        assert!(!whole.contains("changed"), "{whole}");
    }

    /// Counted, never hidden. A ratchet that conceals the backlog is how a codebase
    /// ends up with violations nobody remembers agreeing to.
    #[test]
    fn pre_existing_violations_are_counted_in_the_summary() {
        let rendered = render_check_with(&outcome(CheckScope::Files(vec![]), 12, 4), false);
        assert!(
            rendered.contains("12 pre-existing violation(s)"),
            "{rendered}"
        );

        let clean = render_check_with(&outcome(CheckScope::Files(vec![]), 0, 4), false);
        assert!(!clean.contains("pre-existing"), "{clean}");
    }

    /// A hook that blocks nothing is worse than no hook: it is a hook everyone
    /// believes is working.
    #[test]
    fn a_ruleset_with_no_rules_says_so() {
        let rendered = render_check_with(&outcome(CheckScope::Repository, 0, 0), false);
        assert!(rendered.contains("no rules were evaluated"), "{rendered}");
    }

    /// A widened run examined everything, so calling those files "changed" would
    /// misreport what was looked at.
    #[test]
    fn a_widened_run_does_not_call_the_repository_a_change() {
        let mut widened = outcome(CheckScope::Files(vec![]), 0, 4);
        widened.widened_by_ruleset_change = true;
        let rendered = render_check_with(&widened, false);
        assert!(rendered.contains("checked 6 file(s)"), "{rendered}");
        assert!(
            rendered.contains("the ruleset itself changed"),
            "{rendered}"
        );
    }

    /// Principle 5: same input, same bytes out.
    #[test]
    fn rendering_is_deterministic() {
        let report = sample_report();
        assert_eq!(render_with(&report, false), render_with(&report, false));
    }

    /// A check that never ran must be visible, or an empty finding list reads as a
    /// clean bill of health.
    #[test]
    fn unavailable_checks_appear_in_output() {
        let rendered = render_with(&sample_report(), false);
        assert!(rendered.contains("unavailable"), "{rendered}");
        assert!(rendered.contains("no lockfile found"), "{rendered}");
        assert!(rendered.contains("1 check(s) did not run"), "{rendered}");
    }

    #[test]
    fn skipped_files_are_reported() {
        let rendered = render_with(&sample_report(), false);
        assert!(rendered.contains("1 file(s) skipped"), "{rendered}");
        assert!(rendered.contains("cycle/generated.go"), "{rendered}");
    }

    #[test]
    fn an_empty_report_says_so_rather_than_looking_clean() {
        let report = Report::new(".");
        assert!(render_with(&report, false).contains("no projects found"));
    }

    #[test]
    fn line_span_covers_trimmed_line_content() {
        let text = "first\n    second\nthird\n";
        let span = line_span(text, Some(2)).unwrap();
        assert_eq!(&text[span], "second");
    }

    #[test]
    fn line_span_rejects_out_of_range_and_missing_lines() {
        let text = "only\n";
        assert!(line_span(text, Some(9)).is_none());
        assert!(line_span(text, Some(0)).is_none());
        assert!(line_span(text, None).is_none());
    }

    #[test]
    fn line_span_handles_a_file_without_a_trailing_newline() {
        let text = "alpha\nbeta";
        let span = line_span(text, Some(2)).unwrap();
        assert_eq!(&text[span], "beta");
    }
}
