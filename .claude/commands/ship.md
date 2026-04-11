---
description: Push the current worktree branch and open a PR — runs make all first, drafts title/body from the branch's commits.
---

# /ship — push + open PR for the current worktree

Half-automated "work is done, ship it" handoff. Collapses the
repetitive three-step at the end of every feature —
`make all` → `git push -u` → `gh pr create` — into a single command.

Deliberately **not** a Stop hook. Completion-of-work does not imply
readiness-to-ship: you might want to stack more commits, amend, or
wait for a review before opening a PR. See
`.claude/rules/harness-engineering.md` for the broader speed-hierarchy
argument; `/ship` sits at the boundary between the local loop and the
shared-state loop, which is exactly where human intent should still be
explicit.

## Preconditions (Claude verifies these before shipping)

1. **Inside a `.worktree/<branch>/` dir.** Shipping from the main
   worktree is a bug; bail out.
2. **Not on `main`.** Feature branches only.
3. **Working tree clean.** No staged/unstaged changes — those should
   already be committed via `commit-oss` or a regular commit.
4. **At least one commit ahead of `origin/main`.** Otherwise there is
   nothing to ship.
5. **`gh auth status` is OK.** Otherwise `gh pr create` will fail.
6. **`make all` is green.** Last line of defense — matches what
   `commit-gate.sh` runs, but re-verifies in case tooling drifted.

If any precondition fails, print what is wrong and stop. Never push or
create a PR with a failing verification — `--no-verify` and hook
bypasses are forbidden per `.claude/rules/harness-engineering.md`.

## What Claude does on this command

```bash
set -euo pipefail

# Resolve repo root from the current worktree. Do NOT cd to the main
# worktree; /ship is meant to operate on the current branch.
TOP=$(git rev-parse --show-toplevel)
cd "$TOP"

# 1. Must be inside a .worktree/<branch>/ directory.
case "$TOP" in
    */.worktree/*) ;;
    *) echo "❌ /ship must run from inside .worktree/<branch>/. Abort."; exit 1 ;;
esac

# 2. Must not be on main/master.
BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [[ "$BRANCH" == "main" || "$BRANCH" == "master" || "$BRANCH" == "HEAD" ]]; then
    echo "❌ Refusing to ship from '$BRANCH'. Create a feature branch first."
    exit 1
fi

# 3. Working tree must be clean.
if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "❌ Working tree has uncommitted changes. Commit them first (e.g. /commit-oss)."
    git status --short
    exit 1
fi

# 4. Must be ahead of origin/main.
git fetch --quiet origin main || true
AHEAD=$(git rev-list --count origin/main..HEAD 2>/dev/null || echo 0)
if [[ "$AHEAD" == "0" ]]; then
    echo "❌ No commits ahead of origin/main. Nothing to ship."
    exit 1
fi
BEHIND=$(git rev-list --count HEAD..origin/main 2>/dev/null || echo 0)
if [[ "$BEHIND" != "0" ]]; then
    echo "⚠️  Branch is $BEHIND commit(s) behind origin/main. Consider rebasing first:"
    echo "     git fetch origin && git rebase origin/main"
fi
echo "ℹ️  $AHEAD commit(s) ahead of origin/main on $BRANCH."

# 5. gh auth.
if ! gh auth status >/dev/null 2>&1; then
    echo "❌ gh is not authenticated. Run: gh auth login"
    exit 1
fi

# 6. make all — last line of defense.
echo "▶ make all"
if ! make all; then
    echo "❌ make all failed. Fix before shipping."
    exit 1
fi

# 7. Push (tracking branch may not exist yet → -u).
echo "▶ git push -u origin $BRANCH"
git push -u origin "$BRANCH"

# 8. If a PR already exists for this branch, show it and stop.
if gh pr view --json url >/dev/null 2>&1; then
    URL=$(gh pr view --json url -q .url)
    echo "ℹ️  PR already exists for $BRANCH — $URL"
    gh pr view
    exit 0
fi

# 9. Collect material for Claude to draft the PR from.
echo "---- commits on this branch ----"
git log origin/main..HEAD --format='%h %s%n%n%b%n----'
echo "---- diffstat ----"
git diff --stat origin/main...HEAD
```

After the bash block succeeds, Claude drafts the PR title and body
from the branch's commits + diffstat above, then opens the PR.

## Drafting the PR title & body

Claude reads the branch's full commit history and diffstat (from the
last two bash sections), then drafts:

- **Title**: ≤70 chars, Conventional Commits prefix matching the
  branch prefix (`feat:`, `fix:`, `chore:`, …). Never copy a single
  commit subject blindly — if the branch has multiple commits,
  summarize the overall change.
- **Body**: following the repo template
  - `## Summary` — 1–3 bullets explaining *why*, not just *what*
  - `## Test plan` — concrete checklist items. At minimum `make all`
    passing locally, plus any manual verification that applies to the
    change (TUI render tests for `src/tui/**`, hook self-tests for
    `.claude/hooks/**`, etc.)

Then it calls:

```bash
gh pr create --title "$TITLE" --body "$(cat <<'EOF'
$BODY
EOF
)"
```

via a HEREDOC to preserve formatting. On success, print the PR URL so
the user can click through.

## Not done by /ship (intentional)

- **Does not merge.** PR approval and merge are a human decision.
- **Does not run `/cleanup-worktree`.** That belongs in a separate
  post-merge step — the worktree is still useful until the PR lands.
- **Does not force-push.** If the remote branch has diverged, abort
  and tell the user to resolve manually (rebase or reset on purpose).
- **Does not bypass any hook.** `--no-verify`, `--no-gpg-sign`, and
  `-c commit.gpgsign=false` are globally forbidden.
