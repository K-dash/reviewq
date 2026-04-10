#!/usr/bin/env bash
# PreToolUse gate: hard-block destructive shell commands and bypass attempts.
#
# Rationale (from harness-engineering articles):
#   "PreToolUse Hooks: Block risky operations (rm -rf, git push --force)"
#     — nyosegawa, coding-agent-workflow-2026
#   "pre-bash-guard: Blocks destructive commands, --no-verify bypasses,
#    procrastination language" — ignission, Claude Code で実践する
#
# Policy:
#   - Destructive file ops (`rm -rf /`, `rm -rf ~`, wildcard rm)
#   - Irreversible git ops (`git push --force` on any branch, `git reset --hard`,
#     `git clean -fdx`, `git branch -D`)
#   - Hook/verification bypass (`--no-verify`, `--no-gpg-sign`)
#   - Config file edits routed through Bash (`echo > Cargo.toml`, etc.)
#   - Exit-2 block with a specific fix message for each class.
#
# Non-policy (intentionally allowed):
#   - `rm -rf <specific-dir>` under the current worktree (needed for cleanup)
#   - `git push` without --force
#   - `git reset` without --hard
#
# All decisions are logged to hook-log.jsonl for audit.

. "$(dirname "$0")/lib/common.sh"
reviewq_require_jq
reviewq_read_input

tool_name=$(reviewq_jq '.tool_name')
[[ "$tool_name" != "Bash" ]] && exit 0

cmd=$(reviewq_jq '.tool_input.command')
[[ -z "$cmd" ]] && exit 0

# ---- Class 1: indiscriminate rm ----
# Block `rm -rf /`, `rm -rf ~`, `rm -rf *`, `rm -rf /*`, `rm -rf .`.
# We use word-boundary regex so things like `rmdir` or `./rm` don't trip.
if printf '%s' "$cmd" | grep -Eq '(^|[^a-zA-Z_/-])rm[[:space:]]+(-[A-Za-z]*r[A-Za-z]*f|-[A-Za-z]*f[A-Za-z]*r)[[:space:]]+((-{1,2}[A-Za-z-]+[[:space:]]+)*)(/|~|\*|/\*|\.|\.\.)($|[[:space:]])'; then
    reviewq_block "Indiscriminate 'rm -rf' target detected in: $cmd

This command would delete the root, home, or a wildcard expansion.
Never run this — even on a dev machine.

Fix:
  Specify an explicit, scoped directory path (absolute or relative),
  and verify it first with 'ls -la <path>'.
  Prefer: git clean -fdx (inside a worktree only)
  Prefer: rm -rf .worktree/<specific-branch-slug>"
fi

# ---- Class 2: force-push ----
if printf '%s' "$cmd" | grep -Eq 'git[[:space:]]+push[[:space:]]+.*(--force([[:space:]]|$)|-f([[:space:]]|$))'; then
    # Allow --force-with-lease (safer) as an escape hatch for intentional
    # rewrites of a feature branch the user owns.
    if ! printf '%s' "$cmd" | grep -Eq 'git[[:space:]]+push[[:space:]]+.*--force-with-lease'; then
        reviewq_block "'git push --force' / 'git push -f' detected: $cmd

Force-push rewrites remote history and can destroy other contributors'
work. reviewq forbids it on any branch by default.

Fix:
  - If you need to rewrite a feature branch, use --force-with-lease
    which aborts when the remote has moved:
      git push --force-with-lease
  - If you need to update main, rebase and open a PR instead."
    fi
fi

# ---- Class 3: hook / verification bypass ----
if printf '%s' "$cmd" | grep -Eq '(--no-verify|--no-gpg-sign|-c[[:space:]]+commit\.gpgsign=false)'; then
    reviewq_block "Hook / signing bypass detected in: $cmd

Global ~/.claude/CLAUDE.md explicitly forbids --no-verify, --no-gpg-sign,
and any other mechanism that skips pre-commit or pre-push hooks.

Fix:
  Fix whatever is causing the hook to fail. Do not bypass it.
  If the hook itself is wrong, patch the hook in a chore/ worktree
  and re-run 'make test-hooks'."
fi

# ---- Class 4: hard reset / destructive git clean ----
if printf '%s' "$cmd" | grep -Eq 'git[[:space:]]+reset[[:space:]]+(--hard|.*[[:space:]]--hard)'; then
    reviewq_block "'git reset --hard' detected in: $cmd

Hard-reset discards uncommitted work and rewrites the index silently.
This is how ~/.claude/CLAUDE.md 'in-progress work loss' incidents happen.

Fix:
  - Use 'git stash push -u' to save work-in-progress before resetting.
  - Use 'git restore <file>' / 'git restore --staged <file>' for targeted
    reverts.
  - Use 'git reset --keep <ref>' to reset while aborting on conflicts."
fi

if printf '%s' "$cmd" | grep -Eq 'git[[:space:]]+clean[[:space:]]+.*(-[A-Za-z]*f[A-Za-z]*x|-[A-Za-z]*x[A-Za-z]*f)'; then
    reviewq_block "'git clean -fdx' (or -fx) detected in: $cmd

This deletes EVERY untracked file including .gitignored build artifacts,
.env files, and IDE settings. It has destroyed hours of uncommitted work
in past incidents.

Fix:
  Use 'git clean -fd' (without -x) to preserve ignored files, or
  specify the exact path: 'git clean -fd src/generated/'."
