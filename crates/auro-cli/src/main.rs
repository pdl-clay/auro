// SPDX-License-Identifier: MIT OR Apache-2.0

mod bench;
mod dev;
mod doctor;
mod project;
mod release;

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
    /// Build the app and produce its installers
    Bundle {
        /// App directory (defaults to the current one)
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Generate an Ed25519 release signing key
    GenerateKey {
        /// Key file to create
        #[arg(long)]
        out: PathBuf,
    },
    /// Create a signed update manifest for an artifact
    Manifest {
        /// Artifact to describe
        #[arg(long)]
        artifact: PathBuf,
        /// Release version
        #[arg(long)]
        version: String,
        /// URL where the artifact will be served
        #[arg(long)]
        url: String,
        /// Signing key file
        #[arg(long)]
        key: PathBuf,
        /// Manifest file to write
        #[arg(long)]
        out: PathBuf,
        /// Release notes
        #[arg(long, default_value = "")]
        notes: String,
        /// Platform key (defaults to the current host)
        #[arg(long)]
        platform: Option<String>,
        /// Publication timestamp (RFC 3339 or any stable text)
        #[arg(long)]
        published_at: String,
    },
    /// Measure startup, idle RSS and binary size for an app
    Bench {
        /// App directory (defaults to the current one)
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Number of process runs
        #[arg(long, default_value_t = 5)]
        runs: u32,
        /// Seconds to wait before sampling idle RSS
        #[arg(long, default_value_t = 5)]
        idle_seconds: u64,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
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
        Command::Bundle { dir } => project::bundle(&dir.unwrap_or_else(current)),
        Command::GenerateKey { out } => release::generate_key(&out),
        Command::Manifest {
            artifact,
            version,
            url,
            key,
            out,
            notes,
            platform,
            published_at,
        } => release::manifest(
            &artifact,
            &version,
            &url,
            &key,
            &out,
            &notes,
            platform.as_deref(),
            &published_at,
        ),
        Command::Bench {
            dir,
            runs,
            idle_seconds,
            json,
        } => bench::run(&dir.unwrap_or_else(current), runs, idle_seconds, json),
        Command::Doctor { dir } => doctor::run(&dir.unwrap_or_else(current)),
    };

    if let Err(error) = result {
        eprintln!("auro: {error}");
        std::process::exit(1);
    }
}
