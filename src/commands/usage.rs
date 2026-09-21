use crate::utils::auth::{
    UsageInfo, auth_mode_has_api_key, auth_mode_has_chatgpt, auth_mode_label, detect_auth_mode,
    extract_usage_info,
};
use crate::utils::config::Config;
use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use colored::Colorize as _;
use prettytable::{Cell, Row, Table, format};
use serde::Serialize;

pub async fn execute(
    config: Config,
    all: bool,
    realtime: bool,
    json: bool,
    quiet: bool,
) -> Result<()> {
    if all {
        return show_all_profiles_usage(config, json, quiet).await;
    }

    let codex_dir = config.codex_dir();
    let auth_path = codex_dir.join("auth.json");

    if !auth_path.exists() {
        anyhow::bail!(
            "No `Codex` authentication found. Run `codex` and sign in with ChatGPT or an API key."
        );
    }

    let Ok(content) = tokio::fs::read_to_string(&auth_path).await else {
        anyhow::bail!("Failed to read auth.json");
    };

    let Ok(auth_json) = serde_json::from_str::<serde_json::Value>(&content) else {
        anyhow::bail!("Failed to parse auth.json");
    };
    let auth_mode = detect_auth_mode(&auth_json);

    let usage_info = extract_usage_info(&auth_json).ok();
    let realtime_result = if realtime && auth_mode_has_api_key(&auth_mode) {
        Some(fetch_realtime_quota(&auth_json).await)
    } else {
        None
    };

    if json {
        let payload = UsageCommandJson {
            auth_mode: auth_mode.clone(),
            auth_mode_label: auth_mode_label(&auth_mode).to_string(),
            plan_claims_available: auth_mode_has_chatgpt(&auth_mode) && usage_info.is_some(),
            api_realtime_available: auth_mode_has_api_key(&auth_mode),
            realtime_requested: realtime,
            plan_claims: usage_info.clone(),
            realtime_quota: realtime_result
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .cloned(),
            realtime_error: realtime_result.and_then(|result| result.err().map(|e| e.to_string())),
        };
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    // Extract usage info from ChatGPT/Codex JWT claims when present.
    if auth_mode_has_chatgpt(&auth_mode) {
        if let Some(info) = usage_info.as_ref() {
            display_usage_table(info);
        } else if !quiet {
            println!("\n{}", "`Codex` plan claims unavailable".yellow().bold());
            println!(
                "  Current auth mode reports ChatGPT/Codex tokens, but claims could not be parsed."
            );
        }
    } else if !quiet {
        println!("\n{}", "API-key auth mode detected".cyan().bold());
        println!("  ChatGPT/Codex plan claims are not available in API-key-only mode.");
    }

    // Fetch real-time quota if requested and API-key auth is available.
    if realtime {
        if !auth_mode_has_api_key(&auth_mode) {
            if !quiet {
                eprintln!(
                    "\n{} `--realtime` requires API key auth. Current mode: {}",
                    "⚠".yellow(),
                    auth_mode
                );
            }
        } else {
            match realtime_result.expect("realtime result prepared when API key is available") {
                Ok(quota) => display_realtime_quota(&quota),
                Err(e) => {
                    if !quiet {
                        eprintln!("\n{} Could not fetch real-time quota: {}", "⚠".yellow(), e);
                    }
                }
            }
        }
    }

    // Display subscription status when ChatGPT/Codex plan claims are available.
    if auth_mode_has_chatgpt(&auth_mode)
        && let Some(info) = usage_info.as_ref()
    {
        display_subscription_status(info);
    }

    // Display helpful info relevant to the current auth mode.
    display_limits_info(&auth_mode, usage_info.is_some());

    Ok(())
}

/// Fetch real-time quota from `OpenAI` API
async fn fetch_realtime_quota(
    auth_json: &serde_json::Value,
) -> anyhow::Result<crate::utils::api::RealTimeQuota> {
    use crate::utils::api::{extract_api_key, fetch_quota};

    let api_key = extract_api_key(auth_json).context("No API key found in auth.json")?;

    fetch_quota(&api_key).await
}

/// Display real-time quota information
#[allow(clippy::cast_precision_loss)]
fn display_realtime_quota(quota: &crate::utils::api::RealTimeQuota) {
    println!(
        "\n{}",
        "📈 Legacy API billing estimate (unverified; separate from ChatGPT/Codex plans)"
            .bold()
            .cyan()
    );
    println!();

    let mut table = Table::new();
    table.set_format(*format::consts::FORMAT_NO_BORDER_LINE_SEPARATOR);

    table.add_row(Row::new(vec![
        Cell::new("Account ID"),
        Cell::new(&quota.account_id),
    ]));

    table.add_row(Row::new(vec![
        Cell::new("Plan"),
        Cell::new(&quota.plan.to_uppercase()),
    ]));

    // Format as dollars
    let limit_dollars = format!("${:.2}", quota.quota_limit as f64 / 100.0);
    let usage_dollars = format!("${:.2}", quota.usage_this_month as f64 / 100.0);
    let remaining_dollars = format!("${:.2}", quota.remaining_quota as f64 / 100.0);

    table.add_row(Row::new(vec![
        Cell::new("Monthly Limit"),
        Cell::new(&limit_dollars),
    ]));

    table.add_row(Row::new(vec![
        Cell::new("Used This Month"),
        Cell::new(&usage_dollars),
    ]));

    let remaining_style = if quota.is_critical() {
        "Fr"
    } else if quota.is_low() {
        "Fy"
    } else {
        "Fg"
    };
    table.add_row(Row::new(vec![
        Cell::new("Remaining"),
        Cell::new(&remaining_dollars).style_spec(remaining_style),
    ]));

    let percent_style = if quota.is_critical() {
        "Fr"
    } else if quota.is_low() {
        "Fy"
    } else {
        "Fg"
    };
    table.add_row(Row::new(vec![
        Cell::new("Percent Used"),
        Cell::new(&format!("{:.1}%", quota.percent_used)).style_spec(percent_style),
    ]));

    if let Some(days) = quota.days_until_reset() {
        let days_text = if days > 0 {
            format!("{days} days until reset")
        } else {
            "Resets today".to_string()
        };
        table.add_row(Row::new(vec![Cell::new("Reset"), Cell::new(&days_text)]));
    }

    table.printstd();

    // Warning messages
    if quota.is_critical() {
        println!("\n{}", "⚠️  WARNING: Quota critically low!".red().bold());
        println!(
            "   Only {:.1}% remaining. Consider switching profiles.",
            100.0 - quota.percent_used
        );
    } else if quota.is_low() {
        println!("\n{}", "⚠️  Quota running low".yellow());
        println!("   {:.1}% used. Monitor usage closely.", quota.percent_used);
    }
}

/// Show usage information for all profiles
#[allow(clippy::too_many_lines)]
async fn show_all_profiles_usage(config: Config, json: bool, quiet: bool) -> Result<()> {
    let profiles_dir = config.profiles_dir();
    if !profiles_dir.exists() {
        anyhow::bail!("No profiles directory found");
    }

    let mut entries = tokio::fs::read_dir(profiles_dir).await?;
    let mut profiles = Vec::new();

    // Scan all profiles
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if name == "backups" || name.starts_with('.') {
            continue;
        }

        // Read auth.json from profile
        let meta = read_profile_meta(&path).await;
        profiles.push(build_profile_usage_row(&path, name, meta).await);
    }

    if profiles.is_empty() {
        anyhow::bail!("No profiles found");
    }

    profiles.sort_by(|a, b| {
        b.sort_score
            .cmp(&a.sort_score)
            .then_with(|| a.name.cmp(&b.name))
    });

    if json {
        println!("{}", serde_json::to_string_pretty(&profiles)?);
    } else if !quiet {
        println!("\n{}", "📊 Usage Across All Profiles".bold().cyan());
        println!();

        let mut table = Table::new();
        table.set_format(*format::consts::FORMAT_NO_BORDER_LINE_SEPARATOR);

        // Header
        table.add_row(Row::new(vec![
            Cell::new("Profile").style_spec("Fb"),
            Cell::new("Auth Mode").style_spec("Fb"),
            Cell::new("Identity").style_spec("Fb"),
            Cell::new("Access").style_spec("Fb"),
            Cell::new("Status").style_spec("Fb"),
        ]));

        for profile in profiles {
            table.add_row(Row::new(vec![
                Cell::new(&profile.name).style_spec("Fg"),
                Cell::new(auth_mode_label(&profile.auth_mode)),
                Cell::new(&profile.identity),
                Cell::new(&profile.access),
                Cell::new(&profile.status).style_spec(&profile.status_style),
            ]));
        }

        table.printstd();
        println!();
        println!(
            "{}",
            "💡 Tip: Use 'codexctl load auto' to switch toward the strongest available profile"
                .dimmed()
        );
    }

    Ok(())
}

