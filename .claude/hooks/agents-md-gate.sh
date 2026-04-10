#!/usr/bin/env bash
# PreToolUse gate: require that AGENTS.md has been read before any edit.
#
# Rationale: AGENTS.md is the authoritative source for the build, git, and
# review workflow. The project CLAUDE.md delegates every rule to it. Past
# sessions have shown Claude will skip it unless the workflow is mechanically
# enforced, so we require a session-scoped marker before allowing any edit.
#
# The marker is set by mark-post-tool.sh when Claude reads AGENTS.md via the
# Read tool. It lives in .claude/.session/<session_id>/agents-md-read and is
# automatically discarded at the end of the session.

. "$(dirname "$0")/lib/common.sh"
reviewq_require_jq
reviewq_read_input

tool_name=$(reviewq_jq '.tool_name')
case "$tool_name" in
    Write|Edit|MultiEdit|NotebookEdit) ;;
    *) exit 0 ;;
esac

# Bootstrap: allow edits that only touch the hook infrastructure or the
# session directory, so we can install/patch the gate itself.
file_path=$(reviewq_jq '.tool_input.file_path')
project_dir=$(reviewq_project_dir)
case "$file_path" in
    "$project_dir"/.claude/hooks/*|"$project_dir"/.claude/.session/*) exit 0 ;;
    .claude/hooks/*|.claude/.session/*) exit 0 ;;
esac

if reviewq_has_mark agents-md-read; then
    exit 0
fi

reviewq_block "AGENTS.md has not been read in this session.

The project CLAUDE.md points to AGENTS.md as the single source of truth
for the build, git-worktree, quality, and review workflow. Every session
must consult it before editing code.

Fix:
  Use the Read tool on $project_dir/AGENTS.md.
  A post-tool hook will record the read and unblock this gate."
