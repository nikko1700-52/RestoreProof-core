//! `type: sql` checks.
//!
//! This is the check that answers "is the *data* there", which is what a
//! recovery drill is ultimately about. It is also the check with the most
//! potential to do damage, so it runs under three independent safeguards:
//!
//! 1. the statement was proven to be a single read-only `SELECT`/`WITH` at
//!    configuration time (`restoreproof_config::sql_guard`);
//! 2. it executes inside an explicit `READ ONLY` transaction with a server-side
//!    `statement_timeout`, and the transaction is always rolled back;
//! 3. the connection string is read from an environment variable and must point
//!    at loopback unless `security.allow_external_sql_targets` is enabled.

use async_trait::async_trait;
use restoreproof_config::checks::{CheckKind, CheckSpec, SqlCheck};
use sqlx::{Column, Connection, Row, postgres::PgConnection, postgres::PgRow};

use crate::context::CheckContext;
use crate::executor::{CheckEvaluation, CheckExecutor};

/// Server-side ceiling for a check query.
const STATEMENT_TIMEOUT: &str = "SET LOCAL statement_timeout = '30s'";

/// Maximum number of rows materialised for assertions.
const MAX_ROWS: i64 = 10_000;

/// Executes `type: sql` checks.
pub struct SqlExecutor;

#[async_trait]
impl CheckExecutor for SqlExecutor {
    fn kind(&self) -> &'static str {
        "sql"
    }

    async fn execute(&self, spec: &CheckSpec, context: &CheckContext) -> CheckEvaluation {
        let CheckKind::Sql(sql) = &spec.kind else {
            return CheckEvaluation::error("internal: check kind mismatch");
        };

        let Some(dsn) = context.secret(&sql.dsn_env) else {
            return CheckEvaluation::error(format!(
                "the environment variable `{}` is not set, so this check has no database to query",
                sql.dsn_env
            ));
        };

        if !context.security.allow_external_sql_targets {
            match dsn_host(dsn.expose_secret()) {
                Some(host) if is_loopback_host(&host) => {}
                Some(host) => {
                    return CheckEvaluation::error(format!(
                        "`{}` points at `{host}`, which is not a loopback address. A drill queries \
                         the database it just restored; set \
                         `security.allow_external_sql_targets: true` to allow another host.",
                        sql.dsn_env
                    ));
                }
                None => {
                    return CheckEvaluation::error(format!(
                        "the connection string in `{}` could not be parsed",
                        sql.dsn_env
                    ));
                }
            }
        }

        match query(sql, dsn.expose_secret()).await {
            Ok(rows) => evaluate(sql, &rows),
            // A `Database` error means the server answered: the table does not
            // exist, the column is unknown, permission was denied. Retrying
            // cannot change that, and doing so only delays the report.
            Err(err @ sqlx::Error::Database(_)) => {
                CheckEvaluation::failed(format!("the recovered database rejected the query: {err}"))
                    .final_answer()
            }
            Err(err) => CheckEvaluation::failed(format!(
                "the query could not be executed against the recovered database: {err}"
            )),
        }
    }
}

/// Run the query inside a read-only transaction that is always rolled back.
async fn query(sql: &SqlCheck, dsn: &str) -> Result<Vec<PgRow>, sqlx::Error> {
    let mut connection = PgConnection::connect(dsn).await?;
    let result = run_in_read_only_transaction(&mut connection, sql).await;
    // Closing is best effort: the check result must not depend on it.
    let _ = connection.close().await;
    result
}

async fn run_in_read_only_transaction(
    connection: &mut PgConnection,
    sql: &SqlCheck,
) -> Result<Vec<PgRow>, sqlx::Error> {
    let mut transaction = connection.begin().await?;
    sqlx::query("SET TRANSACTION READ ONLY")
        .execute(&mut *transaction)
        .await?;
    sqlx::query(STATEMENT_TIMEOUT)
        .execute(&mut *transaction)
        .await?;

    let rows = sqlx::query(sql.query.trim().trim_end_matches(';'))
        .fetch_all(&mut *transaction)
        .await;

    // A drill never commits, whatever happened above.
    let _ = transaction.rollback().await;
    rows
}

