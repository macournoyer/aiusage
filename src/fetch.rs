use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local};
use serde_json::Value;
use std::path::PathBuf;

const CLAUDE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

fn credentials_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".claude/.credentials.json")
}

fn codex_auth_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".codex/auth.json")
}

#[derive(Debug, Clone)]
pub struct Window {
    pub used_percent: u8,
    pub reset_label: String,
}

#[derive(Debug, Clone)]
pub struct ExtraUsage {
    pub used_credits: f64,
    pub monthly_limit: f64,
    pub used_percent: u8,
    pub currency: String,
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeUsage {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
    pub extra: Option<ExtraUsage>,
}

#[derive(Debug, Clone, Default)]
pub struct CodexUsage {
    pub five_hour: Option<Window>,
    pub weekly: Option<Window>,
    pub plan: String,
}

#[derive(Debug, Clone, Default)]
pub struct UsageData {
    pub claude: Option<ClaudeUsage>,
    pub claude_error: Option<String>,
    pub codex: Option<CodexUsage>,
    pub codex_error: Option<String>,
}

fn format_reset(ts: &Value) -> String {
    // Try unix timestamp (integer or float)
    if let Some(secs) = ts.as_f64() {
        if let Some(dt) = DateTime::from_timestamp(secs as i64, 0) {
            return dt.with_timezone(&Local).format("%-I:%M %p").to_string();
        }
    }
    // Try ISO string
    if let Some(s) = ts.as_str() {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return dt.with_timezone(&Local).format("%-I:%M %p").to_string();
        }
    }
    String::new()
}

fn window_from_obj(obj: &Value, pct_key: &str, reset_key: &str) -> Option<Window> {
    let pct = obj.get(pct_key)?.as_f64()?;
    let reset_label = obj.get(reset_key).map(format_reset).unwrap_or_default();
    Some(Window {
        used_percent: pct.round().clamp(0.0, 100.0) as u8,
        reset_label,
    })
}

async fn get_claude_token(client: &reqwest::Client) -> Result<String> {
    let path = credentials_path();
    let raw = tokio::fs::read_to_string(&path)
        .await
        .context("reading ~/.claude/.credentials.json")?;
    let mut creds: Value = serde_json::from_str(&raw).context("parsing credentials")?;

    let oauth = creds
        .get_mut("claudeAiOauth")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| anyhow!("no claudeAiOauth in credentials"))?;

    let token = oauth
        .get("accessToken")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("no accessToken"))?
        .to_string();

    let expires_at = oauth
        .get("expiresAt")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    // expiresAt is in ms if > 1e10
    let exp_secs = if expires_at > 1e10 {
        expires_at / 1000.0
    } else {
        expires_at
    };
    let now = chrono::Utc::now().timestamp() as f64;

    if exp_secs < now + 300.0 {
        let refresh_token = oauth
            .get("refreshToken")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("token expired, no refreshToken"))?
            .to_string();

        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
            ("client_id", CLAUDE_OAUTH_CLIENT_ID),
        ];
        let resp = client
            .post(CLAUDE_TOKEN_URL)
            .form(&params)
            .send()
            .await
            .context("token refresh request")?
            .error_for_status()
            .context("token refresh HTTP error")?;
        let result: Value = resp.json().await.context("token refresh response")?;

        let new_token = result
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("no access_token in refresh response"))?
            .to_string();
        let new_refresh = result
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .unwrap_or(&refresh_token)
            .to_string();
        let expires_in = result.get("expires_in").and_then(|v| v.as_f64()).unwrap_or(3600.0);
        let new_expires_at = ((now + expires_in) * 1000.0) as u64;

        // Write back credentials atomically
        let oauth2 = creds["claudeAiOauth"].as_object_mut().unwrap();
        oauth2.insert("accessToken".into(), Value::String(new_token.clone()));
        oauth2.insert("refreshToken".into(), Value::String(new_refresh));
        oauth2.insert("expiresAt".into(), Value::Number(new_expires_at.into()));

        let tmp = path.with_extension("json.tmp");
        tokio::fs::write(&tmp, serde_json::to_string_pretty(&creds)?).await?;
        tokio::fs::rename(&tmp, &path).await?;

        return Ok(new_token);
    }

    Ok(token)
}

pub async fn fetch_claude(client: &reqwest::Client) -> Result<ClaudeUsage> {
    let token = get_claude_token(client).await?;
    let resp = client
        .get(CLAUDE_USAGE_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
        .context("Claude usage request")?
        .error_for_status()
        .context("Claude usage HTTP error")?;
    let data: Value = resp.json().await.context("Claude usage response")?;

    let five_hour = data
        .get("five_hour")
        .and_then(|w| window_from_obj(w, "utilization", "resets_at"));
    let seven_day = data
        .get("seven_day")
        .and_then(|w| window_from_obj(w, "utilization", "resets_at"));

    let extra = data.get("extra_usage").and_then(|e| {
        if e.get("is_enabled")?.as_bool()? {
            let used = e.get("used_credits")?.as_f64()?;
            let limit = e.get("monthly_limit")?.as_f64()?;
            let pct = if limit > 0.0 {
                (used / limit * 100.0).round().clamp(0.0, 100.0) as u8
            } else {
                0
            };
            let currency = e
                .get("currency")
                .and_then(|v| v.as_str())
                .unwrap_or("CAD")
                .to_string();
            Some(ExtraUsage {
                used_credits: used,
                monthly_limit: limit,
                used_percent: pct,
                currency,
            })
        } else {
            None
        }
    });

    Ok(ClaudeUsage { five_hour, seven_day, extra })
}

pub async fn fetch_codex(client: &reqwest::Client) -> Result<CodexUsage> {
    let path = codex_auth_path();
    let raw = tokio::fs::read_to_string(&path)
        .await
        .context("reading ~/.codex/auth.json")?;
    let auth: Value = serde_json::from_str(&raw).context("parsing codex auth")?;

    let tokens = auth.get("tokens").ok_or_else(|| anyhow!("no tokens in codex auth"))?;
    let access_token = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("no access_token in codex auth"))?
        .to_string();
    let account_id = tokens
        .get("account_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut req = client
        .get(CODEX_USAGE_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .header("User-Agent", "aiusage-tui");
    if !account_id.is_empty() {
        req = req.header("chatgpt-account-id", &account_id);
    }
    let resp = req
        .send()
        .await
        .context("Codex usage request")?
        .error_for_status()
        .context("Codex usage HTTP error")?;
    let data: Value = resp.json().await.context("Codex usage response")?;

    let plan = data
        .get("plan_type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let rate_limit = data.get("rate_limit").cloned().unwrap_or(Value::Null);
    let five_hour = rate_limit
        .get("primary_window")
        .and_then(|w| window_from_obj(w, "used_percent", "reset_at"));
    let weekly = rate_limit
        .get("secondary_window")
        .and_then(|w| window_from_obj(w, "used_percent", "reset_at"));

    Ok(CodexUsage { five_hour, weekly, plan })
}

pub async fn fetch_all() -> UsageData {
    let client = reqwest::Client::new();
    let (claude_res, codex_res) =
        tokio::join!(fetch_claude(&client), fetch_codex(&client));

    UsageData {
        claude: claude_res.as_ref().ok().cloned(),
        claude_error: claude_res.err().map(|e| e.to_string()),
        codex: codex_res.as_ref().ok().cloned(),
        codex_error: codex_res.err().map(|e| e.to_string()),
    }
}
