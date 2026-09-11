//! Builders used by the unit tests of this crate.

use std::collections::BTreeMap;
use std::path::PathBuf;

use restoreproof_config::SecuritySpec;
use restoreproof_config::checks::{
    CheckKind, CheckSpec, CommandCheck, ContainerCheck, ContainerState, FileBase, FileCheck,
    HttpCheck, HttpMethod, ScriptCheck, SqlCheck,
};
use restoreproof_core::Redactor;
use tempfile::TempDir;

use crate::context::CheckContext;

pub(crate) fn context_with() -> CheckContext {
    CheckContext {
        project_root: PathBuf::from("/tmp"),
        restore_dir: PathBuf::from("/tmp"),
        compose_project: "test-project".to_owned(),
        security: SecuritySpec::default(),
        redactor: Redactor::new(),
        secrets: BTreeMap::new(),
        probe: None,
    }
}

pub(crate) fn context_in(dir: &TempDir) -> CheckContext {
    let root = std::fs::canonicalize(dir.path()).unwrap_or_else(|_| dir.path().to_path_buf());
    CheckContext {
        project_root: root.clone(),
        restore_dir: root,
        ..context_with()
    }
}

fn base(id: &str, kind: CheckKind) -> CheckSpec {
    CheckSpec {
        id: id.to_owned(),
        name: None,
        description: None,
        enabled: true,
        required: true,
        timeout_seconds: Some(5),
        retry: None,
        kind,
    }
}

pub(crate) fn http_spec(id: &str, url: &str) -> CheckSpec {
    base(
        id,
        CheckKind::Http(HttpCheck {
            method: HttpMethod::Get,
            url: url.to_owned(),
            expected_status: 200,
            expect_body_contains: None,
            headers: BTreeMap::new(),
            headers_from_env: BTreeMap::new(),
            body: None,
            follow_redirects: false,
        }),
    )
}

pub(crate) fn command_spec(id: &str, command: Vec<String>) -> CheckSpec {
    base(
        id,
        CheckKind::Command(CommandCheck {
            command,
            workdir: None,
            expected_exit_code: 0,
            expect_stdout_contains: None,
            environment: BTreeMap::new(),
        }),
    )
}

pub(crate) fn script_spec(id: &str, path: &str, expected_exit_code: i32) -> CheckSpec {
    base(
        id,
        CheckKind::Script(ScriptCheck {
            path: PathBuf::from(path),
            args: Vec::new(),
            expected_exit_code,
            workdir: None,
            environment: BTreeMap::new(),
        }),
    )
}

pub(crate) fn file_spec(id: &str, path: &str) -> CheckSpec {
    base(
        id,
        CheckKind::File(FileCheck {
            path: PathBuf::from(path),
            base: FileBase::Restore,
            exists: true,
            min_size_bytes: None,
            max_size_bytes: None,
            sha256: None,
        }),
    )
}

pub(crate) fn container_spec(service: &str, state: ContainerState) -> CheckSpec {
    base(
        service,
        CheckKind::Container(ContainerCheck {
            service: service.to_owned(),
            state,
            expected_exit_code: None,
            port: None,
        }),
    )
}

pub(crate) fn sql_spec(id: &str, dsn_env: &str, query: &str) -> CheckSpec {
    base(
        id,
        CheckKind::Sql(SqlCheck {
            dsn_env: dsn_env.to_owned(),
            query: query.to_owned(),
            expect_row_count: None,
            min_rows: Some(1),
            max_rows: None,
            expect_value: None,
            min_value: None,
            max_value: None,
        }),
    )
}
