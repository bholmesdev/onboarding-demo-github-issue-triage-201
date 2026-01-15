use std::fs;
use std::path::PathBuf;

use rusqlite::{params, Connection, Result};

use crate::github::{Author, Comment, Issue, Label};

pub struct Cache {
    conn: Connection,
}

impl Cache {
    /// Open or create cache DB for the given repo (e.g. "owner/repo")
    pub fn new(repo: &str) -> Result<Self, String> {
        let db_path = Self::db_path(repo)?;

        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create cache directory: {e}"))?;
        }

        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open cache database: {e}"))?;

        let cache = Self { conn };
        cache.init_schema()?;
        Ok(cache)
    }

    /// Returns path like ~/.cache/issue-triage/owner_repo.db
    fn db_path(repo: &str) -> Result<PathBuf, String> {
        let cache_dir = dirs::cache_dir().ok_or("Could not determine cache directory")?;
        let safe_repo = repo.replace('/', "_");
        Ok(cache_dir
            .join("issue-triage")
            .join(format!("{safe_repo}.db")))
    }

    fn init_schema(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS issues (
                    number INTEGER PRIMARY KEY,
                    title TEXT NOT NULL,
                    body TEXT,
                    author TEXT NOT NULL,
                    state TEXT NOT NULL DEFAULT 'open',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    labels_json TEXT NOT NULL,
                    comments_json TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS metadata (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                ",
            )
            .map_err(|e| format!("Failed to initialize schema: {e}"))?;

        // Migration: add state column if missing (for existing DBs)
        let _ = self.conn.execute(
            "ALTER TABLE issues ADD COLUMN state TEXT NOT NULL DEFAULT 'open'",
            [],
        );

        Ok(())
    }

    /// Insert or update issues in cache
    pub fn upsert_issues(&self, issues: &[Issue]) -> Result<(), String> {
        for issue in issues {
            let labels_json =
                serde_json::to_string(&issue.labels).unwrap_or_else(|_| "[]".to_string());
            let comments_json =
                serde_json::to_string(&issue.comments).unwrap_or_else(|_| "[]".to_string());

            self.conn
                .execute(
                    "INSERT OR REPLACE INTO issues 
                     (number, title, body, author, state, created_at, updated_at, labels_json, comments_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        issue.number as i64,
                        issue.title,
                        issue.body,
                        issue.author.login,
                        issue.state,
                        issue.created_at,
                        issue.updated_at,
                        labels_json,
                        comments_json,
                    ],
                )
                .map_err(|e| format!("Failed to upsert issue {}: {e}", issue.number))?;
        }
        Ok(())
    }

    /// Load all cached open issues
    pub fn load_all_issues(&self) -> Result<Vec<Issue>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT number, title, body, author, state, created_at, updated_at, labels_json, comments_json 
                 FROM issues WHERE state = 'open' ORDER BY number DESC",
            )
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let rows = stmt
            .query_map([], |row| {
                let labels_json: String = row.get(7)?;
                let comments_json: String = row.get(8)?;

                let labels: Vec<Label> = serde_json::from_str(&labels_json).unwrap_or_default();
                let comments: Vec<Comment> =
                    serde_json::from_str(&comments_json).unwrap_or_default();

                Ok(Issue {
                    number: row.get::<_, i64>(0)? as u64,
                    title: row.get(1)?,
                    body: row.get(2)?,
                    author: Author { login: row.get(3)? },
                    state: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                    labels,
                    comments,
                })
            })
            .map_err(|e| format!("Failed to query issues: {e}"))?;

        let mut issues = Vec::new();
        for row in rows {
            issues.push(row.map_err(|e| format!("Failed to read row: {e}"))?);
        }
        Ok(issues)
    }

    /// Get last fetch timestamp from metadata
    pub fn get_last_fetch_time(&self) -> Option<String> {
        self.conn
            .query_row(
                "SELECT value FROM metadata WHERE key = 'last_fetch_time'",
                [],
                |row| row.get(0),
            )
            .ok()
    }

    /// Set last fetch timestamp in metadata
    pub fn set_last_fetch_time(&self, timestamp: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO metadata (key, value) VALUES ('last_fetch_time', ?1)",
                params![timestamp],
            )
            .map_err(|e| format!("Failed to set last_fetch_time: {e}"))?;
        Ok(())
    }

    /// Remove an issue from cache (e.g. when closed)
    pub fn remove_issue(&self, number: u64) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM issues WHERE number = ?1",
                params![number as i64],
            )
            .map_err(|e| format!("Failed to remove issue {number}: {e}"))?;
        Ok(())
    }

    /// Check if cache has any issues
    pub fn has_issues(&self) -> bool {
        self.conn
            .query_row("SELECT COUNT(*) FROM issues", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|count| count > 0)
            .unwrap_or(false)
    }
}