fn display_usage_table(info: &UsageInfo) {
    println!("\n{}", "`Codex` Usage & Subscription Info".bold().cyan());
    println!();

    let mut table = Table::new();
    table.set_format(*format::consts::FORMAT_NO_BORDER_LINE_SEPARATOR);

    // Header
    table.add_row(Row::new(vec![
        Cell::new("Property").style_spec("Fb"),
        Cell::new("Value").style_spec("Fb"),
    ]));

    // Email
    table.add_row(Row::new(vec![
        Cell::new("Email"),
        Cell::new(&info.email).style_spec("Fg"),
    ]));

    // Plan Type with color coding
    let plan_style = match info.plan_type.as_str() {
        "team" => "Fb",
        "enterprise" => "Fm",
        "personal" => "Fy",
        _ => "",
    };
    table.add_row(Row::new(vec![
        Cell::new("Plan Type"),
        Cell::new(&info.plan_type.to_uppercase()).style_spec(plan_style),
    ]));

    // Account ID (truncated)
    let short_account_id = if info.account_id.len() > 12 {
        format!("{}...", &info.account_id[..12])
    } else {
        info.account_id.clone()
    };
    table.add_row(Row::new(vec![
        Cell::new("Account ID"),
        Cell::new(&short_account_id),
    ]));

    // Organizations
    if !info.organizations.is_empty() {
        let orgs = info.organizations.join(", ");
        table.add_row(Row::new(vec![Cell::new("Organizations"), Cell::new(&orgs)]));
    }

    // Subscription period
    if let (Some(start), Some(end)) = (&info.subscription_start, &info.subscription_end) {
        let start_formatted = format_date(start);
        let end_formatted = format_date(end);

        table.add_row(Row::new(vec![
            Cell::new("Subscription"),
            Cell::new(&format!("{start_formatted} to {end_formatted}")),
        ]));

        // Calculate days remaining
        if let Ok(days) = calculate_days_remaining(end) {
            let (days_text, color) = if days > 0 {
                (format!("{days} days remaining"), "Fg")
            } else {
                (format!("Expired {} days ago", days.abs()), "Fr")
            };
            table.add_row(Row::new(vec![
                Cell::new("Status"),
                Cell::new(&days_text).style_spec(color),
            ]));
        }
    }

    table.printstd();
}

