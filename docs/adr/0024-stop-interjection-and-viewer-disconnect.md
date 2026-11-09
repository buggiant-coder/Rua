# ADR 0024: Stop, interjection, and Viewer Disconnect

**Status:** Accepted

## Context

Several events can end a stream without having the same business meaning. A
user may intentionally stop a Turn, insert queued input, navigate away, lose a
browser connection, or experience a provider failure. Treating all of these as
stream cancellation loses output, corrupts queue behavior, and lets frontend
lifecycle control backend execution.

Stop and interjection also differ. Stop ends current work and may pause inputs
that were already queued. Passive interjection waits for a safe boundary.
Active interjection ends the old Turn specifically so one accepted queue entry
can become a new Turn.

These transitions must be owned by Conversation Authority and committed through
the Turn Writer before viewers observe a terminal fact.

## Decision

### D1 - User Stop is an authority control

User Stop requests that Conversation Authority terminate the current Active
Turn. The authority cancels the provider stream, prevents unstarted foreground
Tools, requests cancellation of cancellable running Tools, and preserves the
actual outcome of work that cannot be cancelled.

Already-produced output remains visible and durable. If a partial assistant
message was displayed, terminalization persists it once as interrupted output
before the stop marker and Turn lifecycle are committed.

### D2 - Stop records a typed terminal fact

The stopped Turn becomes `interrupted`. A model-visible system marker records
that the user intentionally interrupted the Turn. The marker is not ordinary
assistant speech and the frontend must not fabricate a follow-up response.

Provider completion, driver EOF, and stream closure are only candidates. The
public stop result is emitted after Turn persistence and authority handoff.

### D3 - Stop conditionally pauses prior queue entries

Stop never deletes queued input. If entries already existed when Stop was
requested, automatic queue drain is paused at that boundary. Those entries may
be edited, deleted, reclaimed, or explicitly sent later.

If the queue was empty at the Stop boundary, Stop does not create a persistent
pause. Input accepted while the Turn is finishing may start normally after the
interrupted Turn commits.

### D4 - Passive interjection waits for a safe boundary

Normal input submitted during an Active Turn enters the Conversation Input
Queue. It may be promoted after the current model segment completes, after a
running foreground Tool reaches a terminal result, and before another model
call begins.

Passive interjection does not cancel already-started side effects. Any Tool Use
that will not execute must still receive a provider-valid terminal Tool Result
before the old Turn is ended.

### D5 - Active interjection targets an accepted queue entry

Send Now is a control command over an existing authority `input_id`. It does not
resubmit text, attachments, or a new Submission ID.

If eligible, Conversation Authority ends the current Turn as `interjected`,
persists an interjection marker, claims the selected queue entry, assigns its
new `turn_id`, and starts it after the old Turn has a valid terminal shape.

During model streaming, the provider stream may be cancelled after preserving
partial output. During a transferable foreground Tool, execution may move to a
Durable Task. A non-transferable side-effecting Tool is not silently killed;
the stronger cancellation action must be explicit.

### D6 - Viewer Disconnect is not Stop

Route change, page unmount, refresh, browser connection loss, and stream-reader
cancellation detach only the viewer subscription. They do not cancel the Active
Turn, Tools, approvals, ACP session, or queued input.

Returning viewers reconnect to Conversation Live Activity when the Turn is
active, or replace their view from durable history plus the current Control
View when it has become terminal.

### D7 - Provider failure is a runtime outcome

Provider disconnect, timeout, or protocol failure is persisted as a failed or
interrupted Turn according to the runtime error contract. It is neither User
Stop nor Viewer Disconnect.

The visible frame preserves all content received before failure and presents
the runtime-owned failure fact without converting it into assistant-authored
text.

## Hard invariants

- Normal completion and User Stop both permit Task and collaboration result
  wake. Stop pauses only previously queued user input. Background results may
  start a follow-up Turn or enter an active Turn at a safe boundary, without
  claiming paused user input or clearing its pause. Service shutdown closes
  runtime admission; it must not persist a user-visible continuation Stop.
