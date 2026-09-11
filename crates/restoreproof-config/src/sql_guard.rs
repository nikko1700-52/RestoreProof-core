//! Read-only guard for SQL checks.
//!
//! Defence in depth, in three independent layers:
//!
//! 1. **This module** — the statement is scanned at *configuration* time. Only a
//!    single `SELECT`/`WITH` statement is accepted; comments, dollar-quoting,
//!    statement separators and a deny-list of writing or file-accessing
//!    keywords are refused.
//! 2. **The session** — at execution time the query runs inside
//!    `BEGIN READ ONLY` with a `statement_timeout`, so even a statement that
//!    slipped through cannot write or hang.
//! 3. **The database role** — the documentation requires a role with no write
//!    privileges on the recovery database.
//!
//! Layer 1 alone is a parser-less scanner and must never be treated as a
//! sufficient defence; it exists to give an early, readable error.

use crate::error::{ConfigError, Result};

/// Maximum accepted length for a check query.
const MAX_QUERY_LEN: usize = 4096;

/// Statement keywords allowed in first position.
const ALLOWED_LEADING: [&str; 2] = ["SELECT", "WITH"];

/// Keywords and function names refused anywhere outside a string literal.
///
/// The list covers three families: statements that write, statements that
/// change the session, and functions that reach outside the database (server
/// filesystem, other backends, the scheduler).
const DENIED_KEYWORDS: &[&str] = &[
    "ALTER",
    "ANALYZE",
    "BEGIN",
    "CALL",
    "CLUSTER",
    "COMMENT",
    "COMMIT",
    "COPY",
    "CREATE",
    "DEALLOCATE",
    "DELETE",
    "DISCARD",
    "DO",
    "DROP",
    "EXECUTE",
    "GRANT",
    "IMPORT",
    "INSERT",
    "INTO",
    "LISTEN",
    "LOCK",
    "MERGE",
    "NOTIFY",
    "PREPARE",
    "REASSIGN",
    "REFRESH",
    "REINDEX",
    "RESET",
    "REVOKE",
    "ROLLBACK",
    "SAVEPOINT",
    "SECURITY",
    "SET",
    "TRUNCATE",
    "UPDATE",
    "VACUUM",
    "DBLINK",
    "LO_EXPORT",
    "LO_IMPORT",
    "PG_LS_DIR",
    "PG_READ_BINARY_FILE",
    "PG_READ_FILE",
    "PG_RELOAD_CONF",
    "PG_SLEEP",
    "PG_STAT_FILE",
    "SET_CONFIG",
    "CURRENT_SETTING",
    "PG_TERMINATE_BACKEND",
    "PG_CANCEL_BACKEND",
    "PG_ROTATE_LOGFILE",
    "PG_ADVISORY_LOCK",
    "PG_ADVISORY_XACT_LOCK",
    "PG_BACKUP_START",
    "PG_BACKUP_STOP",
    "PG_CREATE_RESTORE_POINT",
    "PG_EXPORT_SNAPSHOT",
    "PG_LOGDIR_LS",
    "PG_READ_SERVER_FILES",
    "PG_WRITE_SERVER_FILES",
    "PG_EXECUTE_SERVER_PROGRAM",
];

/// Scanner state while walking the statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Code,
    SingleQuoted,
    DoubleQuoted,
    LineComment,
    BlockComment,
}

/// Result of stripping literals and comments from a statement.
struct Stripped {
    /// The statement with literals blanked out and comments removed.
    code: String,
    /// Whether a statement separator was found outside a literal.
    has_separator: bool,
}

