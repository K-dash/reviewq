# Git Workflow

## ⚠️ Worktree requirement (non-negotiable)

**NEVER commit directly to main.** Every development task — including
single-file edits, typo fixes, and docs-only changes — MUST start by
creating a dedicated `git worktree` under `.worktree/<branch-name>/`.
Editing files in the main worktree is not allowed, even when the change
looks trivial. This is mechanically enforced by `worktree-gate.sh`;
see `.claude/hooks/README.md`.

### Worktree procedure

1. **Before any file edit**, create a worktree + feature branch from the
   repo root (main worktree):
   ```bash
   git worktree add -b feat/your-feature-name .worktree/feat-your-feature-name
   cd .worktree/feat-your-feature-name
   ```
   - Prefixes: `feat/`, `fix/`, `docs/`, `chore/`, `refactor/`,
     `perf/`, `test/`, `ci/` to match Conventional Commits.
   - The directory name should mirror the branch with `/` → `-`.
   - If you already started editing on main by mistake:
     ```bash
     git stash push -u
     git worktree add -b <type>/<slug> .worktree/<type>-<slug>
     git -C .worktree/<type>-<slug> stash pop
     ```
2. **Work inside the worktree**. All edits, tests, and builds happen
   there. Never `cd` back to main mid-task.
3. **After changes**, run quality checks inside the worktree:
   ```bash
   make all  # fmt + clippy -D warnings + test + test-hooks
   ```
4. **Update README.md** if user-facing behavior changed.
5. **Commit** with a Conventional Commits message (see below).
6. **Push + create PR** — never merge directly to main:
   ```bash
   git push -u origin <branch-name>
   gh pr create
   ```
7. **Cleanup after merge**: run `/cleanup-worktree --restore-cwd --delete-branch`
   from inside the worktree. `--restore-cwd` is required per the global
   `~/.claude/CLAUDE.md` rule to keep the session's tools alive after
   the directory is removed.

### Pre-commit checklist

- [ ] Working inside a `.worktree/<branch>` directory?
- [ ] On a feature branch (not main)?
- [ ] `make all` passes (fmt + clippy + test + test-hooks)?
- [ ] README.md updated if user-facing behavior changed?
- [ ] PR will be created?

## Commit message format

```
<type>: <short description>

<optional body>
```

Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`, `ci`.

All commit messages, PR titles, PR descriptions, and code comments must
be written in English.

Author attribution is disabled globally via `~/.claude/settings.json`;
do not add `Signed-off-by` or `Co-authored-by` unless the user asks.

## Pull request workflow

When creating a PR:

1. Analyze the full commit history on the branch (not just the latest
   commit).
2. Use `git diff <base-branch>...HEAD` to see every change that will
   land.
3. Draft a PR summary that explains the *why*, not just the *what*.
4. Include a test plan with concrete checklist items.
5. Push with `-u origin <branch>` on the first push so the branch
   tracks.

See `.claude/rules/development-workflow.md` for the full feature
pipeline (research → plan → TDD → review → commit) that precedes git
operations.
