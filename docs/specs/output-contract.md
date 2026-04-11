# reviewq output contract

- Version: 0.1
- Status: Draft
- Related ADR: [ADR 0001 — output contract: reading aid, not review
  generator](../adr/0001-output-contract-reading-aid.md)

## 1. Purpose and scope

This document defines the contract that every reviewq AI generation
must satisfy before it is exposed to a human user. The contract is
the canonical answer to two questions:

1. What does an AI-produced reading-notes document look like?
2. What does it have to *not* look like, so that it cannot be
   mistaken for, or pasted as, a finished review comment?

The contract is enforced mechanically by the validator described in
§7. Output that does not satisfy the contract is retried once, and
on second failure surfaced as `degraded` rather than presented as
normal output. The validator's authority comes from this document;
the prompt is downstream of the contract, not the other way around.

The motivation for the contract — why reviewq is a reading aid, not
a review generator — is recorded in ADR 0001 and not duplicated
here.

### 1.1 Scope

This spec governs:

- The structure of the artifact written by an AI generation pass
  (the file currently called `READING_NOTES.md`).
- The fields and constraints of every section in that artifact.
- The validator's checks and its retry/degradation behaviour.
- The naming and language of the artifact.

### 1.2 Out of scope (v0.1)

The following are intentionally not part of v0.1. They may be
revisited in later versions; until then they MUST NOT appear in
generated output, and the validator MUST NOT accept them.

- "Out-of-scope observations" overflow section.
- Patch suggestions, suggested diffs, or any code-change proposals.
- A second-pass "compact / verify" stage.
- A draft-comment generation stage.
- Inline review comments addressed to the PR author.
- Any direct interaction with GitHub (posting, status checks,
  reactions, edits).

### 1.3 Locale independence

This contract is locale-independent. It does not specify, prefer,
or defer support for any natural language for the free-text
content of fields. The validator inspects structure, cardinality,
length, ordering, and serialization; it does not inspect grammar,
tense, punctuation, vocabulary, or any locale-specific phrasing of
the free-text content inside fields. The boundary between contract
vocabulary (fixed English identifiers) and free-text content
(locale-undefined) is defined in §8.1.

## 2. Top-level structure

A reading-notes artifact is a single Markdown file with a fixed set
of top-level sections, in this order:

| # | Section          | Required | Notes                                        |
|---|------------------|----------|----------------------------------------------|
| 1 | `Summary`        | Yes      | Neutral factual digest of the change         |
| 2 | `Reading Map`    | Yes      | Where the human should read first, and why   |
| 3 | `Findings`       | Yes      | Concerns the AI raises (may be empty)        |
| 4 | `Open Questions` | Yes      | Questions the AI cannot answer from the repo |

The file MUST contain these four H2 (`##`) headings, in this exact
order, with no additional H2 sections. The validator rejects extra
top-level sections.

The file MUST start with a single H1 (`#`) heading naming the PR,
followed by a metadata block (PR URL, branch, commit SHA, generation
timestamp). The metadata block is informational only and is not
otherwise constrained by this spec.

### 2.1 File naming

- New generations: `READING_NOTES.md`.
- Read compatibility: tools that consume the artifact MUST also
  recognize `REVIEW.md` for one minor release after this contract
  lands, then drop it.
- The name `REVIEW.md` MUST NOT be used for new generations once
  the contract is in effect.

### 2.2 Artifact wire format

This section fixes the canonical Markdown serialization of a
reading-notes artifact. The validator parses input as Markdown
according to these rules; AI generation MUST emit Markdown that
matches them. Every schema field defined in §§3–6 has exactly one
representation on disk, listed below. The schema and the wire
format are not allowed to drift; if a future version needs to
change one, the spec changes both at the same minor bump.

#### 2.2.1 Document skeleton

```
# <H1: PR title line>

<metadata block>

## Summary

<summary body>

## Reading Map

<reading map body>

## Findings

<findings body, possibly empty>

## Open Questions

<open questions body, possibly empty>
```