fn display_subscription_status(info: &UsageInfo) {
    println!();

    match info.plan_type.to_lowercase().as_str() {
        "team" | "business" => {
            println!("{}", "✓ ChatGPT Team/Business Plan Detected".green().bold());
        }
        "enterprise" => {
            println!("{}", "✓ ChatGPT Enterprise Plan Detected".magenta().bold());
        }
        "personal" | "plus" | "pro" | "free" => {
            println!("{}", "✓ Personal ChatGPT Plan Detected".yellow().bold());
        }
        _ => {
            println!("{}", "⚠ Unknown Plan Type".yellow().bold());
        }
    }
    println!("  • Plan claims come from local `auth.json` session tokens");
    println!("  • ChatGPT/Codex plan access and API billing are separate");
}

fn display_limits_info(auth_mode: &str, has_chatgpt_claims: bool) {
    println!();
    println!("{}", "📋 Usage Limits".bold());
    match auth_mode {
        "chatgpt" => {
            if has_chatgpt_claims {
                println!(
                    "  `codexctl usage` shows ChatGPT/Codex plan claims from local auth tokens."
                );
            }
            println!("  `codexctl usage --realtime` requires API key auth.");
        }
        "api_key" => {
            println!("  API-key mode: plan claims are unavailable in `codexctl usage`.");
            println!("  `codexctl usage --realtime` queries OpenAI API billing/quota.");
        }
        "chatgpt+api_key" => {
            if has_chatgpt_claims {
                println!(
                    "  `codexctl usage` shows ChatGPT/Codex plan claims from local auth tokens."
                );
            }
            println!("  `codexctl usage --realtime` queries OpenAI API billing/quota.");
        }
        _ => {
            println!("  Unknown auth mode. Sign in with ChatGPT or API key by running `codex`.");
        }
    }
    if auth_mode_has_api_key(auth_mode) {
        println!(
            "  API limits page: {}",
            "https://platform.openai.com/settings/organization/limits"
                .cyan()
                .underline()
        );
    }
    println!();
    println!("{}", "💡 Tips:".dimmed());
    println!(
        "  {}",
        "• ChatGPT subscriptions and OpenAI API usage are billed separately".dimmed()
    );
    println!(
        "  {}",
        "• `Codex` CLI shows usage warnings when approaching limits".dimmed()
    );
    println!(
        "  {}",
        "• Run 'codexctl status' to check current profile".dimmed()
    );
    println!(
        "  {}",
        "• Use 'codexctl backup' before switching profiles".dimmed()
    );
}

