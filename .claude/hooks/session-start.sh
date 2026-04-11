#!/usr/bin/env bash
# SessionStart hook: bootstrap every Claude Code session with a concise
# situational summary, so the agent can pick up work without re-asking
# the user for context.
#
# Rationale (nyosegawa harness-engineering-best-practices-2026):
#   "Startup Ritual (Automated):
#      git status
#      git log --oneline -20
#      cat PROGRESS.json     # structured state, not Markdown
#      npm run dev           # smoke test
#      echo 'Ready for task selection'"
#
#   "Git as Memory Bridge: each session closes with a descriptive commit;
#    next session reads `git log -5` to understand context."
#
# We adapt this to reviewq's Rust stack:
#
#   - git status --short            (current worktree state)
#   - git log --oneline -15         (recent history)
#   - git worktree list             (so the agent knows where it is)
#   - cat .claude/state/PROGRESS.md if present (free-form resume notes)
#   - A short hook-enforcement reminder so the agent doesn't re-learn
#     the rules from scratch every session.
#
# All output goes on stdout inside a JSON hookSpecificOutput block so
# Claude Code merges it into the session context as additional context.

. "$(dirname "$0")/lib/common.sh"
reviewq_require_jq
reviewq_read_input

event=$(reviewq_jq '.hook_event_name')
[[ "$event" != "SessionStart" ]] && exit 0

cwd=$(reviewq_jq '.cwd')
[[ -z "$cwd" ]] && cwd=$(reviewq_project_dir)
[[ ! -d "$cwd" ]] && exit 0

# Walk up to the nearest directory with .git or Cargo.toml so we run
# git commands from a valid repo root even if the hook fires from a
# subdirectory.
probe="$cwd"
for _ in 1 2 3 4; do
    if [[ -d "$probe/.git" || -f "$probe/.git" ]]; then
        break
    fi
    parent=$(dirname "$probe")
    [[ "$parent" == "$probe" ]] && break
    probe="$parent"
done
cwd="$probe"

# --- stale-state cleanup --------------------------------------------------
# git worktree admin dirs and fsmonitor daemons routinely get left behind
# when a previous session removed a worktree but git internals did not
# fully reconcile. Symptoms seen in iter2: `git rev-parse --show-toplevel`
# returns a deleted worktree path, `git checkout` fails with "fatal: this
# operation must be run in a work tree", and the AGENTS.md size gate
# test picks up a stale index from a wrapper framework.
#
# Best-effort fixes on every session start:
#   1. Prune stale worktree admin dirs. Safe and fast.
#   2. Stop any fsmonitor daemon pointing at a now-deleted path. Only
#      fires when the daemon is alive; silent no-op otherwise.
cleanup_log=$(reviewq_session_dir)/session-start-cleanup.log
{
    echo "== session-start stale-state cleanup =="
    (cd "$cwd" && git worktree prune -v 2>&1) || true
    # `git fsmonitor--daemon stop` exits non-zero if no daemon is running;
    # `|| true` keeps the session bootstrap from soft-failing open.
    (cd "$cwd" && git fsmonitor--daemon stop 2>&1) || true
    # Also remove the IPC socket if it still exists without a live daemon.
    if [[ -S "$cwd/.git/fsmonitor--daemon.ipc" ]]; then
        if ! pgrep -f "fsmonitor--daemon.*$cwd" >/dev/null 2>&1; then
            rm -f "$cwd/.git/fsmonitor--daemon.ipc"
            echo "removed stale fsmonitor ipc socket"
        fi
    fi

    # --- session marker TTL ---
    # Without this, a long-lived markers directory accumulates state from
    # weeks-old sessions and downstream gates (skill-invoked, agents-md-read,
    # rust-files-edited, ...) can spuriously pass because some prior session
    # touched the marker. The iter3-iter4 transition hit exactly this:
    # iter4 inherited iter3's `e2e-done` marker and the e2e gate looked
    # green when it had not actually run.
    #
    # Policy: any session marker dir whose mtime is more than 24h old gets
    # moved (not deleted) to `.archive/`, preserving forensics for review.
    # The current session is always kept; `.archive/` itself is skipped.
    session_root="$cwd/.claude/.session"
    current_sid=$(reviewq_jq '.session_id')
    if [[ -d "$session_root" ]]; then
        archive="$session_root/.archive"
        mkdir -p "$archive"
        while IFS= read -r stale; do
            base=$(basename "$stale")
            [[ "$base" == ".archive" || "$base" == "$current_sid" ]] && continue
            mv "$stale" "$archive/" 2>/dev/null && \
                echo "archived stale session dir (>24h): $base"
        done < <(find "$session_root" -mindepth 1 -maxdepth 1 -type d -mtime +1 2>/dev/null)
    fi
} > "$cleanup_log" 2>&1 || true