/// Remove comments and blank out literals so keywords can be scanned safely.
fn strip(query: &str) -> Result<Stripped> {
    let mut code = String::with_capacity(query.len());
    let mut state = State::Code;
    let mut depth: u32 = 0;
    let mut has_separator = false;

    let chars: Vec<char> = query.chars().collect();
    let mut index = 0usize;

    while let Some(&current) = chars.get(index) {
        let next = chars.get(index + 1).copied();
        match state {
            State::Code => match (current, next) {
                ('-', Some('-')) => {
                    state = State::LineComment;
                    index += 2;
                }
                ('/', Some('*')) => {
                    state = State::BlockComment;
                    depth = 1;
                    index += 2;
                }
                ('\'', _) => {
                    state = State::SingleQuoted;
                    code.push(' ');
                    index += 1;
                }
                ('"', _) => {
                    state = State::DoubleQuoted;
                    code.push(' ');
                    index += 1;
                }
                ('$', _) => {
                    return Err(ConfigError::Unsafe {
                        reason: "sql check: dollar-quoting (`$$`, `$tag$`) and dollar parameters \
                                 are not accepted in check queries"
                            .to_owned(),
                    });
                }
                (';', _) => {
                    // A trailing separator is tolerated; anything after it is not.
                    if chars.get(index + 1..).is_some_and(|rest| {
                        rest.iter().any(|c| !c.is_whitespace())
                    }) {
                        has_separator = true;
                    }
                    index += 1;
                }
                _ => {
                    code.push(current);
                    index += 1;
                }
            },
            State::SingleQuoted => {
                if current == '\'' {
                    if next == Some('\'') {
                        index += 2; // escaped quote inside the literal
                    } else {
                        state = State::Code;
                        index += 1;
                    }
                } else {
                    index += 1;
                }
            }
            State::DoubleQuoted => {
                if current == '"' {
                    if next == Some('"') {
                        index += 2;
                    } else {
                        state = State::Code;
                        index += 1;
                    }
                } else {
                    index += 1;
                }
            }
            State::LineComment => {
                if current == '\n' {
                    state = State::Code;
                    code.push(' ');
                }
                index += 1;
            }
            State::BlockComment => match (current, next) {
                ('/', Some('*')) => {
                    depth = depth.saturating_add(1);
                    index += 2;
                }
                ('*', Some('/')) => {
                    depth = depth.saturating_sub(1);
                    index += 2;
                    if depth == 0 {
                        state = State::Code;
                        code.push(' ');
                    }
                }
                _ => index += 1,
            },
        }
    }

    match state {
        State::Code => Ok(Stripped {
            code,
            has_separator,
        }),
        State::SingleQuoted | State::DoubleQuoted => Err(ConfigError::Unsafe {
            reason: "sql check: the query contains an unterminated string literal".to_owned(),
        }),
        State::LineComment => Ok(Stripped {
            code,
            has_separator,
        }),
        State::BlockComment => Err(ConfigError::Unsafe {
            reason: "sql check: the query contains an unterminated block comment".to_owned(),
        }),
    }
}

/// Split the stripped code into uppercase word tokens.
fn tokens(code: &str) -> Vec<String> {
    code.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|token| !token.is_empty())
        .map(str::to_uppercase)
        .collect()
}

/// Validate that `query` is a single read-only statement.
///
/// # Errors
///
/// Returns [`ConfigError::Unsafe`] with an explanation for every rejection.
pub fn ensure_read_only(field: &str, query: &str) -> Result<()> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err(ConfigError::Unsafe {
            reason: format!("{field}: the query is empty"),
        });
    }
    if trimmed.len() > MAX_QUERY_LEN {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: the query is {} bytes long, the maximum is {MAX_QUERY_LEN}",
                trimmed.len()
            ),
        });
    }

    let stripped = strip(trimmed).map_err(|err| prefix(field, err))?;

    if stripped.has_separator {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: the query contains more than one statement. Only a single read-only \
                 statement is accepted."
            ),
        });
    }

    let tokens = tokens(&stripped.code);
    let Some(first) = tokens.first() else {
        return Err(ConfigError::Unsafe {
            reason: format!("{field}: the query contains no statement"),
        });
    };
    if !ALLOWED_LEADING.contains(&first.as_str()) {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: the query starts with `{first}`. Only SELECT and WITH statements are \
                 accepted in checks."
            ),
        });
    }

    if let Some(denied) = tokens.iter().find(|token| DENIED_KEYWORDS.contains(&token.as_str())) {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: the query contains the keyword `{denied}`, which is refused because it can \
                 modify data, change the session or read the server filesystem."
            ),
        });
    }

    Ok(())
}

