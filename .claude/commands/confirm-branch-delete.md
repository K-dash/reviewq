---
description: Opt-in approval for force-deleting a branch with unmerged commits
argument-hint: "<branch-name>"
---

# /confirm-branch-delete — force-delete a branch with unmerged work

`.claude/hooks/safety-gate.sh` blocks `git branch -D <name>` by default.
Two automatic escape hatches exist:

1. If the target branch was regular-merged, `git branch -d` (lowercase)
   handles it; no opt-in needed.
2. If the target branch was **squash-merged** via a GitHub PR, the gate
   auto-unblocks when `gh pr list --head <name> --state merged` reports
   a `MERGED` PR.

This command is the **third escape hatch**: an explicit human opt-in
for the case where the branch is genuinely unmerged and you want to
discard the work anyway (e.g. an abandoned experiment that never landed
in any form).

## Arguments

- `$1` — the branch name to approve for deletion. Slashes are allowed;
  they are translated to `__` in the marker filename.

## What Claude does on this command

```bash
name="$1"
if [[ -z "$name" ]]; then
    echo "usage: /confirm-branch-delete <branch-name>"
    exit 1
fi

ROOT="$CLAUDE_PROJECT_DIR/.claude/.session"
SID=$(ls -t "$ROOT" 2>/dev/null | head -1)
if [[ -z "$SID" ]]; then
    echo "No active session dir. Run any tool first, then retry."
    exit 1
fi

safe_name="${name//\//__}"
mkdir -p "$ROOT/$SID"
touch "$ROOT/$SID/branch-delete-approved:$safe_name"

echo "✅ Branch-delete approved for '$name' in session $SID."
echo "   The marker will be discarded automatically at session end."
echo "   You can now run: git branch -D $name"
```

After the marker is set, retry the blocked `git branch -D` and
`safety-gate.sh` will allow it exactly once, for exactly this branch.
