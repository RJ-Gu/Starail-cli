mod cli;
mod config;
mod controller;
mod core;
mod paths;
mod platform;
mod profile;
mod proxy;
mod runtime;
mod status;
mod subscription;
mod system_proxy;
mod tui;
mod util;

use anyhow::Result;
use clap::Parser;

use crate::cli::{
    Cli, Command, CoreCommand, ProfileCommand, ProxyCommand, SubscribeCommand, SystemProxyCommand,
};
use crate::paths::StarailPaths;

fn main() {
    if let Err(error) = run() {
        eprintln!("starail: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let paths = StarailPaths::new()?;

    match cli.command.unwrap_or(Command::Tui) {
        Command::Tui => tui::run(&paths),
        Command::Start { config } => runtime::start(&paths, config.as_deref()),
        Command::Stop => runtime::stop(&paths),
        Command::Restart => runtime::restart(&paths),
        Command::Status => status::print(&paths),
        Command::Logs { lines } => runtime::logs(&paths, lines),
        Command::Check { config } => runtime::check(&paths, &config),
        Command::Core { command } => match command {
            CoreCommand::Install | CoreCommand::Update => core::install(&paths),
            CoreCommand::Version => core::version(&paths),
        },
        Command::Profile { command } => match command.unwrap_or(ProfileCommand::List) {
            ProfileCommand::List => profile::print_list(&paths),
            ProfileCommand::Add { name, config } => profile::add_local(&paths, &name, &config),
            ProfileCommand::Use { name } => profile::use_profile(&paths, &name),
            ProfileCommand::Remove { name } => profile::remove(&paths, &name),
        },
        Command::Subscribe { command } => match command {
            SubscribeCommand::Add { url, name } => subscription::add(&paths, &url, name.as_deref()),
            SubscribeCommand::Update { name } => subscription::update(&paths, name.as_deref()),
        },
        Command::Proxy { command } => match command.unwrap_or(ProxyCommand::List) {
            ProxyCommand::List => proxy::list(&paths),
            ProxyCommand::Test => proxy::test(&paths),
            ProxyCommand::Select { group, node } => proxy::select(&paths, &group, &node),
        },
        Command::Mode { mode } => controller::set_mode(&paths, mode.as_str()),
        Command::SystemProxy { command } => match command.unwrap_or(SystemProxyCommand::Status) {
            SystemProxyCommand::On => system_proxy::on(&paths),
            SystemProxyCommand::Off => system_proxy::off(&paths),
            SystemProxyCommand::Status => system_proxy::status(&paths),
        },
    }
}
