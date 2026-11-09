# ADR 0027: Durable History Replacement

**Status:** Accepted

## Context

Long Chats accumulate repeated thinking, Tool calls, Tool results, and
intermediate prose that remain valuable for audit and recall but are expensive
to resend on every model call. Summary Checkpoints bound old history, while
Projection Truncation can shorten Tool results, but neither efficiently replaces
large completed process segments in the active projected tail.

The optimization must not rewrite the append-only Message Log, alter the Chat
UI, block foreground execution, or obscure how exact source content can be
recovered. It also must compose with selected response variants and checkpoint
generation rather than introducing another history authority.

## Decision

### D1 - Replacement is distinct from compression

Compression is the synchronous Summary Checkpoint operation used when a pending
model call cannot fit safely. History Replacement is an independently enabled,
default-off background optimization.

Replacement never blocks a foreground model call. If no valid replacement is
ready, Context Projection uses the original history and its normal truncation
or compaction rules.

### D2 - Replacement targets closed Execution Trace Blocks

An Execution Trace Block contains the assistant-visible text directly preceding
a maximal contiguous thinking, Tool Use, and Tool Result segment, followed by
that complete process segment. The next assistant-visible text, variant change,
or incomplete work closes or delimits the block.

A block is eligible only when complete, selected, checkpoint-uncovered, and at
least 16K estimated tokens under the Projection Planner's configured estimator.
No replacement may split a streaming message, an unresolved Tool call, a
parallel result group, or an Active Turn boundary that is unsafe for projection.

### D3 - One bounded model call produces the digest

Generation uses the active Chat model, a dedicated no-Tool prompt, and a
provider-enforced `max_output_tokens = 4096`. The model receives exactly the
source block as delimited historical data and returns natural language.

The digest transfers intent, material actions, exact reusable values,
side effects, outcomes, failures, and unresolved work. It omits hidden reasoning,
raw protocol duplication, and unproductive exploration. Runtime, not the model,
owns source coordinates and Tool references.

### D4 - Quality gates precede publication

A candidate becomes ready only if generation completes without output cutoff,
saves at least 8K estimated tokens, and reduces the source block by at least
60 percent. Eligibility and savings use the same configured heuristic estimator
as Context Projection.

A terminal quality failure records a rejected generation identity so the same
model and format do not retry indefinitely. Priority cancellation or runtime
shutdown is retryable and does not create a rejection.

### D5 - Projection substitutes without changing history

A ready record renders at the original chronological position as a UI-hidden,
model-visible synthetic assistant history segment. It includes a deterministic
source locator and Tool references owned by runtime.

The raw Message Log and normal Chat display remain unchanged. Exact details are
recoverable through Conversation span and Tool-result recall surfaces. Context
Projection applies valid replacements before optional historical Tool Result
truncation and final measurement.

A replacement is applicable only when no active Projection Exclusion changes
the visibility of its source range. Unrelated exclusions do not invalidate a
record whose complete source range remains visible.

### D6 - Replacement records are durable Chat state

Each source block maps to one deterministic record under:

```text
sessions/<chat-id>/history_replacements/<record-id>.json
```

The ID includes Chat identity, ordered source message identities, content-part
anchors, format version, and prompt version. Ready and rejected records are
written atomically and replicate with the Chat. In-flight candidates do not.

The current implementation uses History Replacement format version 3 and
prompt version 2. Superseded record versions remain harmless files but are not
eligible for projection or outbound replication.

Only Conversation Authority may generate or publish a record. Authority
transfer reuses synchronized valid records rather than regenerating them.

### D7 - Variant and checkpoint reachability control activation

Record validity includes selected variant lineage. Selecting another variant
may deactivate a record; rollback may reactivate it. Branching may reuse a
compatible record for copied selected history.

Summary Checkpoint coverage deactivates a replacement without deleting it.
Records are removed only with the Chat or when their source is unreachable from
every retained variant. There is no age- or count-based garbage collection.

### D8 - Background generation is bounded

At most one replacement generation runs per Chat. Additional eligible blocks
are ordered deterministically, with larger savings opportunities preferred.
Foreground model work and blocking compaction have priority under shared
provider limits. Different Chats may generate concurrently subject to those
shared provider limits; there is no separate fixed global replacement limit.

Disabling the feature stops new substitution, clears queued generation, and
cancels the active lease without deleting already synchronized records.

## Hard invariants

