---
description: Unlock protected config file edits for this session (Cargo.toml, Makefile, CI workflows, etc.)
---

# /config-edit — approve config edits for this session

Protected config files (`Cargo.toml`, `Cargo.lock`, `Makefile`, `clippy.toml`,
`rustfmt.toml`, `deny.toml`, `rust-toolchain.toml`, `.gitignore`, and
`.github/workflows/**`) are locked by default by
`.claude/hooks/config-protect-gate.sh`.

This command sets a session-scoped marker that unblocks edits to those
files for the remainder of the current Claude Code session. The marker
is stored under `.claude/.session/<session_id>/config-edit-approved`
and is automatically discarded when the session ends.

## When to use

- You intentionally want to add a dependency to `Cargo.toml`.
- You are bumping a tool version in `rust-toolchain.toml`.
- You are patching CI in `.github/workflows/`.
- You are tightening linter strictness in `clippy.toml`.

Do **not** use this to silence gate warnings — if the hook is wrong,
fix the hook in a `chore/` worktree instead.

## What Claude does on this command

```bash
SID=$(ls -t "$CLAUDE_PROJECT_DIR/.claude/.session/" 2>/dev/null | head -1)
if [[ -z "$SID" ]]; then
    echo "No active session dir. Try editing any file first, then retry."
    exit 1
fi
mkdir -p "$CLAUDE_PROJECT_DIR/.claude/.session/$SID"
touch "$CLAUDE_PROJECT_DIR/.claude/.session/$SID/config-edit-approved"
echo "✅ Config-edit approved for session $SID."
echo "   The marker will be discarded automatically at session end."
```

After the marker is set, retry the blocked edit and the
`config-protect-gate.sh` will allow it.
