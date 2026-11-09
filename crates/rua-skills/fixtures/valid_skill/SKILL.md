---
name: himalaya
description: "CLI to manage emails via IMAP/SMTP."
triggers:
  - "check email"
  - "send email"
  - "list inbox"
metadata:
  builtin_skill_version: "1.1"
  openclaw:
    emoji: "\U0001F4E7"
    requires:
      bins:
        - himalaya
      env:
        - IMAP_PASSWORD
---
# Himalaya Email CLI

Himalaya is a CLI email client.
