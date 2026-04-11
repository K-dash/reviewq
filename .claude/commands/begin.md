---
description: reviewq task entry point. Drives the full workflow — worktree → AGENTS.md → plan-if-needed → skill → TDD → review → /ship — so the agent never trips a hook by accident. Use this whenever the user requests a code change.
---

# /begin — reviewq task workflow entry point

Task: $ARGUMENTS

You have been asked to start a reviewq task. Execute the steps below in
order. The reviewq harness has 12 hooks under `.claude/hooks/` that
**reactively** block bad actions; this command is the **proactive**
complement that walks you through the correct sequence up front so you
do not trip those hooks in the first place.

Do not skip steps. Each one names the marker or gate it satisfies, so
you can debug missing prerequisites quickly via `/harness-status`.

---

## Step 1 — Grill the user

Invoke `Skill(grill-me)` immediately. Before doing anything else,
interview the user about the task in `$ARGUMENTS` until you have a
shared understanding of what they actually want. Ask questions one at
a time. For each question, propose your recommended answer so the
user can confirm or redirect with minimal friction.

If a question can be answered by exploring the codebase (Read / Glob /
Grep), explore the codebase instead of asking. No file edits are made
in this step — only reads — so you do not yet need to be inside a
worktree, and the `worktree-gate.sh` hook will not fire.

This step exists because requirements ambiguity is the single biggest
source of wasted work. The hooks downstream cannot catch a faithfully
implemented but wrong feature; only this step can. Continue grilling
until **you can write the plan in Step 4 without guessing**.

When the user signals they are satisfied (explicit "ok" / "go" /
"進めて" or equivalent), proceed to Step 2.

## Step 2 — Verify your location

```bash
git rev-parse --show-toplevel
git branch --show-current
git worktree list
```

If you are in the **main worktree**, do NOT edit anything yet — the
`worktree-gate.sh` hook will block any Edit/Write outside `.worktree/`.
You will create a feature worktree in Step 5. If you are already inside
a `.worktree/<branch>/` from a previous turn, reuse it instead of
creating a new one.

## Step 3 — Read AGENTS.md

Use the `Read` tool (not `cat` via Bash) on `AGENTS.md`. This sets the
`agents-md-read` marker that `agents-md-gate.sh` requires before any
Edit/Write call. You only have to do this once per session.

## Step 4 — Estimate scope, decide on plan mode

Estimate how many files this task will touch and whether it introduces
a new architectural pattern.

| Scope                                                            | Required action                                              |
|------------------------------------------------------------------|--------------------------------------------------------------|
| 1–2 files, no new pattern, well-understood task                  | Skip plan mode, jump to Step 4                               |
| ≥3 files OR new architectural pattern OR ambiguous requirements  | **`EnterPlanMode` → draft a plan → `ExitPlanMode` → WAIT for user approval before continuing** |

This is the **plan-first enforcement point**. Without `/begin`,
`.claude/rules/plan-first.md` is paper-only — no PreToolUse hook can
observe Plan Mode because `EnterPlanMode` is a Claude Code mode
transition, not a tool call. The only way to enforce "plan first" is
to write it into the workflow that the user explicitly invokes, which
is exactly what this step does.

If plan mode is required, the plan you draft must include:

- Restated requirements in your own words (proves you understood)
- Concrete file list — every file you expect to touch
- Verification strategy — "how will we know it works?"
- Risk and rollback notes if anything is destructive or ambiguous

After `ExitPlanMode`, **wait silently for the user to approve**. Do not
start editing on a draft plan.

## Step 5 — Create a feature worktree

Pick a Conventional Commits prefix and a slug:

```bash
git worktree add -b <type>/<slug> .worktree/<type>-<slug>
```

Prefixes: `feat/`, `fix/`, `chore/`, `docs/`, `refactor/`, `perf/`,
`test/`, `ci/`. Use **absolute paths** inside the worktree for all
subsequent commands rather than `cd`-ing around — keeps the cwd stable
and avoids stale worktree issues.

## Step 6 — Invoke a routing skill

Per `.claude/rules/skills.md` Phase 3, every Rust source edit requires
at least one `Skill()` call first. Pick the most relevant entry from
the routing table. Sensible defaults:

- `rust-patterns` — general Rust work (always safe)
- `rust-async-patterns` — touching `tokio`, async fn, channels, spawning tasks
- `rust-skills:m06-error-handling` — designing or refactoring error types
- `rust-skills:domain-cli` — touching CLI flag / clap subcommand work
- `rust-testing` — when this is primarily a test-coverage task