The four H2 headings appear in this order, with no other H2
elsewhere in the document. The blank line between sections is
required.

#### 2.2.2 H1 line

```
# PR #<number> — <title>
```

The H1 is informational. The validator only requires that exactly
one H1 exists and that it is the first non-empty line of the file.

#### 2.2.3 Metadata block

The metadata block immediately follows the H1, separated by a
blank line. It is a flat bullet list of `- key: value` pairs in
this order:

```
- url: <PR URL>
- branch: <branch name>
- commit: <commit SHA>
- generated_at: <ISO-8601 timestamp>
```

The validator hard-checks that all four keys are present and in
this order. v0.1 does not constrain the values beyond
non-emptiness.

#### 2.2.4 Summary body

A bullet list. Each bullet is a single line beginning with `- `
and contains a single line of free-text content (§3.3).

```
- <bullet 1>
- <bullet 2>
- <bullet 3>
```

#### 2.2.5 Reading Map body

An ordered list. Each entry is a numbered top-level item whose
text is the file path; the entry's other fields are sub-bullets at
one level of indentation.

```
1. <path>
   - line_range: <line-line or line>
   - fact: <single sentence>
   - priority_reason: <enum value>
   - priority_reason_note: <required only when priority_reason is `other`>
2. <path>
   - line_range: …
   - fact: …
   - priority_reason: …
```

The numbered ordering is meaningful: `1.` is the highest-priority
entry the human should read first.

#### 2.2.6 Findings body

Each finding is an H3 heading containing the finding `id`,
followed by a flat bullet list of labeled fields. `observed` is
the only field whose value is itself a sub-list, indented one
level under the `- observed:` bullet.

```
### F1
- kind: <enum>
- confidence: <enum>
- location: <path:line> or <path:line-line>
- area: <enum>
- area_note: <required only when area is `other`>
- observed:
  - <observation 1>
  - <observation 2>
- inference: <single line>
- why_it_matters: <single line>
- verification_target: <single line>
```

Findings are emitted in `id` order (`F1`, `F2`, …) and the `id`
must match the H3 heading.

If there are no findings, the `## Findings` section body is the
literal single line:

```
_None._
```

#### 2.2.7 Open Questions body

Each open question is an H3 heading containing the question `id`,
followed by a flat bullet list of labeled fields. There are no
sub-lists.

```
### Q1
- context_gap: <single line>
- needs_context_from: <enum>
- related_to: <file path, component name, or feature name>
```

Open questions are emitted in `id` order (`Q1`, `Q2`, …). If
there are none, the section body is the literal single line
`_None._` (same as Findings).

## 3. Section 1: Summary

### 3.1 Purpose

A reader who has 30 seconds should be able to read the Summary and
know what the PR does. The Summary is a neutral factual digest, not
an evaluation.

### 3.2 Schema

```yaml
summary:
  bullets:        # 3 to 5 entries
    - text: string  # single line, ≤120 chars
```

### 3.3 Constraints

REQUIRED:

- 3 to 5 bullets.
- Each bullet is a single line (no embedded newlines).
- Each bullet is ≤120 characters.

FORBIDDEN (structural):

- Multi-line bullets.
- Prose paragraphs (no continuous text spilling onto further
  lines).
- Bullets longer than 120 characters.

INTENT (not validator-enforced; see §7.5):

- Each bullet is a neutral factual statement of what changed,
  not an evaluation of the change. Drift away from this is the
  responsibility of prompt revision, not validator regex.

## 4. Section 2: Reading Map

### 4.1 Purpose

The Reading Map tells the human where to start reading and why
those locations are higher priority than the rest of the diff. Its
job is to compress the diff into an attention budget, not to
evaluate the code at those locations.

### 4.2 Schema

