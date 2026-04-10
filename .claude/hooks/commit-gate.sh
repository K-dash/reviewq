#!/usr/bin/env bash
# PreToolUse gate: before `git commit` succeeds, require that the
# verification loop is green AND that a rust-review has been performed in
# this session (if any Rust source files changed).
#
# Rule (AGENTS.md → Build & Quality, Git Workflow):
#   "REQUIRED: Run before completing any work — make all (format + lint + test)"
#   ".claude/rules/skills.md → Phase 5: Every code change → /rust-review"
#
# Policy:
#   1. Only fire when the Bash command actually runs `git commit` (and not
#      e.g. `git commit-tree`, `git commit --help`, or `echo 'git commit'`).
#   2. Require `cargo fmt --check`, `cargo clippy --all-targets -D warnings`
#      and `cargo test` to all pass in the commit's worktree.
#   3. If any staged .rs file changed in this commit, require the marker
#      `rust-review-done` to exist (set when the rust-reviewer agent ran
#      in this session). Doc-only commits skip this check.
#   4. Block with a detailed fix message on any failure.
#
# Internal failures (missing tools, parse errors) fall through to allow —
# see lib/common.sh.

. "$(dirname "$0")/lib/common.sh"
reviewq_require_jq
reviewq_read_input

tool_name=$(reviewq_jq '.tool_name')
[[ "$tool_name" != "Bash" ]] && exit 0

cmd=$(reviewq_jq '.tool_input.command')
[[ -z "$cmd" ]] && exit 0

# Match `git commit` as a subcommand. We want to catch:
#   git commit -m "..."
#   git -c foo=bar commit ...
#   cd x && git commit ...
# but NOT:
#   git commit-tree ...
#   git commit --help
#   echo "git commit ..."
#   git log --grep='git commit'
#
# Heuristic: require the literal token "git commit" (space-separated) and
# NOT be inside quotes. Good enough in practice; the gate is conservative.
if ! printf '%s' "$cmd" | grep -Eq '(^|[^a-zA-Z_-])git[[:space:]]+(-[^[:space:]]+[[:space:]]+)*commit([[:space:]]|$)'; then
    exit 0
fi
# Skip benign subcommands.
if printf '%s' "$cmd" | grep -Eq 'git[[:space:]]+commit(-tree|[[:space:]]+--help|[[:space:]]+-h([[:space:]]|$))'; then
    exit 0
fi
# Skip quoted / commented uses (echo, grep, log).
if printf '%s' "$cmd" | grep -Eq '(^|[[:space:]])(echo|printf|grep|rg|awk|sed)([[:space:]]|$)'; then
    # Could still be a legit `foo && git commit`; only bail if the only
    # verb is one of the above.
    if ! printf '%s' "$cmd" | grep -Eq '(&&|;|\|\|)[[:space:]]*git[[:space:]]+commit'; then
        exit 0
    fi
fi

# cwd from the hook input is where the Bash tool will execute the command,
# which is where we need to run `cargo`.
cwd=$(reviewq_jq '.cwd')
[[ -z "$cwd" ]] && cwd=$(reviewq_project_dir)
[[ ! -d "$cwd" ]] && exit 0

# The commit must happen inside a worktree. Worktree-gate covers edits but
# not commits, so we enforce it here too.
case "$cwd" in
    */.worktree/*) ;;
    *)
        reviewq_block "git commit attempted outside a .worktree/ directory.

AGENTS.md forbids committing from the main worktree. Create a feature
branch worktree and retry the commit there:
  git worktree add -b <type>/<slug> .worktree/<type>-<slug>
  cd .worktree/<type>-<slug>"
        ;;
esac

# ---- collect staged file list once, for downstream checks ---------------
staged=$(git -C "$cwd" diff --cached --name-only 2>/dev/null || true)
if [[ -z "$staged" ]]; then
    # No staged changes — let git itself complain; nothing for us to gate.
    exit 0
fi
rust_staged=$(printf '%s\n' "$staged" | grep -E '\.rs$' || true)

# ---- 1. cargo fmt --check -----------------------------------------------
if ! (cd "$cwd" && cargo fmt --check >/tmp/reviewq-gate-fmt.log 2>&1); then
    log=$(head -50 /tmp/reviewq-gate-fmt.log)
    reviewq_block "cargo fmt --check FAILED.

Run inside the worktree:
  cargo fmt

Then stage the changes and retry the commit.

--- first 50 lines of fmt output ---
$log"
fi

# ---- 2. cargo clippy -----------------------------------------------------
if ! (cd "$cwd" && cargo clippy --all-targets -- -D warnings >/tmp/reviewq-gate-clippy.log 2>&1); then
    log=$(tail -60 /tmp/reviewq-gate-clippy.log)
    reviewq_block "cargo clippy --all-targets -- -D warnings FAILED.

Fix the warnings and retry the commit. Use:
  /rust-build     (invokes rust-build-resolver for incremental fixes)

--- last 60 lines of clippy output ---
$log"
fi

# ---- 3. cargo test -------------------------------------------------------
if ! (cd "$cwd" && cargo test --quiet >/tmp/reviewq-gate-test.log 2>&1); then
    log=$(tail -80 /tmp/reviewq-gate-test.log)
    reviewq_block "cargo test FAILED.

Fix the failing tests and retry the commit. AGENTS.md requires 'make all'
to pass before every commit.

--- last 80 lines of test output ---
$log"
fi

# ---- 4. rust-review marker (only when .rs files are staged) -------------
if [[ -n "$rust_staged" ]] && ! reviewq_has_mark rust-review-done; then
    file_count=$(printf '%s\n' "$rust_staged" | wc -l | tr -d ' ')
    reviewq_block "$file_count staged Rust file(s) but no rust-review has run in this session.

.claude/rules/skills.md requires a /rust-review pass on every code change
before commit.

Fix: invoke the rust-reviewer via the Agent tool, or run the slash command:
  /rust-review

A post-tool hook will set the rust-review-done marker when the agent
finishes, unblocking this gate. Staged Rust files:
$(printf '  %s\n' $rust_staged)"
fi

# ---- 5. e2e marker (only when src/tui/** files are staged) --------------
tui_staged=$(printf '%s\n' "$staged" | grep -E '^src/tui/' || true)
if [[ -n "$tui_staged" ]] && ! reviewq_has_mark e2e-done; then
    reviewq_block "TUI files staged but reviewq-e2e has not run this session.

.claude/rules/skills.md → Phase 4: 'TUI behavior changes (src/tui/**) → reviewq-e2e'

Fix: run the e2e workflow, then retry the commit.
  Skill(reviewq-e2e)

Once the e2e run marker is set, this gate will unblock."
fi

# All gates passed.
exit 0
