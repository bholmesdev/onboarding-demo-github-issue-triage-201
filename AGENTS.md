# AGENTS.md

This file provides guidance to WARP (warp.dev) when working with code in this repository.

## Project Overview

Terminal UI (TUI) for browsing and triaging GitHub issues. Built with Rust using ratatui for rendering, crossterm for terminal handling, octocrab for GitHub API, and rusqlite for local caching.

## Commands

```bash
# Build
cargo build

# Run (requires owner/repo argument)
cargo run -- facebook/react
cargo run -- owner/repo --limit 500

# Install locally
cargo install --path .

# Run tests (none currently)
cargo test

# Check/lint
cargo check
cargo clippy
```

## Environment

- `GITHUB_TOKEN` - Optional GitHub personal access token for private repos or higher rate limits

## Architecture

```
src/
├── main.rs   # Entry point, terminal setup, event loop
├── app.rs    # App state, business logic (filtering, navigation, clipboard)
├── github.rs # GitHub API client (octocrab), Issue/Label/Comment types
├── cache.rs  # SQLite cache (~/.cache/issue-triage/owner_repo.db)
└── ui.rs     # Ratatui rendering (header, issue list, preview pane, help bar)
```

### Data Flow

1. `main.rs` parses CLI args (clap), sets up terminal, creates `App`, runs event loop
2. `App::refresh()` loads from cache first (instant display), then fetches from GitHub incrementally
3. GitHub fetches use `since` param to get only updated issues; closed issues are removed from cache
4. UI renders split-pane layout: 40% issue list, 60% preview pane
5. Keyboard events in event loop call methods on `App` to mutate state

### Key Patterns

- Blocking async: `App` owns a tokio `Runtime` and uses `block_on()` for GitHub calls in sync context
- Cache-first: Issues load immediately from SQLite; network fetch updates in background
- Filter: Live search on title and label names; resets selection to 0 on change