```yaml
reading_map:
  entries:        # 1 to 8 entries, ordered most-important first
    - path: string                     # repo-relative file path
      line_range: string               # "line-line" or "line", e.g. "42-58" or "42"
      fact: string                     # single line, ≤120 chars
      priority_reason: PriorityReason  # enum, see §4.3
```

### 4.3 `priority_reason` enum

Exactly one of:

- `public-api-change` — exported types, function signatures, or
  trait surface visible to other crates or external callers changed.
- `state-machine-change` — persistent state, status enums, or
  transition rules changed.
- `error-handling-change` — error types, propagation paths, or
  recovery branches changed.
- `concurrency-change` — async, threading, locking, channel, or
  ordering behaviour changed.
- `entrypoint-change` — `main`, CLI subcommand, daemon entry, or
  request handler entry changed.
- `boundary-change` — process, trust, or data-format boundary
  (subprocess invocation, deserialization, FFI, file format)
  changed.
- `other` — does not fit the above; requires `priority_reason_note`
  (see §4.4).

The validator rejects any value outside this enum.

### 4.4 Constraints

REQUIRED:

- 1 to 8 entries.
- Entries are ordered: the entry the human should read first comes
  first.
- Every entry has a `priority_reason` from the enum.
- If `priority_reason` is `other`, the entry MUST also include a
  `priority_reason_note` field of ≤80 characters justifying the
  priority. Without it, `other` is rejected.
- `path` is a real repo-relative path mentioned in the PR diff.
- `fact` is a single line, ≤120 characters.

FORBIDDEN (structural):

- Multi-line `fact` values.
- Entries with no `priority_reason` (the validator does not infer
  a default).

INTENT (not validator-enforced; see §7.5):

- `fact` is a description of what changed at the cited location,
  not an evaluation of whether the change is good or bad.

## 5. Section 3: Findings

### 5.1 Purpose

A finding is a concern the AI raises about the PR. Section 3
contains *only* concerns; pure questions live in Section 4
(`Open Questions`). A finding has structured slots that separate
what the AI literally observed, what it inferred, why it matters,
and what a human should verify. The structure forces the AI to be
specific about its evidence and prevents the slot from drifting
into a finished review comment.

### 5.2 Schema

```yaml
findings:
  - id: string                # F1, F2, ... (unique within the file)
    kind: Kind                 # enum, see §5.3
    confidence: Confidence     # enum, see §5.4
    location: Location         # required, see §5.5
    area: Area                 # enum, see §5.6
    observed: [string]         # 1 to 4 entries, see §5.7
    inference: string          # ≤150 chars, single line, see §5.8
    why_it_matters: string     # ≤120 chars, single line, see §5.9
    verification_target: string  # ≤120 chars, single line, see §5.10
```

### 5.3 `kind` enum

In v0.1 there is exactly one valid value:

- `concern`

The enum exists so future versions can add categories without a
breaking schema change. v0.1 validators MUST reject any other
value.

### 5.4 `confidence` enum

Exactly one of:

- `high` — the concern is supported by direct evidence in the diff
  or repo. A different reviewer reading the same code is expected
  to reach the same conclusion. Example: an unwrap on user-supplied
  input is observable from the diff alone.
- `medium` — the AI suspects a problem from a recognised pattern,
  but verification requires context not visible in the repo (call
  sites, runtime conditions, organizational decisions). Example: a
  timeout value that may be too short, contingent on an SLA the AI
  cannot read.

There is no `low`. Concerns the AI cannot stand behind at `medium`
or above MUST NOT appear in this section. Pure questions go to
Section 4.

### 5.5 `location`

REQUIRED for every finding. The format is `path:line` or
`path:line-line`. The validator rejects findings without a
location, and rejects locations whose path is not present in the PR
diff.

### 5.6 `area` enum

Exactly one of:

- `correctness` — the code does not do what it claims to do.
- `reliability` — runtime stability under expected operating
  conditions (panics, retries, resource exhaustion).
- `security` — confidentiality, integrity, authorization, or
  trust-boundary issues.
