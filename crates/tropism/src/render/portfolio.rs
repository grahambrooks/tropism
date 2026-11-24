//! `tropism portfolio` for a person reading a terminal.
//!
//! The JSON contract is the product — other tools read it — so this is a summary
//! of it, ordered by what someone acts on: libraries waiting on a release, users
//! behind, duplicate copies, and then the full link list.

use std::fmt::Write as _;

use anstyle::{AnsiColor, Style};
use tropism_core::report::Severity;
use tropism_portfolio::model::{
    Edge, LibraryStatus, LinkKind, PortfolioReport, ReleaseStatus, Via,
};

struct Palette {
    bold: Style,
    dim: Style,
    warn: Style,
    error: Style,
}

impl Palette {
    fn new(colour: bool) -> Self {
        if !colour {
            return Self {
                bold: Style::new(),
                dim: Style::new(),
                warn: Style::new(),
                error: Style::new(),
            };
        }
        Self {
            bold: Style::new().bold(),
            dim: Style::new().dimmed(),
            warn: Style::new()
                .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Yellow)))
                .bold(),
            error: Style::new()
                .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Red)))
                .bold(),
        }
    }
}

pub fn render(report: &PortfolioReport) -> String {
    render_with(report, true)
}

fn pin_label(edge: &Edge) -> String {
    let current = edge
        .resolved
        .iter()
        .find_map(|r| r.reference.as_ref().map(ToString::to_string))
        .or_else(|| edge.pinned.as_ref().map(ToString::to_string))
        .unwrap_or_else(|| "unpinned".into());
    match (edge.behind_by, &edge.newest) {
        (Some(0), _) => format!("{current} (current)"),
        (Some(n), Some(newest)) => format!("{current} → {newest}, {n} behind"),
        _ => current,
    }
}

pub fn render_with(report: &PortfolioReport, colour: bool) -> String {
    let Palette {
        bold,
        dim,
        warn,
        error,
    } = Palette::new(colour);
    let mut out = String::new();
    let own = report.repos.iter().filter(|r| r.own).count();
    let _ = writeln!(
        out,
        "{bold}{} repositories{bold:#} ({own} owned by {}), {} libraries, {} links  {dim}tags: {}{dim:#}",
        report.repos.len(),
        report.owner,
        report.libraries.len(),
        report.edges.len(),
        match report.tags_from {
            tropism_portfolio::model::TagSource::Local => "local refs",
            tropism_portfolio::model::TagSource::Remote => "origin (--fetch)",
        }
    );

    let _ = writeln!(out, "\n{bold}Libraries{bold:#}");
    for library in &report.libraries {
        let tag = library.newest_tag.as_deref().unwrap_or("no tag");
        let release = match library.release.status {
            ReleaseStatus::Current => format!("{dim}released{dim:#}"),
            ReleaseStatus::Pending => format!(
                "{warn}release pending{warn:#}: {} commit(s) on {}",
                library.release.commits.len(),
                library
                    .release
                    .branch
                    .as_deref()
                    .unwrap_or("default branch")
            ),
            ReleaseStatus::NoTag => format!("{dim}never tagged{dim:#}"),
            ReleaseStatus::NotComparable => format!(
                "{dim}not comparable: {}{dim:#}",
                library.release.reason.as_deref().unwrap_or_default()
            ),
            ReleaseStatus::Unavailable => format!("{warn}not checked out{warn:#}"),
        };
        let status = if library.status == LibraryStatus::Unavailable {
            " (unavailable)"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "  {bold}{}{bold:#}{status}  {tag}  {release}  {dim}used by {}{dim:#}",
            library.repo,
            library.users.join(", ")
        );
    }

    if !report.findings.is_empty() {
        let _ = writeln!(out, "\n{bold}Findings{bold:#}");
        for finding in &report.findings {
            let (style, label) = match finding.severity {
                Severity::Error => (error, "error"),
                Severity::Warning => (warn, "warning"),
                Severity::Info => (dim, "info"),
            };
            let rule = finding
                .rule
                .as_deref()
                .map(|r| format!(" {dim}[{r}]{dim:#}"))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "  {style}{label}{style:#}[{}]: {}{rule}",
                finding.check, finding.message
            );
            if let Some(evidence) = finding.evidence.first() {
                let line = evidence.line.map(|l| format!(":{l}")).unwrap_or_default();
                let _ = writeln!(out, "    {dim}--> {}{line}{dim:#}", evidence.file);
            }
        }
    }

    let _ = writeln!(out, "\n{bold}Links{bold:#}");
    let mut users: Vec<&str> = report.edges.iter().map(|e| e.from.as_str()).collect();
    users.dedup();
    for user in users {
        let _ = writeln!(out, "  {bold}{user}{bold:#}");
        for edge in report.edges.iter().filter(|e| e.from == user) {
            let via = match edge.via {
                Via::Direct => String::new(),
                Via::Transitive if edge.through.is_empty() => " transitive".into(),
                Via::Transitive => format!(" via {}", edge.through.join(", ")),
                Via::Indirect => " indirect".into(),
            };
            let kind = match edge.kind {
                LinkKind::Binary => format!("{dim}binary, medium{dim:#}"),
                kind => format!("{dim}{}{dim:#}", kind.as_str()),
            };
            let _ = writeln!(
                out,
                "    → {} {}  {}{via}  {kind}",
                edge.to,
                if edge.package == edge.to {
                    String::new()
                } else {
                    format!("({})", edge.package)
                },
                pin_label(edge)
            );
        }
    }

    let unavailable: Vec<&str> = report
        .libraries
        .iter()
        .filter(|l| l.status == LibraryStatus::Unavailable)
        .map(|l| l.remote.as_str())
        .collect();
    if !unavailable.is_empty() {
        let _ = writeln!(
            out,
            "\n{warn}Not checked out{warn:#} — linked to, but unknown beyond that: {}",
            unavailable.join(", ")
        );
    }
    let excluded: Vec<String> = report
        .exclusions
        .iter()
        .map(|e| format!("{} ({}, {} file(s))", e.pattern, e.source, e.matched))
        .collect();
    if !excluded.is_empty() {
        let _ = writeln!(out, "\n{dim}Excluded: {}{dim:#}", excluded.join("; "));
    }
    if !report.notes.is_empty() {
        let _ = writeln!(
            out,
            "\n{dim}{} note(s) in the JSON report (--format json){dim:#}",
            report.notes.len()
        );
    }
    out
}
