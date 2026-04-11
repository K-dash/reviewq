# Plan-First Rule

For changes touching **3 or more files** or introducing **new architectural
patterns**:

1. **Enter plan mode first** — use `EnterPlanMode` to explore the codebase
   and design the approach before writing any code.
2. **Get the plan approved** — the user must approve before execution
   begins. The plan is the contract.
3. **Include a verification strategy** — every plan must answer:
   "How will we verify this works?" (tests, manual checks, CI gates, …).
4. **Stop if scope drifts** — if the implementation diverges from the
   approved plan, stop and re-plan rather than improvising.

For small, well-scoped changes (single-file fix, typo, simple bug fix),
skip the plan mode step and execute directly — but **still inside a
fresh worktree** per `.claude/rules/git-workflow.md`. The worktree
requirement has no size exemption.

## How this rule is enforced

This rule **cannot** be enforced by a PreToolUse hook: `EnterPlanMode`
is a Claude Code mode transition, not a tool call, so no gate can
observe whether the agent entered it. Instead, the rule is operationalized
through the `/begin <task>` slash command (see
`.claude/commands/begin.md`).

`/begin` Step 3 explicitly asks the agent to estimate scope and
**requires** plan mode for any task touching ≥3 files or introducing a
new pattern. The agent must `EnterPlanMode`, draft a plan covering all
four points above, call `ExitPlanMode`, and **wait for user approval**
before any Edit/Write happens. Steps 4 onward (worktree creation,
implementation, review, ship) only run after the user approves.

When you ask the agent to do work, prefer starting with `/begin <task
description>`. Without that command, this rule degrades to a guideline
that the agent may forget — the hook layer cannot catch the violation.

## Why

`plan-and-handoff` / `blueprint` skills exist for this. Without a plan
contract, the agent drifts mid-task, adds scope, and the result does
not match what the user asked for. The three harness-engineering
failure modes — drift, scope creep, silent corruption — are all
mitigated by an explicit, approved plan.
