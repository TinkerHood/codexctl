//! `OpenAI` legacy billing API client for an unverified quota estimate.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Estimated quota from legacy `OpenAI` billing endpoints.
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

/// Fetch a legacy billing estimate; the endpoint's units are not officially documented.
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

    build_quota(&data, fetch_usage(api_key).await)
}

fn build_quota(data: &Value, usage: Result<u64>) -> Result<RealTimeQuota> {
    let usage = usage.context("Failed to fetch current month's usage")?;

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

    let quota_limit = data
        .get("hard_limit_usd")
        .and_then(Value::as_f64)
        .context("Subscription response missing numeric hard_limit_usd")?;
    let quota_limit = amount_to_cents(quota_limit)
        .context("Subscription response hard_limit_usd is out of range")?;

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

    parse_total_usage(&data)
}

fn parse_total_usage(data: &Value) -> Result<u64> {
    let total_usage = data
        .get("total_usage")
        .and_then(Value::as_f64)
        .context("Usage response missing numeric total_usage")?;

    // Preserve the existing conversion until the legacy usage endpoint's
    // unit can be established from an official contract.
    amount_to_cents(total_usage).context("Usage response total_usage is out of range")
}

fn amount_to_cents(value: f64) -> Result<u64> {
    let cents = value * 100.0;
    if !value.is_finite() || value < 0.0 || !cents.is_finite() || cents >= u64::MAX as f64 {
        anyhow::bail!("Amount must be finite, nonnegative, and fit in u64 cents");
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(cents as u64)
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

    #[test]
    fn failed_usage_does_not_become_zero_quota() {
        let subscription = serde_json::json!({"hard_limit_usd": 100.0});
        let error =
            build_quota(&subscription, Err(anyhow::anyhow!("usage unavailable"))).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Failed to fetch current month's usage")
        );
        assert_eq!(error.root_cause().to_string(), "usage unavailable");

        let quota = build_quota(&subscription, Ok(0)).unwrap();
        assert_eq!(quota.usage_this_month, 0);
        assert_eq!(quota.remaining_quota, 10_000);
    }

    #[test]
    fn missing_usage_value_is_an_error() {
        assert!(parse_total_usage(&serde_json::json!({})).is_err());
        assert!(parse_total_usage(&serde_json::json!({"total_usage": "unknown"})).is_err());
        assert_eq!(
            parse_total_usage(&serde_json::json!({"total_usage": 1.5})).unwrap(),
            150
        );
        assert!(parse_total_usage(&serde_json::json!({"total_usage": -1.0})).is_err());
        assert!(parse_total_usage(&serde_json::json!({"total_usage": 1e20})).is_err());
        assert!(amount_to_cents(f64::NAN).is_err());
        assert!(amount_to_cents(f64::INFINITY).is_err());
    }

    #[test]
    fn subscription_limit_must_be_valid_or_exactly_zero() {
        for data in [
            serde_json::json!({}),
            serde_json::json!({"hard_limit_usd": "unknown"}),
            serde_json::json!({"hard_limit_usd": -1.0}),
            serde_json::json!({"hard_limit_usd": 1e20}),
        ] {
            let error = build_quota(&data, Ok(0)).unwrap_err();
            assert!(format!("{error:#}").contains("hard_limit_usd"));
        }

        let quota = build_quota(&serde_json::json!({"hard_limit_usd": 0.0}), Ok(0)).unwrap();
        assert_eq!(quota.quota_limit, 0);
        assert_eq!(quota.remaining_quota, 0);
    }
}
