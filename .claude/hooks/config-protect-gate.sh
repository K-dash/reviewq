#!/usr/bin/env bash
# PreToolUse gate: require explicit confirmation before editing protected
# config files. This is the Edit/Write counterpart to the Bash redirect
# check in safety-gate.sh.
#
# Rationale:
#   nyosegawa harness-engineering:
#     "PreToolUse Hook blocks edits to config files (.eslintrc, biome.json,
#      Cargo.toml, etc.)"
#   ignission hunting-to-farming:
#     "pre-edit-guard: Prevents modification of linter configs, hook
#      scripts themselves"
#
# Protected files:
#   Cargo.toml, Cargo.lock  — dependency drift
#   Makefile               — build pipeline
#   rustfmt.toml, .rustfmt.toml, clippy.toml — lint strictness
#   deny.toml, rust-toolchain.toml — toolchain / security
#   .github/workflows/**   — CI gates
#   .gitignore             — can hide generated artifacts
#
# Policy:
#   Always block. The user can unblock by explicitly telling the agent
#   to edit the file (which will surface in conversation), and the hook
#   will re-run on the retry and block again unless the session has the
#   `config-edit-approved` marker, which is set only by a `/config-edit`
#   slash command the user runs manually.
#
#   In other words: the only way to edit protected configs is for the
#   HUMAN to opt in once per session. This is intentional friction.

. "$(dirname "$0")/lib/common.sh"
reviewq_require_jq
reviewq_read_input

tool_name=$(reviewq_jq '.tool_name')
case "$tool_name" in
    Write|Edit|MultiEdit|NotebookEdit) ;;
    *) exit 0 ;;
esac

file_path=$(reviewq_jq '.tool_input.file_path')
[[ -z "$file_path" ]] && exit 0

# Extract basename for simple comparisons, and keep the full path for
# directory-scoped checks (e.g. .github/workflows).
base=$(basename "$file_path")

protected_basenames=(
    "Cargo.toml"
    "Cargo.lock"
    "Makefile"
    "rustfmt.toml"
    ".rustfmt.toml"
    "clippy.toml"
    "deny.toml"
    "rust-toolchain.toml"
    "rust-toolchain"
    ".gitignore"
)

is_protected=0
for name in "${protected_basenames[@]}"; do
    if [[ "$base" == "$name" ]]; then
        is_protected=1
        break
    fi
done

# Directory-scoped protection: any file under .github/workflows/**
case "$file_path" in
    *"/.github/workflows/"*) is_protected=1 ;;
esac

[[ "$is_protected" -eq 0 ]] && exit 0

# User escape hatch: a marker set by the /config-edit slash command (or
# a manual touch) unlocks config edits for the remainder of the session.
if reviewq_has_mark config-edit-approved; then
    reviewq_log_event allow "config edit approved this session: $file_path"
    exit 0
fi

reviewq_block "Edit of protected config file '$file_path' requires explicit approval.

Protected files (Cargo.toml / Cargo.lock / Makefile / clippy.toml /
rustfmt.toml / deny.toml / rust-toolchain.toml / .gitignore /
.github/workflows/**) are locked by default because silent changes to
them cause dependency drift, CI pipeline regressions, or loosened
linter strictness that gets caught weeks later.

Fix:
  Have the human user run:
    /config-edit
  to set the 'config-edit-approved' marker for this session. The marker
  is session-scoped and expires when the session ends.

  If no /config-edit command exists yet, the user can manually touch:
    .claude/.session/<session_id>/config-edit-approved"
