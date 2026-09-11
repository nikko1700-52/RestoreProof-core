//! `restoreproof init`.

use std::path::Path;

use restoreproof_core::ExitCode;

use crate::output::Output;
use crate::scaffold;

/// Create a runnable example scenario.
pub fn run(out: &Output, directory: &Path, force: bool) -> ExitCode {
    let files: Vec<(&str, String)> = vec![
        ("restoreproof.yaml", scaffold::CONFIG.to_owned()),
        ("checks.yaml", scaffold::CHECKS.to_owned()),
        ("docker-compose.recovery.yml", scaffold::COMPOSE.to_owned()),
        ("backup/dump.sql", scaffold::DUMP.to_owned()),
        (
            "backup/backup-metadata.json",
            scaffold::metadata(chrono::Utc::now()),
        ),
        (".gitignore", scaffold::GITIGNORE.to_owned()),
    ];

    if !force {
        let existing: Vec<&str> = files
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| directory.join(name).exists())
            .collect();
        if !existing.is_empty() {
            out.error(&format!(
                "`{}` already contains: {}.\nUse `--force` to overwrite, or choose another \
                 directory.",
                directory.display(),
                existing.join(", ")
            ));
            return ExitCode::Usage;
        }
    }

    for (name, contents) in &files {
        let path = directory.join(name);
        if let Some(parent) = path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            out.error(&format!("cannot create `{}`: {err}", parent.display()));
            return ExitCode::Internal;
        }
        if let Err(err) = std::fs::write(&path, contents) {
            out.error(&format!("cannot write `{}`: {err}", path.display()));
            return ExitCode::Internal;
        }
    }

    let display = directory.display();
    out.block(&format!(
        "\nCreated a recovery drill in {display}\n\n  \
         restoreproof.yaml             the scenario\n  \
         checks.yaml                   what the drill proves\n  \
         docker-compose.recovery.yml   the isolated environment\n  \
         backup/dump.sql               a stand-in for your own backup\n  \
         backup/backup-metadata.json   when that backup was taken\n\n\
         Next steps\n\n  \
         1. Look at what it would do, without doing it:\n\n       \
         restoreproof plan --config {display}/restoreproof.yaml\n\n  \
         2. The SQL checks read their connection string from the environment,\n     \
         so it never sits in a file you commit:\n\n       \
         export RESTOREPROOF_DEMO_DATABASE_URL='postgres://restoreproof:restoreproof-local-drill@127.0.0.1:15432/app'\n\n  \
         3. Run the drill (this needs Docker with the Compose v2 plugin):\n\n       \
         restoreproof run --config {display}/restoreproof.yaml\n\n  \
         4. Point `backup.path` at the directory your own backup job writes to,\n     \
         and replace the checks with assertions about your data.\n\n"
    ));

    ExitCode::Success
}
