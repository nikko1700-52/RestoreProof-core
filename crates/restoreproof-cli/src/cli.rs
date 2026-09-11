//! Command line surface.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// Prove that your application can actually be restored and restarted.
#[derive(Debug, Parser)]
#[command(
    name = "restoreproof",
    version,
    about = "Prove that your application can actually be restored and restarted.",
    long_about = "RestoreProof runs a recovery drill: it restores a backup into an isolated \
                  environment, starts your services from it, checks that the application answers \
                  and that the data is there, measures how long it took, and writes a report you \
                  can show to someone.\n\n\
                  It is a verification tool. It never writes to your backups and never touches \
                  your production environment.",
    propagate_version = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Global options, accepted by every subcommand.
    #[command(flatten)]
    pub global: GlobalArgs,

    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Options accepted by every subcommand.
#[derive(Debug, Clone, clap::Args)]
pub struct GlobalArgs {
    /// Path to the scenario configuration.
    #[arg(
        short = 'c',
        long,
        global = true,
        value_name = "PATH",
        default_value = "restoreproof.yaml"
    )]
    pub config: PathBuf,

    /// How to render the result.
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Terminal)]
    pub format: OutputFormat,

    /// Write the rendered result to this file instead of standard output.
    #[arg(short = 'o', long, global = true, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Print more detail. Repeat for trace-level logs.
    #[arg(short = 'v', long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Print nothing but errors.
    #[arg(short = 'q', long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Keep the recovery environment and the restored data after the drill.
    ///
    /// They hold a copy of whatever was in the backup. Remove them yourself.
    #[arg(long, global = true)]
    pub keep_environment: bool,

    /// Do not destroy the recovery environment at the end of the drill.
    #[arg(long, global = true)]
    pub no_cleanup: bool,

    /// Override the total time budget of the drill, in seconds.
    #[arg(long, global = true, value_name = "SECONDS")]
    pub timeout: Option<u64>,

    /// Show what would happen without restoring or starting anything.
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Never emit ANSI colour, whatever the terminal says.
    ///
    /// The `NO_COLOR` environment variable has the same effect, following the
    /// convention that any non-empty value disables colour.
    #[arg(long, global = true)]
    pub no_color: bool,
}

/// Output rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable summary for a terminal.
    Terminal,
    /// Machine-readable JSON.
    Json,
    /// Markdown, suitable for a ticket.
    Markdown,
}

/// Subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a ready-to-run example scenario in a directory.
    Init {
        /// Directory to create. It must not already contain a scenario.
        #[arg(value_name = "DIRECTORY")]
        directory: PathBuf,

        /// Overwrite existing files.
        #[arg(long)]
        force: bool,
    },

    /// Check the configuration without running anything.
    Validate,

    /// Show what a drill would do, without doing it.
    Plan,

    /// Run the drill: restore, start, check, measure, report.
    Run,

    /// Run only the checks, against an environment that is already running.
    Check {
        /// Directory holding already-restored data, for `file` checks.
        #[arg(long, value_name = "PATH")]
        restore_dir: Option<PathBuf>,
    },

    /// Display a report that was written earlier.
    Report {
        /// Report file. JSON reports can also be verified.
        #[arg(value_name = "FILE")]
        file: PathBuf,

        /// Recompute the integrity digest and compare it with the recorded one.
        #[arg(long)]
        verify: bool,
    },

    /// Print version, build and platform information.
    Version,
}
