---
description: Show the current harness-engineering gate state (markers + recent hook log)
---

# /harness-status — inspect the enforcement layer

Prints:

1. The session id currently in use.
2. Every marker file under `.claude/.session/<session_id>/` — each one
   corresponds to a phase of the workflow that has been completed.
3. The last 20 hook decisions from `hook-log.jsonl` — so you can see
   exactly which gates fired and why.
4. Which gates are currently passing and which would still block.

Useful to run at any time to answer *"why did that hook block me?"*
and *"what do I still need to do before I can commit?"*.

## What Claude does on this command

```bash
# Pick the freshest session dir.
ROOT="$CLAUDE_PROJECT_DIR/.claude/.session"
SID=$(ls -t "$ROOT" 2>/dev/null | head -1)
if [[ -z "$SID" ]]; then
    echo "No session state yet — no hook has fired this session."
    exit 0
fi
DIR="$ROOT/$SID"

echo "## Session"
echo "  id:  $SID"
echo "  dir: $DIR"
echo

echo "## Markers (workflow phases completed)"
if [[ -d "$DIR" ]]; then
    for f in "$DIR"/*; do
        [[ -f "$f" ]] || continue
        base=$(basename "$f")
        case "$base" in
            hook-log.jsonl|stop-gate-check.log) continue ;;
        esac
        printf "  ✅ %s\n" "$base"
    done
fi
echo

echo "## Recent hook decisions (last 20)"
if [[ -f "$DIR/hook-log.jsonl" ]]; then
    tail -20 "$DIR/hook-log.jsonl" | jq -r '
        "[\(.ts)] \(.hook) -> \(.decision): \(.reason)"
    '
else
    echo "  (no log yet)"
fi
echo

echo "## Commit readiness"
need=()
[[ -f "$DIR/agents-md-read" ]]  || need+=("Read AGENTS.md")
[[ -f "$DIR/skill-invoked" ]]   || need+=("Invoke any Skill()")
if [[ -f "$DIR/rust-files-edited" ]]; then
    [[ -f "$DIR/tdd-tests-written" ]] || need+=("Write tests or invoke rust-testing/tdd-workflow skill")
    [[ -f "$DIR/rust-review-done" ]]  || need+=("Run /rust-review or Agent(rust-reviewer)")
fi
if ls "$DIR"/skill:reviewq-e2e >/dev/null 2>&1 || [[ -f "$DIR/e2e-done" ]]; then
    :
else
    if [[ -f "$DIR/tui-files-edited" ]]; then
        need+=("Invoke reviewq-e2e skill (TUI changes detected)")
    fi
fi

if [[ ${#need[@]} -eq 0 ]]; then
    echo "  ✅ All visible gates should pass. (commit-gate will still run"
    echo "     cargo fmt --check / clippy / test on the actual commit.)"
else
    echo "  ❌ Still required before commit-gate will allow a commit:"
    for item in "${need[@]}"; do
        echo "     - $item"
    done
fi
```