- Message Log content is never rewritten by History Replacement.
- Replacement is optional and never blocks foreground execution.
- Only complete safe trace blocks are eligible.
- Eligibility and publication use the shared estimator and the fixed
  16K/8K/60-percent gates.
- One deterministic source block has one versioned record identity.
- Runtime owns source coordinates and Tool references.
- Projection can always fall back to raw history.
- Projection Exclusions take precedence over replacement activation.
- In-flight candidates are not visible or replicated.
- Checkpoint coverage alone never makes a replacement record collectible.
- Authority transfer cannot create competing records for the same generation.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Conversation Authority schedules generation and publishes valid
records. Context Projection decides whether a record is applicable. The model
produces digest text but does not decide source identity or activation.

**Persistence.** Ready and rejected records are atomic durable Conversation
State adjacent to the Chat. Raw history, Turn Index, and checkpoints remain
unchanged. Projection Cache is not a record store.

**Concurrency.** One per-Chat generation lease prevents duplicate work. Source
identity and selected lineage are revalidated before publication so a stale
candidate cannot replace newly changed history.

**Failure.** Generation error, output cutoff, insufficient savings, stale
lineage, cancellation, and write failure leave raw projection valid. Permanent
quality rejection and retryable operational failure remain distinct outcomes.

## Consequences

- Long process-heavy tails can become cheaper without losing audit history.
- Recall paths become the explicit escape hatch for omitted exact detail.
- Replacement records add portable storage and synchronization cost.
- Model or variant changes may require new generation while retaining old
  records as inactive history.
- Projection Exclusions may temporarily deactivate otherwise valid replacement
  records without deleting them.
- Summary generation must consume the effective replaced projection to avoid
  repeatedly summarizing raw Tool protocol.

## Rejected alternatives

- Rewrite or delete source Message Log records.
- Show generated digests in the normal Chat transcript.
- Reuse the term silent compression.
- Block the next model call while replacement generation runs.
- Ask the model to generate record IDs or source coordinates.
- Use a separate summary model or second LLM judge.
- Keep replacement only in memory or Projection Cache.

## Implementation and verification

Current implementation anchors include:

- `crates/rua-session/src/history_replacement.rs`;
- `crates/rua-server/src/services/history_replacement.rs`;
- `crates/rua-server/src/services/context_projection.rs`;
- `crates/rua-server/src/services/conversation_authority.rs`;
- `crates/rua-agent/src/projection_preparation.rs`;
- `crates/rua-session/src/store.rs`.

### Current implementation gaps

Publication is not yet fenced by Conversation Authority Epoch.
`HistoryReplacementJobContext` in
`crates/rua-server/src/services/history_replacement.rs` captures
`authority_device_id` but not the
accepted `authority_epoch`. Its `can_publish` check validates only the
coordinator generation, cancellation token, enabled state, and running-config
flag. The ready/rejected store closures therefore do not re-read or compare
current placement before committing a record, and
`ConversationAuthority::switch_authority` does not cancel the coordinator's
in-flight job.

The existing
`crates/rua-server/tests/infra/history_replacement_coordinator.rs` suite covers
generation, quality gates, disable/reenable cancellation, and Agent-config
backfill. It does not cover an authority switch between provider completion and
record publication. Until the job captures and revalidates the accepted epoch,
an old authority can publish and emit a synchronized snapshot after losing
authority, contrary to D6 and the final hard invariant.

Context Projection also currently bypasses every History Replacement whenever
the Projection Exclusion Set is non-empty, rather than invalidating only records
whose source range overlaps an excluded Message. This is a correctness-first
but overly broad fallback; ADR 0026 records the related checkpoint-prefix
preservation gap.

Verification must cover trace-block boundaries, deterministic IDs, quality
gates, atomic ready/rejected writes, selected-variant invalidation, checkpoint
interaction, authority transfer reuse, epoch-fenced late publication,
cancellation, and exact raw-history fallback. Relevant suites include
`crates/rua-session/tests/all/execution_trace_blocks.rs`,
`crates/rua-session/tests/all/history_replacement_store.rs`, and
`crates/rua-server/tests/infra/history_replacement_coordinator.rs`.

## Related decisions

- ADR 0020: Append-only Message Log.
- ADR 0022: Conversation Authority.
- ADR 0025: Runtime Notes.
- ADR 0026: Context Projection.
- ADR 0028: Chat and Turn configuration snapshots.
