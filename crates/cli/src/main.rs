//! `rekindle`: build, run, sign and install sandboxed plugins.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod cmd;

#[derive(Parser)]
#[command(
    name = "rekindle",
    version,
    about = "Build, preview, sign and install sandboxed rekindle plugins"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    #[command(flatten)]
    Author(cmd::author::Commands),
    #[command(flatten)]
    Community(cmd::community::Commands),
    #[command(flatten)]
    Dev(cmd::dev::Commands),
    #[command(flatten)]
    Plugins(cmd::plugins::Commands),
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,rekindle_plugin_host::runtime=off".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().cmd {
        Cmd::Author(c) => cmd::author::run(c),
        Cmd::Community(c) => cmd::community::run(c),
        Cmd::Dev(c) => cmd::dev::run(c),
        Cmd::Plugins(c) => cmd::plugins::run(c),
    }
}
