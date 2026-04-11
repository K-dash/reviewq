#!/usr/bin/env bash
# PostToolUse dispatcher: sets session-scoped marker files based on the
# tool that just finished executing.
#
# Markers we maintain (all live under .claude/.session/<session_id>/):
#
#   agents-md-read       — Read tool was called with file_path matching
#                          AGENTS.md. Unlocks agents-md-gate.sh.
#
#   skill-invoked        — Any Skill tool call. Unlocks
#                          skill-routing-gate.sh for Rust source edits.
#
#   skill:<name>         — Per-skill marker for future fine-grained gates.
#
#   rust-review-done     — The rust-reviewer agent (via Agent/Task tool)
#                          or the rust-review skill finished. Unlocks
#                          the rust-review check in commit-gate.sh.
#
#   tdd-tests-written    — tdd-workflow / rust-testing skill was invoked,
#                          OR a test file has been edited this session.
#                          Unlocks tdd-gate.sh.
#
#   tests-edited         — A test file (explicit path under tests/,
#                          *_test.rs, or a file containing #[cfg(test)])
#                          was edited.
#
#   rust-files-edited    — Any *.rs file was edited this session. Used by
#                          stop-gate.sh to decide whether to run
#                          `cargo check` on Stop.
#
# This hook is attached to PostToolUse with a broad matcher. It never
# blocks (always exit 0) and its only side effect is `touch`-ing marker
# files. Buggy marker logic therefore cannot stall legitimate work.

. "$(dirname "$0")/lib/common.sh"
reviewq_require_jq
reviewq_read_input

tool_name=$(reviewq_jq '.tool_name')

# Helper: given a file path, mark rust-files-edited and tests-edited if
# the path or its contents indicate a Rust file / test file.
_mark_rust_file() {
    local fp="$1"
    [[ -z "$fp" ]] && return 0
    case "$fp" in
        *.rs) ;;
        *) return 0 ;;
    esac
    reviewq_mark rust-files-edited
    case "$fp" in
        *_test.rs|*/tests/*|*/test_*.rs)
            reviewq_mark tests-edited
            reviewq_mark tdd-tests-written
            return 0
            ;;
    esac
    # Peek inside the file (if it exists) for in-file test markers.
    if [[ -f "$fp" ]] && grep -qE '^\s*#\[(cfg\(test\)|test)' "$fp" 2>/dev/null; then
        reviewq_mark tests-edited
        reviewq_mark tdd-tests-written
    fi
}

case "$tool_name" in
    Read)
        file_path=$(reviewq_jq '.tool_input.file_path')
        case "$file_path" in
            */AGENTS.md|AGENTS.md)
                reviewq_mark agents-md-read
                reviewq_log_event mark "agents-md-read set"
                ;;
        esac
        ;;

    Skill)
        skill_name=$(reviewq_jq '.tool_input.skill')
        reviewq_mark skill-invoked
        if [[ -n "$skill_name" ]]; then
            # Marker filenames cannot contain slashes; replace with `__`.
            safe=$(printf '%s' "$skill_name" | tr '/:' '__')
            reviewq_mark "skill:$safe"
            case "$skill_name" in
                rust-testing|tdd-workflow)
                    reviewq_mark tdd-tests-written
                    ;;
                rust-review)
                    reviewq_mark rust-review-done
                    ;;
            esac
            reviewq_log_event mark "skill:$skill_name marker set"
        fi
        ;;

    Agent|Task)
        subagent=$(reviewq_jq '.tool_input.subagent_type')
        case "$subagent" in
            rust-reviewer)
                reviewq_mark rust-review-done
                reviewq_log_event mark "rust-review-done set via Agent(rust-reviewer)"
                ;;
            tdd-guide)
                reviewq_mark tdd-tests-written
                reviewq_log_event mark "tdd-tests-written set via Agent(tdd-guide)"
                ;;
        esac
        ;;

    Write|Edit|MultiEdit)
        file_path=$(reviewq_jq '.tool_input.file_path')
        _mark_rust_file "$file_path"
        ;;
esac

exit 0
