# ADR-0055: Bounded Conversation Rendering and History Residency

> **Status:** Withdrawn 

The viewport virtualization and active-history eviction design was withdrawn
after scrolling regressions, including crashes, blank content, flicker, and
loading gaps. Its implementation was rolled back; its former requirements and
implementation plan are no longer current.

[ADR-0056](0056-whole-turn-conversation-dom-residency.md) defines the accepted
replacement using whole-Turn DOM reclamation with separate view state and stable
reading anchors. [ADR-0052](0052-owned-console-and-conversation-view-runtime.md)
continues to define the owning Conversation View Runtime.

This record reserves the identifier and withdrawal reason. It does not define
additional requirements for the current UI.