This sets the `skill-invoked` marker that `skill-routing-gate.sh`
requires before the first `*.rs` Edit. Skip-to-save-tokens is
explicitly forbidden by `.claude/rules/skills.md`.

## Step 7 — Write a failing test FIRST

Test before implementation. Per `.claude/rules/testing.md` and the
`tdd-workflow` / `rust-testing` skills, the test must be written before
any production change. This sets the `tdd-tests-written` /
`tests-edited` marker that `tdd-gate.sh` checks.

For TUI changes specifically, add a TestBackend-based render test in
`tests/tui_render.rs` (see PR #41 for the pattern). For hook changes,
add a case to `.claude/hooks/tests/run-tests.sh` so `make test-hooks`
covers the new behavior.

## Step 8 — Implement minimally

Make the failing test green with the smallest possible diff. No bonus
refactors. No "while I'm here" cleanups. No new abstractions for
hypothetical future requirements. Each unrelated improvement gets its
own future `/begin` invocation.

## Step 9 — Local verification

```bash
make all
```

Must be green: fmt + clippy `-D warnings` + `cargo test` + 68+ hook
self-tests. The Stop hook will *also* run
`cargo clippy --all-targets -- -D warnings` automatically when this
turn ends (per PR #42), so even if you forget to run `make all`, the
turn cannot end with a broken build or a clippy warning.

## Step 10 — Code review

```
/rust-review
```

Or call the agent directly via the `Agent` tool with
`subagent_type: rust-reviewer`. Address all CRITICAL and HIGH issues.
This sets the `rust-review-done` marker that `commit-gate.sh` requires
when any `.rs` file is staged.

## Step 11 — Commit

**ASK THE USER FIRST.** The global `~/.claude/CLAUDE.md` rule is
absolute: never `git commit` without explicit user approval. Once
approved:

```bash
git add <specific files>      # never `git add .` or `-A`
git commit -m "<type>: <subject>"
```

Conventional Commits, English only. The `commit-gate.sh` hook will run
fmt-check + clippy + test + verify the `rust-review-done` marker.

## Step 12 — Ship

```
/ship
```

`/ship` is the closing complement to `/begin`. It runs `make all` one
more time as a last line of defense, pushes the branch with `-u`, and
drafts the PR title/body from the branch's full commit history. It
refuses to run from the main worktree, from `main` branch, with a
dirty tree, with no commits ahead, or with a failing `make all`. See
`.claude/commands/ship.md` for the full preconditions list.

`/ship` is intentionally NOT a Stop hook — opening a PR is a human
decision, and the local loop must hand off explicitly to the
shared-state loop.

## Step 13 — After merge

Once the PR merges, run from inside the worktree:

```
/cleanup-worktree --restore-cwd --delete-branch
```

`--restore-cwd` is required by the global `~/.claude/CLAUDE.md` rule
to keep the session's tools alive after the directory is removed.

---

## Why this command exists

The reviewq harness has two layers:

```
ユーザー入力
    ↓
[/begin <task>]   ← pro-flow (proactively presents the correct sequence)
    ↓
agent runs steps 1–13
    ↓
[12 hooks]        ← anti-violation (reactively blocks broken actions)
    ↓
完成
```

Until `/begin` existed, the agent only had the reactive layer. Hooks
caught the agent *after* it tried to do something wrong, which wasted
turns and produced confusing block messages. `/begin` is the missing
proactive layer: it tells the agent the entire workflow up front, in
order, so the hooks rarely have to fire.

The hooks remain in place as a safety net. `/begin` does not bypass
any of them — every Edit/Write/Bash inside steps 4–11 still passes
through the normal PreToolUse gates, the Stop hook still runs clippy
at turn end, and `/ship` still has its own preconditions. Defense in
depth.

It also makes `.claude/rules/plan-first.md` finally executable. Plan
Mode is invisible to PreToolUse hooks, so no gate can enforce it
mechanically. By embedding the plan-first decision into Step 3 of a
user-invoked command, the rule moves from paper to practice.

## When NOT to use /begin

- **One-off questions** ("what does this function do?") — no edits needed.
- **Pure observability** (`/harness-status`, `git log`, reading files).
- **Continuing an already-started task** in the same worktree — re-enter
  the worktree directly, the markers from the previous turn are still
  in place. Skipping `/begin` here is fine because steps 1–5 are
  already satisfied.

For any task that produces a PR, **always** start with `/begin`.
