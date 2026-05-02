mod amp;
mod claude;
mod codex;
mod copilot;
mod helpers;
mod kimi;
mod minimax;
mod zai;

use anyhow::Result;

// ── Shared types ──

#[derive(Debug, Clone, serde::Serialize)]
pub struct UsageMetric {
    pub label: String,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub remaining_label: Option<String>,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UsageOutput {
    pub provider: String,
    pub plan: Option<String>,
    pub email: Option<String>,
    pub metrics: Vec<UsageMetric>,
}

// ── Public API ──

pub fn fetch_all() -> Vec<UsageOutput> {
    let mut results = Vec::new();

    macro_rules! try_fetch {
        ($name:expr, $func:expr) => {
            match $func() {
                Ok(o) => results.push(o),
                Err(e) => eprintln!("{}: {e}", $name),
            }
        };
    }

    try_fetch!("Claude", claude::fetch);
    try_fetch!("Codex", codex::fetch);
    try_fetch!("Z.ai", zai::fetch);
    try_fetch!("Amp", amp::fetch);
    try_fetch!("Copilot", copilot::fetch);
    try_fetch!("Kimi", kimi::fetch);
    try_fetch!("MiniMax", minimax::fetch);

    results
}

// ── Light-mode rendering ──

const BAR_WIDTH: usize = 12;
const CARD_WIDTH: usize = 58;

fn render_light(output: &UsageOutput) {
    println!("╭{}╮", "─".repeat(CARD_WIDTH));
    for m in &output.metrics {
        let rem = m.remaining_label.clone().unwrap_or_else(|| format!("{:.0}% left", m.remaining_percent));
        let bar = helpers::render_ascii_bar(m.remaining_percent, BAR_WIDTH);
        let reset = m.resets_at.as_ref().map(|r| helpers::format_reset_time(r)).unwrap_or_default();
        println!("│ {:<10}{:<11}{:<14}{:<20}│", m.label, rem, bar, reset);
    }
    if let Some(ref email) = output.email {
        println!("│ {:<width$}│", format!("Account  {email}"), width = CARD_WIDTH);
    }
    if let Some(ref plan) = output.plan {
        println!("│ {:<width$}│", format!("Plan     {plan}"), width = CARD_WIDTH);
    }
    println!("╰{}╯", "─".repeat(CARD_WIDTH));
}

pub fn run(json: bool, _light: bool) -> Result<()> {
    let outputs = fetch_all();
    if json {
        println!("{}", serde_json::to_string_pretty(&outputs)?);
    } else {
        for o in &outputs {
            render_light(o);
        }
    }
    Ok(())
}
