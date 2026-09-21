//! `CodexCTL` - Codex Controller

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueHint};
use tracing::{debug, info};

mod commands;
mod utils;

use commands::{backup, delete, list, load, run, save, status};
use utils::config::Config;

/// `CodexCTL` - Codex Controller
#[derive(Parser, Debug, Clone)]
#[command(
    name = "codexctl",
    bin_name = "codexctl",
    version,
    about = "Codex Controller - Full control plane for Codex CLI",
    long_about = None
)]
#[command(arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Config directory override
    #[arg(long, global = true, env = "CODEXCTL_DIR", value_hint = ValueHint::DirPath)]
    config_dir: Option<PathBuf>,

    /// Quiet mode (minimal output)
    #[arg(short, long, global = true, env = "CODEXCTL_QUIET")]
    quiet: bool,
}

/// Available CLI commands
#[derive(Subcommand, Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Commands {
    /// Save current Codex auth as a named profile
    #[command(alias = "s")]
    Save {
        /// Profile name
        name: String,
        /// Optional description
        #[arg(short, long)]
        description: Option<String>,
        /// Force overwrite if exists
        #[arg(short, long)]
        force: bool,
        /// Encrypt profile with passphrase (optional, leave empty for no encryption)
        #[arg(short = 'p', long, env = "CODEXCTL_PASSPHRASE")]
        passphrase: Option<String>,
    },

    /// Load a saved profile and switch to it
    #[command(alias = "l")]
    Load {
        /// Profile name
        name: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
        /// Dry run (show what would happen without doing it)
        #[arg(long)]
        dry_run: bool,
        /// Passphrase to decrypt profile (if encrypted)
        #[arg(short = 'p', long, env = "CODEXCTL_PASSPHRASE")]
        passphrase: Option<String>,
    },

    /// List all saved profiles
    #[command(alias = "ls")]
    List {
        /// Show detailed information
        #[arg(short, long)]
        detailed: bool,
    },

    /// Delete a saved profile
    #[command(alias = "rm", alias = "remove")]
    Delete {
        /// Profile name
        name: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },

    /// Show current profile status
    #[command(alias = "st", alias = "current")]
    Status {
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
    },

    /// Show ChatGPT/Codex plan claims and API usage context
    #[command(alias = "u")]
    Usage {
        /// Show usage for all profiles
        #[arg(short, long)]
        all: bool,
        /// Fetch real-time quota from `OpenAI` API (API billing is separate from ChatGPT/Codex plans)
        #[arg(short, long)]
        realtime: bool,
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
    },

    /// Verify all profiles' authentication status
    #[command(alias = "v")]
    Verify {
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
    },

    /// Create a backup of current profile
    #[command(alias = "b")]
    Backup {
        /// Custom backup name
        #[arg(short, long)]
        name: Option<String>,
    },

    /// Run a command with a specific profile (auto-switches back after)
    #[command(alias = "r")]
    Run {
        /// Profile name to use
        #[arg(short, long)]
        profile: String,
        /// Passphrase to decrypt profile (if encrypted)
        #[arg(short = 'P', long, env = "CODEXCTL_PASSPHRASE")]
        passphrase: Option<String>,
        /// Command to run
        #[arg(required = true)]
        command: Vec<String>,
    },

    /// Export shell commands to use a profile (for concurrent usage)
    #[command(alias = "e")]
    Env {
        /// Profile name
        profile: String,
        /// Shell type (bash, zsh, fish)
        #[arg(short, long, default_value = "bash")]
        shell: String,
        /// Print unset commands to clear
        #[arg(long)]
        unset: bool,
    },

    /// Compare/diff two profiles
    #[command(alias = "d")]
    Diff {
        /// First profile
        profile1: String,
        /// Second profile
        profile2: String,
        /// Show only differences
        #[arg(short, long)]
        changes_only: bool,
    },

    /// Switch to a profile interactively (fzf)
    #[command(alias = "sw")]
    Switch,

    /// View command history
    #[command(alias = "hist")]
    History {
        /// Number of entries to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Show only for specific profile
        #[arg(short, long)]
        profile: Option<String>,
    },

    /// Run health check on profiles
    #[command(alias = "doc")]
    Doctor {
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
    },

    /// Generate shell completions
    #[command(alias = "comp")]
    Completions {
        /// Shell type
        #[arg(value_enum)]
        shell: ShellType,
        /// Print to stdout instead of installing
        #[arg(short, long)]
        print: bool,
    },

    /// Import a profile from another machine (base64 encoded)
    Import {
        /// Profile name
        name: String,
        /// Base64 encoded profile data
        data: String,
    },

    /// Export a profile for transfer to another machine
    Export {
        /// Profile name
        name: String,
    },

    /// Interactive setup wizard
    #[command(alias = "init")]
    Setup,
}