- Stop targets this Chat's Active Turn and foreground work, not independently
  executing collaboration Chats. Those Chats require their own explicit Stop.
- Interjection ending a delegated Turn returns an incomplete interjected result
  for that invocation; the new Turn does not silently take over the old call.
- User intervention inside a collaboration uses these same controls; replies
  to user input are not misattributed to the earlier delegated request.

- Only Conversation Authority can stop or interject an Active Turn.
- Viewer detachment never stops backend work.
- Stop preserves already-produced user, assistant, Tool, and note output.
- Stop and interjection produce distinct terminal facts.
- Send Now targets one accepted queue `input_id`.
- A terminal Turn remains provider-valid around Tool Use and Tool Result pairs.
- Existing queue entries are never silently discarded by Stop.
- Provider failure is not reported as intentional user cancellation.

## Authority, persistence, concurrency, and failure boundaries

**Authority.** Endpoints and viewers request controls; Conversation Authority
validates Turn ID, input ID, Authority Epoch, phase, and allowed action before
mutating state.

**Persistence.** Partial complete output, Tool results, stop/interjection
markers, and terminal lifecycle are durable. Viewer state and stream readers
are not. No history reload may erase the stop-time frame.

**Concurrency.** Stop, queue mutation, Send Now, natural completion, and next
Turn claim serialize on the per-Chat transition gate. Exactly one terminal path
wins; late commands return current authority state without duplicating work.

**Failure.** Transport loss detaches the viewer. Provider or Tool failure enters
the Turn failure path. Stale epoch, stale Turn, missing queue entry, or unsafe
Tool phase rejects the requested control explicitly.

## Consequences

- Frontends need explicit interrupted and interjected render states.
- Queue controls remain meaningful after a Stop.
- Navigation and browser suspension do not change execution outcome.
- Tool runtimes must expose cancellation and transfer capability truthfully.
- Reconnect uses authoritative activity and durable state rather than local
  stream completion guesses.

## Rejected alternatives

- Implement Stop by cancelling the viewer stream.
- Reload history immediately and use it to decide the visible stop result.
- Clear all queued input whenever Stop is pressed.
- Treat passive and active interjection as the same action.
- Resend queued content to implement Send Now.
- Kill side-effecting Tools based on guessed remaining duration.
- Append a fabricated assistant apology after interruption.

## Implementation and verification

Current implementation anchors include:

- `crates/rua-server/src/services/conversation_authority.rs`;
- `crates/rua-server/src/services/conversation_input_intake.rs`;
- `crates/rua-server/src/services/console_chat.rs`;
- `crates/rua-server/src/control_adapters.rs`;
- `console/src/pages/Chat/owned/useConversationViewRuntime.ts`;
- `console/src/pages/Chat/owned/conversationProjectionReducer.ts`.

### Current implementation gaps

The terminal publication ordering still needs independent verification:

- `complete_active_turn_and_drain_queue` in
  `crates/rua-server/src/services/conversation_authority.rs` invokes its
  `publish_terminal` callback
  before removing the Active Turn, publishing the completed control snapshot,
  and claiming any next input. A viewer can therefore observe the terminal event
  before the authority handoff required by D2 is visible.

Verification must cover Stop before and after output, queue-present and
queue-empty Stop branches, passive safe-boundary promotion, active interjection
by `input_id`, Tool transfer refusal, Viewer Disconnect, provider failure, and
duplicate or stale controls. It must include facade-level queue preservation and
an assertion that the terminal event is not externally visible until the
post-terminal Control View/handoff is established. Relevant suites include
`crates/rua-server/tests/control_path/stop_command.rs`,
`crates/rua-server/tests/infra/conversation_input_intake.rs`, and
`console/src/pages/Chat/owned/__tests__/OwnedConversationSurface.test.tsx`.

## Related decisions

- ADR 0022: Conversation Authority and epoch fencing.
- ADR 0023: Active Turn, Turn Writer, and queue.
- ADR 0031: Durable Task Runtime.
- ADR 0032: Task Runner control and authority routing.
- ADR 0033: ACP permission and runner state.
