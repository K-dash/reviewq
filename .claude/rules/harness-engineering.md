# Harness Engineering for reviewq

> "The human's job shifts from 'writing correct code' to 'designing
> environments where agents reliably produce correct code.'"
> — Mitchell Hashimoto

This document explains *why* every hook in `.claude/hooks/` exists and
how they compose into an autonomous-but-safe workflow for the reviewq
Rust CLI/TUI. It is the project's theoretical spine. The mechanical
enforcement lives in `.claude/hooks/`; the operational how-to lives in
`AGENTS.md` and `.claude/rules/skills.md`.

## The three failure modes (OpenAI)

Every gate below targets at least one of these:

1. **Drift** — the agent copies a nearby anti-pattern instead of the
   ideal pattern. Counter: `skill-routing-gate`, `rust-review` marker.
2. **Scope creep** — the agent sacrifices non-target goals to get a
   narrow objective done. Counter: `worktree-gate`, plan-first rule,
   `config-protect-gate`.
3. **Silent corruption** — the agent declares "done" without verifying
   behavior. Counter: `stop-gate`, `commit-gate`, `post-edit-rust`.

## Feedback loop speed hierarchy

From nyosegawa's harness-engineering best-practices article:

```
  milliseconds   — PostToolUse formatter (post-edit-rust.sh → rustfmt --check)
  seconds        — PreToolUse gates (worktree / agents-md / skill / tdd)
  ~2 sec         — Stop hook (cargo clippy --all-targets -- -D warnings)
  tens of sec    — commit-gate (cargo fmt --check + clippy + test)
  minutes        — CI (full test matrix, not yet wired)
  hours+         — human review (PR approval, only on risky changes)
```

**Rule**: whenever a check exists only at CI, ask whether it can migrate
into commit-gate. Whenever it exists at commit-gate, ask whether it can
migrate into the Stop or PostToolUse layer. Faster feedback = smaller
defect windows = less wasted agent work.

## Two layers: pro-flow and anti-violation

The harness has two distinct enforcement layers that compose:

```
ユーザー入力
    ↓
[/begin <task>]    ← pro-flow      (proactively presents the correct
    ↓                                workflow before any action)
agent runs steps 1-13
    ↓
[12 hooks]         ← anti-violation (reactively blocks broken actions
    ↓                                when the agent slips)
[/ship]            ← exit gate     (push + PR with full preconditions)
    ↓
完成
```

### Anti-violation (the 12 hooks)

The Hook index below catalogs the reactive layer. Each hook fires on a
specific tool event and blocks the agent if a precondition is missing.
This layer is **necessary but not sufficient**: it tells the agent
*after the fact* that something is wrong, which wastes turns and
produces fragmented error messages spread across the session.

### Pro-flow (the `/begin` command)

`/begin <task>` (`.claude/commands/begin.md`) is the user-invoked
entry point that hands the agent the entire workflow up front, in
order, before any action is taken. Steps 1–13 cover: **`Skill(grill-me)`
to interview the user until requirements are clear** → location check →
AGENTS.md read → **plan-mode decision** → worktree creation → skill
routing → write tests first → implement → `make all` → `/rust-review`
→ commit (with user approval) → `/ship` → cleanup after merge.

`/begin` does **not** bypass any hook. Every Edit/Write/Bash inside
its steps still passes through the normal PreToolUse gates, the Stop
hook still runs clippy at turn end, and `/ship` still has its own
preconditions. The two layers are defense in depth, not alternatives.

### Why both are needed

- Pro-flow alone is **best-effort**: the agent might forget a step
  mid-task. The hooks catch the slip.
- Anti-violation alone is **reactive**: the agent has to *try* to do
  something wrong before being stopped. Pro-flow primes the correct
  sequence so hooks rarely have to fire.
- Plan-first specifically **cannot** live in the hook layer because
  Plan Mode is invisible to PreToolUse hooks. It only lives in
  `/begin` Step 3. See `.claude/rules/plan-first.md` for the
  reasoning.

When the user requests a code change, they should start with `/begin
<task>`. When the agent inevitably slips (forgets to read AGENTS.md,
edits before invoking a skill, tries to commit without rust-review),
the hooks catch it.

## Hook index

