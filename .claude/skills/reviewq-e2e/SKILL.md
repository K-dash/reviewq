---
name: reviewq-e2e
description: >
  reviewq プロジェクトの TUI 機能を E2E テストするための協調ワークフロー。
  デーモン起動・ログ監視を Claude が担当し、ユーザーが別ターミナルで TUI を操作して動作確認する。
  Triggers on: 'reviewq E2E', 'TUI E2E', 'reviewq 動作確認', 'TUI 動作確認',
  'デーモン起動してログ見て', 'E2Eテスト reviewq'
---

# reviewq TUI E2E Testing

Collaborative E2E testing workflow for reviewq TUI features.
Claude handles daemon startup and log monitoring while the user operates the TUI in a separate terminal.

## Prerequisites

- cwd is the reviewq repository
- `cargo build` succeeds
- `~/.reviewq/config.yml` is configured

## Workflow

### Step 0: Ask about DB reset

Ask the user whether to delete the existing SQLite database before starting.
This ensures a clean state for testing.

If yes:

```bash
rm -f ~/.reviewq/state/reviewq.db
```

The DB path may vary — check `~/.reviewq/config.yml` for `state.sqlite_path` if configured.

### Step 1: Build

```bash
cargo build
```

### Step 2: Check existing daemon

```bash
cat ~/.reviewq/logs/reviewq.pid 2>/dev/null && \
  ps -p $(cat ~/.reviewq/logs/reviewq.pid 2>/dev/null) 2>/dev/null || \
  echo "no daemon running"
```

If a daemon is already running, ask the user whether to use the existing one or restart.
To kill an existing daemon: `kill $(cat ~/.reviewq/logs/reviewq.pid)` (confirm with user first).

### Step 3: Start daemon (background)

```bash
target/debug/reviewq &   # run_in_background: true
```

Wait 2 seconds, then check the background task output to confirm startup succeeded.
Verify: output should contain `starting reviewq daemon`.

### Step 4: Inform user

Tell the user:
- Daemon is running
- Instruct them to open a separate terminal and run `target/debug/reviewq tui`
- Tell them which TUI operations to perform (depends on what is being tested)
- Ask them to report back when done

### Step 5: Monitor logs

When the user reports they have performed the TUI operation, check the latest logs:

```bash
# Try today's UTC date first, fall back to yesterday
tail -100 ~/.reviewq/logs/reviewq.log.$(date -u +%Y-%m-%d) 2>/dev/null || \
  tail -100 ~/.reviewq/logs/reviewq.log.$(date -u -v-1d +%Y-%m-%d)
```

Parse the log to verify the expected flow occurred. Common flows:

| Test scenario | Expected log sequence |
|---|---|
| Cancel running job | `leased job` → `spawned review process` → `received SIGUSR1` → `cancel requested` → `job completed status=canceled` |
| Retry canceled job | `received SIGUSR1` → `leased job` (same job_id) → `spawned review process` → `job completed status=succeeded` |
| Start queued job | `received SIGUSR1` → `leased job` → `spawned review process` |

### Step 6: Report results

Summarize what the logs show:
- Which job IDs were affected
- The state transitions observed
- Whether the test passed or failed
- Any unexpected warnings or errors

## Notes

- Daemon log path: `~/.reviewq/logs/reviewq.log.YYYY-MM-DD` (UTC date)
- PID file: `~/.reviewq/logs/reviewq.pid`
- The daemon responds to `SIGUSR1` (nudge/wake) and `SIGINT` (shutdown)
- If `another reviewq instance is running` error appears, check and kill the existing process