fn prefix(field: &str, err: ConfigError) -> ConfigError {
    match err {
        ConfigError::Unsafe { reason } => ConfigError::Unsafe {
            reason: format!("{field}: {reason}"),
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(query: &str) -> bool {
        ensure_read_only("checks[0].query", query).is_ok()
    }

    #[test]
    fn plain_selects_are_accepted() {
        assert!(ok("SELECT count(*) FROM orders"));
        assert!(ok("select 1"));
        assert!(ok("SELECT count(*) FROM orders;"));
        assert!(ok(
            "WITH recent AS (SELECT * FROM orders WHERE created_at > now() - interval '1 day') \
             SELECT count(*) FROM recent"
        ));
    }

    #[test]
    fn writing_statements_are_refused() {
        for query in [
            "DELETE FROM orders",
            "UPDATE orders SET total = 0",
            "INSERT INTO orders VALUES (1)",
            "DROP TABLE orders",
            "TRUNCATE orders",
            "ALTER TABLE orders ADD COLUMN x int",
            "CREATE TABLE t (id int)",
            "GRANT ALL ON orders TO public",
        ] {
            assert!(!ok(query), "should have been refused: {query}");
        }
    }

    #[test]
    fn stacked_statements_are_refused() {
        assert!(!ok("SELECT 1; DROP TABLE orders"));
        assert!(!ok("SELECT 1;DELETE FROM orders;"));
    }

    #[test]
    fn a_single_trailing_semicolon_is_tolerated() {
        assert!(ok("SELECT 1;   "));
        assert!(ok("SELECT 1;\n"));
    }

    #[test]
    fn comment_smuggling_is_refused() {
        assert!(!ok("SELECT 1 -- harmless\n; DROP TABLE orders"));
        assert!(!ok("SELECT /* hidden */ 1; DELETE FROM t"));
        assert!(!ok("SELECT 1 /* nested /* comment */ still hidden */ ; DROP TABLE t"));
    }

    #[test]
    fn keywords_inside_string_literals_are_not_flagged() {
        assert!(ok("SELECT count(*) FROM events WHERE kind = 'delete'"));
        assert!(ok("SELECT 'drop table orders' AS sample"));
        assert!(ok("SELECT count(*) FROM t WHERE name = 'it''s an update'"));
    }

    #[test]
    fn quoted_identifiers_are_not_flagged() {
        assert!(ok(r#"SELECT "update" FROM "table""#));
    }

    #[test]
    fn select_into_is_refused_because_it_creates_a_table() {
        assert!(!ok("SELECT * INTO backup_orders FROM orders"));
    }

    #[test]
    fn select_for_update_is_refused() {
        assert!(!ok("SELECT * FROM orders FOR UPDATE"));
    }

    #[test]
    fn server_side_file_access_is_refused() {
        assert!(!ok("SELECT pg_read_file('/etc/passwd')"));
        assert!(!ok("SELECT pg_ls_dir('/')"));
        assert!(!ok("SELECT lo_import('/etc/shadow')"));
    }

    #[test]
    fn denial_of_service_helpers_are_refused() {
        assert!(!ok("SELECT pg_sleep(600)"));
    }

    #[test]
    fn session_mutation_is_refused() {
        assert!(!ok("SET statement_timeout = 0"));
        assert!(!ok("SELECT 1 FROM t WHERE x = (SELECT set_config('a','b',false))"));
        assert!(!ok("SELECT current_setting('data_directory')"));
    }

    #[test]
    fn backend_manipulation_is_refused() {
        assert!(!ok("SELECT pg_terminate_backend(pid) FROM pg_stat_activity"));
        assert!(!ok("SELECT pg_advisory_lock(1)"));
    }

    #[test]
    fn dollar_quoting_is_refused() {
        assert!(!ok("SELECT $$ anything $$"));
        assert!(!ok("SELECT $tag$ DROP TABLE t $tag$"));
    }

    #[test]
    fn unterminated_literals_and_comments_are_refused() {
        assert!(!ok("SELECT 'unterminated"));
        assert!(!ok("SELECT 1 /* unterminated"));
    }

    #[test]
    fn empty_and_oversized_queries_are_refused() {
        assert!(!ok("   "));
        assert!(!ok(&format!("SELECT '{}'", "a".repeat(MAX_QUERY_LEN))));
    }

    #[test]
    fn non_select_leading_keywords_are_refused() {
        assert!(!ok("EXPLAIN ANALYZE SELECT 1"));
        assert!(!ok("SHOW ALL"));
        assert!(!ok("TABLE orders"));
    }
}
