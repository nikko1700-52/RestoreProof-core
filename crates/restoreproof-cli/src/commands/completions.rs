//! `restoreproof completions`: print a shell completion script.

use clap::CommandFactory as _;
use restoreproof_core::ExitCode;

use crate::cli::Cli;
use crate::output::Output;

/// Print a completion script for the given shell.
///
/// ```bash
/// restoreproof completions bash > /etc/bash_completion.d/restoreproof
/// restoreproof completions zsh  > ~/.zfunc/_restoreproof
/// restoreproof completions fish > ~/.config/fish/completions/restoreproof.fish
/// ```
pub fn run(out: &Output, shell: clap_complete::Shell) -> ExitCode {
    let mut command = Cli::command();
    let mut buffer: Vec<u8> = Vec::new();
    clap_complete::generate(shell, &mut command, "restoreproof", &mut buffer);

    match String::from_utf8(buffer) {
        Ok(script) => {
            out.block(&script);
            ExitCode::Success
        }
        Err(_) => {
            out.error("the generated completion script is not valid UTF-8");
            ExitCode::Internal
        }
    }
}
