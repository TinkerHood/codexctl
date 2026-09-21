use crate::utils::config::Config;
use anyhow::Result;
use chrono::{DateTime, Utc};
use colored::Colorize as _;
use prettytable::{Cell, Row, Table, format};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: DateTime<Utc>,
    pub profile: String,
    pub command: String,
    pub duration_secs: Option<u64>,
}

pub async fn execute(
    config: Config,
    limit: usize,
    profile_filter: Option<String>,
    quiet: bool,
) -> Result<()> {
    let history_file = config.profiles_dir().join(".history.jsonl");

    if !history_file.exists() {
        if !quiet {
            println!(
                "{} No history yet. Use profiles to build history.",
                "ℹ".blue()
            );
        }
        return Ok(());
    }

    let content = tokio::fs::read_to_string(&history_file).await?;
    let mut entries: Vec<HistoryEntry> = Vec::new();

    for line in content.lines() {
        if let Ok(entry) = serde_json::from_str::<HistoryEntry>(line) {
            entries.push(entry);
        }
    }

    // Sort by timestamp (newest first)
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.timestamp));

    // Filter by profile if specified
    if let Some(ref profile) = profile_filter {
        entries.retain(|e| e.profile == *profile);
    }

    // Limit results
    entries.truncate(limit);

    if entries.is_empty() {
        if !quiet {
            println!("{} No matching history entries found.", "ℹ".blue());
        }
        return Ok(());
    }

    if !quiet {
        println!(
            "\n{} Command History (last {} entries)\n",
            "📜".cyan(),
            entries.len()
        );

        let mut table = Table::new();
        table.set_format(*format::consts::FORMAT_NO_BORDER_LINE_SEPARATOR);

        table.add_row(Row::new(vec![
            Cell::new("Time").style_spec("Fb"),
            Cell::new("Profile").style_spec("Fb"),
            Cell::new("Command").style_spec("Fb"),
        ]));

        for entry in &entries {
            let time_str = entry.timestamp.format("%Y-%m-%d %H:%M").to_string();

            table.add_row(Row::new(vec![
                Cell::new(&time_str),
                Cell::new(&entry.profile).style_spec("Fg"),
                Cell::new(&entry.command).style_spec("Fy"),
            ]));
        }

        table.printstd();
        println!();
    }

    Ok(())
}

/// Log a command to history
pub async fn log_command(config: &Config, profile: &str, command: &str) -> Result<()> {
    let history_file = config.profiles_dir().join(".history.jsonl");

    let entry = HistoryEntry {
        timestamp: Utc::now(),
        profile: profile.to_string(),
        command: command.to_string(),
        duration_secs: None,
    };

    let line = serde_json::to_string(&entry)?;

    // Append to file
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&history_file)
        .await?;

    file.write_all(line.as_bytes()).await?;
    file.write_all(b"\n").await?;
    file.flush().await?;

    Ok(())
}
