#!/usr/bin/env bash
# PreToolUse gate: enforce "tests first" before any production Rust file
# edit in a session.
#
# Rationale:
#   AGENTS.md → Plan-First Rule + `.claude/rules/skills.md` Phase 4:
#     "TDD Approach: Write tests first (RED), implement to pass tests
#      (GREEN), refactor (IMPROVE)."
#   nyosegawa coding-agent-workflow-2026: "tdd-guard Hook: Prevents code
#     generation without passing tests."
#   ignission hunting-to-farming: "rules don't block; hooks do."
#
# Policy:
#   Before an agent may edit any *production* Rust file in this session,
#   at least ONE of these must hold:
#     (a) A test file has been edited or created in this session
#         (marker: tests-edited)
#     (b) The `tdd-workflow` or `rust-testing` skill has been invoked
#         (marker: tdd-tests-written — set by mark-post-tool.sh)
#     (c) The edit itself targets a test file (it's a test, not prod code)
#     (d) The edit is under .claude/hooks/ (bootstrap exemption)
#
# Rationale for (a): manually editing a test file counts as "tests first"
# because the agent has at minimum *touched* the test surface before
# touching production. This is looser than a strict git-history check but
# much harder to cheat than marker-only.
#
# We deliberately do NOT enforce that the test failed before implementation,
# because detecting Red→Green mechanically requires running `cargo test`
# between every edit, which is too slow for the PreToolUse layer.

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

# Non-Rust edits never trip this gate.
case "$file_path" in
    *.rs) ;;
    *) exit 0 ;;
esac

project_dir=$(reviewq_project_dir)

# Bootstrap: hook scripts and session state can always be edited.
case "$file_path" in
    "$project_dir"/.claude/*) exit 0 ;;
    .claude/*) exit 0 ;;
esac

# (c) The edit itself IS a test — allow.
case "$file_path" in
    *_test.rs|*/tests/*|*/test_*.rs) exit 0 ;;
esac

# For *.rs files that aren't explicitly under tests/, peek inside to see
# if this file is an in-file test module. If the existing file contains
# #[cfg(test)] or #[test], treat the edit as a test edit.
if [[ -f "$file_path" ]] && grep -qE '^\s*#\[(cfg\(test\)|test)' "$file_path" 2>/dev/null; then
    exit 0
fi

# (a) & (b): check markers.
if reviewq_has_mark tests-edited || reviewq_has_mark tdd-tests-written; then
    exit 0
fi

reviewq_block "TDD gate: editing production Rust file without any prior test.

'$file_path' is a production Rust source file, but this session has not
yet:
  (a) edited or created any test file, or
  (b) invoked the tdd-workflow / rust-testing skill, or
  (c) edited a file that already contains #[cfg(test)] / #[test].

Test-Driven Development is mandatory per AGENTS.md and
.claude/rules/skills.md Phase 4:
  1. Red    — write a failing test that expresses the requirement.
  2. Green  — implement the minimum code needed to make the test pass.
  3. Refactor — keep tests green, clean logic.

Fix (pick one):
  - Invoke the tdd-workflow or rust-testing skill to plan the test:
      Skill(tdd-workflow)     or     Skill(rust-testing)
  - Or start by creating/editing the relevant test file first. Any
    *_test.rs, tests/**/*.rs, or *.rs file containing #[cfg(test)]
    qualifies and will set the marker automatically.

This gate exists because soft 'please write tests first' guidance has
historically been ignored. The only way past it is to actually write a
test or explicitly plan one."
