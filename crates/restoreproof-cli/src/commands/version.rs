//! `restoreproof version`.

use restoreproof_core::ExitCode;

use crate::cli::{GlobalArgs, OutputFormat};
use crate::commands::tool_info;
use crate::output::Output;

/// Print version, build and platform information.
pub fn run(global: &GlobalArgs, out: &Output) -> ExitCode {
    let tool = tool_info();

    let rendered = match global.format {
        OutputFormat::Json => {
            let document = serde_json::json!({
                "version": tool.version,
                "git_commit": tool.git_commit,
                "platform": tool.platform,
                "edition": "open-source",
                "exit_codes": ExitCode::all()
                    .iter()
                    .map(|code| serde_json::json!({
                        "code": code.code(),
                        "meaning": code.describe(),
                    }))
                    .collect::<Vec<_>>(),
            });
            format!("{document:#}\n")
        }
        OutputFormat::Terminal | OutputFormat::Markdown => {
            let mut text = format!(
                "\nRestoreProof Core {}\n  build      {}\n  platform   {}\n  edition    open source (Apache-2.0)\n\nExit codes\n",
                tool.version,
                tool.git_commit.as_deref().unwrap_or("unknown"),
                tool.platform,
            );
            for code in ExitCode::all() {
                text.push_str(&format!("  {}  {}\n", code.code(), code.describe()));
            }
            text.push('\n');
            text
        }
    };

    if out.emit(&rendered, global.output.as_deref()).is_err() {
        return ExitCode::Internal;
    }
    ExitCode::Success
}
