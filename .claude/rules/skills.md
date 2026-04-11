# Skill Routing (ECC)

This project ships 127+ skills from [Everything Claude Code](https://github.com/affaan-m/everything-claude-code). Claude MUST consult this file at the start of every development task and invoke the skills listed below **without waiting for explicit user instruction**. Each trigger row is binding: when the condition matches, run the skill via the `Skill` tool before (or in parallel with) writing code.

> reviewq is a **Rust CLI/TUI**. Frontend/web/ML/domain skills are intentionally excluded from routing. If a request falls outside Rust + agentic engineering, fall back to the global ECC catalog but do not add it here.

## Operating Principles

1. **Skills are not optional.** Treat the rows below as mandatory defaults; only skip a skill when its pre-conditions clearly do not apply, and say so in the reply.
2. **Invoke early.** Planning and research skills run *before* code edits. Verification skills run *before* commits. Learning skills run *before* the session ends.
3. **Compose, do not duplicate.** Multiple skills can run in one turn. Prefer running them in parallel over sequential narration.
4. **Never skip to save tokens.** Use `context-budget` / `strategic-compact` to protect context instead of dropping skills.

## Phase-to-Skill Map

### Phase 0 — Workspace Setup (before touching any file)

**Mandatory for every task**, no exemptions for "tiny" changes. reviewq requires a dedicated git worktree per feature branch (see `AGENTS.md` → Git Workflow).

| Trigger                                                                             | Skill(s) / Action                                             |
|-------------------------------------------------------------------------------------|---------------------------------------------------------------|
| First filesystem edit of a task, any size                                           | Verify `git worktree list` + `git rev-parse --show-toplevel`; if in the main worktree, run `git worktree add -b <type>/<slug> .worktree/<type>-<slug>` and `cd` into it before editing. |
| User asks for a "quick fix" / "one-liner" / "just commit this"                      | Still create a worktree first. There is **no** small-change exemption. |
| Found yourself editing on `main` by mistake                                         | `git stash push -u`, create the worktree, `git -C .worktree/<name> stash pop`, continue there. |
| Worktree already exists for this task                                               | Re-enter it instead of creating a new one (`cd .worktree/<name>`). |

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
| TUI behavior changes (`src/tui/**`)                                                 | Add / update render-layer tests in `tests/tui_render.rs` (TestBackend-based) so `cargo test` covers the change. `reviewq-e2e` is optional for extra interactive verification but no longer required by the commit gate. |
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
| Branch is committed and ready to open a PR                                          | `/ship` command (runs `make all`, pushes, drafts PR title/body, opens PR) |
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

0. **`git worktree add -b <type>/<slug> .worktree/<type>-<slug>` and `cd` into it.** Mandatory first step, no exceptions.
1. `plan-and-handoff` → produce plan. (For complex tasks this may create its own worktree; if so, reuse it and skip step 0.)
2. `search-first` + `documentation-lookup` → research existing solutions / crate APIs.
3. `tdd-workflow` + `rust-testing` → write failing tests first.
4. `rust-patterns` (+ `rust-async-patterns` if async) → implement.
5. `rust-review` + `security-review` → review before commit.
6. `verification-loop` → run `make all`, fix fallout.
7. `commit-oss` → conventional commit on the feature branch.
8. `/ship` → `make all`, push, draft PR title/body, open the PR.
9. `continuous-learning-v2` → capture reusable instincts before ending the session.
10. After merge: `/cleanup-worktree --restore-cwd --delete-branch`.

## Explicitly Out of Scope for reviewq

Skills for other language stacks (Python/Go/Java/Spring/Kotlin/Swift/PHP/Laravel/Django/Perl/C++/Flutter/JS frontend), unrelated business domains (logistics/fintech/energy/investor/market), and content/marketing workflows have been **removed** from `.claude/skills/` to keep the catalog Rust-focused. If a task genuinely needs one of those stacks, pull it from the global ECC catalog rather than re-adding it here.

## Escalation

If a task appears to need a skill that is **not** listed here, prefer the global ECC catalog over inventing a new approach, and propose adding it to this file so the routing stays current.
