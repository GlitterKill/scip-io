pub mod clean;
pub mod detect;
pub mod index;
pub mod merge;
pub mod progress_handler;
pub mod status;
pub mod update_registry;
pub mod validate;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "scip-io",
    about = "SCIP Index Orchestrator — detect, install, run, and merge SCIP indexers",
    version
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Suppress GUI launch and show help instead
    #[arg(long)]
    pub no_gui: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Scan project for languages
    Detect(DetectArgs),
    /// Download indexers, run them, and merge SCIP output
    Index(IndexArgs),
    /// Show installed indexers and their versions
    Status(StatusArgs),
    /// Merge multiple .scip index files into one
    Merge(MergeArgs),
    /// Remove cached indexer binaries
    Clean(CleanArgs),
    /// Validate a .scip index file
    Validate(ValidateArgs),
    /// Launch the GUI (not yet implemented)
    Gui(GuiArgs),
    /// Update the indexer registry from remote (not yet implemented)
    UpdateRegistry(UpdateRegistryArgs),
}

#[derive(Parser)]
pub struct DetectArgs {
    /// Project root directory (defaults to current directory)
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Output format: text or json
    #[arg(short, long, default_value = "text")]
    pub format: String,

    /// Maximum directory depth to scan
    #[arg(short, long)]
    pub depth: Option<usize>,
}

#[derive(Parser)]
pub struct IndexArgs {
    /// Project root directory (defaults to current directory)
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Only index specific language(s), comma-separated
    #[arg(short, long, value_delimiter = ',')]
    pub lang: Vec<String>,

    /// Output file for the merged SCIP index
    #[arg(short, long, default_value = "index.scip")]
    pub output: PathBuf,

    /// Skip merging (keep individual .scip files)
    #[arg(long)]
    pub no_merge: bool,

    /// Number of parallel indexer invocations
    #[arg(long)]
    pub parallel: Option<u32>,

    /// Timeout in seconds for each indexer run
    #[arg(long)]
    pub timeout: Option<u64>,

    /// Output format: text or json
    #[arg(short, long, default_value = "text")]
    pub format: String,

    /// Show what would be done without actually running indexers
    #[arg(long)]
    pub dry_run: bool,

    /// Explicit sub-project roots to index (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub roots: Vec<PathBuf>,

    /// Discover and index all sub-project roots in a monorepo
    #[arg(long)]
    pub all_roots: bool,
}

#[derive(Parser)]
pub struct StatusArgs {
    /// Show verbose details about each indexer
    #[arg(short, long)]
    pub verbose: bool,

    /// Output format: text or json
    #[arg(short, long, default_value = "text")]
    pub format: String,

    /// Check for available updates (placeholder)
    #[arg(long)]
    pub check_updates: bool,
}

#[derive(Parser)]
pub struct MergeArgs {
    /// Input .scip files to merge
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,

    /// Output file path
    #[arg(short, long, default_value = "index.scip")]
    pub output: PathBuf,

    /// Validate the merged output after writing
    #[arg(long)]
    pub validate: bool,
}

#[derive(Parser)]
pub struct CleanArgs {
    /// Only remove indexer for the specified language
    #[arg(short, long)]
    pub lang: Option<String>,

    /// Remove entire cache directory
    #[arg(long)]
    pub all: bool,

    /// Show what would be removed without deleting anything
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Parser)]
pub struct ValidateArgs {
    /// Path to the .scip index file to validate
    pub input: PathBuf,

    /// Output format: text or json
    #[arg(short, long)]
    pub format: Option<String>,
}

#[derive(Parser)]
pub struct GuiArgs {
    /// Project root directory (defaults to current directory)
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Port for the GUI server
    #[arg(long, default_value = "3120")]
    pub port: u16,
}

#[derive(Parser)]
pub struct UpdateRegistryArgs {
    /// URL to fetch registry from
    #[arg(long)]
    pub url: Option<String>,

    /// Force update even if registry is up to date
    #[arg(long)]
    pub force: bool,
}
