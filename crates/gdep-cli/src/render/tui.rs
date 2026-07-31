//! Interactive report browser.
//!
//! Opt-in via `--format tui`. It is deliberately *not* the default: the alternate
//! screen cannot be piped, redirected, or read by CI, and `--format auto` has to
//! keep working in all three. See `design/05-interfaces.md`.
//!
//! The state machine ([`App`]) is separated from drawing ([`draw`]) so navigation is
//! unit-testable and the layout is snapshot-testable against `TestBackend`, without
//! a terminal in either case.

use std::io::IsTerminal;

use gdep_core::report::{
    CheckId, CheckStatus, Confidence, Finding, ProjectReport, Report, Severity,
};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};

/// One navigable line in the left pane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Row<'a> {
    Project(&'a ProjectReport),
    Finding(&'a ProjectReport, &'a Finding),
    /// A check that did not run. Given equal billing with findings on purpose: the
    /// whole point of `CheckStatus` is that a short finding list is not good news
    /// until you know what was actually checked.
    Unavailable(&'a ProjectReport, CheckId, &'a str),
}

pub struct App<'a> {
    report: &'a Report,
    rows: Vec<Row<'a>>,
    state: ListState,
    should_quit: bool,
}

impl<'a> App<'a> {
    pub fn new(report: &'a Report) -> Self {
        let mut rows = Vec::new();
        for project in &report.projects {
            rows.push(Row::Project(project));
            for finding in &project.findings {
                rows.push(Row::Finding(project, finding));
            }
            for (check, status) in &project.checks {
                if let CheckStatus::Unavailable { reason } = status {
                    rows.push(Row::Unavailable(project, *check, reason.as_str()));
                }
            }
        }

        let mut state = ListState::default();
        if !rows.is_empty() {
            state.select(Some(0));
        }

        Self {
            report,
            rows,
            state,
            should_quit: false,
        }
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn selected(&self) -> Option<Row<'a>> {
        self.state
            .selected()
            .and_then(|index| self.rows.get(index).copied())
    }

    /// Accessors the drawing code does not need, but the navigation tests do.
    #[cfg(test)]
    pub fn selected_index(&self) -> Option<usize> {
        self.state.selected()
    }

    #[cfg(test)]
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Navigation saturates rather than wrapping: in a long report, wrapping from
    /// the last finding back to the first reads as a bug.
    pub fn select_next(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let next = match self.state.selected() {
            Some(current) => (current + 1).min(self.rows.len() - 1),
            None => 0,
        };
        self.state.select(Some(next));
    }

    pub fn select_previous(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let previous = self
            .state
            .selected()
            .map_or(0, |current| current.saturating_sub(1));
        self.state.select(Some(previous));
    }

    pub fn select_first(&mut self) {
        if !self.rows.is_empty() {
            self.state.select(Some(0));
        }
    }

    pub fn select_last(&mut self) {
        if !self.rows.is_empty() {
            self.state.select(Some(self.rows.len() - 1));
        }
    }

    pub fn on_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c')) {
            self.should_quit = true;
            return;
        }
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_previous(),
            KeyCode::Home | KeyCode::Char('g') => self.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.select_last(),
            _ => {}
        }
    }
}

/// Runs the browser. Blocks until the user quits.
pub fn run(report: &Report) -> anyhow::Result<()> {
    if !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "--format tui needs a terminal; use --format text or --format json when redirecting"
        );
    }

    ratatui::run(|terminal| -> anyhow::Result<()> {
        let mut app = App::new(report);
        loop {
            terminal.draw(|frame| draw(frame, &mut app))?;
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                app.on_key(key.code, key.modifiers);
            }
            if app.should_quit() {
                return Ok(());
            }
        }
    })
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_header(frame, chunks[0], app.report);
    draw_body(frame, chunks[1], app);
    draw_footer(frame, chunks[2]);
}