/// Supported shell types for completions
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ShellType {
    /// Bash shell
    Bash,
    /// Zsh shell
    Zsh,
    /// Fish shell
    Fish,
    /// `PowerShell`
    PowerShell,
    /// Elvish shell
    Elvish,
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging with modern Rust 2024 formatting
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| if cli.verbose { "debug" } else { "warn" }.into()),
        )
        .with_target(false)
        .with_level(true)
        .with_writer(std::io::stderr)
        .init();

    debug!("Starting codexctl");
    info!("Config directory: {:?}", cli.config_dir);

    let config = Config::new(cli.config_dir.clone())?;

    // Auto-migrate profiles on startup (silent, no user intervention)
    if let Err(e) = crate::utils::migrate::auto_migrate(&config).await {
        tracing::warn!("Auto-migration warning: {}", e);
    }

    match cli.command {
        Commands::Save {
            name,
            description,
            force,
            passphrase,
        } => {
            save::execute(config, name, description, force, cli.quiet, passphrase).await?;
        }
        Commands::Load {
            name,
            force,
            dry_run,
            passphrase,
        } => {
            load::execute(config, name, force, dry_run, cli.quiet, passphrase).await?;
        }

        Commands::List { detailed } => {
            list::execute(config, detailed, cli.quiet).await?;
        }
        Commands::Delete { name, force } => {
            delete::execute(config, name, force, cli.quiet).await?;
        }
        Commands::Status { json } => {
            status::execute(config, json, cli.quiet).await?;
        }
        Commands::Usage {
            all,
            realtime,
            json,
        } => {
            commands::usage::execute(config, all, realtime, json, cli.quiet).await?;
        }
        Commands::Verify { json } => {
            commands::verify::execute(config, json, cli.quiet).await?;
        }
        Commands::Backup { name } => {
            backup::execute(config, name, cli.quiet)?;
        }
        Commands::Run {
            profile,
            passphrase,
            command,
        } => {
            let status = run::execute(config, profile, passphrase, command, cli.quiet).await?;
            if !status.success() {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt as _;
                    std::process::exit(
                        status
                            .code()
                            .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)),
                    );
                }
                #[cfg(not(unix))]
                std::process::exit(status.code().unwrap_or(1));
            }
        }
        Commands::Env {
            profile,
            shell,
            unset,
        } => {
            commands::env::execute(config, profile, shell, unset, cli.quiet)?;
        }
        Commands::Diff {
            profile1,
            profile2,
            changes_only,
        } => {
            commands::diff::execute(config, profile1, profile2, changes_only, cli.quiet).await?;
        }
        Commands::Switch => {
            commands::switch::execute(config, cli.quiet).await?;
        }
        Commands::History { limit, profile } => {
            commands::history::execute(config, limit, profile, cli.quiet).await?;
        }
        Commands::Doctor { json } => {
            commands::doctor::execute(config, json, cli.quiet).await?;
        }
        Commands::Completions { shell, print } => {
            let shell_str = match shell {
                ShellType::Bash => "bash",
                ShellType::Zsh => "zsh",
                ShellType::Fish => "fish",
                ShellType::PowerShell => "powershell",
                ShellType::Elvish => "elvish",
            };

            if print {
                let output = commands::completions::generate_completions(shell_str)?;
                println!("{output}");
            } else {
                commands::completions::install_completions(shell_str)?;
            }
        }
        Commands::Import { name, data } => {
            commands::import::execute(config, name, data, cli.quiet).await?;
        }
        Commands::Export { name } => {
            commands::export::execute(config, name, cli.quiet).await?;
        }
        Commands::Setup => {
            commands::setup::execute(config, cli.quiet).await?;
        }
    }

    Ok(())
}