- `concurrency` — races, deadlocks, ordering, async lifetime.
- `error-handling` — error types, propagation, swallowed errors.
- `api-contract` — public interface promises (signatures, return
  conventions, documented invariants).
- `performance` — time or space cost in a way that matters for the
  expected workload.
- `test-coverage` — missing or insufficient tests for the changed
  behaviour.
- `maintainability` — structural cost the next reader will pay.
  This category is intentionally broad in v0.1; future versions may
  split it into `complexity`, `consistency`, and `layering` if it
  becomes a dumping ground in practice.
- `other` — does not fit the above. Requires `area_note`
  (≤80 chars) explaining why no other category fits. Without it,
  `other` is rejected.

### 5.7 `observed[]`

`observed` is an array of literal observations the AI made from the
diff and repo. It is the factual layer of a finding. Each entry is
a single line; the array is the only place a finding can carry
more than one line, and each entry is itself constrained.

REQUIRED:

- 1 to 4 entries per finding.
- Each entry is a single line, ≤120 characters.
- Each entry refers to a specific construct in the diff or repo
  (a path, an identifier, an inline-quoted snippet, or a
  combination of those).

FORBIDDEN (structural):

- Multi-line entries.
- Entries longer than 120 characters.
- Over-extrapolated absences: claims about what is missing from the
  wider repo when the AI did not actually look. Absences that are
  directly visible in the diff are allowed (e.g., "the PR diff adds
  no test cases for the new branch in `src/foo.rs:30-50`" is
  permitted because the diff itself shows it). Claims about the
  whole codebase are forbidden unless the AI grep-verified them, in
  which case the verification path belongs in the same observation.

INTENT (not validator-enforced; see §7.5):

- Each entry is a literal observation rather than an inference.
  Mixing observation and inference inside `observed[]` is the most
  common drift mode and is countered by prompt revision and the
  separation between `observed[]` and `inference`.

### 5.8 `inference`

`inference` is the single-line causal chain that links the
observations to the concern.

REQUIRED:

- A single line, ≤150 characters.
- References at least one token (path, identifier, or inline-quoted
  construct) that also appears in the same finding's `observed[]`.

FORBIDDEN (structural):

- Multi-line content.
- Length over 150 characters.
- Code blocks of any kind. Inline code spans are allowed.

INTENT (not validator-enforced; see §7.5):

- The line describes a *possibility* — what could go wrong given
  the observations — not a fix or a recommendation. Drift toward
  imperative or recommendation phrasing is the responsibility of
  prompt revision.

### 5.9 `why_it_matters`

`why_it_matters` is a single-line statement of the consequence the
AI is worried about.

REQUIRED:

- A single line, ≤120 characters.

FORBIDDEN (structural):

- Multi-line content.
- Length over 120 characters.

INTENT (not validator-enforced; see §7.5):

- The line names a runtime, user-facing, or maintenance
  consequence. Recommendation framing belongs nowhere in the
  contract; this slot is for impact, not advice.

### 5.10 `verification_target`

`verification_target` is a single-line label naming what a human
should verify about this finding. It is *not* a question, *not* a
recommendation, and *not* a fix. It is a short label of the
checkpoint the human is being handed, in whatever natural language
the AI is configured to emit (see §1.3, §8.1).

REQUIRED:

- A single line, ≤120 characters.

FORBIDDEN (structural):

- Multi-line content.
- Length over 120 characters.
- Code blocks of any kind. Inline code spans are allowed.

INTENT (not validator-enforced; see §7.5):

- The value names the *target* of verification rather than
  prescribing the verification procedure. Examples (free-text,
  not part of the contract):
  - `Whether 4xx retries are intentional for this caller mix`
  - `Receiver behaviour during repeated 500 ms attempts`
  - `Metric consumption by existing dashboards`

### 5.11 Count limits

Per generated artifact:

- `confidence: high` findings: at most 3.
- `confidence: medium` findings: at most 5.
- Total findings: at most 8 (the sum of the two limits).

