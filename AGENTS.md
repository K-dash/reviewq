# Agent Instructions

## Project Overview

**reviewq** is a CLI/TUI tool that automatically detects PRs where you are a requested reviewer and triggers AI code review agents.

## Build & Quality

```bash
# REQUIRED: Run before completing any work
make all          # format + lint + test

# Individual commands
make fmt          # cargo fmt
make lint         # cargo clippy -- -D warnings
make test         # cargo test
```

## Git Workflow (MUST FOLLOW)

⚠️ **NEVER commit directly to main. Always use feature branches.**

1. **BEFORE any code changes**: Create a feature branch
   ```bash
   git checkout -b feat/your-feature-name
   ```
2. **After changes**: Run quality checks
   ```bash
   make all  # format + lint + test
   ```
3. **Update documentation**: If user-facing behavior changes, update README.md
4. **Commit**: Use conventional commits (feat:, fix:, docs:, etc.)
5. **Push and create PR**: Never merge directly to main
   ```bash
   git push -u origin <branch-name>
   gh pr create
   ```

### Pre-Commit Checklist

Before committing, verify:
- [ ] On a feature branch (not main)?
- [ ] `make all` passes?
- [ ] README.md updated if needed?
- [ ] PR will be created?

## Instructions for AI Agents

- Before committing, ALWAYS re-read this Workflow section
- When user says "commit", first check current branch and create feature branch if on main
- When user-facing behavior changes, proactively update README.md before committing
- **All code comments, commit messages, PR titles, PR descriptions, and review comments MUST be written in English**

### Plan-First Rule

For changes touching **3 or more files** or introducing **new architectural patterns**:

1. **Enter plan mode first** — use `EnterPlanMode` to explore the codebase and design the approach before writing any code.
2. **Get the plan approved** — the user must approve before execution begins. The plan is the contract.
3. **Include a verification strategy** — every plan must answer: "How will we verify this works?" (tests, manual checks, CI gates, etc.)
4. **Stop if scope drifts** — if the implementation diverges from the approved plan, stop and re-plan rather than improvising.

For small, well-scoped changes (single-file fixes, typo corrections, simple bug fixes), skip planning and execute directly.

### Proactive Skill Usage (rust-skills)

The following skills are NOT auto-triggered by hooks and must be used proactively:

- `rust-skills:sync-crate-skills` — Run after adding/updating dependencies in Cargo.toml
- `rust-skills:docs` — Use to look up crate API documentation from docs.rs
- `rust-skills:coding-guidelines` — Reference when reviewing or writing Rust code style

## Code Style

- Rust 2024 edition
- Use `cargo fmt` for formatting
- All clippy warnings treated as errors (`-D warnings`)

## Testing

- Run single test: `cargo test test_name`
- Run all tests: `cargo test` or `make test`
- Tests located alongside source in same module or in tests/ directory

## Project Structure

<!-- Update as the codebase grows -->

- `src/main.rs` — Application entry point

## Known Mistakes & Lessons Learned

Record AI-generated mistakes and the rules that prevent them from recurring. Update this section after every code review where the AI got something wrong.

<!-- Add entries in reverse-chronological order (newest first) -->
<!-- Format: ### YYYY-MM-DD: Short description -->
<!-- - **What happened**: ... -->
<!-- - **Root cause**: ... -->
<!-- - **Rule**: The constraint to prevent recurrence -->

## Architecture Decisions

Key design choices and their rationale. Helps AI agents understand *why* things are the way they are, not just *what* they are.

<!-- Add entries as architectural decisions are made -->
<!-- Format: ### Decision title -->
<!-- - **Context**: ... -->
<!-- - **Decision**: ... -->
<!-- - **Alternatives considered**: ... -->
<!-- - **Trade-off**: ... -->
