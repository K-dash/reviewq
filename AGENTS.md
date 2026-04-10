# Agent Instructions

**reviewq** is a Rust CLI/TUI that detects PRs where you are a requested
reviewer and triggers AI code-review agents.

## Non-negotiable constraints

- **Work inside a `.worktree/<branch>/` directory**, always. Never edit
  files in the main worktree. Mechanically enforced by
  `.claude/hooks/worktree-gate.sh`. → full procedure:
  `.claude/rules/git-workflow.md`.
- **Read this file** (AGENTS.md) before the first edit — required by
  `.claude/hooks/agents-md-gate.sh`.
- **Invoke a routing skill** (`.claude/rules/skills.md`) before the
  first `*.rs` edit — required by `.claude/hooks/skill-routing-gate.sh`.
- **Write tests first** — production Rust edits are blocked until a
  test file is touched or the `tdd-workflow` / `rust-testing` skill is
  invoked (`.claude/hooks/tdd-gate.sh`).
- **`make all` must be green before every commit** (fmt + clippy
  `-D warnings` + test + test-hooks). Enforced by
  `.claude/hooks/commit-gate.sh`.
- **All commit messages, PR text, and code comments must be written in
  English.** No exceptions.

## Build & quality

```bash
make all          # fmt + lint + test + test-hooks   ← run before every commit
make test-hooks   # workflow-enforcement hook self-tests
```

Individual targets: `make fmt` / `make lint` / `make test` / `make build`.

Rust 2024 edition. `cargo clippy -- -D warnings`. Format with `cargo fmt`.

## Where to find the detailed rules

| Topic | File |
|-------|------|
| Git worktree + commit + PR procedure | `.claude/rules/git-workflow.md` |
| Full feature pipeline (research → plan → TDD → review → commit) | `.claude/rules/development-workflow.md` |
| Plan-first rule (≥3 files or new patterns) | `.claude/rules/plan-first.md` |
| Skill routing table (ECC + rust-skills) | `.claude/rules/skills.md` |
| Agent orchestration (which agent for what) | `.claude/rules/agents.md` |
| Rust coding style / patterns / testing / security | `.claude/rules/coding-style.md`, `.claude/rules/patterns.md`, `.claude/rules/testing.md`, `.claude/rules/security.md` |
| Harness-engineering theory behind the hooks | `.claude/rules/harness-engineering.md` |
| Every hook's purpose + marker vocabulary | `.claude/hooks/README.md` |
| Model selection / context budgeting | `.claude/rules/performance.md` |

## Project structure

<!-- Keep this section short; point to rules files for details. -->

- `src/main.rs` — CLI entry point.
- `src/tui/**` — ratatui TUI (see `.claude/rules/skills.md` for the
  `reviewq-e2e` skill that must run on TUI changes).
- `src/` — everything else is domain-separated by folder (jobs, repos,
  executor, …).
- `.claude/hooks/` — mechanically enforced workflow gates.
- `.claude/rules/` — referenced by this file; the source of truth for
  *how* to do each phase of work.

## Known mistakes & lessons learned

Record AI-generated mistakes and the rule that prevents recurrence.
Newest first. Keep each entry ≤5 lines so this section does not bloat.

<!-- ### YYYY-MM-DD: short description -->
<!-- - What happened: … -->
<!-- - Root cause: … -->
<!-- - Rule: the constraint that prevents recurrence. -->

## Architecture decisions

Short ADR notes that explain *why* the code is the way it is. Full ADRs
go under `docs/adr/` when we add them; this section is for quick refs.

<!-- ### Decision title -->
<!-- - Context: … -->
<!-- - Decision: … -->
<!-- - Trade-off: … -->