Findings beyond the limits MUST be dropped by the AI before
emission. The validator rejects an artifact that exceeds these
counts.

## 6. Section 4: Open Questions

### 6.1 Purpose

Open Questions surface things the AI cannot determine from the diff
and repo alone, where answering would require organizational,
operational, or external knowledge. They exist so the human is
explicitly handed the gaps in the AI's view, instead of the AI
guessing.

This section is not an overflow bucket for weak findings. The
validator enforces structural constraints designed to keep it from
becoming one.

### 6.2 Schema

```yaml
open_questions:
  - id: string                       # Q1, Q2, ... (unique within the file)
    context_gap: string              # single line, ≤150 chars, see §6.4
    needs_context_from: ContextSource  # enum, see §6.3
    related_to: string               # soft anchor, see §6.5
```

### 6.3 `needs_context_from` enum

Exactly one of:

- `product` — answer requires product intent, user requirements, or
  feature decisions not visible in the repo.
- `ops` — answer requires operational knowledge: SLAs, runtime
  metrics, deployment topology, incident history.
- `org-history` — answer requires past decisions, prior incidents,
  team agreements, or commit-message archaeology beyond the diff.
- `external-contract` — answer requires a contract with an external
  system: a vendor API, a downstream consumer, a partner service,
  a regulatory rule.

REQUIRED for every open question. Validator rejects entries
without it.

### 6.4 `context_gap` field

`context_gap` is a single-line description of the gap that
prevents the AI from settling the matter from the repo alone. The
value is a fragment in whatever natural language the AI is
configured to emit; the contract does not require any particular
syntactic form (it does not have to be a question).

REQUIRED:

- A single line, ≤150 characters.

FORBIDDEN (structural):

- Multi-line content.
- Length over 150 characters.

INTENT (not validator-enforced; see §7.5):

- The gap is *genuine*: a human reading the repo cannot close it
  without reaching for product, ops, history, or external-contract
  context. Drift toward "weak findings dressed as questions" is
  the responsibility of prompt revision, not validator regex.

### 6.5 `related_to` field

REQUIRED. A soft anchor that ties the `context_gap` back to
something concrete in the change. The validator hard-checks that
this field is non-empty and is a single line ≤120 characters. The
acceptable shapes are:

- A file path (`src/foo.rs`).
- A component name (`detector`, `runner`, `reading-notes view`).
- A feature name (`merge-queue`, `worktree cleanup`).

This field is not required to include line numbers; the gap is
allowed to span an entire file or feature. The point is that the
human can locate what the gap is *about*.

INTENT (not validator-enforced; see §7.5):

- The anchor is *plausibly* connected to the `context_gap`. The
  validator cannot detect a malicious anchor that points at the
  wrong place; the prompt is responsible for honesty here.

### 6.6 Count limit

At most 3 open questions per artifact. Beyond the limit, the AI
MUST drop the lowest-priority entries before emission.

### 6.7 Empty section

`Open Questions` MAY be empty (zero entries). An empty section
still requires the H2 heading; the body is rendered as `_None._`.

## 7. Validator behaviour

### 7.1 Authority

The validator is a deterministic Rust function that runs on every
AI generation before the artifact is exposed to the user. The
contract in this document is the validator's specification. The
validator is the contract's enforcement mechanism. Disagreements
between this document and the validator are resolved by treating
the document as the source of truth and patching the validator.

### 7.2 Primary checks (structural)

The validator's only checks are structural. The full set is listed
below; every check is locale-independent and operates on shape,
cardinality, length, ordering, enum membership, and serialization.

1. The file has the required H1, metadata block, and exactly the
   four H2 sections in §2, in order. No extra H2 anywhere in the
   document.
2. The metadata block contains the four keys `url`, `branch`,
   `commit`, `generated_at`, in this order, each with a non-empty
   value.