fn format_date(iso_date: &str) -> String {
    // Parse ISO 8601 date and format nicely
    let parts: Vec<&str> = iso_date.split('T').collect();
    if let Some(date_part) = parts.first() {
        let date_parts: Vec<&str> = date_part.split('-').collect();
        if date_parts.len() == 3 {
            let year = date_parts[0];
            let month = match date_parts[1] {
                "01" => "Jan",
                "02" => "Feb",
                "03" => "Mar",
                "04" => "Apr",
                "05" => "May",
                "06" => "Jun",
                "07" => "Jul",
                "08" => "Aug",
                "09" => "Sep",
                "10" => "Oct",
                "11" => "Nov",
                "12" => "Dec",
                _ => date_parts[1],
            };
            let day = date_parts[2];
            return format!("{month} {day}, {year}");
        }
    }

    iso_date.to_string()
}

fn calculate_days_remaining(iso_date: &str) -> Result<i64> {
    use chrono::{DateTime, Utc};

    let end_date = DateTime::parse_from_rfc3339(iso_date)
        .map_err(|e| anyhow::anyhow!("Failed to parse date: {e}"))?;

    let now = Utc::now();
    let duration = end_date.with_timezone(&Utc) - now;

    Ok(duration.num_days())
}

#[derive(Debug, Serialize)]
struct ProfileUsageRow {
    name: String,
    auth_mode: String,
    identity: String,
    access: String,
    status: String,
    #[serde(skip_serializing)]
    status_style: String,
    #[serde(skip_serializing)]
    sort_score: i32,
}

#[derive(Debug, Serialize)]
struct UsageCommandJson {
    auth_mode: String,
    auth_mode_label: String,
    plan_claims_available: bool,
    api_realtime_available: bool,
    realtime_requested: bool,
    plan_claims: Option<UsageInfo>,
    realtime_quota: Option<crate::utils::api::RealTimeQuota>,
    realtime_error: Option<String>,
}

async fn read_profile_meta(
    profile_dir: &std::path::Path,
) -> Option<crate::utils::profile::ProfileMeta> {
    let meta_path = profile_dir.join("profile.json");
    let content = tokio::fs::read_to_string(meta_path).await.ok()?;
    serde_json::from_str(&content).ok()
}

