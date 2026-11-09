# ADR-0047: Best-effort Device notifications

> **Status:** Accepted

## Context

Product features sometimes need to attract the user's attention without
creating durable content. Completion alerts, approval reminders, Task events,
and future producers may target the Device where the relevant user action
originated. These signals are not Inbox Messages, Conversation history, or Chat
Live Activity.

Rua already has one authenticated Device Session per active peer and local
platform adapters for Android, Tauri, and standalone browsers. Notifications
should reuse that routing without creating another durable delivery system or
forcing producers to understand platform placement.

## Decision

### D1 - User Notification is an independent capability

A product domain explicitly calls one User Notification `send` interface when
it wants an attention signal. The notification service does not subscribe
implicitly to every Agent, Chat, Task, or Hook lifecycle event.

User Inbox remains a separate durable message domain. A producer may explicitly
create an Inbox Message, send a notification, do both, or do neither.

### D2 - Delivery is pure best effort

Notifications have no persistent queue, retry worker, expiry processor,
delivery guarantee, durable history, unread state, or acknowledgement contract.
An unavailable target, missing local presenter, or denied platform permission
does not change the producer's business result.

### D3 - Requests are Device-addressed

A request contains target Device id, sender id and display name, title,
optional body, and optional Rua URI. The notification service treats producer
semantics and resource URI shape as opaque.

Addressing stops at Device identity. Rua does not introduce browser-tab,
WebView, Tauri-window, or mobile-client identities below one Device.

### D4 - One routing interface hides location

The service compares the target Device with the local Device. Local targets go
to the configured local adapter; remote targets use one transient notification
envelope over the existing shared Device Session Command stream. The transport
returns a bounded command response internally, but the producer receives no
durable delivery acknowledgement or retry contract.

Producers do not select local versus remote transport and do not access the
Device Session Manager directly.

### D5 - Notification transport is not Sync or Chat

Remote notification delivery is transient Device Session traffic. It is not a
SyncChange, Inbox record, Chat SSE event, Conversation Message, Runtime Note,
or durable command. Missed notifications are not replayed after reconnect.

### D6 - Exactly one local adapter presents

On the target Device, one adapter presents the notification. An explicit native
capability selects Android first; Tauri runtime detection selects the desktop
adapter; the browser Notifications API is used only for a standalone browser.

An embedded Console must not present the same notification through both native
and browser APIs. User-Agent guessing is not the shell-detection contract.

### D7 - Platform permission remains local

Android, macOS, Windows, and browsers own notification permission and display
behavior. The adapter may request or observe platform permission and may drop
the notification when presentation is unavailable.

Rua does not add notification categories, quiet hours, preferences, or a
cross-platform policy engine in this decision.

### D8 - Producers own when and what to notify

Conversation and Task services may map their own durable provenance, such as
the Device that submitted an input, into a notification target. That mapping
belongs to the producer. The generic service does not inspect conversation
state or infer completion events.

## Hard invariants

- Notification sending never creates an Inbox Message implicitly.
- Notifications are transient and are never synchronized.
- Failure never rolls back or fails the producer's completed business action.
- Producers call one location-transparent interface.
- Remote delivery reuses the authenticated shared Device Session.
- Exactly one local platform adapter is selected.
- Native shell detection uses explicit capability/runtime signals.
- No same-Device client identity or fan-out protocol is introduced.
- Rua URI handling remains optional and resource-specific.

## Authority, persistence, and failure boundaries

The producing domain owns the decision, wording, target, and Rua URI. The
User Notification service owns Device routing. Device Session owns remote
transport. The selected local adapter owns platform presentation.

No layer persists delivery state. Session unavailability, process restart,
permission denial, adapter failure, or absent viewers may lose the signal.
Those are accepted failures and are observable only through bounded diagnostics
or logs, not user-visible delivery status.

## Consequences

- Features can request attention without polluting durable user content.
- Local and remote delivery share one producer interface.
- Mobile, desktop, and browser presentation can evolve independently.
- Notification loss is an explicit product property.
- Durable workflows that require acknowledgement must use another domain.

## Rejected alternatives

- Model every notification as a User Inbox Message.
- Reuse Chat SSE as the generic notification bus.
- Persist notifications in Sync for later delivery.
- Create a second cross-device transport.
- Let each producer choose Android, Tauri, or browser APIs.
- Fan out to every client attached to one Device.
- Add retries, read state, preferences, or notification history.

## Implementation and verification

- `crates/rua-server/src/services/user_notification.rs`
- `crates/rua-server/src/services/console_user_notification.rs`
- `crates/rua-server/src/services/chat_completion_notification.rs`
- `crates/rua-server/src/services/task_notification.rs`
- `console/src/userNotifications/adapters.ts`
- `console/src/userNotifications/adapters.test.ts`
- `console/src-tauri/src/user_notification.rs`
- `crates/rua-server/tests/infra/user_notification.rs`
- `mobile/mobile-shell/app/src/test/kotlin/com/rua/mobile/MobileNotificationBridgeTest.kt`
- `console/src-tauri/tests/user_notification.rs`

Verification covers local/remote routing, session failure tolerance, producer
success independence, adapter precedence, denied permission, Rua URI
activation, and absence of durable queue or replay behavior.

## Related decisions

- ADR-0037 provides shared Device Session transport.
- ADR-0043 explains why a suspended mobile Device may miss a notification.
- ADR-0023 owns accepted input and Active Turn provenance.
- ADR-0044 owns Conversation Live Activity rather than notification delivery.
- ADR-0046 distinguishes durable deletion facts from transient attention.
