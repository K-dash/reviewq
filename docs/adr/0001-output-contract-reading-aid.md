# ADR 0001: AI output contract — reading aid, not review generator

- Status: Accepted
- Date: 2026-04-11
- Deciders: project owner
- Supersedes: —
- Superseded by: —

## Context

reviewq started as an "automatic PR review queue powered by AI agents".
The README, the `Cargo.toml` description, the detector, the runner, and
the default executor prompt all point at the same shape: detect a PR
where the user is a requested reviewer, spawn an AI agent in an
isolated worktree, and produce a structured review document
(`REVIEW.md`) covering Summary / Major Findings / Minor Findings /
Questions / Suggested patches / Risk & Rollout / Checklist.

That shape made the implementation easy to reason about, but it has
quietly drifted away from the reason the project exists. The intended
positioning is **"a tool that helps a human reviewer read a PR they
are reviewing more effectively"** — the AI is supposed to be a first-pass
reading aid that frees the human to focus on the parts that matter,
not a substitute that produces a finished review the human only has to
proofread.

The drift surfaces in three places:

1. **The default prompt produces a finished review.** It asks the
   model for `Suggested patches` (concrete code changes), risk
   classification, and a checklist. That output is one short polish
   away from a comment that could be posted to GitHub. It is not
   "reading aid" output; it is "review generator" output.
2. **The artifact name reinforces the drift.** A file called
   `REVIEW.md` invites the reader to treat it as a review, not as
   notes. Names shape behaviour.
3. **There is no boundary between "AI material" and "human comment".**
   Nothing in the codebase or the prompt prevents the AI output from
   being copy-pasted as a final review. The only thing standing
   between AI prose and a posted GitHub comment is the user's
   discipline.

A separate concern motivated by AI-assisted development culture made
the drift more urgent. AI coding agents are now common, and one of
their side effects is that junior engineers lose chances to read code,
form judgments, and write their own words. PR review is one of the
last reliable venues where a human is forced to actually read code.
A tool that automates that venue removes the practice. A tool that
helps a human practice it more efficiently preserves the venue. The
project owner wants reviewq to be the second kind, not the first.

The decision in this ADR is which kind reviewq is going to be, and
what mechanical guardrails ensure it stays that way as features are
added.

## Decision

reviewq adopts the following positioning, and every future feature is
evaluated against it:

> reviewq is a local AI assist tool for human PR reviewers. It
> prepares reading material that helps a human read, understand, and
> form judgments about a Pull Request. It does not generate finished
> review comments, does not post to GitHub, and does not act as a
> merge gate.

To make that positioning enforceable rather than aspirational, the
following architectural commitments are accepted together:

1. **Output is governed by a formal contract**, not by the default
   prompt. The contract specifies which sections must exist, which
   slots each finding must fill, the structural shape of every
   leaf field, and the maximum density of any section. The
   contract is versioned and lives at
   `docs/specs/output-contract.md`. The contract is intentionally
   locale-independent: it constrains structure, cardinality,
   length, and ordering, and explicitly does NOT inspect grammar,
   tense, vocabulary, or punctuation of free-text content. This
   removes future localization pressure rather than deferring it.
2. **The contract is mechanically enforced** by a deterministic
   validator implemented in Rust under `src/output_contract/`. The
   validator runs after every AI generation and before the artifact
   is exposed to the user. Output that fails validation is retried
   once with a corrective prompt and, on second failure, surfaced as
   `degraded` so the human can see exactly which constraint the AI
   violated.
3. **The validator's primary signal is structure, not phrase
   matching.** Required slots, enum membership, length limits, sentence
   counts, finding counts, and code-block restrictions are the main
   weapons. A small forbidden-phrase list is a secondary backstop, not
   the principal defense, because phrase lints are trivially bypassed
   by paraphrase.
4. **The existing queue/daemon/runner/worktree infrastructure
   survives unchanged.** Auto-detection of review-requested PRs and
   auto-execution of the AI prep pass are kept. Detector and runner
   are concept-neutral: they prepare reading material so it is ready
   when the human arrives. They do not replace the human's act of
   reading or judging.
5. **The `REVIEW.md` artifact is renamed** to a name that does not
   imply "this is a review" — `READING_NOTES.md` is the working
   choice, finalized in the spec. Read-side compatibility for the old
   name is kept for one minor release; new generation always uses the
   new name.
