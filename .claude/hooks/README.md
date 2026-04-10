# reviewq Claude Code Hooks

Mechanical enforcement of the AGENTS.md + `.claude/rules/skills.md`
workflow. The rules were soft guidance and Claude skipped them; these
hooks make the most important steps **blocking**.

## Layout

```
.claude/hooks/
├── lib/
│   └── common.sh                   # shared bash helpers (session dir, jq, block)
├── worktree-gate.sh                # PreToolUse  Edit/Write — worktree required
├── agents-md-gate.sh               # PreToolUse  Edit/Write — AGENTS.md must be read
├── skill-routing-gate.sh           # PreToolUse  Edit/Write on *.rs — Skill() required
├── commit-gate.sh                  # PreToolUse  Bash(git commit) — fmt/clippy/test + markers
├── mark-post-tool.sh               # PostToolUse Read/Skill/Agent — sets session markers
└── README.md                       # this file
```

Markers are touched under `.claude/.session/<session_id>/` and are
namespaced per session, so they do not leak between runs and do not
need explicit cleanup.

## Enforcement Matrix

| Phase | Rule source | Gate | Block condition |
|-------|-------------|------|-----------------|
| Workspace | AGENTS.md → Git Workflow | `worktree-gate.sh` | Edit target is inside the main reviewq repo and **not** under `.worktree/**` |
| Planning | CLAUDE.md → delegates all rules to AGENTS.md | `agents-md-gate.sh` | Any Edit/Write before `AGENTS.md` was Read this session |
| Implement | `.claude/rules/skills.md` Phase 3 | `skill-routing-gate.sh` | Edit of any `*.rs` file before **any** Skill was invoked this session |
| Commit — verification | AGENTS.md → Build & Quality | `commit-gate.sh` | `cargo fmt --check` / `cargo clippy -D warnings` / `cargo test` failing |
| Commit — review | `.claude/rules/skills.md` Phase 5 | `commit-gate.sh` | Rust files staged but no `rust-review-done` marker for this session |
| Commit — TUI e2e | `.claude/rules/skills.md` Phase 4 | `commit-gate.sh` | `src/tui/**` files staged but no `e2e-done` marker |
| Commit — worktree | AGENTS.md → Git Workflow | `commit-gate.sh` | `git commit` run from outside a `.worktree/` directory |

## Escape hatches

Only **two** bypasses exist, both intentional and scoped:

1. `worktree-gate.sh` allows edits under `$CLAUDE_PROJECT_DIR/.claude/hooks/**`
   and `.claude/.session/**`. Without this, the hooks could not be
   bootstrapped or patched (you would be unable to fix the very gate
   that is blocking you).
2. `agents-md-gate.sh` applies the same two-path allow-list for the same
   reason.

There is **no** environment variable, CLI flag, or config switch that
disables the gates. The global CLAUDE.md forbids `--no-verify` and any
hook bypass. If a gate is wrong, fix the hook — do not bypass it.

## Soft-fail policy

All hooks source `lib/common.sh`, which installs an `EXIT` trap that
converts any unexpected non-zero exit (unset variable, `jq` error, etc.)
into `exit 0`. Blocks are therefore **always explicit** — a buggy hook
can never stall legitimate work. The tradeoff is that a silently broken
hook no-ops; run `bash -n <hook>.sh` and the self-test below after any
edit to catch this.

## Self-test (no live session required)

Each hook can be exercised locally by feeding it a synthetic JSON
payload on stdin and checking the exit code + stderr:

```bash
# Should allow (Rust edit under .worktree/**):
echo '{
  "session_id": "test",
  "tool_name": "Edit",
  "tool_input": {"file_path": "/Users/me/reviewq/.worktree/foo/src/lib.rs"},
  "cwd": "/Users/me/reviewq/.worktree/foo"
}' | .claude/hooks/worktree-gate.sh; echo "exit=$?"

# Should block (Rust edit in main tree):
echo '{
  "session_id": "test",
  "tool_name": "Edit",
  "tool_input": {"file_path": "/Users/me/reviewq/src/lib.rs"},
  "cwd": "/Users/me/reviewq"
}' | CLAUDE_PROJECT_DIR=/Users/me/reviewq .claude/hooks/worktree-gate.sh; echo "exit=$?"
```

A curated set of these cases lives in `.claude/hooks/tests/` (see
`run-tests.sh`). CI should invoke it via `make test-hooks`.

## Adding a new gate

1. Create `<name>-gate.sh` following the pattern of the existing hooks:
   source `lib/common.sh`, call `reviewq_require_jq`, read input via
   `reviewq_read_input`, check a marker or a condition, and call
   `reviewq_block "…"` to stop the tool call.
2. Make it executable: `chmod +x .claude/hooks/<name>-gate.sh`.
3. Wire it up under the right matcher in `.claude/settings.json`.
4. Add a row to the Enforcement Matrix above.
5. Add positive and negative test cases to `.claude/hooks/tests/`.
