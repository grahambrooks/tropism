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
use tropism_core::report::{CheckStatus, Finding, Report, Severity};

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
