use rusqlite::{Connection, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::github::Issue;

/// Get the cache database file path
fn get_cache_path() -> Result<PathBuf, String> {
    let cache_dir = dirs::data_local_dir()
        .ok_or("Failed to get data directory".to_string())?
        .join("issue-triage");
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("Failed to create cache dir: {e}"))?;
    Ok(cache_dir.join("cache.db"))
}

/// Initialize the database and return a connection wrapped in Arc<Mutex>
pub fn init_db() -> Result<Arc<Mutex<Connection>>, String> {
    let db_path = get_cache_path()?;
    let conn = Connection::open(db_path).map_err(|e| format!("Failed to open DB: {e}"))?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS issues (
            repo TEXT NOT NULL,
            number INTEGER NOT NULL,
            title TEXT NOT NULL,
            body TEXT,
            author_login TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            labels TEXT NOT NULL,
            comments TEXT NOT NULL,
            PRIMARY KEY (repo, number)
        )",
        [],
    )
    .map_err(|e| format!("Failed to create table: {e}"))?;

    Ok(Arc::new(Mutex::new(conn)))
}

/// Save issues to the cache
pub fn save_issues(
    conn: &Arc<Mutex<Connection>>,
    repo: &str,
    issues: &[Issue],
) -> Result<(), String> {
    let conn = conn
        .lock()
        .map_err(|e| format!("Failed to lock connection: {e}"))?;

    for issue in issues {
        let labels_json = serde_json::to_string(&issue.labels)
            .map_err(|e| format!("Failed to serialize labels: {e}"))?;
        let comments_json = serde_json::to_string(&issue.comments)
            .map_err(|e| format!("Failed to serialize comments: {e}"))?;

        conn.execute(
            "INSERT OR REPLACE INTO issues 
             (repo, number, title, body, author_login, created_at, updated_at, labels, comments)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            (
                repo,
                issue.number as i64,
                &issue.title,
                &issue.body,
                &issue.author.login,
                &issue.created_at,
                &issue.updated_at,
                labels_json,
                comments_json,
            ),
        )
        .map_err(|e| format!("Failed to insert issue: {e}"))?;
    }

    Ok(())
}

/// Load issues from the cache
pub fn load_issues(conn: &Arc<Mutex<Connection>>, repo: &str) -> Result<Vec<Issue>, String> {
    let conn = conn
        .lock()
        .map_err(|e| format!("Failed to lock connection: {e}"))?;

    let mut stmt = conn
        .prepare("SELECT number, title, body, author_login, created_at, updated_at, labels, comments FROM issues WHERE repo = ?1")
        .map_err(|e| format!("Failed to prepare query: {e}"))?;

    let issue_iter = stmt
        .query_map([repo], |row| {
            let labels_json: String = row.get("labels")?;
            let comments_json: String = row.get("comments")?;

            let labels = serde_json::from_str(&labels_json).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    6,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            let comments = serde_json::from_str(&comments_json).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    7,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;

            Ok(Issue {
                number: row.get::<_, i64>("number")? as u64,
                title: row.get("title")?,
                body: row.get("body")?,
                author: crate::github::Author {
                    login: row.get("author_login")?,
                },
                created_at: row.get("created_at")?,
                updated_at: row.get("updated_at")?,
                labels,
                comments,
            })
        })
        .map_err(|e| format!("Failed to query issues: {e}"))?;

    let mut issues = Vec::new();
    for issue in issue_iter {
        issues.push(issue.map_err(|e| format!("Failed to parse issue: {e}"))?);
    }

    Ok(issues)
}
