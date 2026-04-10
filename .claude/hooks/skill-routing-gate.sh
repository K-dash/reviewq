#!/usr/bin/env bash
# PreToolUse gate: before editing any Rust source file, require that at
# least one Skill has been invoked in this session.
#
# Rule (`.claude/rules/skills.md`):
#   "Skills are not optional. Treat the rows below as mandatory defaults."
#   "Phase 3 — Implement. Trigger: Writing or editing any Rust source file.
#    Skill(s): rust-patterns, rust-skills:coding-guidelines"
#
# This gate is intentionally *coarse*: we only check that *some* Skill was
# invoked this session, not that the exact skill for the current phase was
# used. The point is to create friction that forces the router to be
# consulted at all. Fine-grained per-phase gating would produce too many
# false positives (e.g. refactors that legitimately don't need rust-async).

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

# Only enforce for Rust source files. Tests count; doc-only changes don't.
case "$file_path" in
    *.rs) ;;
    *) exit 0 ;;
esac

# Bootstrap: the hook scripts themselves are shell, not Rust, so they can
# never trip this gate. Leaving this for defense in depth.
project_dir=$(reviewq_project_dir)
case "$file_path" in
    "$project_dir"/.claude/hooks/*|.claude/hooks/*) exit 0 ;;
esac

if reviewq_has_mark skill-invoked; then
    exit 0
fi

reviewq_block "Editing a Rust source file without first invoking any Skill.

.claude/rules/skills.md makes skill routing mandatory for every task.
At minimum, before touching Rust code you are expected to have consulted:
  - rust-patterns              (Phase 3 — Implement)
  - rust-skills:coding-guidelines
And, if applicable:
  - search-first               (Phase 2 — before any new capability)
  - documentation-lookup       (Phase 2 — crate / API usage)
  - tdd-workflow + rust-testing (Phase 4 — tests first)

Fix:
  Invoke the relevant Skill via the Skill tool. Any Skill invocation this
  session will unblock the gate, so start with the routing phase that
  matches your task (planning, research, or implement)."
