use anyhow::Result;
use chrono::{TimeZone, Utc};
use serde::Deserialize;

use super::{UsageMetric, UsageOutput};
use super::helpers::capitalize;

#[derive(Debug, Deserialize)]
struct ApiResponse {
    base_resp: Option<BaseResp>,
    model_remains: Option<Vec<ModelRemains>>,
    data: Option<ApiData>,
}

#[derive(Debug, Deserialize)]
struct BaseResp {
    status_code: Option<i64>,
    status_msg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiData {
    model_remains: Option<Vec<ModelRemains>>,
}

#[derive(Debug, Deserialize)]
struct ModelRemains {
    current_interval_total_count: Option<i64>,
    current_interval_usage_count: Option<i64>,
    current_interval_remaining_count: Option<i64>,
    current_subscribe_title: Option<String>,
    #[allow(dead_code)]
    start_time: Option<i64>,
    end_time: Option<i64>,
    #[allow(dead_code)]
    remains_time: Option<i64>,
}

fn read_api_key() -> Result<String> {
    std::env::var("MINIMAX_API_KEY")
        .or_else(|_| std::env::var("MINIMAX_API_TOKEN"))
        .map_err(|_| anyhow::anyhow!("No MINIMAX_API_KEY or MINIMAX_API_TOKEN set."))
}

fn is_error(resp: &ApiResponse) -> bool {
    if let Some(ref base) = resp.base_resp {
        if base.status_code.unwrap_or(0) != 0 {
            return true;
        }
        if let Some(ref msg) = base.status_msg {
            let lower = msg.to_lowercase();
            if lower.contains("cookie") || lower.contains("log in") || lower.contains("login") {
                return true;
            }
        }
    }
    false
}

fn infer_plan(total: i64) -> String {
    match total {
        0..=15 => "Starter".into(),
        16..=300 => "Plus".into(),
        301..=1000 => "Max".into(),
        _ => "Ultra".into(),
    }
}

fn parse_end_time(ts: i64) -> String {
    // Auto-detect seconds vs milliseconds
    let secs = if ts > 1_000_000_000_0 { ts / 1000 } else { ts };
    Utc.timestamp_opt(secs, 0)
        .single()
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| ts.to_string())
}

async fn fetch_api(client: &reqwest::Client, key: &str) -> Result<ApiResponse> {
    let resp = client
        .get("https://api.minimax.io/v1/api/openplatform/coding_plan/remains")
        .header("Authorization", format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("MiniMax usage request failed (HTTP {})", resp.status());
    }
    Ok(resp.json().await?)
}

pub fn fetch() -> Result<UsageOutput> {
    let api_key = read_api_key()?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let client = reqwest::Client::new();
        let resp = fetch_api(&client, &api_key).await?;

        if is_error(&resp) {
            let msg = resp.base_resp.as_ref()
                .and_then(|b| b.status_msg.clone())
                .unwrap_or_else(|| "Unknown error".into());
            anyhow::bail!("MiniMax API error: {msg}");
        }

        // model_remains can be top-level or nested under "data"
        let remains = resp.model_remains.as_ref()
            .or_else(|| resp.data.as_ref().and_then(|d| d.model_remains.as_ref()))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        let mut metrics = Vec::new();
        let mut plan: Option<String> = None;

        for model in remains.iter() {
            // Try explicit plan title first
            if plan.is_none() {
                plan = model.current_subscribe_title.as_ref()
                    .map(|t| {
                        let cleaned = t.trim_start_matches("MiniMax Coding Plan").trim();
                        if cleaned.is_empty() { t.clone() } else { capitalize(cleaned) }
                    });
            }

            let total = model.current_interval_total_count.unwrap_or(0);
            if total <= 0 {
                continue;
            }

            // MiniMax's usage_count is often actually remaining count
            let remaining = model.current_interval_remaining_count
                .or_else(|| model.current_interval_usage_count.map(|u| if u <= total { total - u } else { u }))
                .unwrap_or(0);

            let used = (total - remaining).max(0);
            let used_pct = (used as f64 / total as f64 * 100.0).clamp(0.0, 100.0);

            let resets_at = model.end_time.map(|ts| parse_end_time(ts));

            metrics.push(UsageMetric {
                label: "Prompts".into(),
                used_percent: used_pct,
                remaining_percent: 100.0 - used_pct,
                remaining_label: Some(format!("{remaining}/{total} left")),
                resets_at,
            });
        }

        // Infer plan from total count if not explicitly provided
        if plan.is_none() {
            if let Some(first) = remains.first() {
                plan = first.current_interval_total_count.map(infer_plan);
            }
        }

        Ok(UsageOutput {
            provider: "MiniMax".into(),
            plan,
            email: None,
            metrics,
        })
    })
}
