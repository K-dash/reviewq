#!/usr/bin/env bash
# Stop hook: when the agent signals "done", verify the build actually
# compiles before letting the turn end. Emits a JSON `{"decision":"block"}`
# response when the check fails so Claude is forced to keep working.
#
# Rationale (from harness-engineering articles):
#
#   "Stop Hook: End-of-response hooks... test verification before agent
#    claims done." — nyosegawa, coding-agent-workflow-2026
#
#   "Silent corruption: Agents declare 'done' without verifying end-to-end
#    behavior." — OpenAI, one of the three harness failure modes
#
# Policy:
#   - If no Rust source files were edited this session (no
#     rust-files-edited marker), exit 0 (nothing to verify).
#   - Otherwise run `cargo check --quiet` from the session's cwd.
#     We use `check`, not `test`, because:
#       * check catches ~95% of "silent done" lies (compile errors,
#         missing imports, broken signatures),
#       * test is slow enough that running it on every turn would make
#         the agent loop sluggish,
#       * commit-gate.sh already enforces full `cargo test` before a
#         commit can land, which is the real quality gate.
#   - On failure, emit `{"decision":"block","reason":...}` JSON so Claude
#     sees it and keeps working.
#   - Also honor a `tests-just-passed` marker: if the commit-gate (or a
#     /verify slash command) just ran a full suite, skip the check to
#     avoid double-work.

. "$(dirname "$0")/lib/common.sh"
reviewq_require_jq
reviewq_read_input

# SessionStart / Stop inputs don't carry a tool_name. Guard by hook_event_name.
event=$(reviewq_jq '.hook_event_name')
[[ "$event" != "Stop" ]] && exit 0

# Skip if the agent never touched Rust code this session.
if ! reviewq_has_mark rust-files-edited; then
    reviewq_log_event allow "Stop: no rust files edited, nothing to check"
    exit 0
fi

# Skip if a full verification just ran. The marker is set by commit-gate
# after a successful build, and naturally expires at session end.
if reviewq_has_mark tests-just-passed; then
    reviewq_log_event allow "Stop: tests-just-passed marker present, skip"
    exit 0
fi

cwd=$(reviewq_jq '.cwd')
[[ -z "$cwd" ]] && cwd=$(reviewq_project_dir)
[[ ! -d "$cwd" ]] && exit 0

# Only run if this directory looks like a Rust crate.
if [[ ! -f "$cwd/Cargo.toml" ]]; then
    # Walk up a few levels in case the Stop hook fires from a subdir.
    found=""
    probe="$cwd"
    for _ in 1 2 3 4; do
        probe=$(dirname "$probe")
        if [[ -f "$probe/Cargo.toml" ]]; then
            found="$probe"
            break
        fi
    done
    if [[ -z "$found" ]]; then
        reviewq_log_event allow "Stop: no Cargo.toml found near $cwd"
        exit 0
    fi
    cwd="$found"
fi

log_file="$(reviewq_session_dir)/stop-gate-check.log"
if (cd "$cwd" && cargo check --quiet --all-targets) >"$log_file" 2>&1; then
    reviewq_mark tests-just-passed
    reviewq_log_event allow "Stop: cargo check passed"
    exit 0
fi

tail_output=$(tail -40 "$log_file")
reason=$(printf 'Stop blocked: `cargo check` does not compile. You cannot claim "done" while the build is broken.\n\nLast 40 lines:\n%s\n\nFix the compile errors (and their root causes — do not #[allow] them) before ending the turn. The commit-gate will additionally enforce full clippy + test on git commit.' "$tail_output")

reviewq_log_event block "Stop: cargo check failed"

# Emit Stop-hook JSON on stdout so Claude treats this as a block-with-
# guidance rather than a fatal error.
jq -n --arg reason "$reason" '{
    decision: "block",
    reason: $reason
}'
exit 0