3. `Summary` has 3 to 5 bullets. Each bullet is a single line
   (no embedded newline) and ≤120 characters.
4. `Reading Map` has 1 to 8 entries. Each entry has `path`,
   `line_range`, `fact`, `priority_reason`. `priority_reason` is
   in the enum. `other` requires `priority_reason_note` ≤80
   characters. `fact` is a single line ≤120 characters. `path` is
   a real repo-relative path mentioned in the PR diff.
5. `Findings` count: `high` ≤3, `medium` ≤5, total ≤8.
6. Each finding has every required slot from §5.2.
7. `kind` is in the enum (v0.1: only `concern`).
8. `confidence` is in the enum (`high` or `medium`).
9. `area` is in the enum; `other` requires `area_note` ≤80
   characters.
10. `location` matches `path:line` or `path:line-line`, and `path`
    exists in the PR diff.
11. `observed[]` has 1 to 4 entries; each entry is a single line
    ≤120 characters.
12. `inference` is a single line ≤150 characters, and references at
    least one token (file path, identifier, or inline-quoted
    construct) that also appears in the same finding's
    `observed[]`.
13. `why_it_matters` is a single line ≤120 characters.
14. `verification_target` is a single line ≤120 characters.
15. `Open Questions` count ≤3. Each entry has `context_gap`,
    `needs_context_from`, `related_to`. `needs_context_from` is in
    the enum. `context_gap` is a single line ≤150 characters.
    `related_to` is non-empty and a single line ≤120 characters.
16. The artifact contains zero fenced code blocks. Inline code
    spans (`` `like this` ``) are allowed without limit.

The validator does not inspect grammar, tense, vocabulary,
punctuation, sentence count, question form, or any other
locale-specific property of the free-text content inside fields.
This boundary is defined in §1.3 and §8.1.

### 7.3 Failure modes

The validator returns one of two states for every artifact:

1. `pass` — every primary check in §7.2 succeeds. The artifact is
   exposed to the user normally.
2. `fail` — at least one primary check fails. The reading-notes
   generation is retried exactly once with a corrective prompt
   that includes the validator's structured error report. If the
   retry also fails, the job is marked `degraded`.

There is no separate phrase-level backstop layer in v0.1. The
contract intentionally pushes drift control onto structure rather
than vocabulary; see §1.3 and §8.1 for the rationale, and §7.5
for the intent constraints that are explicitly out of validator
scope.

### 7.4 `degraded` job behaviour

A `degraded` job:

- Stores the raw AI output and the validator's structured error
  report side by side.
- Is shown in the TUI/CLI with a clear `degraded` label.
- Does NOT present the raw output as if it were valid reading
  notes; the user must explicitly opt in to viewing it, with the
  errors visible at the same time.
- Counts as a contract violation incident for observability /
  later prompt-tuning purposes.

The point of the `degraded` state is to make contract violations
visible rather than silently surfacing broken material.

### 7.5 Intent constraints (not validator-enforced)

Some commitments in this spec are design intent that cannot be
checked deterministically. They are listed here so that prompt
authors and reviewers know they exist and are expected, but the
validator does NOT hard-fail on them. They are the responsibility
of the prompt and of human review when the contract evolves.

- **Summary neutrality** (§3.3 INTENT): each Summary bullet
  states what changed rather than evaluating it. The validator
  enforces shape and length; it does not detect evaluative
  vocabulary.
- **`fact` neutrality** (§4.4 INTENT): each Reading Map `fact`
  describes the change at the cited location rather than judging
  it.
- **`observed[]` honesty** (§5.7 INTENT): each observation
  reflects something the AI looked at and is a literal observation
  rather than an inference. The validator can check shape but
  cannot prove inspection.
- **`inference` shape** (§5.8 INTENT): the line describes a
  possibility, not a fix or recommendation. Imperative or
  recommendation framing is countered by prompt revision rather
  than regex.
- **`why_it_matters` framing** (§5.9 INTENT): the line names a
  consequence, not advice.
