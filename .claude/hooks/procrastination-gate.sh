#!/usr/bin/env bash
# PreToolUse gate: block Write / Edit operations whose new content
# contains procrastination language ("TODO later", "FIXME: fix in next
# iteration", "後で対応", …).
#
# Rationale (from ignission's "hunting to farming" article):
#
#   "pre-bash-guard detects patterns like '次回対応' (next time) /
#    '今後改善' (future improvement). Then enforces: either fix now OR
#    create GitHub Issue first, then reply."
#
# This is the coarse Claude-Code equivalent. We use simple case-insensitive
# substring matching against well-known procrastination phrases. The
# agent can still defer legitimately by creating a real GitHub issue and
# referencing it in a comment (e.g. `// see #123`), which will bypass the
# pattern check because the phrase is replaced with a concrete pointer.
#
# We only scan the *new* content being written (new_string for Edit,
# content for Write). Existing file content is never checked, so
# legacy TODOs aren't flagged.

. "$(dirname "$0")/lib/common.sh"
reviewq_require_jq
reviewq_read_input

tool_name=$(reviewq_jq '.tool_name')
case "$tool_name" in
    Write|Edit|MultiEdit) ;;
    *) exit 0 ;;
esac

# Concat all the "new content" fields from the tool input so we can grep
# them in one pass. For Edit, that's new_string. For MultiEdit, it's
# edits[*].new_string. For Write, it's content.
new_text=$(printf '%s' "$REVIEWQ_INPUT" | jq -r '
    (.tool_input.new_string // ""),
    (.tool_input.content // ""),
    ((.tool_input.edits // []) | map(.new_string // "") | join("\n"))
' 2>/dev/null | tr -d '\r')
[[ -z "$new_text" ]] && exit 0

# Normalize to lowercase for the English patterns. Japanese patterns
# are matched as-is (case doesn't matter for kanji).
lower=$(printf '%s' "$new_text" | tr '[:upper:]' '[:lower:]')

patterns_en=(
    "todo: later"
    "todo later"
    "fixme later"
    "fix in the next iteration"
    "fix in next iteration"
    "will fix later"
    "implement later"
    "handle this later"
    "deferred to a future"
    "come back to this"
)
patterns_ja=(
    "後で対応"
    "後ほど対応"
    "次回対応"
    "次回の対応"
    "今後改善"
    "今後の改善"
    "あとで対応"
    "後日対応"
    "将来対応"
)

hit=""
for p in "${patterns_en[@]}"; do
    if printf '%s' "$lower" | grep -Fq -- "$p"; then
        hit="$p"
        break
    fi
done
if [[ -z "$hit" ]]; then
    for p in "${patterns_ja[@]}"; do
        if printf '%s' "$new_text" | grep -Fq -- "$p"; then
            hit="$p"
            break
        fi
    done
fi

[[ -z "$hit" ]] && exit 0

reviewq_block "Procrastination language detected in the new content: '$hit'

reviewq enforces the 'fix it now or track it concretely' rule. Vague
deferrals like 'TODO: later' accumulate and are never revisited.

Fix (pick one):
  1. Fix the issue in this turn and remove the comment.
  2. Open a GitHub issue NOW (gh issue create …) and reference it:
       // see #<issue-number>
     Concrete issue references are allowed; vague 'later' comments are
     not.
  3. If the deferral is truly the right call, use a precise form like:
       // blocked by upstream crate X, revisit once vX.Y ships
     so the trigger is unambiguous.

This gate is documented in .claude/rules/harness-engineering.md."
