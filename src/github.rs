use std::process::{Command, Stdio};

use chrono::{DateTime, Utc};
use octocrab::Octocrab;

#[derive(Debug, Clone)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub author: Author,
    pub created_at: String,
    pub updated_at: String,
    pub state: String,
    pub labels: Vec<Label>,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone)]
pub struct Author {
    pub login: String,
}

#[derive(Debug, Clone)]
pub struct Label {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub author: String,
    pub body: String,
}

/// Build octocrab client, using GITHUB_TOKEN if available
fn build_client() -> Result<Octocrab, String> {
    let builder = Octocrab::builder();
    let builder = if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        builder.personal_token(token)
    } else {
        builder
    };
    builder
        .build()
        .map_err(|e| format!("Failed to build client: {e}"))
}

/// Fetch issues from a GitHub repository using the GitHub REST API
/// If `since` is provided, only issues updated after that time are fetched.
pub async fn fetch_issues(
    repo: &str,
    limit: u32,
    since: Option<DateTime<Utc>>,
) -> Result<Vec<Issue>, String> {
    let (owner, repo_name) = repo
        .split_once('/')
        .ok_or_else(|| "Invalid repo format, expected owner/repo".to_string())?;

    let client = build_client()?;
    let issue_handler = client.issues(owner, repo_name);

    // Build query - if doing incremental sync, fetch all states to catch closed issues
    let mut builder = issue_handler.list();
    if let Some(since_time) = since {
        builder = builder
            .since(since_time)
            .state(octocrab::params::State::All)
            .sort(octocrab::params::issues::Sort::Updated);
    } else {
        builder = builder.state(octocrab::params::State::Open);
    }
    builder = builder.per_page(limit.min(100) as u8);

    let page = builder
        .send()
        .await
        .map_err(|e| format!("Failed to fetch issues: {e}"))?;

    let mut issues = Vec::new();
    for gh_issue in page.items {
        // Skip pull requests (GitHub API returns PRs as issues too)
        if gh_issue.pull_request.is_some() {
            continue;
        }

        // Fetch comments for this issue
        let comments = fetch_comments(&client, owner, repo_name, gh_issue.number).await;

        let state = match gh_issue.state {
            octocrab::models::IssueState::Open => "open",
            octocrab::models::IssueState::Closed => "closed",
            _ => "unknown",
        };

        let issue = Issue {
            number: gh_issue.number,
            title: gh_issue.title,
            body: gh_issue.body,
            author: Author {
                login: gh_issue.user.login,
            },
            created_at: gh_issue.created_at.to_rfc3339(),
            updated_at: gh_issue.updated_at.to_rfc3339(),
            state: state.to_string(),
            labels: gh_issue
                .labels
                .into_iter()
                .map(|l| Label {
                    name: l.name,
                    color: l.color,
                })
                .collect(),
            comments,
        };
        issues.push(issue);
    }

    Ok(issues)
}

const MAX_COMMENTS: u8 = 10;

async fn fetch_comments(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    issue_number: u64,
) -> Vec<Comment> {
    let Ok(page) = client
        .issues(owner, repo)
        .list_comments(issue_number)
        .per_page(MAX_COMMENTS)
        .send()
        .await
    else {
        return Vec::new();
    };

    page.items
        .into_iter()
        .map(|c| Comment {
            author: c.user.login,
            body: c.body.unwrap_or_default(),
        })
        .collect()
}

/// Open an issue in the browser
pub fn open_in_browser(repo: &str, issue_number: u64) -> Result<(), String> {
    let url = format!("https://github.com/{}/issues/{}", repo, issue_number);

    Command::new("open")
        .arg(&url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to open browser: {e}"))?;

    Ok(())
}
