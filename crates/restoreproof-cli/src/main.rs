//! `RestoreProof` Core command line interface.
//!
//! The binary is a thin shell: it parses arguments, sets up logging and output,
//! and delegates to the library crates. Everything a future front-end would
//! need — a scheduler, an HTTP API, a web console — lives in those crates, not
//! here.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]
// This is a binary crate: `pub` items are module-level API within the binary,
// never exported, so `unreachable_pub` has nothing useful to say here.
#![allow(unreachable_pub)]
// The CLI is the one place that is *supposed* to write to the terminal.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod cli;
mod commands;
mod output;
mod scaffold;

use clap::Parser as _;
use restoreproof_core::ExitCode;
use restoreproof_runner::RunMode;

use crate::cli::{Cli, Command};
use crate::output::Output;

fn main() -> std::process::ExitCode {
    let code = dispatch();
    let numeric = u8::try_from(code.code()).unwrap_or(6);
    std::process::ExitCode::from(numeric)
}

/// Parse arguments and run the requested subcommand.
fn dispatch() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            // `--help` and `--version` arrive here too; they are not failures.
            let is_help = matches!(
                err.kind(),
                clap::error::ErrorKind::DisplayHelp
                    | clap::error::ErrorKind::DisplayVersion
                    | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            );
            let _ = err.print();
            return if is_help {
                ExitCode::Success
            } else {
                ExitCode::Usage
            };
        }
    };

    output::init_tracing(&cli.global);
    let out = Output::new(&cli.global);

    match cli.command {
        Command::Init { directory, force } => commands::init::run(&out, &directory, force),
        Command::Validate => commands::validate::run(&cli.global, &out),
        Command::Plan => commands::plan::run(&cli.global, &out),
        Command::Version => commands::version::run(&cli.global, &out),
        Command::Report { file, verify } => commands::report::run(&cli.global, &out, &file, verify),
        Command::Run => block_on(
            commands::run::run(&cli.global, &out, RunMode::Full, None),
            &out,
        ),
        Command::Check { restore_dir } => {
            block_on(commands::check::run(&cli.global, &out, restore_dir), &out)
        }
    }
}

/// Run an async command on a multi-threaded runtime.
fn block_on<F: std::future::Future<Output = ExitCode>>(future: F, out: &Output) -> ExitCode {
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(future),
        Err(err) => {
            out.error(&format!("cannot start the async runtime: {err}"));
            ExitCode::Internal
        }
    }
}