/// A value read from the first column of the first row.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Text(String),
    Null,
    Unsupported(String),
}

impl std::fmt::Display for Cell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Integer(value) => write!(f, "{value}"),
            Self::Float(value) => write!(f, "{value}"),
            Self::Boolean(value) => write!(f, "{value}"),
            Self::Text(value) => write!(f, "{value}"),
            Self::Null => f.write_str("NULL"),
            Self::Unsupported(kind) => write!(f, "<value of type {kind}>"),
        }
    }
}

impl Cell {
    fn as_number(&self) -> Option<f64> {
        match self {
            #[allow(clippy::cast_precision_loss)]
            Self::Integer(value) => Some(*value as f64),
            Self::Float(value) => Some(*value),
            Self::Text(value) => value.parse().ok(),
            _ => None,
        }
    }
}

fn first_cell(row: &PgRow) -> Cell {
    if row.columns().is_empty() {
        return Cell::Null;
    }
    if let Ok(value) = row.try_get::<Option<i64>, _>(0) {
        return value.map_or(Cell::Null, Cell::Integer);
    }
    if let Ok(value) = row.try_get::<Option<i32>, _>(0) {
        return value.map_or(Cell::Null, |v| Cell::Integer(i64::from(v)));
    }
    if let Ok(value) = row.try_get::<Option<f64>, _>(0) {
        return value.map_or(Cell::Null, Cell::Float);
    }
    if let Ok(value) = row.try_get::<Option<bool>, _>(0) {
        return value.map_or(Cell::Null, Cell::Boolean);
    }
    if let Ok(value) = row.try_get::<Option<String>, _>(0) {
        return value.map_or(Cell::Null, Cell::Text);
    }
    let type_name = row
        .columns()
        .first()
        .map(|column| {
            use sqlx::TypeInfo as _;
            column.type_info().name().to_owned()
        })
        .unwrap_or_else(|| "unknown".to_owned());
    Cell::Unsupported(type_name)
}

#[allow(clippy::too_many_lines)]
fn evaluate(sql: &SqlCheck, rows: &[PgRow]) -> CheckEvaluation {
    let row_count = rows.len() as u64;
    let first = rows.first().map(first_cell);

    let mut details = vec![format!("rows: {row_count}")];
    if let Some(cell) = &first {
        details.push(format!("first value: {cell}"));
    }
    if row_count as i64 > MAX_ROWS {
        details.push(format!(
            "the query returned more than {MAX_ROWS} rows; assertions still apply but consider \
             aggregating in SQL"
        ));
    }

    if let Some(expected) = sql.expect_row_count
        && row_count != expected
    {
        return CheckEvaluation::failed(format!("expected {expected} row(s), got {row_count}"))
            .with_details(details);
    }
    if let Some(minimum) = sql.min_rows
        && row_count < minimum
    {
        return CheckEvaluation::failed(format!(
            "expected at least {minimum} row(s), got {row_count}"
        ))
        .with_details(details);
    }
    if let Some(maximum) = sql.max_rows
        && row_count > maximum
    {
        return CheckEvaluation::failed(format!(
            "expected at most {maximum} row(s), got {row_count}"
        ))
        .with_details(details);
    }

    let needs_value =
        sql.expect_value.is_some() || sql.min_value.is_some() || sql.max_value.is_some();
    if needs_value {
        let Some(cell) = first else {
            return CheckEvaluation::failed(
                "the query returned no row, so its value cannot be compared",
            )
            .with_details(details);
        };

        if let Some(expected) = &sql.expect_value
            && cell.to_string() != *expected
        {
            return CheckEvaluation::failed(format!(
                "expected the first value to be `{expected}`, got `{cell}`"
            ))
            .with_details(details);
        }

        if sql.min_value.is_some() || sql.max_value.is_some() {
            let Some(number) = cell.as_number() else {
                return CheckEvaluation::failed(format!(
                    "the first value `{cell}` is not numeric and cannot be compared"
                ))
                .with_details(details);
            };
            if let Some(minimum) = sql.min_value
                && number < minimum
            {
                return CheckEvaluation::failed(format!(
                    "expected the first value to be at least {minimum}, got {number}"
                ))
                .with_details(details);
            }
            if let Some(maximum) = sql.max_value
                && number > maximum
            {
                return CheckEvaluation::failed(format!(
                    "expected the first value to be at most {maximum}, got {number}"
                ))
                .with_details(details);
            }
        }
    }

    CheckEvaluation::passed(format!("query returned {row_count} row(s) as expected"))
        .with_details(details)
}

