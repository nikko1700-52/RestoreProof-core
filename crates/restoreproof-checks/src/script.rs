//! `type: script` checks.
//!
//! A script check runs a file that lives inside the project, which is what
//! makes it auditable: the code that decides whether a recovery succeeded is
//! versioned next to the drill that runs it. Path confinement is enforced at
//! configuration time and re-checked here.

use async_trait::async_trait;
use restoreproof_config::checks::{CheckKind, CheckSpec};

use crate::context::CheckContext;
use crate::executor::{CheckEvaluation, CheckExecutor};
use crate::program::{ProgramRequest, run};

/// Executes `type: script` checks.
pub struct ScriptExecutor;

#[async_trait]
impl CheckExecutor for ScriptExecutor {
    fn kind(&self) -> &'static str {
        "script"
    }

    async fn execute(&self, spec: &CheckSpec, context: &CheckContext) -> CheckEvaluation {
        let CheckKind::Script(script) = &spec.kind else {
            return CheckEvaluation::error("internal: check kind mismatch");
        };

        let resolved = context.project_root.join(&script.path);
        let canonical = match std::fs::canonicalize(&resolved) {
            Ok(path) => path,
            Err(err) => {
                return CheckEvaluation::error(format!(
                    "cannot resolve `{}`: {err}",
                    script.path.display()
                ));
            }
        };

        // Defence in depth: configuration validation already refused a script
        // outside the project, but a symlink could have been swapped in since.
        if !canonical.starts_with(&context.project_root) {
            return CheckEvaluation::error(format!(
                "`{}` resolves outside the project directory and will not be executed",
                script.path.display()
            ));
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            match std::fs::metadata(&canonical) {
                Ok(metadata) => {
                    let mode = metadata.permissions().mode();
                    if mode & 0o002 != 0 {
                        return CheckEvaluation::error(format!(
                            "`{}` is world-writable (mode {:o}) and will not be executed",
                            script.path.display(),
                            mode & 0o7777
                        ));
                    }
                    if mode & 0o111 == 0 {
                        return CheckEvaluation::error(format!(
                            "`{}` is not executable; run `chmod +x` on it",
                            script.path.display()
                        ));
                    }
                }
                Err(err) => {
                    return CheckEvaluation::error(format!(
                        "cannot read the permissions of `{}`: {err}",
                        script.path.display()
                    ));
                }
            }
        }

        run(
            ProgramRequest {
                program: canonical,
                args: &script.args,
                workdir: script.workdir.as_deref(),
                environment: &script.environment,
                expected_exit_code: script.expected_exit_code,
                expect_stdout_contains: None,
                label: format!("`{}`", script.path.display()),
            },
            context,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{context_in, script_spec};
    use restoreproof_core::CheckStatus;
    use std::os::unix::fs::PermissionsExt as _;
    use tempfile::TempDir;

    fn write_script(dir: &TempDir, name: &str, body: &str, mode: u32) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    #[tokio::test]
    async fn a_passing_script_passes() {
        let dir = TempDir::new().unwrap();
        write_script(&dir, "ok.sh", "#!/bin/sh\nexit 0\n", 0o755);
        let context = context_in(&dir);
        let spec = script_spec("ok", "ok.sh", 0);
        assert_eq!(
            ScriptExecutor.execute(&spec, &context).await.status,
            CheckStatus::Passed
        );
    }

    #[tokio::test]
    async fn a_failing_script_reports_its_exit_code() {
        let dir = TempDir::new().unwrap();
        write_script(&dir, "ko.sh", "#!/bin/sh\nexit 7\n", 0o755);
        let context = context_in(&dir);
        let spec = script_spec("ko", "ko.sh", 0);
        let evaluation = ScriptExecutor.execute(&spec, &context).await;
        assert_eq!(evaluation.status, CheckStatus::Failed);
        assert!(evaluation.message.contains("exited with code 7"));
    }

    #[tokio::test]
    async fn a_world_writable_script_is_refused() {
        let dir = TempDir::new().unwrap();
        write_script(&dir, "loose.sh", "#!/bin/sh\nexit 0\n", 0o777);
        let context = context_in(&dir);
        let spec = script_spec("loose", "loose.sh", 0);
        let evaluation = ScriptExecutor.execute(&spec, &context).await;
        assert_eq!(evaluation.status, CheckStatus::Error);
        assert!(evaluation.message.contains("world-writable"));
    }

    #[tokio::test]
    async fn a_non_executable_script_is_refused_with_advice() {
        let dir = TempDir::new().unwrap();
        write_script(&dir, "plain.sh", "#!/bin/sh\nexit 0\n", 0o644);
        let context = context_in(&dir);
        let spec = script_spec("plain", "plain.sh", 0);
        let evaluation = ScriptExecutor.execute(&spec, &context).await;
        assert_eq!(evaluation.status, CheckStatus::Error);
        assert!(evaluation.message.contains("chmod +x"));
    }

    #[tokio::test]
    async fn a_symlink_pointing_outside_the_project_is_refused() {
        let dir = TempDir::new().unwrap();
        std::os::unix::fs::symlink("/bin/true", dir.path().join("escape.sh")).unwrap();
        let context = context_in(&dir);
        let spec = script_spec("escape", "escape.sh", 0);
        let evaluation = ScriptExecutor.execute(&spec, &context).await;
        assert_eq!(evaluation.status, CheckStatus::Error);
        assert!(evaluation.message.contains("outside the project"));
    }
}
