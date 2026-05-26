pub mod clean;
pub mod detect;
pub mod index;
pub mod indexer_target;
pub mod install;
pub mod merge;
pub mod progress_handler;
pub mod status;
pub mod uninstall;
pub mod update;
pub mod update_registry;
pub mod validate;

use anyhow::{Result, anyhow};
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
    /// Install one SCIP indexer
    Install(InstallArgs),
    /// Uninstall one SCIP indexer from the SCIP-IO managed cache
    Uninstall(UninstallArgs),
    /// Check for indexer updates and optionally install them
    Update(UpdateArgs),
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
    #[arg(long, value_delimiter = ',', conflicts_with = "all_roots")]
    pub roots: Vec<PathBuf>,

    /// Discover and index all sub-project roots in a monorepo
    #[arg(long, conflicts_with = "roots")]
    pub all_roots: bool,

    /// Include extra language config files supported by each indexer
    #[arg(long)]
    pub include_additional_configs: bool,
}

#[derive(Parser)]
pub struct StatusArgs {
    /// Show verbose details about each indexer
    #[arg(short, long)]
    pub verbose: bool,

    /// Output format: text or json
    #[arg(short, long, default_value = "text")]
    pub format: String,

    /// Check for available updates
    #[arg(long)]
    pub check_updates: bool,
}

#[derive(Parser)]
pub struct InstallArgs {
    /// Language, indexer name, or binary name to install
    #[arg(value_name = "TARGET", conflicts_with = "lang")]
    pub target: Option<String>,

    /// Language, indexer name, or binary name to install
    #[arg(short, long)]
    pub lang: Option<String>,
}

impl InstallArgs {
    pub fn target_identifier(&self) -> Result<&str> {
        one_target(self.target.as_deref(), self.lang.as_deref(), "install")
    }
}

#[derive(Parser)]
pub struct UninstallArgs {
    /// Language, indexer name, or binary name to uninstall
    #[arg(value_name = "TARGET", conflicts_with = "lang")]
    pub target: Option<String>,

    /// Language, indexer name, or binary name to uninstall
    #[arg(short, long)]
    pub lang: Option<String>,

    /// Show what would be removed without deleting anything
    #[arg(long)]
    pub dry_run: bool,
}

impl UninstallArgs {
    pub fn target_identifier(&self) -> Result<&str> {
        one_target(self.target.as_deref(), self.lang.as_deref(), "uninstall")
    }
}

#[derive(Parser)]
pub struct UpdateArgs {
    /// Optional language, indexer name, or binary name to update
    #[arg(value_name = "TARGET", conflicts_with_all = ["lang", "all"])]
    pub target: Option<String>,

    /// Update a specific language, indexer name, or binary name without a menu
    #[arg(short, long, conflicts_with = "all")]
    pub lang: Option<String>,

    /// Update every installed managed indexer with an available newer version
    #[arg(long)]
    pub all: bool,
}

impl UpdateArgs {
    pub fn target_identifier(&self) -> Option<&str> {
        self.target.as_deref().or(self.lang.as_deref())
    }
}

fn one_target<'a>(
    positional: Option<&'a str>,
    lang: Option<&'a str>,
    command: &str,
) -> Result<&'a str> {
    positional
        .or(lang)
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "{} requires a language, indexer name, or binary name",
                command
            )
        })
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

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use clap::Parser;

    #[test]
    fn parses_interactive_update_command() {
        let cli = Cli::try_parse_from(["scip-io", "update"]).unwrap();

        let Some(Command::Update(args)) = cli.command else {
            panic!("expected update command");
        };

        assert!(args.target.is_none());
        assert!(args.lang.is_none());
        assert!(!args.all);
    }

    #[test]
    fn parses_non_interactive_update_language_argument() {
        let cli = Cli::try_parse_from(["scip-io", "update", "--lang", "rust"]).unwrap();

        let Some(Command::Update(args)) = cli.command else {
            panic!("expected update command");
        };

        assert_eq!(args.lang.as_deref(), Some("rust"));
        assert!(!args.all);
    }

    #[test]
    fn parses_update_all_argument() {
        let cli = Cli::try_parse_from(["scip-io", "update", "--all"]).unwrap();

        let Some(Command::Update(args)) = cli.command else {
            panic!("expected update command");
        };

        assert!(args.all);
        assert!(args.target.is_none());
    }

    #[test]
    fn parses_install_target_argument() {
        let cli = Cli::try_parse_from(["scip-io", "install", "python"]).unwrap();

        let Some(Command::Install(args)) = cli.command else {
            panic!("expected install command");
        };

        assert_eq!(args.target.as_deref(), Some("python"));
        assert!(args.lang.is_none());
    }

    #[test]
    fn parses_uninstall_target_argument() {
        let cli = Cli::try_parse_from(["scip-io", "uninstall", "scip-python"]).unwrap();

        let Some(Command::Uninstall(args)) = cli.command else {
            panic!("expected uninstall command");
        };

        assert_eq!(args.target.as_deref(), Some("scip-python"));
        assert!(!args.dry_run);
    }
}
