use arboard::Clipboard;
use chrono::DateTime;

use crate::cache::Cache;
use crate::github::{self, Issue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Filter,
}

pub struct App {
    pub repo: String,
    pub issues: Vec<Issue>,
    pub selected: usize,
    pub filter: String,
    pub input_mode: InputMode,
    pub loading: bool,
    pub error: Option<String>,
    pub status_message: Option<String>,
    runtime: tokio::runtime::Runtime,
    cache: Option<Cache>,
}

impl App {
    pub fn new(repo: String) -> Self {
        let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        let cache = Cache::new(&repo).ok();
        Self {
            repo,
            issues: Vec::new(),
            selected: 0,
            filter: String::new(),
            input_mode: InputMode::Normal,
            loading: true,
            error: None,
            status_message: None,
            runtime,
            cache,
        }
    }

    /// Fetch issues from GitHub with cache-first strategy
    pub fn refresh(&mut self) {
        self.loading = true;
        self.error = None;

        let repo = self.repo.clone();

        // Determine if we should do incremental fetch
        let (since, has_cache) = if let Some(ref cache) = self.cache {
            let last_fetch = cache
                .get_last_fetch_time()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc));
            let has_cache = cache.has_issues();
            (last_fetch, has_cache)
        } else {
            (None, false)
        };

        // If we have cache, load it first for instant display
        if has_cache {
            if let Some(ref cache) = self.cache {
                if let Ok(cached_issues) = cache.load_all_issues() {
                    self.issues = cached_issues;
                    self.selected = 0;
                }
            }
        }

        // Fetch from GitHub (incremental if we have a last fetch time)
        match self
            .runtime
            .block_on(github::fetch_issues(&repo, 100, since))
        {
            Ok(fetched_issues) => {
                if let Some(ref cache) = self.cache {
                    // Process fetched issues: update cache, handle closed issues
                    for issue in &fetched_issues {
                        if issue.state == "open" {
                            // Upsert open issues
                            let _ = cache.upsert_issues(&[issue.clone()]);
                        } else {
                            // Remove closed issues from cache
                            let _ = cache.remove_issue(issue.number);
                        }
                    }

                    // Update last fetch time
                    let now = chrono::Utc::now().to_rfc3339();
                    let _ = cache.set_last_fetch_time(&now);

                    // Reload from cache to get merged results
                    if let Ok(all_issues) = cache.load_all_issues() {
                        self.issues = all_issues;
                    }
                } else {
                    // No cache, just use fetched issues (filter to open only)
                    self.issues = fetched_issues
                        .into_iter()
                        .filter(|i| i.state == "open")
                        .collect();
                }
                self.selected = 0;
            }
            Err(e) => {
                // If we already loaded from cache, just show a warning
                if has_cache {
                    self.status_message = Some(format!("Using cached data (fetch failed: {e})"));
                } else {
                    self.error = Some(e);
                }
            }
        }

        self.loading = false;
    }

    /// Get filtered issues based on current filter text
    pub fn filtered_issues(&self) -> Vec<&Issue> {
        if self.filter.is_empty() {
            self.issues.iter().collect()
        } else {
            let filter_lower = self.filter.to_lowercase();
            self.issues
                .iter()
                .filter(|issue| {
                    issue.title.to_lowercase().contains(&filter_lower)
                        || issue
                            .labels
                            .iter()
                            .any(|l| l.name.to_lowercase().contains(&filter_lower))
                })
                .collect()
        }
    }

    /// Move selection down
    pub fn next(&mut self) {
        let filtered_len = self.filtered_issues().len();
        if filtered_len > 0 {
            self.selected = (self.selected + 1).min(filtered_len - 1);
        }
    }

    /// Move selection up
    pub fn previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Get currently selected issue
    pub fn selected_issue(&self) -> Option<&Issue> {
        self.filtered_issues().get(self.selected).copied()
    }

    /// Open selected issue in browser
    pub fn open_selected(&self) {
        if let Some(issue) = self.selected_issue() {
            let _ = github::open_in_browser(&self.repo, issue.number);
        }
    }

    /// Start filter input mode
    pub fn start_filter(&mut self) {
        self.input_mode = InputMode::Filter;
    }

    /// Exit filter input mode
    pub fn exit_filter(&mut self) {
        self.input_mode = InputMode::Normal;
    }

    /// Clear filter
    pub fn clear_filter(&mut self) {
        self.filter.clear();
        self.selected = 0;
    }

    /// Add character to filter
    pub fn filter_push(&mut self, c: char) {
        self.filter.push(c);
        self.selected = 0;
    }

    /// Remove last character from filter
    pub fn filter_pop(&mut self) {
        self.filter.pop();
        self.selected = 0;
    }

    /// Clear status message
    pub fn clear_status(&mut self) {
        self.status_message = None;
    }

    /// Copy a prompt for the selected issue to the clipboard
    pub fn copy_issue_prompt(&mut self) -> Result<(), String> {
        let issue = self
            .selected_issue()
            .ok_or_else(|| "No issue selected".to_string())?;

        let mut prompt = String::new();

        // Start with /plan command and action
        prompt.push_str("/plan Investigate and fix this issue:\n\n");

        // Header
        prompt.push_str(&format!("GitHub Issue: {}#{}\n", self.repo, issue.number));
        prompt.push_str(&format!("Title: {}\n", issue.title));
        prompt.push_str(&format!("Author: {}\n", issue.author.login));
        prompt.push_str(&format!("Created: {}\n", issue.created_at));

        // Labels
        if !issue.labels.is_empty() {
            let label_names: Vec<&str> = issue.labels.iter().map(|l| l.name.as_str()).collect();
            prompt.push_str(&format!("Labels: {}\n", label_names.join(", ")));
        }

        prompt.push_str("\n---\n\n");

        // Body
        if let Some(body) = &issue.body {
            prompt.push_str("## Description\n\n");
            prompt.push_str(body);
            prompt.push_str("\n");
        }

        // Comments
        if !issue.comments.is_empty() {
            prompt.push_str("\n## Comments\n\n");
            for comment in &issue.comments {
                prompt.push_str(&format!("**@{}**:\n{}\n\n", comment.author, comment.body));
            }
        }

        // Copy to clipboard
        let mut clipboard =
            Clipboard::new().map_err(|e| format!("Failed to access clipboard: {e}"))?;
        clipboard
            .set_text(prompt)
            .map_err(|e| format!("Failed to copy to clipboard: {e}"))?;

        self.status_message = Some("Copied to clipboard!".to_string());
        Ok(())
    }
}
