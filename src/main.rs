//! FileCleaner CLI entrypoint (Wave 1 minimal scaffold).

use clap::{Parser, Subcommand};
use filecleaner::Config;
use std::path::PathBuf;
use std::process;

#[derive(Debug, Parser)]
#[command(
    name = "filecleaner",
    version,
    about = "Deterministic Rust directory retention, backup safety guardrails, and lifecycle cleaner"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Validate a TOML or YAML FileCleaner configuration file.
    Validate {
        /// Path to the configuration file (.toml, .yaml, or .yml).
        #[arg(short, long)]
        config: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Validate { config } => match Config::load_validated(&config) {
            Ok(loaded) => {
                println!(
                    "configuration valid: {} rule(s) loaded from {}",
                    loaded.rules.len(),
                    config.display()
                );
            }
            Err(err) => {
                eprintln!("error: {err}");
                process::exit(1);
            }
        },
    }
}