fi

# ---- Class 5: branch hard-delete ----
# We want to block `git branch -D` for UNMERGED branches but allow it when
# the branch was squash-merged to main via a GitHub PR (`git branch -d`
# refuses those because the SHA history differs even though the content
# landed). Detection rules in order:
#
#   1. `git` must be at a command boundary (start, or after `;`, `&&`,
#      `||`, `(`, `$(`, or a pipe). This avoids false positives when
#      "git branch -D" appears inside a quoted string argument to `grep`
#      or `echo`, which tripped the earlier, looser regex.
#   2. Extract the target branch name from the command.
#   3. If we can't parse it, block (conservative default).
#   4. If a session marker `branch-delete-approved:<name>` exists, allow
#      (explicit opt-in via /confirm-branch-delete slash command).
#   5. If `gh pr list --head <name> --state merged` returns a merged PR,
#      allow and log.
#   6. Otherwise block with a helpful message that explains both escape
#      hatches.
if printf '%s' "$cmd" | grep -Eq '(^|[;&|(]|\$\()[[:space:]]*git[[:space:]]+branch[[:space:]]+[^"'"'"']*(-D|--delete[[:space:]]+--force)'; then
    # Pull the branch name from the tail of the command. Accept any of:
    #   git branch -D foo
    #   git branch -Df foo
    #   git branch --delete --force foo
    #   git branch -D foo bar        (multiple — we check the first one)
    target=$(printf '%s' "$cmd" | \
        sed -E 's/.*git[[:space:]]+branch[[:space:]]+//' | \
        awk '{
            for (i = 1; i <= NF; i++) {
                if ($i !~ /^-/) { print $i; exit }
            }
        }')

    allow=0
    reason=""

    if [[ -z "$target" ]]; then
        reason="could not parse branch name from command"
    else
        # Escape slashes for the marker filename — markers live in a flat
        # directory so slashes need to become a safe separator.
        safe_name=${target//\//__}

        if reviewq_has_mark "branch-delete-approved:$safe_name"; then
            allow=1
            reason="session opt-in via /confirm-branch-delete"
        elif command -v gh >/dev/null 2>&1; then
            # Query GitHub for a merged PR on this branch. Suppress stderr
            # so network / auth failures do not trip the soft-fail trap.
            pr_state=$(gh pr list --head "$target" --state merged \
                --json state --jq '.[0].state // empty' 2>/dev/null || true)
            if [[ "$pr_state" == "MERGED" ]]; then
                allow=1
                reason="gh confirms a merged PR on head=$target"
            fi
        fi
    fi

    if [[ "$allow" -eq 1 ]]; then
        reviewq_log_event allow "branch -D $target allowed: $reason"
    else
        reviewq_block "'git branch -D' (force-delete) detected in: $cmd

Force-deleting a branch with unmerged commits discards work silently.
The lowercase 'git branch -d <name>' refuses unmerged branches, but
it also refuses branches that were *squash-merged* via a GitHub PR
(because the SHAs differ even though the content landed on main).

Fix — pick one depending on the branch state:
  (a) Regular merge / rebase already landed → 'git branch -d <name>'
      should already work. If it doesn't, your local main is stale:
        git fetch origin && git branch -d <name>

  (b) Squash-merged via GitHub PR → this gate will auto-unblock once
      'gh' confirms the PR is MERGED. Make sure you're authenticated:
        gh auth status
        gh pr list --head <name> --state merged
      Then retry the same 'git branch -D' command.

  (c) Branch is genuinely unmerged and you want to discard the work →
      run '/confirm-branch-delete <name>' to set the
      'branch-delete-approved:<name>' session marker, then retry.
      This is the only explicit opt-in path; it exists so destructive
      deletes still require a human acknowledgement."
    fi
fi

# ---- Class 6: piping remote scripts to a shell ----
if printf '%s' "$cmd" | grep -Eq '(curl|wget)[[:space:]]+.*\|[[:space:]]*(sh|bash|zsh)'; then
    reviewq_block "Piping a remote script directly into a shell detected: $cmd

'curl ... | sh' executes arbitrary code from the network with zero review.
This is a known supply-chain attack vector.

Fix:
  Download to a file, inspect it, then run it:
    curl -fsSLo /tmp/install.sh <url>
    less /tmp/install.sh
    bash /tmp/install.sh"
fi

# ---- Class 7: config file corruption via Bash redirect ----
# Edits to locked config files must go through Edit/Write (which are gated
# by the config-protection side of the edit path). Block Bash-level writes
# here so agents can't smuggle changes past Edit gates via `echo > file`.
for protected in Cargo.toml Cargo.lock Makefile rustfmt.toml clippy.toml deny.toml rust-toolchain.toml .rustfmt.toml; do
    if printf '%s' "$cmd" | grep -Eq "(>|>>|tee|sponge)[[:space:]]+(\./)?${protected//./\\.}([[:space:]]|$)"; then
        reviewq_block "Bash-level write to protected config file '$protected' detected: $cmd

Config files are edited via Write/Edit with explicit user awareness, not
via shell redirects. This is how silent dependency drift happens.

Fix:
  Use the Edit tool on $protected so the diff is visible in the
  conversation and can be reviewed."
    fi
done

reviewq_log_event allow "bash command cleared safety checks"
exit 0
