<!-- SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be> -->
<!-- SPDX-License-Identifier: GPL-2.0-only -->

# Phase 1 repository foundation

Phase 1 remains open. Its deliverables are repository discovery/configuration,
hash-aware object codecs and storage, references, locking and selected low-level
read/write plumbing. [phase1.json](../compat/phase1.json) tracks the work packages.
This phase does not claim full command compatibility.

The implementation supports loose/packed object reads for both hash algorithms,
single-ref create/update/delete operations and the documented reflog subset.
[Reference support](REFERENCES.md) and [packed storage](PACKED_STORAGE.md) define
the actual limits. The deletion slice has 170 passing guarded comparisons on
Windows, Linux, Intel macOS and ARM64 macOS; the
[implementation CI](https://github.com/guido4096/ferragit/actions/runs/41987238342)
also checks Rust regressions, MSRV, architecture, headers and credentials.

## Supported symbolic-reference package

The bounded command surface is implemented for `HEAD` and validated full
`refs/...` names. It remains a partial command implementation:

```sh
ferragit symbolic-ref [-q|--quiet] [--recurse|--no-recurse] NAME
ferragit symbolic-ref [-m REASON] NAME TARGET
```

Reads support unborn targets, quiet non-symbolic/missing results and immediate
or recursive resolution. The pinned boundary is five lookups including the
terminal lookup. A one-level read returns the immediate target even when later
links form a cycle. Reads create no files, directories, locks or logs. Direct
and missing refs return status 1 silently with quiet mode, otherwise status 128;
broken/cyclic recursive reads return status 128 even with quiet mode.

Writes replace the named pointer under lock, preserving its previous
destination and unrelated packed data. Targets are full `refs/...` names and
may be unborn. Direct-to-symbolic conversion, unchanged targets and cycles are
accepted. Empty explicit reasons and invalid targets fail before mutation.
Additional one-level/pseudoref forms, `--short` and symbolic deletion are outside
this slice.

[Reference support](REFERENCES.md) specifies reflog policy, lazy identity,
named and associated HEAD locks, prepared pointer bytes, publication order and
partial failure effects. Hook/backend/filesystem-indirection refusals remain.
No runtime child processes or external implementation libraries are used.

### Ownership and verification

`git-cli` owns parsing, exact output and status. `git-core/refs/symbolic.rs`
owns inspection and named-pointer writes; ordinary terminal-ID updates retain
their separate resolver behavior. `git-platform::LockFile` separates synced
preparation from publication. The shared reflog module owns policy and framing.

The regression and guarded comparison cases cover both hashes:

| Area | Cases |
| --- | --- |
| Reads | Unborn/attached HEAD, quiet direct/packed/missing refs, chains, cycles, five-lookup boundary, malformed/invalid targets |
| Writes | Create, unchanged/retargeted pointer, packed/direct/current-symbolic source, existing/missing target, empty source directory |
| Logs | No/explicit/empty reason, logging disabled/always, existing logs, bare repositories, exact whitespace bytes, lazy identity failures |
| Safety | Existing source locks, competing symbolic/object-ID writers, failed named/HEAD logs, prepared lock visibility and cleanup |

The guarded suite has 226 comparisons per profile. Native qualification runs
on Windows, Linux, Intel macOS and ARM64 macOS with formatting, Clippy,
regressions, architecture, headers, MSRV and credential gates. Each revision
requires inspected CI before its native results can be claimed. Focused tests
do not establish broad crash durability, complete I/O diagnostics, all
filesystem cases or full Git identity discovery. Phase 1 remains open.

## Next planned package: enumeration and shortening

Implement bounded enumeration shared by reference consumers: merge loose and
packed names, preserve loose precedence, validate symbolic/broken references
and refuse unsupported backends or filesystem indirections. Define explicit
byte/count/traversal bounds and deterministic ordering. Enumeration is read-only
and must not create directories, locks or logs.

Use that primitive for `symbolic-ref --short`, matching ambiguity-aware
shortening against the pinned oracle. Cover colliding branch/tag/remote names,
packed/loose duplicates, dangling symbolic refs, malformed data and both hash
algorithms with exact bytes/status on every native profile. Do not approximate
shortening by stripping `refs/heads/`. General `for-each-ref` formatting/sorting
and reference transactions remain separate packages.

## Following work

Symbolic-ref deletion is a separate planned write slice. It must remove the
named pointer and preserve its target, unlike the dereferencing `update-ref -d`
operation. Reference transactions, discovery/configuration, structured object
codecs/plumbing and failure-safety qualification remain in the Phase 1
checklist. Full worktree workflows, reftable and advanced maintenance remain in
their later phases; unsupported mandatory formats must still fail safely.

Reference behavior is pinned to
[Git 2.56 command implementation](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/symbolic-ref.c)
and its
[command specification](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-symbolic-ref.adoc).
