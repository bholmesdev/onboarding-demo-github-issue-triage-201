use arboard::Clipboard;
use chrono::Utc;

use crate::cache::Cache;
use crate::github::{self, Issue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Filter,
}

#[derive(Debug, Clone)]
pub enum StatusMessage {
    Success(String),
    Error(String),
}

pub struct App {
    pub repo: String,
    pub issues: Vec<Issue>,
    pub selected: usize,
    pub filter: String,
    pub input_mode: InputMode,
    pub loading: bool,
    pub error: Option<String>,
    pub status_message: Option<StatusMessage>,
    runtime: tokio::runtime::Runtime,
    cache: Option<Cache>,
}

impl App {
    pub fn new(repo: String) -> Self {
        let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

        // Try to open cache
        let cache = match Cache::open(&repo) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("Warning: Could not open cache: {e}");
                None
            }
        };

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

    /// Fetch issues from GitHub with caching
    pub fn refresh(&mut self) {
        self.loading = true;
        self.error = None;

        // Try to load from cache first for instant display
        let last_sync = if let Some(ref cache) = self.cache {
            match cache.load_issues() {
                Ok(cached) if !cached.is_empty() => {
                    self.issues = cached.into_iter().filter(|i| i.state == "open").collect();
                    self.selected = 0;
                }
                _ => {}
            }
            cache.get_last_sync().ok().flatten()
        } else {
            None
        };

        // Fetch from API (incremental if we have last_sync)
        let repo = self.repo.clone();
        let sync_time = Utc::now();
        match self
            .runtime
            .block_on(github::fetch_issues(&repo, 100, last_sync))
        {
            Ok(fetched) => {
                // Separate open and closed issues
                let closed_numbers: Vec<u64> = fetched
                    .iter()
                    .filter(|i| i.state == "closed")
                    .map(|i| i.number)
                    .collect();

                // If incremental, merge; otherwise replace
                if last_sync.is_some() {
                    // Remove closed issues from our list
                    self.issues.retain(|i| !closed_numbers.contains(&i.number));

                    // Update/add fetched open issues
                    for issue in fetched.iter().filter(|i| i.state == "open") {
                        if let Some(existing) =
                            self.issues.iter_mut().find(|i| i.number == issue.number)
                        {
                            *existing = issue.clone();
                        } else {
                            self.issues.push(issue.clone());
                        }
                    }
                } else {
                    self.issues = fetched
                        .clone()
                        .into_iter()
                        .filter(|i| i.state == "open")
                        .collect();
                }

                self.selected = 0;

                // Save to cache
                if let Some(ref cache) = self.cache {
                    if let Err(e) = cache.save_issues(&fetched, sync_time) {
                        eprintln!("Warning: Failed to save cache: {e}");
                    }
                    if !closed_numbers.is_empty() {
                        let _ = cache.remove_closed_issues(&closed_numbers);
                    }
                }
            }
            Err(e) => {
                // Only show error if we don't have cached data
                if self.issues.is_empty() {
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

    /// Copy a prompt for the selected issue to the clipboard
    pub fn copy_issue_prompt(&mut self) {
        match self.build_and_copy_prompt() {
            Ok(_) => {
                self.status_message = Some(StatusMessage::Success(
                    "Prompt copied to clipboard!".to_string(),
                ))
            }
            Err(e) => self.status_message = Some(StatusMessage::Error(e)),
        }
    }

    fn build_and_copy_prompt(&self) -> Result<(), String> {
        let issue = self
            .selected_issue()
            .ok_or_else(|| "No issue selected".to_string())?;

        let mut prompt = String::new();

        // Agent instruction
        prompt.push_str("/plan fix this issue for me\n\n");

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

        Ok(())
    }

    /// Clear the status message
    pub fn clear_status(&mut self) {
        self.status_message = None;
    }
}
