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
        Command::Validate { strict } => commands::validate::run(&cli.global, &out, strict),
        Command::Plan => commands::plan::run(&cli.global, &out),
        Command::Version => commands::version::run(&cli.global, &out),
        Command::Report { file, verify } => commands::report::run(&cli.global, &out, &file, verify),
        Command::Diff { before, after } => commands::diff::run(&cli.global, &out, &before, &after),
        Command::Completions { shell } => commands::completions::run(&out, shell),
        Command::Run => block_on(
            commands::run::run(&cli.global, &out, RunMode::Full, None),
            &out,
        ),
        Command::Check { restore_dir } => {
            block_on(commands::check::run(&cli.global, &out, restore_dir), &out)
        }
    }
}

/// Run an async command on a multi-threaded runtime, with signal handling.
fn block_on<F: std::future::Future<Output = ExitCode>>(future: F, out: &Output) -> ExitCode {
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(async move {
            // Watch for interruption for as long as the command runs. The task
            // is aborted on the normal path, so it never delays a clean exit.
            let watcher = tokio::spawn(watch_for_interruption());
            let code = future.await;
            watcher.abort();
            code
        }),
        Err(err) => {
            out.error(&format!("cannot start the async runtime: {err}"));
            ExitCode::Internal
        }
    }
}

/// Destroy any live recovery environment when the process is interrupted.
///
/// `Ctrl-C` in a terminal and a cancelled CI job both kill the process outright.
/// Without this they would leave containers and volumes holding a copy of
/// production data on the machine — the exact outcome this tool exists to
/// prevent. Everything still registered is destroyed before exiting, and
/// anything that could not be destroyed is named so the operator can finish the
/// job by hand.
async fn watch_for_interruption() {
    let signal_name = wait_for_signal().await;

    let live = restoreproof_runner::cleanup::active();
    if live.is_empty() {
        eprintln!("\n{signal_name}: nothing to clean up, exiting.");
        exit_after_interruption(signal_name);
    }

    eprintln!(
        "\n{signal_name}: destroying {} recovery environment(s) before exiting. Do not kill this \
         process again — restored data would be left behind.",
        live.len()
    );

    // Awaited, not spawned and abandoned: the process exits as soon as this
    // returns, so a teardown that was merely started would leave containers
    // running.
    let failed = restoreproof_runner::teardown_all().await;

    if failed.is_empty() {
        eprintln!("{signal_name}: recovery environments destroyed.");
    } else {
        eprintln!(
            "{signal_name}: could NOT destroy {} environment(s). Remove them by hand:",
            failed.len()
        );
        for (project, reason) in &failed {
            eprintln!("  {project}: {reason}");
            eprintln!("    docker compose -p {project} down --volumes --remove-orphans");
        }
    }
    exit_after_interruption(signal_name);
}

/// Wait for the first termination signal, and report which one arrived.
#[cfg(unix)]
async fn wait_for_signal() -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};

    let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
        return pending_forever().await;
    };
    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        return pending_forever().await;
    };

    tokio::select! {
        _ = interrupt.recv() => "interrupted",
        _ = terminate.recv() => "terminated",
    }
}

#[cfg(not(unix))]
async fn wait_for_signal() -> &'static str {
    match tokio::signal::ctrl_c().await {
        Ok(()) => "interrupted",
        Err(_) => pending_forever().await,
    }
}

/// Never resolves.
///
/// Used when a signal stream cannot be installed, so the command runs without
/// interruption handling rather than failing outright.
async fn pending_forever() -> &'static str {
    std::future::pending().await
}

/// Leave the process with the conventional code for the signal received.
///
/// Exiting here rather than unwinding is deliberate: the environment is already
/// destroyed, and an interrupted drill has no report worth writing.
#[allow(clippy::exit)]
fn exit_after_interruption(signal_name: &str) -> ! {
    // 128 + SIGINT(2) = 130, 128 + SIGTERM(15) = 143.
    let code = if signal_name == "terminated" {
        143
    } else {
        130
    };
    std::process::exit(code);
}
