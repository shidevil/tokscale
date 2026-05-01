use anyhow::Result;
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::Deserialize;
use serde_json;

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

// ── Claude ──

const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_BETA: &str = "oauth-2025-04-20";

#[derive(Debug, Deserialize)]
struct ClaudeCredentials {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeOauth>,
}

#[derive(Debug, Deserialize)]
struct ClaudeOauth {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeUsageResponse {
    five_hour: Option<Window>,
    seven_day: Option<Window>,
    seven_day_opus: Option<Window>,
    #[allow(dead_code)]
    extra_usage: Option<ClaudeExtraUsage>,
}

#[derive(Debug, Deserialize)]
struct Window {
    utilization: f64,
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ClaudeExtraUsage {
    is_enabled: Option<bool>,
    used_credits: Option<f64>,
    monthly_limit: Option<f64>,
    currency: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeTokenRefresh {
    access_token: Option<String>,
}

fn read_claude_keychain() -> Result<String> {
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("Keychain lookup failed");
    }
    Ok(String::from_utf8(out.stdout)?.trim_end().to_string())
}

fn read_claude_credentials() -> Result<ClaudeCredentials> {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let path = home.join(".claude").join(".credentials.json");
    let content = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        read_claude_keychain()?
    };
    Ok(serde_json::from_str(&content)?)
}

async fn claude_refresh(client: &reqwest::Client, rt: &str) -> Result<ClaudeTokenRefresh> {
    let resp = client
        .post("https://platform.claude.com/v1/oauth/token")
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": rt,
            "client_id": CLAUDE_CLIENT_ID,
            "scope": "user:profile user:inference user:sessions:claude_code user:mcp_servers"
        }))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Claude token refresh failed (HTTP {})", resp.status());
    }
    Ok(resp.json().await?)
}

async fn claude_fetch(client: &reqwest::Client, token: &str) -> Result<ClaudeUsageResponse> {
    let resp = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("anthropic-beta", CLAUDE_BETA)
        .send()
        .await?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        anyhow::bail!("NEEDS_AUTH");
    }
    if !status.is_success() {
        anyhow::bail!("Claude usage request failed (HTTP {status})");
    }
    Ok(resp.json().await?)
}

fn fetch_claude() -> Result<UsageOutput> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let creds = read_claude_credentials()?;
        let oauth = creds.claude_ai_oauth.ok_or_else(|| {
            anyhow::anyhow!("No Claude OAuth credentials. Run 'claude' to log in.")
        })?;
        let access_token = oauth
            .access_token
            .ok_or_else(|| anyhow::anyhow!("No Claude access token."))?;
        let plan = oauth.subscription_type.map(|s| {
            let tier = oauth.rate_limit_tier.as_deref().and_then(|t| {
                // "default_claude_max_20x" -> "20x", "default_claude_max_5x" -> "5x"
                t.rsplit('_').next()
            });
            match tier {
                Some(mult) => format!("{} {}", capitalize(&s), mult),
                None => capitalize(&s),
            }
        });

        let client = reqwest::Client::new();
        let resp = match claude_fetch(&client, &access_token).await {
            Ok(r) => r,
            Err(e) if e.to_string().contains("NEEDS_AUTH") => {
                let rt = oauth
                    .refresh_token
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("No refresh token."))?;
                let refreshed = claude_refresh(&client, rt).await?;
                let new = refreshed
                    .access_token
                    .ok_or_else(|| anyhow::anyhow!("Refresh returned no token."))?;
                claude_fetch(&client, &new).await?
            }
            Err(e) => return Err(e),
        };

        let mut metrics = Vec::new();
        if let Some(ref w) = resp.five_hour {
            metrics.push(window_metric("Session", w));
        }
        if let Some(ref w) = resp.seven_day {
            metrics.push(window_metric("Weekly", w));
        }
        if let Some(ref w) = resp.seven_day_opus {
            metrics.push(window_metric("Opus", w));
        }

        Ok(UsageOutput {
            provider: "Claude".into(),
            plan,
            email: None,
            metrics,
        })
    })
}

// ── Codex (OpenAI) ──

const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

#[derive(Debug, Deserialize)]
struct CodexAuth {
    tokens: Option<CodexTokens>,
}

