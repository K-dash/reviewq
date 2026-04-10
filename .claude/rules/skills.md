# Skill Routing (ECC)

This project ships 127+ skills from [Everything Claude Code](https://github.com/affaan-m/everything-claude-code). Claude MUST consult this file at the start of every development task and invoke the skills listed below **without waiting for explicit user instruction**. Each trigger row is binding: when the condition matches, run the skill via the `Skill` tool before (or in parallel with) writing code.

> reviewq is a **Rust CLI/TUI**. Frontend/web/ML/domain skills are intentionally excluded from routing. If a request falls outside Rust + agentic engineering, fall back to the global ECC catalog but do not add it here.

## Operating Principles

1. **Skills are not optional.** Treat the rows below as mandatory defaults; only skip a skill when its pre-conditions clearly do not apply, and say so in the reply.
2. **Invoke early.** Planning and research skills run *before* code edits. Verification skills run *before* commits. Learning skills run *before* the session ends.
3. **Compose, do not duplicate.** Multiple skills can run in one turn. Prefer running them in parallel over sequential narration.
4. **Never skip to save tokens.** Use `context-budget` / `strategic-compact` to protect context instead of dropping skills.

## Phase-to-Skill Map

### Phase 1 — Understand & Plan (before any edit)

| Trigger                                                                             | Skill(s)                                                      |
|-------------------------------------------------------------------------------------|---------------------------------------------------------------|
| Feature / refactor touching ≥3 files, or any architectural change                   | `plan-and-handoff`, `blueprint`                               |
| Requirements are ambiguous, conflicting, or under-specified                         | `grill-me`                                                    |
| New subsystem, non-trivial design choice                                            | `architecture-decision-records`                               |
| "What should we do about X?" style exploration                                      | `agentic-engineering`, `ai-first-engineering`                 |
| Fresh codebase areas you have not touched in this session                           | `codebase-onboarding`                                         |

### Phase 2 — Research (before writing code)

| Trigger                                                                             | Skill(s)                                                      |
|-------------------------------------------------------------------------------------|---------------------------------------------------------------|
| About to implement any new capability                                               | `search-first` (mandatory — port/adopt beats hand-rolling)    |
| Need API / crate / framework usage details                                          | `documentation-lookup`, `rust-skills:docs`                    |
| Added / updated a dependency in `Cargo.toml`                                        | `rust-skills:sync-crate-skills`                               |
| Evaluating regex vs parsing vs LLM extraction                                       | `regex-vs-llm-structured-text`                                |

### Phase 3 — Implement (Rust-specific)

| Trigger                                                                             | Skill(s)                                                      |
|-------------------------------------------------------------------------------------|---------------------------------------------------------------|
| Writing or editing any Rust source file                                             | `rust-patterns`, `rust-skills:coding-guidelines`              |
| Touching `async fn`, `tokio`, channels, spawning tasks                              | `rust-async-patterns`                                         |
| Designing or refactoring error types (`Result`, `anyhow`, `thiserror`)              | `rust-skills:m06-error-handling`                              |
| Ownership / borrow checker errors (E0382, E0597, E0506, E0515, E0716, …)            | `rust-skills:m01-ownership`                                   |
| `Send` / `Sync` / thread-safety errors                                              | `rust-skills:m07-concurrency`                                 |
| CLI flag / subcommand work (clap, argument parsing)                                 | `rust-skills:domain-cli`                                      |
| Build or dependency failure                                                         | `rust-build` command (invokes `rust-build-resolver` agent)    |

### Phase 4 — Test (before marking work "done")

| Trigger                                                                             | Skill(s)                                                      |
|-------------------------------------------------------------------------------------|---------------------------------------------------------------|
| Any new feature or bug fix                                                          | `tdd-workflow`, `rust-testing` (write tests FIRST)            |
| TUI behavior changes (`src/tui/**`)                                                 | `reviewq-e2e`                                                 |
| Regression risk on existing behavior                                                | `ai-regression-testing`                                       |
| Coverage check before hand-off                                                      | `test-coverage` command                                       |

### Phase 5 — Review & Verify (before commit)

| Trigger                                                                             | Skill(s)                                                      |
|-------------------------------------------------------------------------------------|---------------------------------------------------------------|
| **Every** code change                                                               | `rust-review` command (invokes `rust-reviewer` agent)         |
| Code handling user input, auth, subprocess, filesystem, or network                  | `security-review`, `unsafe-checker` (if `unsafe` is present)  |
| Pre-commit gate                                                                     | `verification-loop`, `quality-gate` command, `make all`       |
| Want independent second opinion on risky changes                                    | `santa-method`                                                |

### Phase 6 — Commit & Ship

| Trigger                                                                             | Skill(s)                                                      |
|-------------------------------------------------------------------------------------|---------------------------------------------------------------|
| Starting work on main                                                               | `create-branch`                                               |
| Staged changes ready                                                                | `commit-oss` (Conventional Commits)                           |
| PR review requested on an existing PR                                               | `pr-review`                                                   |
| Worktree session wrapping up                                                        | `cleanup-worktree` (with `--restore-cwd` per global CLAUDE.md)|

### Phase 7 — Context & Learning (continuous)

| Trigger                                                                             | Skill(s)                                                      |
|-------------------------------------------------------------------------------------|---------------------------------------------------------------|
| Session crosses task boundaries or feels heavy                                      | `strategic-compact`, `context-budget`                         |
| Repeated pattern surfaced, correction from user, non-obvious fix                    | `continuous-learning-v2`, `learn` command                     |
| End of a substantive work block                                                     | `chronicle` (daily note log)                                  |
| Skill seems missing or outdated                                                     | `skill-stocktake`, `skill-create`                             |

## Default "Happy Path" for a New Feature

1. `plan-and-handoff` → produce plan, enter worktree.
2. `search-first` + `documentation-lookup` → research existing solutions / crate APIs.
3. `tdd-workflow` + `rust-testing` → write failing tests first.
4. `rust-patterns` (+ `rust-async-patterns` if async) → implement.
5. `rust-review` + `security-review` → review before commit.
6. `verification-loop` → run `make all`, fix fallout.
7. `commit-oss` → conventional commit on the feature branch.
8. `continuous-learning-v2` → capture reusable instincts before ending the session.

## Explicitly Out of Scope for reviewq

Do **not** auto-invoke these (wrong stack or off-topic): `django-*`, `laravel-*`, `springboot-*`, `nextjs-turbopack`, `bun-runtime`, `flutter-*`, `kotlin-*`, `swift-*`, `pytorch-*`, `clickhouse-io`, `frontend-*`, `design-system`, `liquid-glass-design`, `energy-procurement`, `customs-trade-compliance`, `carrier-relationship-management`, `investor-*`, `market-research`, `crosspost`, `content-engine`, `article-writing`. They remain installed for ad-hoc use, but must not be part of the default loop.

## Escalation

If a task appears to need a skill that is **not** listed here, prefer the global ECC catalog over inventing a new approach, and propose adding it to this file so the routing stays current.
