use anyhow::Result;
use serde::Deserialize;

use super::{UsageMetric, UsageOutput};
use super::helpers::capitalize;

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const BETA_HEADER: &str = "oauth-2025-04-20";

#[derive(Debug, Deserialize)]
struct Credentials {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<Oauth>,
}

#[derive(Debug, Deserialize)]
struct Oauth {
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
struct UsageResponse {
    five_hour: Option<Window>,
    seven_day: Option<Window>,
    seven_day_opus: Option<Window>,
}

#[derive(Debug, Deserialize)]
struct Window {
    utilization: f64,
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenRefresh {
    access_token: Option<String>,
}

fn read_keychain() -> Result<String> {
    super::helpers::read_keychain("Claude Code-credentials")
}

pub fn has_credentials() -> bool {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join(".claude").join(".credentials.json").exists()
        || super::helpers::read_keychain("Claude Code-credentials").is_ok()
}

fn read_credentials() -> Result<Credentials> {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let path = home.join(".claude").join(".credentials.json");
    let content = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        read_keychain()?
    };
    Ok(serde_json::from_str(&content)?)
}

async fn refresh_token(client: &reqwest::Client, rt: &str) -> Result<TokenRefresh> {
    let resp = client
        .post("https://platform.claude.com/v1/oauth/token")
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": rt,
            "client_id": CLIENT_ID,
            "scope": "user:profile user:inference user:sessions:claude_code user:mcp_servers"
        }))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Claude token refresh failed (HTTP {})", resp.status());
    }
    Ok(resp.json().await?)
}

async fn fetch_usage(client: &reqwest::Client, token: &str) -> Result<UsageResponse> {
    let resp = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("anthropic-beta", BETA_HEADER)
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

pub fn fetch() -> Result<UsageOutput> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let creds = read_credentials()?;
        let oauth = creds.claude_ai_oauth.ok_or_else(|| {
            anyhow::anyhow!("No Claude OAuth credentials. Run 'claude' to log in.")
        })?;
        let access_token = oauth
            .access_token
            .ok_or_else(|| anyhow::anyhow!("No Claude access token."))?;
        let plan = oauth.subscription_type.map(|s| {
            let tier = oauth.rate_limit_tier.as_deref().and_then(|t| {
                t.rsplit('_').next()
            });
            match tier {
                Some(mult) => format!("{} {}", capitalize(&s), mult),
                None => capitalize(&s),
            }
        });

        let client = reqwest::Client::new();
        let resp = match fetch_usage(&client, &access_token).await {
            Ok(r) => r,
            Err(e) if e.to_string().contains("NEEDS_AUTH") => {
                let rt = oauth
                    .refresh_token
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("No refresh token."))?;
                let refreshed = refresh_token(&client, rt).await?;
                let new = refreshed
                    .access_token
                    .ok_or_else(|| anyhow::anyhow!("Refresh returned no token."))?;
                fetch_usage(&client, &new).await?
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