| Hook | Event | Purpose |
|------|-------|---------|
| `session-start.sh`        | SessionStart | Emit git status / log / worktree list / PROGRESS.md as context so the agent resumes with state. Also expires session marker dirs older than 24h to `.claude/.session/.archive/`. |
| `worktree-gate.sh`        | PreToolUse Edit/Write | Block edits outside `.worktree/` per AGENTS.md. |
| `agents-md-gate.sh`       | PreToolUse Edit/Write | Require AGENTS.md was Read this session. |
| `config-protect-gate.sh`  | PreToolUse Edit/Write | Block edits to Cargo.toml / Makefile / CI / linter config unless `/config-edit` unlocks. |
| `skill-routing-gate.sh`   | PreToolUse Edit/Write on `*.rs` | Require at least one Skill() call before editing Rust. |
| `tdd-gate.sh`             | PreToolUse Edit/Write on `*.rs` | Require a test file or tdd skill before production Rust edits. |
| `procrastination-gate.sh` | PreToolUse Edit/Write | Block "TODO: later" / "後で対応" patterns in new content. |
| `safety-gate.sh`          | PreToolUse Bash | Block `rm -rf /`, `git push --force`, `git reset --hard`, `--no-verify`, `curl \| sh`, Bash-level config writes. |
| `commit-gate.sh`          | PreToolUse Bash (`git commit`) | Run fmt-check / clippy / test + require `rust-review-done` marker when Rust is staged. |
| `post-edit-rust.sh`       | PostToolUse Edit/Write | Run `rustfmt --check` on edited Rust files and inject the diff back as `additionalContext`. |
| `mark-post-tool.sh`       | PostToolUse Read/Skill/Agent/Write/Edit | Update session markers so downstream gates can check state. |
| `stop-gate.sh`            | Stop | Run `cargo clippy --all-targets -- -D warnings` if Rust was edited; emit `{decision: "block"}` on failure. Catches both compile errors and clippy lints (e.g. `clippy::doc_markdown`) at turn boundary instead of letting them escape to commit-gate. |

## Marker vocabulary

Markers live under `.claude/.session/<session_id>/`. They are just empty
files; their presence encodes workflow state.

| Marker | Set by | Read by | Meaning |
|--------|--------|---------|---------|
| `agents-md-read`        | Read(AGENTS.md) | agents-md-gate | AGENTS.md consumed this session |
| `skill-invoked`         | any Skill() | skill-routing-gate | some routing skill was used |
| `skill:<name>`          | Skill(name) | *(future gates)* | per-skill marker |
| `rust-files-edited`     | Edit/Write on `*.rs` | stop-gate, harness-status | Rust was touched |
| `tests-edited`          | Edit/Write on test files | *(audit)* | a test file was edited |
| `tdd-tests-written`     | rust-testing/tdd skill OR test file edit | tdd-gate | TDD prerequisite satisfied |
| `rust-review-done`      | Agent(rust-reviewer) / Skill(rust-review) | commit-gate | review completed |
| `tests-just-passed`     | commit-gate after `cargo test` | stop-gate | skip redundant check |
| `config-edit-approved`  | /config-edit slash command | config-protect-gate | config edits unlocked |
| `branch-delete-approved:<safe_name>` | /confirm-branch-delete slash command | safety-gate (class 5) | force-delete of that specific branch unlocked once |

## Never bypass

Per global `~/.claude/CLAUDE.md`:

- `--no-verify`, `--no-gpg-sign`, `-c commit.gpgsign=false` are forbidden.
- Any environment variable or CLI flag that disables hooks is forbidden.
- If a hook is wrong, patch it in a `chore/hook-*` worktree, add a
  regression test in `.claude/hooks/tests/run-tests.sh`, and re-run
  `make test-hooks`. Do not bypass.
- `.claude/hooks/**` and `.claude/.session/**` are the only paths where
  edits on the main tree are allowed, and only to bootstrap the hooks
  themselves.

## Observability

Every block decision is appended to
`.claude/.session/<session_id>/hook-log.jsonl` as a single JSON line
`{ts, hook, tool, decision, reason}`. Use `/harness-status` to inspect
the current session at any time. The log is session-scoped and does
not persist across sessions — for long-lived debugging, copy relevant
lines into `.claude/state/PROGRESS.md`.

## Known git worktree gotchas

Removing a worktree does **not** always clean up git's internal state
cleanly on macOS:

- **fsmonitor daemon survival**: if `git worktree remove` runs while a
  per-worktree `fsmonitor--daemon` is still alive, the daemon keeps
  running and pointing at the deleted path. Subsequent `git rev-parse
  --show-toplevel` calls from anywhere in the repo may return the
  deleted path, and `git checkout -- .` fails with *"fatal: this
  operation must be run in a work tree"*.
- **stale admin dir**: the `.git/worktrees/<name>/` dir can be left
  behind with dangling `gitdir` pointer.
- **stale IPC socket**: `.git/fsmonitor--daemon.ipc` can point at a
  dead daemon.

`session-start.sh` now runs a best-effort cleanup on every session
bootstrap: `git worktree prune`, `git fsmonitor--daemon stop`, and
removal of a dangling IPC socket. Output is logged under
`.claude/.session/<session_id>/session-start-cleanup.log`.

If you hit this mid-session, the manual recovery is:

```bash
git worktree prune
git fsmonitor--daemon stop || true
rm -f .git/fsmonitor--daemon.ipc
# Or: disable fsmonitor on this repo entirely
git config core.fsmonitor false
```

