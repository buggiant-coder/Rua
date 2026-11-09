# docx upstream attribution

This skill incorporates material from Anthropic's public skills repository:
https://github.com/anthropics/skills/tree/2c5958ecac7cee2b583967a2c4981d0f4a75c579/skills/docx

Upstream commit: `2c5958ecac7cee2b583967a2c4981d0f4a75c579`. Licensed under Apache-2.0;
see the accompanying `LICENSE.txt`. Repository/archive pins and the list of
modified files are in `crates/rua-skills/document-skills-upstream.json` in Rua's
source distribution.

Modified by The Rua Authors, 2026-11-07: builtin version metadata, Runtime
working-directory/dependency guidance, and text whitespace normalization.
LibreOffice discovery works with Windows installation paths; the socket shim is Linux-only.
Tracked-change acceptance uses a platform temporary path and a proper profile URI.