/// Extract the host from a `PostgreSQL` connection string.
///
/// Supports both the URL form (`postgres://user@host/db`) and the keyword form
/// (`host=... port=...`).
fn dsn_host(dsn: &str) -> Option<String> {
    if let Ok(url) = url::Url::parse(dsn) {
        if matches!(url.scheme(), "postgres" | "postgresql") {
            return url
                .host_str()
                .map(str::to_owned)
                .or_else(|| Some("localhost".to_owned()));
        }
        return None;
    }
    for token in dsn.split_whitespace() {
        if let Some(value) = token.strip_prefix("host=") {
            return Some(value.to_owned());
        }
    }
    // A keyword DSN without `host=` uses the local Unix socket.
    dsn.contains('=').then(|| "localhost".to_owned())
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.starts_with('/') {
        return true; // Unix domain socket directory
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(address) => address.is_loopback(),
        Err(_) => {
            let lowered = host.to_ascii_lowercase();
            lowered == "localhost" || lowered.ends_with(".localhost")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{context_with, sql_spec};
    use restoreproof_core::{CheckStatus, Secret};

    #[test]
    fn dsn_hosts_are_extracted_from_both_forms() {
        assert_eq!(
            dsn_host("postgres://app:pw@127.0.0.1:5432/app").as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(
            dsn_host("postgresql://db.example.com/app").as_deref(),
            Some("db.example.com")
        );
        assert_eq!(
            dsn_host("host=10.0.0.5 port=5432 dbname=app").as_deref(),
            Some("10.0.0.5")
        );
        assert_eq!(dsn_host("dbname=app").as_deref(), Some("localhost"));
        assert_eq!(dsn_host("not a dsn"), None);
    }

    #[test]
    fn loopback_hosts_are_recognised() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("/var/run/postgresql"));
        assert!(!is_loopback_host("10.0.0.5"));
        assert!(!is_loopback_host("db.production.internal"));
    }

    #[tokio::test]
    async fn a_missing_dsn_variable_is_an_error() {
        let spec = sql_spec("orders", "DATABASE_URL", "SELECT count(*) FROM orders");
        let evaluation = SqlExecutor.execute(&spec, &context_with()).await;
        assert_eq!(evaluation.status, CheckStatus::Error);
        assert!(evaluation.message.contains("DATABASE_URL"));
    }

    #[tokio::test]
    async fn a_remote_database_is_refused_by_default() {
        let mut context = context_with();
        context.secrets.insert(
            "DATABASE_URL".to_owned(),
            Secret::new("postgres://app@db.production.internal/app"),
        );
        let spec = sql_spec("orders", "DATABASE_URL", "SELECT count(*) FROM orders");
        let evaluation = SqlExecutor.execute(&spec, &context).await;
        assert_eq!(evaluation.status, CheckStatus::Error);
        assert!(evaluation.message.contains("loopback"));
    }

    #[tokio::test]
    async fn an_unreachable_local_database_fails_rather_than_errors() {
        let mut context = context_with();
        context.secrets.insert(
            "DATABASE_URL".to_owned(),
            Secret::new("postgres://app:pw@127.0.0.1:1/app"),
        );
        let spec = sql_spec("orders", "DATABASE_URL", "SELECT count(*) FROM orders");
        let evaluation = SqlExecutor.execute(&spec, &context).await;
        assert_eq!(evaluation.status, CheckStatus::Failed);
    }

    #[test]
    fn cells_render_and_convert() {
        assert_eq!(Cell::Integer(42).to_string(), "42");
        assert_eq!(Cell::Null.to_string(), "NULL");
        assert_eq!(Cell::Integer(42).as_number(), Some(42.0));
        assert_eq!(Cell::Text("7.5".to_owned()).as_number(), Some(7.5));
        assert_eq!(Cell::Null.as_number(), None);
    }
}
