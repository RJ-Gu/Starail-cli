use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "starail",
    version,
    about = "A small Linux terminal client for managing a local mihomo core.",
    long_about = "Starail manages a local mihomo core, Clash-compatible profiles, subscriptions, proxy groups, and shell proxy environment settings."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Open the interactive terminal UI.
    Tui,
    /// Start mihomo with a config file or the active profile.
    Start {
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Stop the managed mihomo process.
    Stop,
    /// Restart mihomo with the active profile.
    Restart,
    /// Show process, port, active profile, and controller state.
    Status,
    /// Print recent mihomo logs.
    Logs {
        #[arg(short, long, default_value_t = 80)]
        lines: usize,
    },
    /// Validate a Clash/mihomo config with the local core.
    Check {
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Download, update, or inspect the local mihomo core.
    Core {
        #[command(subcommand)]
        command: CoreCommand,
    },
    /// Manage local and subscription-backed profiles.
    Profile {
        #[command(subcommand)]
        command: Option<ProfileCommand>,
    },
    /// Add or refresh subscription-backed profiles.
    Subscribe {
        #[command(subcommand)]
        command: SubscribeCommand,
    },
    /// List, test, and switch proxies through the mihomo controller.
    Proxy {
        #[command(subcommand)]
        command: Option<ProxyCommand>,
    },
    /// Change mihomo proxy mode.
    Mode {
        #[arg(value_enum)]
        mode: Mode,
    },
    /// Manage Starail-owned shell proxy variables in ~/.bashrc.
    #[command(name = "system-proxy")]
    SystemProxy {
        #[command(subcommand)]
        command: Option<SystemProxyCommand>,
    },
}

#[derive(Debug, Subcommand)]
pub enum CoreCommand {
    Install,
    Update,
    Version,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ProfileCommand {
    List,
    Add {
        name: String,
        config: PathBuf,
    },
    Use {
        name: String,
    },
    #[command(alias = "rm", alias = "delete")]
    Remove {
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum SubscribeCommand {
    Add { url: String, name: Option<String> },
    Update { name: Option<String> },
}

#[derive(Debug, Clone, Subcommand)]
pub enum ProxyCommand {
    List,
    Test,
    Select { group: String, node: String },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "lower")]
pub enum Mode {
    Rule,
    Global,
    Direct,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Global => "global",
            Self::Direct => "direct",
        }
    }
}

#[derive(Debug, Clone, Subcommand)]
pub enum SystemProxyCommand {
    On,
    Off,
    Status,
}
