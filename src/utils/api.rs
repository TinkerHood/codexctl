//! `OpenAI` API client for fetching real-time quota information

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Real-time quota information from `OpenAI` API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealTimeQuota {
    pub account_id: String,
    pub plan: String,
    pub usage_this_month: u64,
    pub quota_limit: u64,
    pub remaining_quota: u64,
    pub percent_used: f64,
    pub reset_date: Option<String>,
}

impl RealTimeQuota {
    /// Calculate days until quota reset
    pub fn days_until_reset(&self) -> Option<i64> {
        use chrono::{DateTime, Utc};

        self.reset_date.as_ref().and_then(|date| {
            DateTime::parse_from_rfc3339(date)
                .ok()
                .map(|d| (d.with_timezone(&Utc) - Utc::now()).num_days())
        })
    }

    /// Check if quota is critically low (< 20%)
    pub fn is_critical(&self) -> bool {
        self.percent_used > 80.0
    }

    /// Check if quota is low (< 50%)
    pub fn is_low(&self) -> bool {
        self.percent_used > 50.0
    }
}

/// Fetch real-time quota from `OpenAI` API
pub async fn fetch_quota(api_key: &str) -> Result<RealTimeQuota> {
    let client = reqwest::Client::new();

    let response = client
        .get("https://api.openai.com/v1/dashboard/billing/subscription")
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .context("Failed to connect to OpenAI API")?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("OpenAI API error ({status}): {text}");
    }

    let data: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse OpenAI API response")?;

    // Parse the response
    let account_id = data
        .get("account_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let plan = data
        .get("plan")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let quota_limit = data
        .get("hard_limit_usd")
        .and_then(Value::as_f64)
        .map(|v| (v * 100.0) as u64)
        .unwrap_or(0);

    // Fetch usage data
    let usage = fetch_usage(api_key).await.unwrap_or(0);
    let remaining = quota_limit.saturating_sub(usage);
    #[allow(clippy::cast_precision_loss)]
    let percent_used = if quota_limit > 0 {
        (usage as f64 / quota_limit as f64) * 100.0
    } else {
        0.0
    };

    let reset_date = data
        .get("reset_date")
        .or_else(|| data.get("billing_cycle_anchor"))
        .and_then(Value::as_str)
        .map(std::string::ToString::to_string);

    Ok(RealTimeQuota {
        account_id,
        plan,
        usage_this_month: usage,
        quota_limit,
        remaining_quota: remaining,
        percent_used,
        reset_date,
    })
}

/// Fetch current month's usage from `OpenAI` API
async fn fetch_usage(api_key: &str) -> Result<u64> {
    use chrono::{Datelike, Utc};

    let now = Utc::now();
    let start_of_month = format!("{}-{:02}-01", now.year(), now.month());
    let today = format!("{}-{:02}-{:02}", now.year(), now.month(), now.day());

    let client = reqwest::Client::new();
    let url = format!(
        "https://api.openai.com/v1/dashboard/billing/usage?start_date={start_of_month}&end_date={today}"
    );

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .context("Failed to fetch usage data")?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("OpenAI API error ({status}): {text}");
    }

    let data: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse usage response")?;

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let total_usage = data
        .get("total_usage")
        .and_then(Value::as_f64)
        .map(|v| (v * 100.0) as u64) // Convert to cents
        .unwrap_or(0);

    Ok(total_usage)
}

/// Extract API key from auth.json
pub fn extract_api_key(auth_json: &serde_json::Value) -> Option<String> {
    // Try various locations where the API key might be stored
    auth_json
        .get("api_key")
        .or_else(|| auth_json.get("key"))
        .or_else(|| auth_json.get("access_token"))
        .and_then(Value::as_str)
        .map(std::string::ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_realtime_quota_calculations() {
        let quota = RealTimeQuota {
            account_id: "test".to_string(),
            plan: "personal".to_string(),
            usage_this_month: 7500, // $75.00
            quota_limit: 10000,     // $100.00
            remaining_quota: 2500,  // $25.00
            percent_used: 75.0,
            reset_date: None,
        };

        assert!(quota.is_low());
        assert!(!quota.is_critical());
        assert_eq!(quota.remaining_quota, 2500);
    }

    #[test]
    fn test_extract_api_key() {
        let auth = serde_json::json!({
            "api_key": "sk-test123",
            "email": "test@example.com"
        });

        assert_eq!(extract_api_key(&auth), Some("sk-test123".to_string()));
    }

    #[test]
    fn test_extract_api_key_alt_field() {
        let auth = serde_json::json!({
            "access_token": "sk-token456",
        });

        assert_eq!(extract_api_key(&auth), Some("sk-token456".to_string()));
    }
}