fn draw_header(frame: &mut Frame, area: Rect, report: &Report) {
    let findings = report.findings().count();
    let unavailable = report.unavailable().count();
    let header = Line::from(vec![
        Span::styled(
            " gdep ",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(Color::Blue),
        ),
        Span::raw(format!(" {} ", report.scan_root)),
        Span::styled(
            format!("· {findings} finding(s) "),
            Style::default().fg(if findings == 0 {
                Color::Green
            } else {
                Color::Yellow
            }),
        ),
        Span::styled(
            format!("· {unavailable} check(s) did not run"),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(header), area);
}

fn draw_body(frame: &mut Frame, area: Rect, app: &mut App) {
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| ListItem::new(row_line(*row)))
        .collect();
    let list = List::new(items)
        .block(Block::bordered().title(" report "))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, panes[0], &mut app.state);

    let detail = app.selected().map_or_else(
        || vec![Line::from("no projects found")],
        |row| detail_lines(row),
    );
    frame.render_widget(
        Paragraph::new(detail)
            .block(Block::bordered().title(" detail "))
            .wrap(Wrap { trim: false }),
        panes[1],
    );
}

fn draw_footer(frame: &mut Frame, area: Rect) {
    let keys = Line::from(Span::styled(
        " ↑/↓ or j/k move · g/G first/last · q quit ",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(Paragraph::new(keys), area);
}

fn row_line(row: Row<'_>) -> Line<'_> {
    match row {
        Row::Project(project) => Line::from(vec![
            Span::styled(
                project.project.root.as_str().to_owned(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({})", project.project.language),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Row::Finding(_, finding) => Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{:<9}", finding.severity.as_str()),
                Style::default().fg(severity_color(finding.severity)),
            ),
            Span::raw(finding.message.clone()),
        ]),
        Row::Unavailable(_, check, _) => Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{:<9}", "skipped"),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(check.to_string(), Style::default().fg(Color::DarkGray)),
        ]),
    }
}

fn detail_lines(row: Row<'_>) -> Vec<Line<'_>> {
    match row {
        Row::Project(project) => {
            let mut lines = vec![
                Line::from(Span::styled(
                    project.project.root.as_str().to_owned(),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(format!("language: {}", project.project.language)),
                Line::from(format!(
                    "lockfile: {}",
                    project
                        .project
                        .lockfile
                        .as_ref()
                        .map_or("none", |p| p.as_str())
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "checks",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
            ];
            for (check, status) in &project.checks {
                let (label, color) = match status {
                    CheckStatus::Ran { finding_count: 0 } => ("ok".to_owned(), Color::Green),
                    CheckStatus::Ran { finding_count } => {
                        (format!("{finding_count} found"), Color::Yellow)
                    }
                    CheckStatus::Unavailable { .. } => ("unavailable".to_owned(), Color::DarkGray),
                    CheckStatus::Failed { .. } => ("failed".to_owned(), Color::Red),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {label:<12}"), Style::default().fg(color)),
                    Span::raw(check.to_string()),
                ]));
            }
            lines
        }
        Row::Finding(_, finding) => {
            let mut lines = vec![
                Line::from(Span::styled(
                    finding.message.clone(),
                    Style::default()
                        .fg(severity_color(finding.severity))
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    finding.id.clone(),
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(format!("check:      {}", finding.check)),
                Line::from(format!("severity:   {}", finding.severity.as_str())),
                Line::from(vec![
                    Span::raw("confidence: "),
                    Span::styled(
                        finding.confidence.as_str().to_owned(),
                        Style::default().fg(confidence_color(finding.confidence)),
                    ),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "evidence",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
            ];
            for evidence in &finding.evidence {
                let location = match evidence.line {
                    Some(line) => format!("  {}:{}", evidence.file, line),
                    None => format!("  {}", evidence.file),
                };
                lines.push(Line::from(Span::styled(
                    location,
                    Style::default().fg(Color::Cyan),
                )));
                lines.push(Line::from(format!("    {}", evidence.note)));
            }
            lines
        }
        Row::Unavailable(_, check, reason) => vec![
            Line::from(Span::styled(
                format!("{check} did not run"),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(reason.to_owned()),
            Line::from(""),
            Line::from(Span::styled(
                "This is not a clean result — the check produced no answer at all.",
                Style::default().fg(Color::DarkGray),
            )),
        ],
    }
}

fn severity_color(severity: Severity) -> Color {
    match severity {
        Severity::Error => Color::Red,
        Severity::Warning => Color::Yellow,
        Severity::Info => Color::Blue,
    }
}

fn confidence_color(confidence: Confidence) -> Color {
    match confidence {
        Confidence::High => Color::Green,
        Confidence::Medium => Color::Yellow,
        Confidence::Low => Color::DarkGray,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::render::testdata::sample_report;

    #[test]
    fn rows_cover_projects_findings_and_unavailable_checks() {
        let report = sample_report();
        let app = App::new(&report);

        assert_eq!(
            app.row_count(),
            3,
            "one project, one finding, one unavailable check"
        );
        assert!(matches!(app.rows[0], Row::Project(_)));
        assert!(matches!(app.rows[1], Row::Finding(..)));
        assert!(matches!(app.rows[2], Row::Unavailable(..)));
    }

    #[test]
    fn navigation_saturates_at_both_ends() {
        let report = sample_report();
        let mut app = App::new(&report);
        assert_eq!(app.selected_index(), Some(0));

        app.select_previous();
        assert_eq!(app.selected_index(), Some(0), "must not wrap to the end");

        for _ in 0..10 {
            app.select_next();
        }
        assert_eq!(
            app.selected_index(),
            Some(app.row_count() - 1),
            "must not wrap to the start"
        );
    }

    #[test]
    fn jump_keys_reach_first_and_last() {
        let report = sample_report();
        let mut app = App::new(&report);

        app.on_key(KeyCode::Char('G'), KeyModifiers::NONE);
        assert_eq!(app.selected_index(), Some(app.row_count() - 1));

        app.on_key(KeyCode::Char('g'), KeyModifiers::NONE);
        assert_eq!(app.selected_index(), Some(0));
    }

    #[test]
    fn q_esc_and_ctrl_c_all_quit() {
        let report = sample_report();

        for (code, modifiers) in [
            (KeyCode::Char('q'), KeyModifiers::NONE),
            (KeyCode::Esc, KeyModifiers::NONE),
            (KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            let mut app = App::new(&report);
            app.on_key(code, modifiers);
            assert!(app.should_quit(), "{code:?} with {modifiers:?} should quit");
        }
    }

    #[test]
    fn an_empty_report_navigates_without_panicking() {
        let report = Report::new(".");
        let mut app = App::new(&report);

        assert_eq!(app.row_count(), 0);
        assert_eq!(app.selected_index(), None);
        app.select_next();
        app.select_previous();
        app.select_last();
        assert_eq!(app.selected(), None);
    }

    fn render_to_string(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        format!("{}", terminal.backend())
    }

    #[test]
    fn renders_a_finding_detail() {
        let report = sample_report();
        let mut app = App::new(&report);
        app.select_next(); // the finding
        insta::assert_snapshot!(render_to_string(&mut app, 100, 24));
    }

    /// An unavailable check must read as "no answer", never as "nothing wrong".
    #[test]
    fn renders_an_unavailable_check_as_an_absent_answer() {
        let report = sample_report();
        let mut app = App::new(&report);
        app.select_last();
        let rendered = render_to_string(&mut app, 100, 24);
        assert!(rendered.contains("did not run"), "{rendered}");
        assert!(rendered.contains("not a clean result"), "{rendered}");
    }

    #[test]
    fn renders_an_empty_report_without_panicking() {
        let report = Report::new(".");
        let mut app = App::new(&report);
        let rendered = render_to_string(&mut app, 80, 12);
        assert!(rendered.contains("no projects found"), "{rendered}");
    }
}
