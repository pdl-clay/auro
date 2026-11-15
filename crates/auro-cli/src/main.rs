// SPDX-License-Identifier: MIT OR Apache-2.0

mod dev;
mod doctor;
mod project;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "auro",
    version,
    about = "Native Rust desktop apps: Tauri's app model, GPUI's renderer"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new Auro app
    New {
        /// Directory to create
        path: PathBuf,
    },
    /// Build, run and watch the app
    Dev {
        /// App directory (defaults to the current one)
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Override a config key, e.g. `app.product-name=Demo`
        #[arg(long = "config", value_name = "KEY=VALUE")]
        overrides: Vec<String>,
    },
    /// Build the app in release mode
    Build {
        /// App directory (defaults to the current one)
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Report toolchain, config and platform status
    Doctor {
        /// App directory (defaults to the current one)
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();
    let current = || std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let result = match cli.command {
        Command::New { path } => project::new(&path),
        Command::Dev { dir, overrides } => dev::run(dir.unwrap_or_else(current), &overrides),
        Command::Build { dir } => project::build(&dir.unwrap_or_else(current)),
        Command::Doctor { dir } => doctor::run(&dir.unwrap_or_else(current)),
    };

    if let Err(error) = result {
        eprintln!("auro: {error}");
        std::process::exit(1);
    }
}