#[derive(Debug, Deserialize)]
struct CodexTokens {
    access_token: Option<String>,
    refresh_token: Option<String>,
    account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CodexUsage {
    email: Option<String>,
    plan_type: Option<String>,
    rate_limit: Option<CodexRateLimit>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CodexRateLimit {
    primary_window: Option<CodexWindow>,
    secondary_window: Option<CodexWindow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CodexWindow {
    used_percent: Option<i64>,
    reset_at: Option<i64>,
    #[allow(dead_code)]
    limit_window_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CodexRefresh {
    access_token: Option<String>,
}

fn read_codex_credentials() -> Result<CodexAuth> {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let paths = [
        home.join(".config").join("codex").join("auth.json"),
        home.join(".codex").join("auth.json"),
    ];
    for p in &paths {
        if p.exists() {
            let content = std::fs::read_to_string(p)?;
            if let Ok(auth) = serde_json::from_str::<CodexAuth>(&content) {
                if auth.tokens.is_some() {
                    return Ok(auth);
                }
            }
        }
    }
    anyhow::bail!("No Codex credentials found. Run 'codex' to log in.")
}

async fn codex_refresh(client: &reqwest::Client, rt: &str) -> Result<CodexRefresh> {
    let resp = client
        .post("https://auth.openai.com/oauth/token")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=refresh_token&client_id={CODEX_CLIENT_ID}&refresh_token={rt}"
        ))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Codex token refresh failed (HTTP {})", resp.status());
    }
    Ok(resp.json().await?)
}

async fn codex_fetch(client: &reqwest::Client, token: &str, account_id: Option<&str>) -> Result<CodexUsage> {
    let mut req = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
    if let Some(id) = account_id {
        req = req.header("ChatGPT-Account-Id", id);
    }
    let resp = req.send().await?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        anyhow::bail!("NEEDS_AUTH");
    }
    if !status.is_success() {
        anyhow::bail!("Codex usage request failed (HTTP {status})");
    }
    let body = resp.text().await?;
    if body.trim().starts_with('<') {
        anyhow::bail!("NEEDS_AUTH");
    }
    Ok(serde_json::from_str(&body)?)
}

fn fetch_codex() -> Result<UsageOutput> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let auth = read_codex_credentials()?;
        let tokens = auth
            .tokens
            .ok_or_else(|| anyhow::anyhow!("No Codex tokens."))?;
        let access_token = tokens
            .access_token
            .ok_or_else(|| anyhow::anyhow!("No Codex access token."))?;
        let account_id = tokens.account_id.as_deref();

        let client = reqwest::Client::new();
        let resp = match codex_fetch(&client, &access_token, account_id).await {
            Ok(r) => r,
            Err(e) if e.to_string().contains("NEEDS_AUTH") => {
                let rt = tokens
                    .refresh_token
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("No refresh token."))?;
                let refreshed = codex_refresh(&client, rt).await?;
                let new = refreshed
                    .access_token
                    .ok_or_else(|| anyhow::anyhow!("Refresh returned no token."))?;
                codex_fetch(&client, &new, account_id).await?
            }
            Err(e) => return Err(e),
        };

        let plan = resp.plan_type.as_deref().map(capitalize);
        let mut metrics = Vec::new();
        if let Some(ref rl) = resp.rate_limit {
            if let Some(ref w) = rl.primary_window {
                let pct = w.used_percent.unwrap_or(0).clamp(0, 100) as f64;
                metrics.push(UsageMetric {
                    label: "Session".into(),
                    used_percent: pct,
                    remaining_percent: 100.0 - pct,
                    remaining_label: None,
                    resets_at: w.reset_at.and_then(|ts| Utc.timestamp_opt(ts, 0).single())
                        .map(|dt| dt.to_rfc3339()),
                });
            }
            if let Some(ref w) = rl.secondary_window {
                let pct = w.used_percent.unwrap_or(0).clamp(0, 100) as f64;
                metrics.push(UsageMetric {
                    label: "Weekly".into(),
                    used_percent: pct,
                    remaining_percent: 100.0 - pct,
                    remaining_label: None,
                    resets_at: w.reset_at.and_then(|ts| Utc.timestamp_opt(ts, 0).single())
                        .map(|dt| dt.to_rfc3339()),
                });
            }
        }

        Ok(UsageOutput {
            provider: "Codex".into(),
            plan,
            email: resp.email,
            metrics,
        })
    })
}

// ── Z.ai ──

#[derive(Debug, Deserialize)]
struct ZaiQuotaResp {
    data: Option<ZaiQuotaData>,
}

#[derive(Debug, Deserialize)]
struct ZaiQuotaData {
    limits: Option<Vec<ZaiLimit>>,
    level: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ZaiLimit {
    #[serde(rename = "type")]
    limit_type: Option<String>,
    #[allow(dead_code)]
    usage: Option<f64>,
    #[allow(dead_code)]
    remaining: Option<f64>,
    percentage: Option<f64>,
    #[allow(dead_code)]
    current_value: Option<f64>,
    number: Option<i64>,
    unit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ZaiSubResp {
    data: Option<Vec<ZaiSub>>,
}

#[derive(Debug, Deserialize)]
struct ZaiSub {
    product_name: Option<String>,
    next_renew_time: Option<String>,
}

async fn zai_fetch_quota(client: &reqwest::Client, key: &str) -> Result<ZaiQuotaResp> {
    let resp = client
        .get("https://api.z.ai/api/monitor/usage/quota/limit")
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Z.ai quota request failed (HTTP {})", resp.status());
    }
    Ok(resp.json().await?)
}

async fn zai_fetch_sub(client: &reqwest::Client, key: &str) -> Result<ZaiSubResp> {
    let resp = client
        .get("https://api.z.ai/api/biz/subscription/list")
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Z.ai subscription request failed (HTTP {})", resp.status());
    }
    Ok(resp.json().await?)
}

