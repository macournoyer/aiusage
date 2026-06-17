use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, BorderType, Borders, Gauge, Paragraph},
    Frame,
};

use crate::app::App;
use crate::fetch::{ClaudeUsage, CodexUsage};

fn pct_color(pct: u8) -> Color {
    if pct >= 80 {
        Color::Red
    } else if pct >= 50 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn draw_gauge(f: &mut Frame, area: Rect, label: &str, pct: u8, reset: &str) {
    let color = pct_color(pct);

    // name | bar (fixed 24 cols) | pct | reset
    let reset_text = if reset.is_empty() {
        String::new()
    } else {
        format!("resets {reset}")
    };
    let pct_text = format!("{pct:>3}%");

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(10),  // row label
            Constraint::Length(24),  // bar
            Constraint::Length(6),   // "  XX%"
            Constraint::Min(0),      // reset time
        ])
        .split(area);

    let gauge = Gauge::default()
        .block(Block::default())
        .gauge_style(Style::default().fg(color).bg(Color::DarkGray))
        .percent(pct as u16)
        .label("");

    f.render_widget(
        Paragraph::new(label).style(Style::default().fg(Color::Gray)),
        cols[0],
    );
    f.render_widget(gauge, cols[1]);
    f.render_widget(
        Paragraph::new(format!("  {pct_text}")).style(Style::default().fg(color)),
        cols[2],
    );
    f.render_widget(
        Paragraph::new(format!("  {reset_text}")).style(Style::default().fg(Color::DarkGray)),
        cols[3],
    );
}

fn claude_lines(usage: &ClaudeUsage) -> Vec<(&'static str, u8, String)> {
    let mut rows: Vec<(&str, u8, String)> = Vec::new();
    if let Some(w) = &usage.five_hour {
        rows.push(("5-hour   ", w.used_percent, w.reset_label.clone()));
    }
    if let Some(w) = &usage.seven_day {
        rows.push(("7-day    ", w.used_percent, w.reset_label.clone()));
    }
    if let Some(e) = &usage.extra {
        let label = format!("${:.0} / ${:.0} {}", e.used_credits, e.monthly_limit, e.currency);
        rows.push(("credits  ", e.used_percent, label));
    }
    rows
}

fn codex_lines(usage: &CodexUsage) -> Vec<(&'static str, u8, String)> {
    let mut rows: Vec<(&str, u8, String)> = Vec::new();
    if let Some(w) = &usage.five_hour {
        rows.push(("5-hour   ", w.used_percent, w.reset_label.clone()));
    }
    if let Some(w) = &usage.weekly {
        rows.push(("weekly   ", w.used_percent, w.reset_label.clone()));
    }
    rows
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();

    // Outer border
    let outer = Block::default()
        .title(Span::styled(
            " aiusage ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    // Determine row counts
    let claude_rows = app.data.claude.as_ref().map(|u| claude_lines(u).len()).unwrap_or(0);
    let codex_rows = app.data.codex.as_ref().map(|u| codex_lines(u).len()).unwrap_or(0);

    // Sections: padding + header + gauge rows + optional error; plus status bar
    let claude_height = if app.data.claude.is_some() || app.data.claude_error.is_some() {
        1 + 1 + claude_rows as u16 + if app.data.claude_error.is_some() { 1 } else { 0 }
    } else {
        0
    };
    let codex_height = if app.data.codex.is_some() || app.data.codex_error.is_some() {
        1 + 1 + codex_rows as u16 + if app.data.codex_error.is_some() { 1 } else { 0 }
    } else {
        0
    };
    let status_height = 1u16;
    let gap = 1u16;

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(claude_height + gap),
            Constraint::Length(codex_height + gap),
            Constraint::Min(0),
            Constraint::Length(status_height),
        ])
        .split(inner);

    // --- CLAUDE section ---
    draw_section(
        f,
        sections[0],
        "CLAUDE",
        app.data.claude.as_ref().map(|u| claude_lines(u)).unwrap_or_default(),
        &app.data.claude_error,
    );

    // --- CODEX section ---
    let codex_title = app
        .data
        .codex
        .as_ref()
        .filter(|u| !u.plan.is_empty() && u.plan != "unknown")
        .map(|u| format!("CODEX ({})", u.plan))
        .unwrap_or_else(|| "CODEX".to_string());
    draw_section(
        f,
        sections[1],
        &codex_title,
        app.data.codex.as_ref().map(|u| codex_lines(u)).unwrap_or_default(),
        &app.data.codex_error,
    );

    // --- Status bar ---
    let updated = app
        .last_updated
        .map(|t| t.format("%-I:%M %p").to_string())
        .unwrap_or_else(|| "—".to_string());
    let secs = app.secs_until_refresh();
    let status_text = if app.fetching {
        format!(" fetching…  ·  [r] refresh  [q] quit")
    } else {
        format!(" updated {updated}  ·  refreshes in {secs}s  ·  [r] refresh  [q] quit")
    };
    let status = Paragraph::new(status_text)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(status, sections[3]);
}

fn draw_section(
    f: &mut Frame,
    area: Rect,
    title: &str,
    rows: Vec<(&'static str, u8, String)>,
    error: &Option<String>,
) {
    if area.height == 0 {
        return;
    }

    let mut constraints = vec![Constraint::Length(1)]; // header
    for _ in &rows {
        constraints.push(Constraint::Length(1));
    }
    if error.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(0)); // padding

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    // Header
    let header = Paragraph::new(title).style(
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(header, chunks[0]);

    // Gauge rows
    for (i, (label, pct, reset)) in rows.iter().enumerate() {
        draw_gauge(f, chunks[i + 1], label, *pct, reset);
    }

    // Error
    if let Some(err) = error {
        let idx = rows.len() + 1;
        if idx < chunks.len() {
            let msg = Paragraph::new(format!("  error: {err}"))
                .style(Style::default().fg(Color::Red));
            f.render_widget(msg, chunks[idx]);
        }
    }
}
