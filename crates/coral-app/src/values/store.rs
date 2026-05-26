use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension as _, params};

use crate::storage::fs::ensure_dir;

const VALUE_SEARCH_DEFAULT_LIMIT: u32 = 20;
const VALUE_SEARCH_MAX_LIMIT: u32 = 100;
const SURFACED_VALUE_RANK_CUTOFF: i64 = 8;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ValueMemoryError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct ValueRollup {
    pub(crate) workspace_name: String,
    pub(crate) schema_name: String,
    pub(crate) table_name: String,
    pub(crate) column_path: String,
    pub(crate) value: String,
    pub(crate) value_truncated: bool,
    pub(crate) search_text: String,
    pub(crate) value_hash: String,
    pub(crate) rank: i64,
    pub(crate) seen_count: u64,
    pub(crate) observed_at: String,
}

impl ValueRollup {
    fn storage_key(&self) -> String {
        [
            self.workspace_name.as_str(),
            self.schema_name.as_str(),
            self.table_name.as_str(),
            self.column_path.as_str(),
            self.value_hash.as_str(),
        ]
        .join("\u{1f}")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StoredValueSearchRequest {
    pub(crate) workspace_name: String,
    pub(crate) term: String,
    pub(crate) schema_name: Option<String>,
    pub(crate) table_name: Option<String>,
    pub(crate) column_path: Option<String>,
    pub(crate) limit: u32,
    pub(crate) offset: u32,
}

impl StoredValueSearchRequest {
    pub(crate) fn normalized_limit(&self) -> u32 {
        if self.limit == 0 {
            VALUE_SEARCH_DEFAULT_LIMIT
        } else {
            self.limit.min(VALUE_SEARCH_MAX_LIMIT)
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct StoredValueSearchResult {
    pub(crate) workspace_name: String,
    pub(crate) schema_name: String,
    pub(crate) table_name: String,
    pub(crate) column_path: String,
    pub(crate) value: String,
    pub(crate) value_truncated: bool,
    pub(crate) seen_count: u64,
    pub(crate) first_seen_at: String,
    pub(crate) last_seen_at: String,
    pub(crate) field_total_count: u32,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct StoredValueSearchPage {
    pub(crate) values: Vec<StoredValueSearchResult>,
    pub(crate) total_count: u32,
    pub(crate) limit: u32,
    pub(crate) offset: u32,
    pub(crate) has_more: bool,
    pub(crate) next_offset: u32,
}

#[derive(Clone)]
pub(crate) struct ValueMemoryStore {
    path: PathBuf,
}

impl ValueMemoryStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn upsert_rollups(&self, rollups: Vec<ValueRollup>) -> Result<(), ValueMemoryError> {
        if rollups.is_empty() {
            return Ok(());
        }
        let mut conn = self.open_connection()?;
        create_schema(&conn)?;
        let tx = conn.transaction()?;
        {
            let mut upsert_rollup = tx.prepare(
                "INSERT INTO value_rollups (
                    key,
                    workspace_name,
                    schema_name,
                    table_name,
                    column_path,
                    value,
                    value_truncated,
                    search_text,
                    value_hash,
                    rank,
                    seen_count,
                    first_seen_at,
                    last_seen_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
                ON CONFLICT(key) DO UPDATE SET
                    value = excluded.value,
                    value_truncated = excluded.value_truncated,
                    search_text = excluded.search_text,
                    rank = min(value_rollups.rank, excluded.rank),
                    seen_count = value_rollups.seen_count + excluded.seen_count,
                    last_seen_at = excluded.last_seen_at",
            )?;
            let mut delete_fts = tx.prepare("DELETE FROM value_rollups_fts WHERE key = ?1")?;
            let mut insert_fts = tx.prepare(
                "INSERT INTO value_rollups_fts(key, search_text, value)
                 VALUES (?1, ?2, ?3)",
            )?;

            for rollup in rollups {
                let key = rollup.storage_key();
                upsert_rollup.execute(params![
                    key,
                    rollup.workspace_name,
                    rollup.schema_name,
                    rollup.table_name,
                    rollup.column_path,
                    rollup.value,
                    i64::from(rollup.value_truncated),
                    rollup.search_text,
                    rollup.value_hash,
                    rollup.rank,
                    i64::try_from(rollup.seen_count).unwrap_or(i64::MAX),
                    rollup.observed_at,
                ])?;
                let stored_search_text: Option<String> = tx
                    .query_row(
                        "SELECT search_text FROM value_rollups WHERE key = ?1",
                        params![key],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(search_text) = stored_search_text {
                    delete_fts.execute(params![key])?;
                    insert_fts.execute(params![key, search_text, rollup.value])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn search(
        &self,
        request: StoredValueSearchRequest,
    ) -> Result<StoredValueSearchPage, ValueMemoryError> {
        let term = build_fts_query(&request.term)?;
        let limit = request.normalized_limit();
        let offset = request.offset;
        if !self.path.exists() {
            return Ok(empty_page(limit, offset));
        }

        let conn = self.open_connection()?;
        create_schema(&conn)?;
        let schema_name = request.schema_name.unwrap_or_default();
        let table_name = request.table_name.unwrap_or_default();
        let column_path = request.column_path.unwrap_or_default();
        let total_count = count_matches(
            &conn,
            &term,
            &request.workspace_name,
            &schema_name,
            &table_name,
            &column_path,
        )?;
        let mut stmt = conn.prepare(
            "WITH scoped_matches AS (
                SELECT
                    r.workspace_name,
                    r.schema_name,
                    r.table_name,
                    r.column_path,
                    r.value,
                    r.value_truncated,
                    r.seen_count,
                    r.first_seen_at,
                    r.last_seen_at,
                    r.rank,
                    bm25(value_rollups_fts) AS match_score
                FROM value_rollups_fts
                JOIN value_rollups r ON r.key = value_rollups_fts.key
                WHERE value_rollups_fts MATCH ?1
                  AND r.workspace_name = ?2
                  AND (?3 = '' OR r.schema_name = ?3)
                  AND (?4 = '' OR r.table_name = ?4)
                  AND (?5 = '' OR r.column_path = ?5)
                  AND r.rank < ?8
            )
            SELECT
                workspace_name,
                schema_name,
                table_name,
                column_path,
                value,
                value_truncated,
                seen_count,
                first_seen_at,
                last_seen_at,
                COUNT(*) OVER (
                    PARTITION BY schema_name, table_name, column_path
                ) AS field_total_count
            FROM scoped_matches
            ORDER BY rank ASC, match_score, seen_count DESC, last_seen_at DESC
            LIMIT ?6 OFFSET ?7",
        )?;
        let rows = stmt.query_map(
            params![
                term,
                request.workspace_name,
                schema_name,
                table_name,
                column_path,
                i64::from(limit),
                i64::from(offset),
                SURFACED_VALUE_RANK_CUTOFF,
            ],
            value_search_result_from_row,
        )?;
        let values = rows.collect::<Result<Vec<_>, _>>()?;
        let next_offset = offset.saturating_add(limit);
        Ok(StoredValueSearchPage {
            values,
            total_count,
            limit,
            offset,
            has_more: next_offset < total_count,
            next_offset,
        })
    }

    fn open_connection(&self) -> Result<Connection, ValueMemoryError> {
        if let Some(parent) = self.path.parent() {
            ensure_dir(parent)?;
        }
        let conn = Connection::open(&self.path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(conn)
    }
}

fn create_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS value_rollups (
            key TEXT PRIMARY KEY,
            workspace_name TEXT NOT NULL,
            schema_name TEXT NOT NULL,
            table_name TEXT NOT NULL,
            column_path TEXT NOT NULL,
            value TEXT NOT NULL,
            value_truncated INTEGER NOT NULL,
            search_text TEXT NOT NULL,
            value_hash TEXT NOT NULL,
            rank INTEGER NOT NULL DEFAULT 10,
            seen_count INTEGER NOT NULL,
            first_seen_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL
        );",
    )?;
    ensure_rank_column(conn)?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS value_rollups_scope_idx
            ON value_rollups(workspace_name, schema_name, table_name, column_path);
        CREATE INDEX IF NOT EXISTS value_rollups_recent_idx
            ON value_rollups(workspace_name, last_seen_at);
        CREATE INDEX IF NOT EXISTS value_rollups_rank_idx
            ON value_rollups(workspace_name, rank, seen_count);
        CREATE VIRTUAL TABLE IF NOT EXISTS value_rollups_fts
            USING fts5(key UNINDEXED, search_text, value, tokenize='unicode61');",
    )?;
    normalize_legacy_rank_defaults(conn)?;
    ensure_rank_column(conn)
}

fn ensure_rank_column(conn: &Connection) -> Result<(), rusqlite::Error> {
    let mut stmt = conn.prepare("PRAGMA table_info(value_rollups)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|column| column == "rank") {
        conn.execute(
            "ALTER TABLE value_rollups ADD COLUMN rank INTEGER NOT NULL DEFAULT 10",
            [],
        )?;
    }
    Ok(())
}

fn normalize_legacy_rank_defaults(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute("UPDATE value_rollups SET rank = 2 WHERE rank = 10", [])?;
    Ok(())
}

fn count_matches(
    conn: &Connection,
    term: &str,
    workspace_name: &str,
    schema_name: &str,
    table_name: &str,
    column_path: &str,
) -> Result<u32, rusqlite::Error> {
    let count: i64 = conn.query_row(
        "WITH scoped_matches AS (
            SELECT r.rank
            FROM value_rollups_fts
            JOIN value_rollups r ON r.key = value_rollups_fts.key
            WHERE value_rollups_fts MATCH ?1
              AND r.workspace_name = ?2
              AND (?3 = '' OR r.schema_name = ?3)
              AND (?4 = '' OR r.table_name = ?4)
              AND (?5 = '' OR r.column_path = ?5)
              AND r.rank < ?6
        )
        SELECT COUNT(*)
        FROM scoped_matches",
        params![
            term,
            workspace_name,
            schema_name,
            table_name,
            column_path,
            SURFACED_VALUE_RANK_CUTOFF,
        ],
        |row| row.get(0),
    )?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

fn value_search_result_from_row(
    row: &rusqlite::Row<'_>,
) -> Result<StoredValueSearchResult, rusqlite::Error> {
    let seen_count: i64 = row.get(6)?;
    let field_total_count: i64 = row.get(9)?;
    Ok(StoredValueSearchResult {
        workspace_name: row.get(0)?,
        schema_name: row.get(1)?,
        table_name: row.get(2)?,
        column_path: row.get(3)?,
        value: row.get(4)?,
        value_truncated: row.get::<_, i64>(5)? != 0,
        seen_count: u64::try_from(seen_count).unwrap_or(u64::MAX),
        first_seen_at: row.get(7)?,
        last_seen_at: row.get(8)?,
        field_total_count: u32::try_from(field_total_count).unwrap_or(u32::MAX),
    })
}

fn empty_page(limit: u32, offset: u32) -> StoredValueSearchPage {
    StoredValueSearchPage {
        values: Vec::new(),
        total_count: 0,
        limit,
        offset,
        has_more: false,
        next_offset: 0,
    }
}

fn build_fts_query(term: &str) -> Result<String, ValueMemoryError> {
    let tokens = term
        .split(|char: char| !char.is_alphanumeric())
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(|token| format!("{}*", token.to_lowercase()))
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err(ValueMemoryError::InvalidInput(
            "search term must contain at least one letter or number".to_string(),
        ));
    }
    Ok(tokens.join(" "))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{StoredValueSearchRequest, ValueMemoryStore, ValueRollup};

    #[test]
    fn search_counts_matching_values_per_field_before_pagination() {
        let tempdir = tempdir().expect("create tempdir");
        let store = ValueMemoryStore::new(tempdir.path().join("values.sqlite"));
        store
            .upsert_rollups(vec![
                rollup("name", "coral", 8),
                rollup("name", "coral-auth", 4),
                rollup("topic", "coral feedback", 1),
            ])
            .expect("seed value memory");

        let page = store
            .search(StoredValueSearchRequest {
                workspace_name: "default".to_string(),
                term: "coral".to_string(),
                schema_name: Some("slack".to_string()),
                table_name: Some("channels".to_string()),
                column_path: Some("name".to_string()),
                limit: 1,
                offset: 0,
            })
            .expect("search value memory");

        assert_eq!(page.total_count, 2);
        assert_eq!(page.limit, 1);
        assert!(page.has_more);
        assert_eq!(page.next_offset, 1);
        assert_eq!(page.values.len(), 1);
        assert_eq!(page.values[0].column_path, "name");
        assert_eq!(page.values[0].field_total_count, 2);
    }

    fn rollup(column_path: &str, value: &str, seen_count: u64) -> ValueRollup {
        ValueRollup {
            workspace_name: "default".to_string(),
            schema_name: "slack".to_string(),
            table_name: "channels".to_string(),
            column_path: column_path.to_string(),
            value: value.to_string(),
            value_truncated: false,
            search_text: value.to_string(),
            value_hash: value.to_string(),
            rank: 2,
            seen_count,
            observed_at: "2026-05-20T10:00:00Z".to_string(),
        }
    }
}
