use rusqlite::{Connection, Result};
use std::path::PathBuf;

use crate::github::Issue;

/// Get the cache database file path
fn get_cache_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    let cache_dir = PathBuf::from(home).join(".local/share/issue-triage");
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("Failed to create cache dir: {e}"))?;
    Ok(cache_dir.join("cache.db"))
}

/// Initialize the database and return a connection
pub fn init_db() -> Result<Connection, String> {
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

    Ok(conn)
}

/// Save issues to the cache
pub fn save_issues(conn: &Connection, repo: &str, issues: &[Issue]) -> Result<(), String> {
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
pub fn load_issues(conn: &Connection, repo: &str) -> Result<Vec<Issue>, String> {
    let mut stmt = conn
        .prepare("SELECT number, title, body, author_login, created_at, updated_at, labels, comments FROM issues WHERE repo = ?1")
        .map_err(|e| format!("Failed to prepare query: {e}"))?;

    let issue_iter = stmt
        .query_map([repo], |row| {
            let labels_json: String = row.get(6)?;
            let comments_json: String = row.get(7)?;

            let labels = serde_json::from_str(&labels_json)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e)))?;
            let comments = serde_json::from_str(&comments_json)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e)))?;

            Ok(Issue {
                number: row.get::<_, i64>(0)? as u64,
                title: row.get(1)?,
                body: row.get(2)?,
                author: crate::github::Author {
                    login: row.get(3)?,
                },
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
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