6. **The default prompt is rewritten** to satisfy the contract. The
   `Suggested patches` section is removed entirely. Findings are
   restricted to `concern` items with two-level confidence
   (`high | medium`); pure questions move to a separate
   `Open Questions` section that requires every entry to declare what
   non-repo context would be needed to answer it.

The contract details — section list, slot schemas, validator
checklist, retry behaviour — are deliberately not duplicated in this
ADR. They live in `docs/specs/output-contract.md`. This ADR records
*why* the contract exists and *why* enforcement is structural; the
spec records *what* the contract says.

Implementation sequencing: the README concept chapter is intentionally
rewritten only after the spec and validator land — prose that explains
the contract should follow the contract that enforces it. Writing the
prose first is what produced the original drift this ADR fixes.

## Consequences

### Positive

- The "reading aid vs. review generator" question is settled and
  every future feature can be checked against it.
- The validator turns the philosophical commitment into a property of
  the build: AI output that violates the contract cannot silently
  reach the user.
- The existing infrastructure investment (detector, runner, worktree
  manager, TUI) is preserved. The change is to what the runner
  produces, not to how it runs.
- Differentiation against the broader landscape of AI review bots is
  sharper and easier to defend in the README.
- Junior engineers — and the project owner's own future self — keep a
  venue in which they have to read code rather than skim a generated
  review.

### Negative

- The default prompt that took meaningful effort to author is being
  retired. That has emotional and time cost.
- Producing a contract that is strict enough to prevent
  copy-pastable output but loose enough to remain useful is a real
  prompt-engineering and validator-engineering exercise. The
  estimate is two to three weeks of iteration, not one or two pull
  requests.
- The artifact rename forces a one-minor-release deprecation window
  in which both file names are recognised on read. That is migration
  work, not concept work.
- Some users may have wanted reviewq to evolve into a posting bot.
  This ADR explicitly rules that out, and they will need a different
  tool.

### Neutral

- The detector and runner are unchanged in this decision. Future ADRs
  may revisit them, but auto-detection of review-requested PRs is
  judged compatible with the reading-aid positioning here.
- Multi-stage features previously sketched in `docs/feature-ideas.md`
  (two-pass "compact" verification, draft material generation) are
  not accepted or rejected by this ADR. They are deferred until the
  contract is in place, at which point they will be re-evaluated
  against the contract rather than against an informal concept.

## Alternatives considered

### Alternative A — Soften the README to match the current
implementation

Keep the current "automatic PR review queue" framing, soften the
README to say the tool "helps human reviewers" without changing the
prompt, the artifact name, or the output structure. Land no validator.

Rejected because it preserves the drift the ADR was written to fix.
The README would describe a reading aid while the implementation
continued to produce review-generator output, which is the worst of
both worlds. It also fails to give the project a defensible position
in a landscape that already has many AI review bots.

### Alternative B — Pivot fully to a manual reading-aid product

Disable the auto-detector, require the user to point at a PR
explicitly, drop the daemon, and reframe the entire CLI around
on-demand prep. Treat the existing queue infrastructure as legacy.

Rejected because the queue/daemon/runner machinery is not the source
of the drift. Auto-preparing reading material is a different action
from auto-generating a posted review, and the philosophical objection
collapses when the output is properly constrained. Pivoting away from
the working infrastructure would discard months of work for a problem
that does not require it.

### Alternative C (chosen) — Keep the infrastructure, change the
meaning of the output

Keep detector, daemon, runner, worktree manager, and TUI. Replace the
prompt, the artifact name, and — critically — the contract that the
output must satisfy. Add a deterministic validator that makes the
contract a property of the build. Defer prose work until the
mechanical pieces land.

This is the chosen alternative. It preserves the infrastructure
investment, fixes the drift at its actual source (the output
contract), and produces a system in which the philosophical claim and
the runtime behaviour cannot diverge without a build failure.

## References

- `docs/specs/output-contract.md` — the contract this ADR commits to
  (defined in a companion spec).
- `.claude/rules/harness-engineering.md` — three failure modes
  (drift, scope creep, silent corruption) referenced when arguing
  that the validator is necessary, not optional.
- Historical note: a predecessor brainstorming note
  (`docs/feature-ideas.md`) existed during discussion and was
  deleted during consolidation; it is not canonical, and its
  load-bearing conclusions are absorbed into this ADR and the spec.
