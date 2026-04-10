#!/usr/bin/env bash
# PostToolUse feedback loop: run rustfmt on the just-edited Rust file and
# inject any formatting diff back into the agent's context via the
# hookSpecificOutput.additionalContext JSON channel.
#
# Rationale (from nyosegawa harness-engineering-best-practices-2026):
#
#   "Feedback Loop Speed Hierarchy:
#     1. Milliseconds (PostToolUse): Formatter auto-executes; agent
#        unaware of violation
#     2. Seconds (pre-commit): Linter + type checker block commit
#     3. Minutes (CI): Full test suite, deep analysis
#    Migrate checks upward whenever possible."
#
# This hook is the millisecond layer for Rust. `rustfmt --check` on a
# single file returns in ~50ms after warmup, which is fast enough to run
# on every Edit/Write without meaningfully slowing the agent loop.
#
# Output format: stdout JSON per Claude Code PostToolUse contract so the
# feedback is injected as *context*, not as a blocking error. The agent
# sees it on the next turn and self-corrects. Blocks happen later, at
# commit-gate time.

. "$(dirname "$0")/lib/common.sh"
reviewq_require_jq
reviewq_read_input

tool_name=$(reviewq_jq '.tool_name')
case "$tool_name" in
    Write|Edit|MultiEdit) ;;
    *) exit 0 ;;
esac

file_path=$(reviewq_jq '.tool_input.file_path')
[[ -z "$file_path" ]] && exit 0

# Only Rust files.
case "$file_path" in
    *.rs) ;;
    *) exit 0 ;;
esac

# Skip hook scripts directory; they are shell, not Rust.
project_dir=$(reviewq_project_dir)
case "$file_path" in
    "$project_dir"/.claude/*) exit 0 ;;
esac

[[ ! -f "$file_path" ]] && exit 0

# Mark that at least one Rust source file has been edited this session.
# The Stop hook uses this to decide whether to run `cargo check`.
reviewq_mark rust-files-edited

# Detect test-flavored files so the TDD gate knows they exist.
case "$file_path" in
    *_test.rs|*/tests/*|*/test_*.rs)
        reviewq_mark tests-edited
        reviewq_mark tdd-tests-written
        ;;
    *)
        # Also detect in-file tests via #[cfg(test)] or #[test].
        if grep -qE '^\s*#\[(cfg\(test\)|test)' "$file_path" 2>/dev/null; then
            reviewq_mark tests-edited
            reviewq_mark tdd-tests-written
        fi
        ;;
esac

# Run rustfmt in check mode. Returns 0 if already formatted, 1 if a diff
# would be applied. We explicitly want the diff, not the auto-fix, so the
# agent sees *what* changed rather than silently rewriting.
if ! command -v rustfmt >/dev/null 2>&1; then
    reviewq_log_event info "rustfmt not on PATH — skipping auto-feedback"
    exit 0
fi

# rustfmt needs the edition; read from Cargo.toml or default to 2024.
edition="2024"
if [[ -f "$project_dir/Cargo.toml" ]]; then
    found_edition=$(grep -E '^edition[[:space:]]*=' "$project_dir/Cargo.toml" | head -1 | sed -E 's/^edition[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/')
    if [[ -n "$found_edition" ]]; then
        edition="$found_edition"
    fi
fi

diff_output=$(rustfmt --edition "$edition" --check "$file_path" 2>&1)
rc=$?
if [[ $rc -eq 0 ]]; then
    reviewq_log_event allow "rustfmt clean on $file_path"
    exit 0
fi

# Truncate the diff so we don't spam the context window.
short_diff=$(printf '%s\n' "$diff_output" | head -120)
msg=$(printf 'rustfmt would reformat %s — please run `cargo fmt` or apply these changes manually before commit:\n\n%s' \
    "$file_path" "$short_diff")

reviewq_log_event feedback "rustfmt diff injected for $file_path"
reviewq_post_feedback "$msg"
exit 0
