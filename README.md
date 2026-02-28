# reviewq

A CLI/TUI tool that automatically detects pull requests where you are a requested reviewer and triggers AI code review agents.

## Installation

```bash
cargo install --path .
```

## Usage

```bash
# Start the daemon (polling + review execution)
reviewq daemon --config config.yaml

# Check daemon status
reviewq status

# List review jobs
reviewq list

# Open a completed review in the browser
reviewq open <job-id>
```

## Configuration

Create a YAML configuration file (default: `~/.reviewq/config.yaml`):

```yaml
repos:
  allowlist:
    - repo: owner/repo-name
      skip_self_authored: true      # Skip PRs you authored (default: true)
      skip_reviewer_check: false    # Process all open PRs, not just those assigned to you (default: false)
      review_on_push: true          # Re-review on every push/force-push (default: true)
      command: "claude -p '{prompt}'" # Per-repo command override (optional)
      prompt_template: "Review PR"  # Per-repo prompt template override (optional)
      max_concurrency: 3            # Per-repo concurrency limit (optional, reserved)
      base_repo_path: /path/to/clone # Per-repo local clone path (optional)

polling:
  interval_seconds: 300             # How often to poll GitHub (default: 300)

auth:
  method: gh                        # Authentication method (default: "gh")
  fallback_env: GITHUB_TOKEN        # Fallback env var for token (default: "GITHUB_TOKEN")

execution:
  base_repo_path: /path/to/repos    # Global base path for local clones (optional)
  worktree_root: /path/to/worktrees # Directory for git worktrees (optional)
  max_concurrency: 10               # Max concurrent review jobs (default: 10)
  lease_minutes: 5                  # Job lease timeout (default: 5)

runner:
  command: "claude -p '{prompt}'"   # Global review command (optional)
  prompt_template: "Review {pr_url}" # Global prompt template (optional)

cancel:
  sigint_timeout_seconds: 5         # SIGINT grace period (default: 5)
  sigterm_timeout_seconds: 15       # SIGTERM grace period (default: 15)
  sigkill_timeout_seconds: 5        # SIGKILL grace period (default: 5)

cleanup:
  ttl_minutes: 1440                 # Job retention period (default: 1440)
  interval_minutes: 30              # Cleanup check interval (default: 30)

logging:
  dir: ~/.reviewq/logs              # Log directory (default: ~/.reviewq/logs)

state:
  sqlite_path: ~/.reviewq/state.db  # SQLite database path (default: ~/.reviewq/state.db)

output:
  dir: ~/.reviewq/output            # Review output directory (default: ~/.reviewq/output)
```

### `review_on_push`

Controls whether SHA changes (force-pushes or additional commits) trigger a re-review.

| Value | Behavior |
|-------|----------|
| `true` (default) | Every push triggers a new review. Standard behavior. |
| `false` | A PR with a prior **succeeded** review is not re-queued. In-flight reviews on stale SHAs are still canceled to prevent outdated reviews from completing. Failed/canceled reviews remain eligible for retry. |

Use `review_on_push: false` for large, high-traffic repositories to avoid exhausting API limits:

```yaml
repos:
  allowlist:
    - repo: org/big-monorepo
      review_on_push: false   # Review only once per PR
    - repo: org/small-repo    # Default: re-review on every push
```

## License

MIT OR Apache-2.0