- **`verification_target` framing** (§5.10 INTENT): the value
  names the target of verification, not the procedure.
- **`context_gap` genuineness** (§6.4 INTENT): the gap is one
  that the repo alone cannot close. Drift toward weak findings
  dressed as gaps is a prompt-revision problem, not a validator
  problem.
- **`related_to` plausibility** (§6.5 INTENT): the anchor is
  *plausibly* connected to the gap.
- **`area` precision** (§5.6 INTENT): the chosen area is the most
  specific match.

When these intent constraints are repeatedly violated in
production output, the corrective action is prompt revision and
spec evolution, not stricter regex. This contract intentionally
does not validate locale-specific grammar, tense, or wording.
Changes to that boundary require an explicit spec revision rather
than ad hoc validator growth.

## 8. Cross-cutting constraints

### 8.1 Field shape and locale boundary

#### 8.1.1 Field shape

All leaf scalar fields in §§3–6 are **single-line UTF-8 text
values**. The validator hard-checks that no leaf field contains a
newline. The single exception is `observed[]`, which is the only
slot where a finding can carry more than one line, and even there
each entry is itself a single line.

The validator does not inspect grammar, tense, vocabulary,
punctuation, sentence count, question form, or any other property
of natural-language phrasing inside a field. Drift control is the
responsibility of structure: slot existence, enum membership, line
count, length cap, ordering, and serialization. Prose policing is
not the contract's job.

#### 8.1.2 Identifier vs. free-text boundary

The contract distinguishes two kinds of strings.

**Identifiers** are fixed English tokens that are part of the
contract vocabulary and are stable across any locale a future user
might emit content in:

- Field names (`kind`, `confidence`, `inference`,
  `verification_target`, `context_gap`, …).
- Enum values (`correctness`, `error-handling-change`,
  `org-history`, `public-api-change`, …).
- Metadata keys (`url`, `branch`, `commit`, `generated_at`).
- Structural literals (`_None._`, `READING_NOTES.md`).

These are programming-language-style tokens, not prose. They are
not subject to the locale-independence rule because they are not
free-text content.

**Free-text content** is the value a field carries: the text after
`inference:`, the bullets under `## Summary`, the entries under
`observed:`, and so on. Free-text content is locale-undefined: it
MAY be in any natural language the AI generation is configured to
produce. The validator applies only structural constraints to
free-text content.

This contract intentionally does not validate locale-specific
grammar, tense, or wording. Changes to that boundary require an
explicit spec revision rather than ad hoc validator growth.

### 8.2 Code blocks

