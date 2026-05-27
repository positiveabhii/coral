//! Workspace-scoped `SQLite` storage for Universal Search retrieval.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "foundation branch; the child branch wires this store into catalog metadata retrieval"
    )
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

use crate::state::AppStateLayout;
use crate::storage::fs::ensure_dir;
use crate::workspaces::WorkspaceName;

pub(crate) const SEARCH_INDEX_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub(crate) struct SearchIndexStore {
    path: PathBuf,
    capabilities: SqliteSearchCapabilities,
}

impl SearchIndexStore {
    pub(crate) fn open_workspace(
        layout: &AppStateLayout,
        workspace_name: &WorkspaceName,
    ) -> Result<Self, SearchIndexError> {
        Self::open(layout.search_index_file(workspace_name))
    }

    pub(crate) fn open(path: impl Into<PathBuf>) -> Result<Self, SearchIndexError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }

        let mut connection = Connection::open(&path)?;
        configure_connection(&connection)?;
        let capabilities = detect_capabilities(&connection)?;
        ensure_supported(&capabilities)?;
        migrate(&mut connection)?;

        Ok(Self { path, capabilities })
    }

    pub(crate) fn connect(&self) -> Result<Connection, SearchIndexError> {
        let connection = Connection::open(&self.path)?;
        configure_connection(&connection)?;
        Ok(connection)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn capabilities(&self) -> &SqliteSearchCapabilities {
        &self.capabilities
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SqliteSearchCapabilities {
    pub(crate) sqlite_version: String,
    pub(crate) fts5: bool,
    pub(crate) trigram: bool,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SearchIndexError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("SQLite {sqlite_version} does not support required search feature: {feature}")]
    UnsupportedCapability {
        feature: &'static str,
        sqlite_version: String,
    },
}

fn configure_connection(connection: &Connection) -> Result<(), SearchIndexError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

fn detect_capabilities(
    connection: &Connection,
) -> Result<SqliteSearchCapabilities, SearchIndexError> {
    let sqlite_version =
        connection.query_row("SELECT sqlite_version()", [], |row| row.get::<_, String>(0))?;
    let fts5 = connection
        .execute_batch(
            "
            CREATE VIRTUAL TABLE temp.coral_search_fts5_check USING fts5(value);
            DROP TABLE temp.coral_search_fts5_check;
            ",
        )
        .is_ok();
    let trigram = connection
        .execute_batch(
            "
            CREATE VIRTUAL TABLE temp.coral_search_trigram_check
            USING fts5(value, tokenize = 'trigram');
            DROP TABLE temp.coral_search_trigram_check;
            ",
        )
        .is_ok();

    Ok(SqliteSearchCapabilities {
        sqlite_version,
        fts5,
        trigram,
    })
}

fn ensure_supported(capabilities: &SqliteSearchCapabilities) -> Result<(), SearchIndexError> {
    if !capabilities.fts5 {
        return Err(SearchIndexError::UnsupportedCapability {
            feature: "FTS5",
            sqlite_version: capabilities.sqlite_version.clone(),
        });
    }
    if !capabilities.trigram {
        return Err(SearchIndexError::UnsupportedCapability {
            feature: "FTS5 trigram tokenizer",
            sqlite_version: capabilities.sqlite_version.clone(),
        });
    }
    Ok(())
}

fn migrate(connection: &mut Connection) -> Result<(), SearchIndexError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS search_index_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        INSERT INTO search_index_meta (key, value, updated_at)
        VALUES ('schema_version', '1', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        ON CONFLICT(key) DO UPDATE SET
            value = excluded.value,
            updated_at = excluded.updated_at;

        CREATE TABLE IF NOT EXISTS catalog_entities (
            workspace TEXT NOT NULL,
            entity_key TEXT NOT NULL,
            result_type TEXT NOT NULL,
            surface_kind TEXT NOT NULL,
            schema_name TEXT NOT NULL,
            surface_name TEXT NOT NULL,
            name TEXT NOT NULL,
            data_type TEXT NOT NULL DEFAULT '',
            is_required INTEGER NOT NULL DEFAULT 0,
            description TEXT NOT NULL DEFAULT '',
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY (workspace, entity_key)
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS catalog_entities_fts USING fts5(
            workspace UNINDEXED,
            entity_key UNINDEXED,
            name,
            qualified_name,
            description,
            searchable_text,
            tokenize = 'trigram'
        );

        CREATE TABLE IF NOT EXISTS observed_values (
            workspace TEXT NOT NULL,
            source_name TEXT NOT NULL,
            surface_kind TEXT NOT NULL,
            surface_name TEXT NOT NULL,
            column_name TEXT NOT NULL,
            normalized_value_key TEXT NOT NULL,
            display_value TEXT NOT NULL DEFAULT '',
            sensitivity_tier TEXT NOT NULL DEFAULT 'low_risk',
            suggested_operator TEXT NOT NULL DEFAULT 'exact',
            first_observed_at TEXT NOT NULL,
            last_observed_at TEXT NOT NULL,
            observed_count INTEGER NOT NULL DEFAULT 1,
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY (
                workspace,
                source_name,
                surface_kind,
                surface_name,
                column_name,
                normalized_value_key
            )
        );

        CREATE INDEX IF NOT EXISTS observed_values_last_observed_idx
            ON observed_values (workspace, last_observed_at);

        CREATE VIRTUAL TABLE IF NOT EXISTS observed_values_fts USING fts5(
            workspace UNINDEXED,
            source_name UNINDEXED,
            surface_kind UNINDEXED,
            surface_name UNINDEXED,
            column_name UNINDEXED,
            normalized_value_key UNINDEXED,
            display_value,
            searchable_text,
            tokenize = 'trigram'
        );

        PRAGMA user_version = 1;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::OptionalExtension as _;
    use tempfile::tempdir;

    use super::{SEARCH_INDEX_SCHEMA_VERSION, SearchIndexStore};
    use crate::state::AppStateLayout;
    use crate::workspaces::WorkspaceName;

    #[test]
    fn open_workspace_creates_search_index_schema() {
        let temp = tempdir().expect("tempdir");
        let layout =
            AppStateLayout::discover(Some(temp.path().join("coral-config"))).expect("layout");
        let workspace = WorkspaceName::parse("default").expect("workspace");
        let store = SearchIndexStore::open_workspace(&layout, &workspace).expect("store");

        assert_eq!(store.path(), layout.search_index_file(&workspace));
        assert!(store.capabilities().fts5);
        assert!(store.capabilities().trigram);

        let connection = store.connect().expect("connect");
        let user_version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version");
        assert_eq!(user_version, SEARCH_INDEX_SCHEMA_VERSION);

        let schema_version = connection
            .query_row(
                "SELECT value FROM search_index_meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .expect("schema_version query")
            .expect("schema_version");
        assert_eq!(schema_version, SEARCH_INDEX_SCHEMA_VERSION.to_string());
    }

    #[test]
    fn catalog_fts_supports_bm25_trigram_matches() {
        let temp = tempdir().expect("tempdir");
        let store = SearchIndexStore::open(temp.path().join("search.sqlite")).expect("store");
        let connection = store.connect().expect("connect");
        connection
            .execute(
                "
                INSERT INTO catalog_entities_fts (
                    workspace,
                    entity_key,
                    name,
                    qualified_name,
                    description,
                    searchable_text
                )
                VALUES ('default', 'function:github.search_commits', 'search_commits',
                    'github.search_commits', 'Search commits', 'github commit sha')
                ",
                [],
            )
            .expect("insert fts row");

        let row_count: u32 = connection
            .query_row(
                "
                SELECT count(*)
                FROM catalog_entities_fts
                WHERE catalog_entities_fts MATCH '\"commit\"'
                ORDER BY bm25(catalog_entities_fts)
                ",
                [],
                |row| row.get(0),
            )
            .expect("fts query");
        assert_eq!(row_count, 1);
    }
}
