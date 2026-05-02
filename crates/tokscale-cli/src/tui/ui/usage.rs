use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::tui::app::App;

const BAR_WIDTH: usize = 20;

fn render_ascii_bar(remaining_percent: f64) -> String {
    let pct = remaining_percent.clamp(0.0, 100.0) / 100.0;
    let filled = (pct * BAR_WIDTH as f64).round() as usize;
    let empty = BAR_WIDTH - filled;
    format!("[{}{}]", "=".repeat(filled), "-".repeat(empty))
}

fn format_reset_time(resets_at: &str) -> String {
    use chrono::{DateTime, Duration, Utc};
    let dt = match DateTime::parse_from_rfc3339(resets_at) {
        Ok(d) => d.with_timezone(&Utc),
        Err(_) => return format!("resets {resets_at}"),
    };
    let diff = dt - Utc::now();
    if diff <= Duration::zero() {
        return "resets now".into();
    }
    let total_mins = diff.num_minutes();
    if total_mins < 60 {
        format!("resets in {total_mins}m")
    } else if total_mins < 24 * 60 {
        let hours = diff.num_hours();
        let mins = (diff - Duration::hours(hours)).num_minutes();
        if mins > 0 {
            format!("resets in {hours}h {mins}m")
        } else {
            format!("resets in {hours}h")
        }
    } else if diff.num_days() < 7 {
        format!("resets {} {}", dt.format("%a"), dt.format("%-I%P"))
    } else {
        format!("resets {}", dt.format("%b %-d"))
    }
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(" Subscription Usage ")
        .title_style(Style::default().fg(app.theme.foreground))
        .style(Style::default().bg(app.theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.subscription_usage.is_empty() {
        render_loading(frame, app, inner);
    } else if app.subscription_usage.iter().all(|o| o.metrics.is_empty()) {
        render_empty(frame, app, inner);
    } else {
        render_loaded(frame, app, inner, &app.subscription_usage);
    }
}

fn render_loading(frame: &mut Frame, app: &App, area: Rect) {
    let center = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Length(3),
            Constraint::Percentage(40),
        ])
        .split(area)[1];

    let msg = if app.data.loading {
        "Loading subscription data..."
    } else {
        "Press 'u' to fetch subscription usage"
    };
    let paragraph = Paragraph::new(msg)
        .style(Style::default().fg(app.theme.muted))
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, center);
}

fn render_empty(frame: &mut Frame, app: &App, area: Rect) {
    let center = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Length(3),
            Constraint::Percentage(40),
        ])
        .split(area)[1];

    let paragraph = Paragraph::new("No subscription data available")
        .style(Style::default().fg(app.theme.muted))
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, center);
}

fn render_loaded(frame: &mut Frame, app: &App, area: Rect, outputs: &[crate::commands::usage::UsageOutput]) {
    let mut lines: Vec<Line> = Vec::new();

    for (i, output) in outputs.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }

        lines.push(Line::from(Span::styled(
            format!(" {} ", output.provider),
            Style::default().fg(app.theme.foreground).add_modifier(Modifier::BOLD),
        )));

        for m in &output.metrics {
            let remaining = m.remaining_label.clone().unwrap_or_else(|| format!("{:.0}% left", m.remaining_percent));
            let bar = render_ascii_bar(m.remaining_percent);
            let reset = m
                .resets_at
                .as_ref()
                .map(|r| format_reset_time(r))
                .unwrap_or_default();

            let label = Span::styled(
                format!(" {:<12}", m.label),
                Style::default().fg(app.theme.foreground),
            );
            let value = Span::styled(
                format!("{:<11}", remaining),
                Style::default().fg(app.theme.foreground),
            );
            let bar_span = Span::styled(
                format!("{:<24}", bar),
                Style::default()
                    .fg(if m.remaining_percent < 10.0 {
                        Color::Red
                    } else if m.remaining_percent < 25.0 {
                        Color::Yellow
                    } else {
                        app.theme.accent
                    }),
            );
            let reset_span = Span::styled(reset, Style::default().fg(app.theme.muted));

            lines.push(Line::from(vec![label, value, bar_span, reset_span]));
        }

        if let Some(ref email) = output.email {
            lines.push(Line::from(Span::styled(
                format!(" {:<12}{email}", "Account"),
                Style::default().fg(app.theme.muted),
            )));
        }
        if let Some(ref plan) = output.plan {
            lines.push(Line::from(Span::styled(
                format!(" {:<12}{plan}", "Plan"),
                Style::default().fg(app.theme.muted),
            )));
        }
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}