fn fetch_zai() -> Result<UsageOutput> {
    let api_key = std::env::var("ZAI_API_KEY")
        .or_else(|_| std::env::var("GLM_API_KEY"))
        .map_err(|_| anyhow::anyhow!("No ZAI_API_KEY or GLM_API_KEY set."))?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let client = reqwest::Client::new();
        let quota = zai_fetch_quota(&client, &api_key).await?;
        let sub = zai_fetch_sub(&client, &api_key).await.ok();

        let plan = sub
            .as_ref()
            .and_then(|s| s.data.as_ref())
            .and_then(|d| d.first())
            .and_then(|s| s.product_name.clone())
            .or_else(|| quota.data.as_ref().and_then(|d| d.level.clone()).map(|l| capitalize(&l)));

        let mut metrics = Vec::new();
        if let Some(ref limits) = quota.data.as_ref().and_then(|d| d.limits.as_ref()) {
            for limit in limits.iter() {
                let pct = limit.percentage.unwrap_or(0.0).clamp(0.0, 100.0);

                match limit.limit_type.as_deref() {
                    Some("TOKENS_LIMIT") => {
                        let label = match (limit.unit, limit.number) {
                            (Some(3), Some(5)) => "Session",
                            (Some(6), Some(1)) => "Monthly",
                            _ => "Tokens",
                        };
                        metrics.push(UsageMetric {
                            label: label.into(),
                            used_percent: pct,
                            remaining_percent: 100.0 - pct,
                            remaining_label: None,
                            resets_at: None,
                        });
                    }
                    Some("TIME_LIMIT") => {
                        let remaining_label = limit.remaining.map(|r| format!("{:.0} left", r));
                        metrics.push(UsageMetric {
                            label: "Web Searches".into(),
                            used_percent: pct,
                            remaining_percent: 100.0 - pct,
                            remaining_label,
                            resets_at: sub
                                .as_ref()
                                .and_then(|s| s.data.as_ref())
                                .and_then(|d| d.first())
                                .and_then(|s| s.next_renew_time.clone()),
                        });
                    }
                    _ => {}
                }
            }
        }

        Ok(UsageOutput {
            provider: "Z.ai".into(),
            plan,
            email: None,
            metrics,
        })
    })
}

// ── Helpers ──

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s.to_string(),
    }
}

fn window_metric(label: &str, w: &Window) -> UsageMetric {
    let used = w.utilization.clamp(0.0, 100.0);
    UsageMetric {
        label: label.into(),
        used_percent: used,
        remaining_percent: 100.0 - used,
        remaining_label: None,
        resets_at: w.resets_at.clone(),
    }
}

// ── Public API ──

pub fn fetch_all() -> Vec<UsageOutput> {
    let mut results = Vec::new();

    match fetch_claude() {
        Ok(o) => results.push(o),
        Err(e) => eprintln!("Claude: {e}"),
    }
    match fetch_codex() {
        Ok(o) => results.push(o),
        Err(e) => eprintln!("Codex: {e}"),
    }
    match fetch_zai() {
        Ok(o) => results.push(o),
        Err(e) => eprintln!("Z.ai: {e}"),
    }

    results
}

const BAR_WIDTH: usize = 12;
const CARD_WIDTH: usize = 58;

fn format_reset_time(resets_at: &str) -> String {
    let dt = match DateTime::parse_from_rfc3339(resets_at) {
        Ok(d) => d.with_timezone(&Utc),
        Err(_) => return resets_at.into(),
    };
    let diff = dt - Utc::now();
    if diff <= Duration::zero() {
        return "resets now".into();
    }
    let total_mins = diff.num_minutes();
    if total_mins < 60 {
        format!("resets in {total_mins}m")
    } else if total_mins < 24 * 60 {
        let h = diff.num_hours();
        let m = (diff - Duration::hours(h)).num_minutes();
        if m > 0 { format!("resets in {h}h {m}m") } else { format!("resets in {h}h") }
    } else if diff.num_days() < 7 {
        format!("resets {} {}", dt.format("%a"), dt.format("%-I%P"))
    } else {
        format!("resets {}", dt.format("%b %-d"))
    }
}

fn render_ascii_bar(pct: f64) -> String {
    let filled = (pct.clamp(0.0, 100.0) / 100.0 * BAR_WIDTH as f64).round() as usize;
    format!("[{}{}]", "=".repeat(filled), "-".repeat(BAR_WIDTH - filled))
}

fn render_light(output: &UsageOutput) {
    println!("╭{}╮", "─".repeat(CARD_WIDTH));
    for m in &output.metrics {
        let rem = m.remaining_label.clone().unwrap_or_else(|| format!("{:.0}% left", m.remaining_percent));
        let bar = render_ascii_bar(m.remaining_percent);
        let reset = m.resets_at.as_ref().map(|r| format_reset_time(r)).unwrap_or_default();
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
