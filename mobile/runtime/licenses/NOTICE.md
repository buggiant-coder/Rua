# Android native payload attribution

- PRoot 5.1.107.102 and its loader: https://github.com/termux/proot ; the package metadata declares GPL-2.0. The pinned main program and loader source headers permit GPL-2.0-or-later (see src/cli/proot.c and src/loader/loader.c). Preserve the upstream notices; the linked distribution with LGPL-3.0-or-later talloc must use compatible GPL v3 terms, rather than treating the package field as GPL-2.0-only.
- libtalloc 2.5.0: https://talloc.samba.org/talloc/doc/html/ ; the library source is LGPL-3.0-or-later (LGPL-3.0.txt together with GPL-3.0.txt). Copyright (C) Andrew Tridgell 2004-2005 and Stefan Metzmacher 2006. The Termux package metadata declares GPL-3.0; both the package declaration and library license are recorded in native-dependencies.json.
- libandroid-shmem 0.7: https://github.com/termux/libandroid-shmem ; BSD notice reproduced in libandroid-shmem.txt.
- xterm.js terminal bridge: https://github.com/xtermjs/xterm.js , MIT. Bundled JavaScript retains its license comments.

Exact native package versions, binary URLs, source archive URLs and SHA-256 digests are in native-dependencies.json, distributed with the APK. Native build staging changes the talloc SONAME/dependency name only; it does not install or execute a Termux userspace. GPL and LGPL license texts accompany this file. Debian/Node/uv distribution license files remain in the separately downloaded guest/toolchain distributions.

These are standalone native components from Termux package builds, not Termux App modules. Source archive hashes identify the upstream inputs; they do not replace the matching Termux build recipes, patches, or binary-release source delivery obligations.
