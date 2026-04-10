#!/usr/bin/env bash
# Shared helpers for all reviewq Claude Code hooks.
#
# Source this at the top of every hook script:
#     . "$(dirname "$0")/lib/common.sh"
#
# All hooks read a single JSON object from stdin (the tool call payload).
# Exit codes:
#   0 = allow the tool call (or: success / no-op for PostToolUse)
#   2 = block the tool call with a message printed to stderr
#   *  = treated as an error — we avoid this so a buggy hook cannot stall
#         legitimate work. On internal errors we print to stderr and exit 0.
#
# Session-scoped marker files live under:
#     $CLAUDE_PROJECT_DIR/.claude/.session/<session_id>/<marker>
#
# The directory is gitignored and namespaced per session, so it does not
# need to be reset explicitly.

set -u

# --- soft-fail trap --------------------------------------------------------
# If anything below blows up (unset var, jq error, missing dep), fall through
# to exit 0 so we never wedge the developer. Blocks are ALWAYS explicit via
# reviewq_block().
reviewq_soft_fail() {
    local status=$?
    if [[ $status -ne 0 && $status -ne 2 ]]; then
        echo "[reviewq-hook] internal error (status=$status) in ${BASH_SOURCE[1]:-hook} — allowing" >&2
        exit 0
    fi
}
trap reviewq_soft_fail EXIT

# --- input capture ---------------------------------------------------------
# Read stdin JSON once and expose it as REVIEWQ_INPUT.
reviewq_read_input() {
    if [[ -z "${REVIEWQ_INPUT:-}" ]]; then
        REVIEWQ_INPUT=$(cat)
    fi
    export REVIEWQ_INPUT
}

# Extract a field from the input JSON. Usage: reviewq_jq '.tool_name'
reviewq_jq() {
    printf '%s' "$REVIEWQ_INPUT" | jq -r "$1 // empty" 2>/dev/null || true
}

# --- project paths ---------------------------------------------------------
reviewq_project_dir() {
    # Prefer the env var Claude Code sets; fall back to the cwd from JSON.
    if [[ -n "${CLAUDE_PROJECT_DIR:-}" ]]; then
        printf '%s' "$CLAUDE_PROJECT_DIR"
    else
        reviewq_jq '.cwd'
    fi
}

# Session state directory (created on demand, gitignored).
reviewq_session_dir() {
    local sid
    sid=$(reviewq_jq '.session_id')
    if [[ -z "$sid" ]]; then
        sid="unknown-session"
    fi
    local root
    root=$(reviewq_project_dir)
    local dir="$root/.claude/.session/$sid"
    mkdir -p "$dir"
    printf '%s' "$dir"
}

# Touch a marker file in the session dir. Usage: reviewq_mark agents-md-read
reviewq_mark() {
    local name="$1"
    local dir
    dir=$(reviewq_session_dir)
    touch "$dir/$name"
}

# Test whether a marker exists. Usage: if reviewq_has_mark x; then ...
reviewq_has_mark() {
    local name="$1"
    local dir
    dir=$(reviewq_session_dir)
    [[ -f "$dir/$name" ]]
}

# --- blocking --------------------------------------------------------------
# Block the tool call with a message. Goes to stderr so Claude sees it.
# Logs the block decision to hook-log.jsonl before exiting so we can audit
# why the agent was stopped even if the stderr output is truncated.
reviewq_block() {
    local msg="$1"
    # Best-effort log; never let logging failure affect the block.
    reviewq_log_event block "$(printf '%s' "$msg" | head -1)" 2>/dev/null || true
    echo "" >&2
    echo "🛑 reviewq workflow gate blocked this action:" >&2
    echo "" >&2
    local line
    while IFS= read -r line; do
        echo "   $line" >&2
    done <<< "$msg"
    echo "" >&2
    exit 2
}

# --- helpers ---------------------------------------------------------------
# True if `jq` is available. We hard-require it; without jq the hooks cannot
# parse the input JSON so they soft-fail open (see the EXIT trap above).
reviewq_require_jq() {
    command -v jq >/dev/null 2>&1 || {
        echo "[reviewq-hook] 'jq' not found on PATH — hook will no-op" >&2
        exit 0
    }
}

# --- observability ---------------------------------------------------------
# Append a structured event to the session's hook-log.jsonl. Used so we can
# reconstruct *why* a given tool call was blocked or allowed after the fact
# without re-running the session. Fields:
#
#   ts      — ISO-8601 UTC timestamp
#   hook    — basename of the hook script
#   tool    — tool_name from the input JSON
#   decision — "allow" | "block" | "feedback" | "mark" | "info"
#   reason  — short free-text
#
# Usage:
#   reviewq_log_event allow "tool in worktree — no-op"
#   reviewq_log_event block "destructive command: rm -rf /"
reviewq_log_event() {
    local decision="$1"
    local reason="${2:-}"
    local dir
    dir=$(reviewq_session_dir) || return 0
    local hook_name
    hook_name=$(basename "${BASH_SOURCE[1]:-unknown}")
    local tool
    tool=$(reviewq_jq '.tool_name')
    local ts
    ts=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    # Single-line JSONL so it's trivially grep/jq-able after the fact.
    jq -nc \
        --arg ts "$ts" \
        --arg hook "$hook_name" \
        --arg tool "$tool" \
        --arg decision "$decision" \
        --arg reason "$reason" \
        '{ts:$ts, hook:$hook, tool:$tool, decision:$decision, reason:$reason}' \
        >> "$dir/hook-log.jsonl" 2>/dev/null || true
}

# --- PostToolUse feedback injection ----------------------------------------
# Emit a JSON object on stdout in the format Claude Code expects for
# PostToolUse hooks that want to inject additional context back to the
# agent. Reference: nyosegawa harness-engineering article.
#
#   reviewq_post_feedback "clippy warnings:\n..."
#
# The agent will see this text as part of the tool result and can self-
# correct on the next turn. This is the "millisecond feedback loop" layer
# at the heart of Harness Engineering.
reviewq_post_feedback() {
    local msg="$1"
    jq -Rn --arg msg "$msg" '{
        hookSpecificOutput: {
            hookEventName: "PostToolUse",
            additionalContext: $msg
        }
    }'
}

# --- Stop hook decision output ---------------------------------------------
# Emit the Stop-hook JSON that blocks the agent from claiming "done".
# Usage: reviewq_stop_block "cargo check failed: ..."
reviewq_stop_block() {
    local reason="$1"
    jq -Rn --arg reason "$reason" '{
        decision: "block",
        reason: $reason
    }'
}
