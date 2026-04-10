#!/usr/bin/env bash
# PreToolUse gate: block edits inside the reviewq main worktree.
#
# Rule (AGENTS.md → Git Workflow):
#   "Every development task — including single-file edits, typo fixes, and
#   docs-only changes — MUST start by creating a dedicated git worktree
#   under .worktree/<branch-name>/. Editing files in the main worktree is
#   not allowed, even when the change looks trivial."
#
# Policy:
#   - Paths outside the reviewq repo           → allow
#   - Paths under $reviewq_root/.worktree/**   → allow
#   - Paths under $reviewq_root/.claude/hooks/** → allow (self-install path;
#       the hooks have to be editable or they cannot be bootstrapped)
#   - Any other path inside $reviewq_root      → BLOCK

. "$(dirname "$0")/lib/common.sh"
reviewq_require_jq
reviewq_read_input

tool_name=$(reviewq_jq '.tool_name')
case "$tool_name" in
    Write|Edit|MultiEdit|NotebookEdit) ;;
    *) exit 0 ;;
esac

file_path=$(reviewq_jq '.tool_input.file_path')
[[ -z "$file_path" ]] && exit 0

project_dir=$(reviewq_project_dir)
[[ -z "$project_dir" ]] && exit 0

# Normalize to absolute path. If the input is relative, join with project_dir.
case "$file_path" in
    /*) abs_path="$file_path" ;;
    *)  abs_path="$project_dir/$file_path" ;;
esac

# Outside the reviewq repo → allow.
case "$abs_path" in
    "$project_dir"/*) ;;
    *) exit 0 ;;
esac

# Inside a worktree → allow.
case "$abs_path" in
    "$project_dir"/.worktree/*) exit 0 ;;
esac

# Bootstrap escape hatch: allow editing the hooks themselves from main.
# Without this, there is no way to install or fix the hooks, because editing
# them would block on the very gate they implement.
case "$abs_path" in
    "$project_dir"/.claude/hooks/*) exit 0 ;;
    "$project_dir"/.claude/.session/*) exit 0 ;;
esac

reviewq_block "Edit of '$file_path' is inside the main reviewq worktree.

AGENTS.md requires every task to run inside a git worktree under
.worktree/<branch>/, with no size or scope exemption.

Fix:
  git worktree add -b <type>/<slug> .worktree/<type>-<slug>
  cd .worktree/<type>-<slug>

Then retry the edit from the worktree. If you already have uncommitted
changes on main, stash first:
  git stash push -u
  git worktree add -b <type>/<slug> .worktree/<type>-<slug>
  git -C .worktree/<type>-<slug> stash pop"