The artifact contains zero fenced code blocks (no ```` ``` ````
sections). Inline code spans (single backticks around an
identifier or short literal) are allowed without limit.

This rule is what physically prevents `Suggested patches` from
creeping back into the output as a "small example".

### 8.3 File location and names

- Generated artifact path inside the worktree:
  `READING_NOTES.md`.
- Old name (`REVIEW.md`): readable for one minor release of
  reviewq for migration; not generated.

## 9. Worked example

The following is a complete, valid v0.1 artifact for an imaginary
PR. The example exists so the validator implementer has a concrete
target.

```markdown
# PR #123 — add retry loop to outbound webhook dispatch

- url: https://github.com/example/repo/pull/123
- branch: feat/webhook-retry
- commit: 0a1b2c3
- generated_at: 2026-04-11T12:00:00Z

## Summary

- Retry loop with three attempts added around outbound webhook dispatch.
- Retry loop uses fixed 500 ms backoff between attempts.
- Webhook dispatch now invoked from the runner task.
- New `webhook_attempts` counter added to the metrics module.

## Reading Map

1. src/dispatch/webhook.rs
   - line_range: 40-95
   - fact: Retry loop added around the existing send call.
   - priority_reason: error-handling-change
2. src/runner/task.rs
   - line_range: 120-155
   - fact: Webhook dispatch invocation moved into this task.
   - priority_reason: entrypoint-change
3. src/metrics/counters.rs
   - line_range: 18-22
   - fact: New `webhook_attempts` counter registered.
   - priority_reason: public-api-change

## Findings

### F1
- kind: concern
- confidence: high
- location: src/dispatch/webhook.rs:60
- area: error-handling
- observed:
  - Retry loop at `webhook.rs:60` catches all errors with one match arm.
  - The match arm at `webhook.rs:60` does not branch on error kind.
  - Network errors and 4xx HTTP responses both reach the retry path.
- inference: 4xx responses at `webhook.rs:60` reach the retry path identically to network errors.
- why_it_matters: Retried 4xx attempts consume the attempt budget without changing the outcome.
- verification_target: Intended retry policy for 4xx responses in this caller mix

### F2
- kind: concern
- confidence: medium
- location: src/dispatch/webhook.rs:78
- area: reliability
- observed:
  - The 500 ms backoff at `webhook.rs:78` is a fixed literal.
  - The retry loop wrapping `webhook.rs:78` makes three attempts.
- inference: A fixed 500 ms backoff at `webhook.rs:78` repeats three times within ~1.5 s under failure.
- why_it_matters: Tight repeated attempts against an unhealthy receiver can worsen the incident.
- verification_target: Receiver behaviour during repeated 500 ms attempts inside ~1.5 s

## Open Questions

### Q1
- context_gap: Whether `webhook_attempts` is consumed by an existing dashboard or alert
- needs_context_from: ops
- related_to: src/metrics/counters.rs

### Q2
- context_gap: Whether the dispatch move from detector to runner was driven by an earlier incident
- needs_context_from: org-history
- related_to: runner
```

This artifact passes every primary check in §7.2: four H2
sections in order, the metadata block has all four required keys
in order, summary bullets are within count and length, reading
map entries each carry a valid `priority_reason`, two findings
(1 high + 1 medium, both within count limits) each have the full
slot set with `inference` references that match `observed[]`
tokens, all leaf fields are single-line and within their length
caps, no fenced code blocks, and two open questions each have
`context_gap`, `needs_context_from`, and a non-empty `related_to`.
None of the validator's checks inspect grammar, tense, vocabulary,
or punctuation; the artifact would also pass if every free-text
field were emitted in another natural language.

## 10. Versioning

The version field at the top of this document is the contract
version. The contract evolves under these rules:

- The version is a single integer pair `MAJOR.MINOR`.
- A `MINOR` bump is backward-compatible: it MAY add optional
  fields, MAY add new enum members, MAY relax limits. Existing
  validators continue to accept previously valid artifacts.
- A `MAJOR` bump is allowed to break compatibility. It requires a
  new ADR explaining the break and a migration note in this
  document.
- The validator embeds its supported contract version. Output
  generated against a higher contract version is treated as
  `degraded` until the validator catches up.

This document is the canonical spec. The validator is the
canonical enforcement mechanism. AI prompts are downstream of both
and may be revised freely without a version bump as long as the
contract behaviour is unchanged.

## 11. Deferred for v0.2 and later

Items intentionally not in v0.1, listed here so they are not lost:

- A `compact` second pass that re-evaluates findings against the
  current code and consolidates duplicates.
- A `material` generation stage that produces draft *talking
  points* (still not finished comments) for a finding the human
  selects.
- Splitting `area: maintainability` into `complexity`,
  `consistency`, and `layering`.
- A `low` confidence tier, if and only if running the contract in
  practice shows a class of useful findings that cannot be
  expressed at `medium`.
- Renaming the `Open Questions` section heading to `Context Gaps`
  to align with the v0.1 field rename. Deferred because section
  rename is a wire-format change with broader impact than a field
  rename.

These are deferred, not rejected. Each will be re-evaluated
against this contract before being adopted. Localized output is
**not** in this list: §1.3 establishes locale independence as a
permanent property of the contract, not a deferred feature.
