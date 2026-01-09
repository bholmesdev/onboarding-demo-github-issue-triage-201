use std::path::PathBuf;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, Result as SqlResult};

use crate::github::{Author, Comment, Issue, Label};

pub struct Cache {
    conn: Connection,
}

impl Cache {
    /// Open or create cache DB for given repo
    pub fn open(repo: &str) -> Result<Self, String> {
        let db_path = Self::db_path(repo)?;

        // Ensure parent dir exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create cache dir: {e}"))?;
        }

        let conn =
            Connection::open(&db_path).map_err(|e| format!("Failed to open cache DB: {e}"))?;

        let cache = Self { conn };
        cache.init_schema()?;
        Ok(cache)
    }

    fn db_path(repo: &str) -> Result<PathBuf, String> {
        let cache_dir = dirs::cache_dir().ok_or("Could not find cache directory")?;
        let safe_name = repo.replace('/', "_");
        Ok(cache_dir
            .join("issue-triage")
            .join(format!("{safe_name}.db")))
    }

    fn init_schema(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "
            CREATE TABLE IF NOT EXISTS repo_meta (
                id INTEGER PRIMARY KEY,
                last_sync TEXT
            );

            INSERT OR IGNORE INTO repo_meta (id, last_sync) VALUES (1, NULL);

            CREATE TABLE IF NOT EXISTS issues (
                number INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                body TEXT,
                author_login TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                state TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS labels (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                issue_number INTEGER NOT NULL,
                name TEXT NOT NULL,
                color TEXT NOT NULL,
                FOREIGN KEY (issue_number) REFERENCES issues(number) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS comments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                issue_number INTEGER NOT NULL,
                author TEXT NOT NULL,
                body TEXT NOT NULL,
                FOREIGN KEY (issue_number) REFERENCES issues(number) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_labels_issue ON labels(issue_number);
            CREATE INDEX IF NOT EXISTS idx_comments_issue ON comments(issue_number);
            ",
            )
            .map_err(|e| format!("Failed to init schema: {e}"))?;
        Ok(())
    }

    /// Get last sync timestamp
    pub fn get_last_sync(&self) -> Result<Option<DateTime<Utc>>, String> {
        let result: Option<String> = self
            .conn
            .query_row("SELECT last_sync FROM repo_meta WHERE id = 1", [], |row| {
                row.get(0)
            })
            .map_err(|e| format!("Failed to get last_sync: {e}"))?;

        match result {
            Some(s) => {
                let dt = DateTime::parse_from_rfc3339(&s)
                    .map_err(|e| format!("Failed to parse last_sync: {e}"))?
                    .with_timezone(&Utc);
                Ok(Some(dt))
            }
            None => Ok(None),
        }
    }

    /// Load all cached issues
    pub fn load_issues(&self) -> Result<Vec<Issue>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT number, title, body, author_login, created_at, updated_at, state FROM issues",
            )
            .map_err(|e| format!("Failed to prepare load query: {e}"))?;

        let issue_rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(|e| format!("Failed to load issues: {e}"))?;

        let mut issues = Vec::new();
        for row in issue_rows {
            let (number, title, body, author_login, created_at, updated_at, state) =
                row.map_err(|e| format!("Failed to read issue row: {e}"))?;

            let labels = self.load_labels(number)?;
            let comments = self.load_comments(number)?;

            issues.push(Issue {
                number,
                title,
                body,
                author: Author {
                    login: author_login,
                },
                created_at,
                updated_at,
                state,
                labels,
                comments,
            });
        }

        Ok(issues)
    }

    fn load_labels(&self, issue_number: u64) -> Result<Vec<Label>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, color FROM labels WHERE issue_number = ?")
            .map_err(|e| format!("Failed to prepare labels query: {e}"))?;

        let labels = stmt
            .query_map([issue_number], |row| {
                Ok(Label {
                    name: row.get(0)?,
                    color: row.get(1)?,
                })
            })
            .map_err(|e| format!("Failed to load labels: {e}"))?
            .collect::<SqlResult<Vec<_>>>()
            .map_err(|e| format!("Failed to collect labels: {e}"))?;

        Ok(labels)
    }

    fn load_comments(&self, issue_number: u64) -> Result<Vec<Comment>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT author, body FROM comments WHERE issue_number = ?")
            .map_err(|e| format!("Failed to prepare comments query: {e}"))?;

        let comments = stmt
            .query_map([issue_number], |row| {
                Ok(Comment {
                    author: row.get(0)?,
                    body: row.get(1)?,
                })
            })
            .map_err(|e| format!("Failed to load comments: {e}"))?
            .collect::<SqlResult<Vec<_>>>()
            .map_err(|e| format!("Failed to collect comments: {e}"))?;

        Ok(comments)
    }

    /// Save issues to cache (upsert) and update last_sync
    pub fn save_issues(&self, issues: &[Issue], sync_time: DateTime<Utc>) -> Result<(), String> {
        for issue in issues {
            // Upsert issue
            self.conn
                .execute(
                    "INSERT OR REPLACE INTO issues (number, title, body, author_login, created_at, updated_at, state)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    params![
                        issue.number,
                        issue.title,
                        issue.body,
                        issue.author.login,
                        issue.created_at,
                        issue.updated_at,
                        issue.state,
                    ],
                )
                .map_err(|e| format!("Failed to save issue {}: {e}", issue.number))?;

            // Delete old labels/comments, insert new
            self.conn
                .execute("DELETE FROM labels WHERE issue_number = ?", [issue.number])
                .map_err(|e| format!("Failed to delete labels: {e}"))?;

            for label in &issue.labels {
                self.conn
                    .execute(
                        "INSERT INTO labels (issue_number, name, color) VALUES (?, ?, ?)",
                        params![issue.number, label.name, label.color],
                    )
                    .map_err(|e| format!("Failed to save label: {e}"))?;
            }

            self.conn
                .execute(
                    "DELETE FROM comments WHERE issue_number = ?",
                    [issue.number],
                )
                .map_err(|e| format!("Failed to delete comments: {e}"))?;

            for comment in &issue.comments {
                self.conn
                    .execute(
                        "INSERT INTO comments (issue_number, author, body) VALUES (?, ?, ?)",
                        params![issue.number, comment.author, comment.body],
                    )
                    .map_err(|e| format!("Failed to save comment: {e}"))?;
            }
        }

        // Update last_sync
        self.conn
            .execute(
                "UPDATE repo_meta SET last_sync = ? WHERE id = 1",
                [sync_time.to_rfc3339()],
            )
            .map_err(|e| format!("Failed to update last_sync: {e}"))?;

        Ok(())
    }

    /// Remove closed issues from cache
    pub fn remove_closed_issues(&self, issue_numbers: &[u64]) -> Result<(), String> {
        for &num in issue_numbers {
            self.conn
                .execute("DELETE FROM issues WHERE number = ?", [num])
                .map_err(|e| format!("Failed to delete issue {num}: {e}"))?;
        }
        Ok(())
    }
}