# --- collect context snippets --------------------------------------------
status_lines=$( (cd "$cwd" && git status --short --branch) 2>/dev/null | head -20 )
log_lines=$( (cd "$cwd" && git log --oneline -15) 2>/dev/null )
worktrees=$( (cd "$cwd" && git worktree list) 2>/dev/null )

progress=""
progress_path="$cwd/.claude/state/PROGRESS.md"
if [[ -f "$progress_path" ]]; then
    progress=$(head -40 "$progress_path")
fi

# Lines currently in AGENTS.md — noisy warning if it has drifted above
# the recommended 50-line target from the harness-engineering article.
agents_md_lines=""
if [[ -f "$cwd/AGENTS.md" ]]; then
    n=$(wc -l < "$cwd/AGENTS.md" | tr -d ' ')
    agents_md_lines="$n"
fi

# --- assemble the context message ----------------------------------------
msg=""
msg+=$'# reviewq session bootstrap\n\n'
msg+=$'You are a Claude Code agent working on the reviewq Rust CLI/TUI\n'
msg+=$'project. The session is governed by mechanically enforced hooks\n'
msg+=$'under .claude/hooks/ — see .claude/hooks/README.md for details.\n\n'
msg+=$'## Enforcement reminder (non-bypassable)\n\n'
msg+=$'- Every edit must be inside a .worktree/ directory.\n'
msg+=$'- AGENTS.md must be Read before the first edit.\n'
msg+=$'- A Skill() must be invoked before the first *.rs edit.\n'
msg+=$'- Production *.rs edits are blocked until a test file or\n'
msg+=$'  tdd-workflow / rust-testing skill has been touched.\n'
msg+=$'- `git commit` runs fmt-check / clippy -D warnings / cargo test\n'
msg+=$'  and requires a rust-review-done marker if *.rs is staged.\n'
msg+=$'- `cargo clippy --all-targets -D warnings` runs on Stop when *.rs\n'
msg+=$'  files were edited; you cannot end the turn with a broken build\n'
msg+=$'  or any clippy warning (including doc-comment lints).\n'
msg+=$'- Destructive bash (rm -rf wildcards, force-push, --no-verify,\n'
msg+=$'  git reset --hard) is blocked by safety-gate.sh.\n\n'

if [[ -n "$status_lines" ]]; then
    msg+=$'## git status\n\n```\n'
    msg+="$status_lines"
    msg+=$'\n```\n\n'
fi

if [[ -n "$log_lines" ]]; then
    msg+=$'## Recent commits (git log --oneline -15)\n\n```\n'
    msg+="$log_lines"
    msg+=$'\n```\n\n'
fi

if [[ -n "$worktrees" ]]; then
    msg+=$'## Active worktrees\n\n```\n'
    msg+="$worktrees"
    msg+=$'\n```\n\n'
fi

if [[ -n "$progress" ]]; then
    msg+=$'## Resume notes (.claude/state/PROGRESS.md)\n\n```\n'
    msg+="$progress"
    msg+=$'\n```\n\n'
fi

if [[ -n "$agents_md_lines" && "$agents_md_lines" -gt 120 ]]; then
    msg+=$'## ⚠️ AGENTS.md is getting long\n\n'
    msg+="Current size: ${agents_md_lines} lines. The harness-engineering\n"
    msg+=$'best practices recommend ≤50–80 lines using the Pointer Pattern\n'
    msg+=$'(reference .claude/rules/*.md instead of inlining). Consider a\n'
    msg+=$'chore/ worktree to trim it.\n\n'
fi

msg+=$'When in doubt, prefer /harness-status or cat\n'
msg+=$'.claude/.session/<session_id>/hook-log.jsonl to see what the\n'
msg+=$'enforcement layer has already decided this session.\n'

reviewq_log_event info "session-start bootstrap emitted"

# Claude Code SessionStart expects additionalContext under the
# hookSpecificOutput key with hookEventName = "SessionStart".
jq -Rn --arg msg "$msg" '{
    hookSpecificOutput: {
        hookEventName: "SessionStart",
        additionalContext: $msg
    }
}'
exit 0