async fn build_profile_usage_row(
    profile_dir: &std::path::Path,
    name: String,
    meta: Option<crate::utils::profile::ProfileMeta>,
) -> ProfileUsageRow {
    let auth_path = profile_dir.join("auth.json");
    let fallback_mode = meta
        .as_ref()
        .map(|m| m.auth_mode.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let fallback_identity = meta
        .as_ref()
        .and_then(|m| m.email.clone())
        .unwrap_or_else(|| "-".to_string());

    if !auth_path.exists() {
        return ProfileUsageRow {
            name,
            auth_mode: fallback_mode,
            identity: fallback_identity,
            access: "No auth data".to_string(),
            status: "Missing auth.json".to_string(),
            status_style: "Fr".to_string(),
            sort_score: 0,
        };
    }

    let auth_content = match tokio::fs::read(&auth_path).await {
        Ok(content) => content,
        Err(_) => {
            return ProfileUsageRow {
                name,
                auth_mode: fallback_mode,
                identity: fallback_identity,
                access: "Unreadable auth".to_string(),
                status: "Cannot read auth.json".to_string(),
                status_style: "Fr".to_string(),
                sort_score: 0,
            };
        }
    };

    if crate::utils::crypto::is_encrypted(&auth_content) {
        return ProfileUsageRow {
            name,
            auth_mode: fallback_mode,
            identity: fallback_identity,
            access: "Decrypt on load".to_string(),
            status: "Locked (encrypted)".to_string(),
            status_style: "Fy".to_string(),
            sort_score: 1,
        };
    }

    let auth_json: serde_json::Value = match serde_json::from_slice(&auth_content) {
        Ok(json) => json,
        Err(_) => {
            return ProfileUsageRow {
                name,
                auth_mode: fallback_mode,
                identity: fallback_identity,
                access: "Invalid auth".to_string(),
                status: "Invalid auth.json".to_string(),
                status_style: "Fr".to_string(),
                sort_score: 0,
            };
        }
    };

    let auth_mode = detect_auth_mode(&auth_json);
    let usage_info = extract_usage_info(&auth_json).ok();

    if auth_mode_has_chatgpt(&auth_mode)
        && let Some(usage) = usage_info
    {
        let days_left = usage
            .subscription_end
            .as_ref()
            .and_then(|end| {
                DateTime::parse_from_rfc3339(end)
                    .ok()
                    .map(|d| (d.with_timezone(&Utc) - Utc::now()).num_days())
            })
            .unwrap_or(0);
        let (status, status_style, sort_score) = if days_left > 7 {
            (format!("Active ({days_left}d)"), "Fg", 4)
        } else if days_left > 0 {
            (format!("Expiring soon ({days_left}d)"), "Fy", 3)
        } else {
            ("Expired".to_string(), "Fr", 2)
        };

        let access = if auth_mode_has_api_key(&auth_mode) {
            format!("{} + API key", usage.plan_type.to_uppercase())
        } else {
            usage.plan_type.to_uppercase()
        };

        return ProfileUsageRow {
            name,
            auth_mode,
            identity: usage.email,
            access,
            status,
            status_style: status_style.to_string(),
            sort_score,
        };
    }

    if auth_mode_has_api_key(&auth_mode) {
        let status = if auth_mode_has_chatgpt(&auth_mode) {
            "API ready; ChatGPT claims unavailable".to_string()
        } else {
            "API ready".to_string()
        };
        return ProfileUsageRow {
            name,
            auth_mode,
            identity: fallback_identity,
            access: "OpenAI API billing/quota".to_string(),
            status,
            status_style: "Fg".to_string(),
            sort_score: 2,
        };
    }

    ProfileUsageRow {
        name,
        auth_mode,
        identity: fallback_identity,
        access: "Unknown".to_string(),
        status: "Unknown auth mode".to_string(),
        status_style: "Fr".to_string(),
        sort_score: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_build_profile_usage_row_api_key_profile() {
        let temp_dir = TempDir::new().unwrap();
        tokio::fs::write(
            temp_dir.path().join("auth.json"),
            r#"{"api_key":"sk-test"}"#,
        )
        .await
        .unwrap();

        let row = build_profile_usage_row(temp_dir.path(), "api".to_string(), None).await;
        assert_eq!(row.auth_mode, "api_key");
        assert_eq!(row.access, "OpenAI API billing/quota");
        assert_eq!(row.status, "API ready");
    }

    #[tokio::test]
    async fn test_build_profile_usage_row_encrypted_profile_uses_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let mut profile = crate::utils::profile::Profile::new(
            "encrypted".to_string(),
            Some("hidden@example.com".to_string()),
            None,
        );
        profile.meta.auth_mode = "chatgpt".to_string();
        profile.add_file(
            "auth.json",
            br#"{"tokens":{"id_token":"header.payload.signature"}}"#.to_vec(),
        );
        profile
            .save_to_disk_encrypted(temp_dir.path(), Some(&"secret".to_string()))
            .unwrap();
        let meta = read_profile_meta(temp_dir.path()).await;

        let row = build_profile_usage_row(temp_dir.path(), "encrypted".to_string(), meta).await;
        assert_eq!(row.status, "Locked (encrypted)");
        assert_eq!(row.identity, "hidden@example.com");
    }

    #[tokio::test]
    async fn test_build_profile_usage_row_hybrid_profile() {
        let temp_dir = TempDir::new().unwrap();
        let auth = make_chatgpt_auth("team", 30, true);
        tokio::fs::write(temp_dir.path().join("auth.json"), auth.to_string())
            .await
            .unwrap();

        let row = build_profile_usage_row(temp_dir.path(), "hybrid".to_string(), None).await;
        assert_eq!(row.auth_mode, "chatgpt+api_key");
        assert_eq!(row.identity, "user@example.com");
        assert_eq!(row.access, "TEAM + API key");
    }

    fn make_chatgpt_auth(
        plan: &str,
        days_until_expiry: i64,
        with_api_key: bool,
    ) -> serde_json::Value {
        use chrono::Duration;

        let now = Utc::now();
        let payload = serde_json::json!({
            "email": "user@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": plan,
                "chatgpt_subscription_active_start": now.to_rfc3339(),
                "chatgpt_subscription_active_until": (now + Duration::days(days_until_expiry)).to_rfc3339(),
                "chatgpt_account_id": "acct_test",
                "organizations": []
            }
        });
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
        let token = format!("{header}.{payload}.sig");

        if with_api_key {
            serde_json::json!({
                "tokens": { "id_token": token },
                "api_key": "sk-test"
            })
        } else {
            serde_json::json!({
                "tokens": { "id_token": token }
            })
        }
    }
}
