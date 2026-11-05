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

## Next package: symbolic references

Symbolic-reference reads are implemented. Creation/update remains planned, as defined below. The command remains a partial implementation.

### Supported: read symbolic references

Supported bounded forms:

```sh
ferragit symbolic-ref NAME
ferragit symbolic-ref -q NAME
ferragit symbolic-ref --quiet NAME
ferragit symbolic-ref --recurse NAME
ferragit symbolic-ref --no-recurse NAME
```

Initially support `HEAD` and validated full `refs/...` names. Read an unborn
symbolic HEAD without requiring the target or an object to exist. Default reads
follow symbolic chains; `--no-recurse` returns the immediate target, including
when subsequent links form a cycle. Read-only operations must not create locks,
directories or logs. Match the pinned reference's chain boundary, failure codes,
output bytes and newline behavior rather than assuming the existing resolver's
limit is the compatibility rule.

The Git 2.56 oracle distinguishes a successful symbolic read (status 0), a quiet
non-symbolic/missing read (status 1, empty output), and a non-quiet detached,
missing or cyclic read (status 128 with its diagnostic). A direct ref, a missing
ref, malformed ref data and an unresolvable chain need separate cases; do not
collapse them into a generic repository error.

### Planned: create and update symbolic references

Implement the bounded forms:

```sh
ferragit symbolic-ref NAME TARGET
ferragit symbolic-ref -m REASON NAME TARGET
```

Initially support `HEAD` or a full `refs/...` source and full `refs/...` targets,
including unborn targets. Other one-level/pseudoref forms remain explicitly
unsupported in this slice. Reject invalid names and an empty explicit reason
before mutation. Keep target objects and destination refs intact.

Lock and replace the **named symbolic ref**, not the terminal ref to which its
old value resolves. The current `Update::finish` writes a terminal object ID and
must not implement this operation. A packed direct ref may receive a loose
symbolic override; its unrelated packed data must survive.

Qualify reflog policy from the reference rather than copying ordinary object-ID
update behavior. The prepared oracle cases show that retargeting HEAD to an existing destination
appends according to logging policy, with or without an explicit reason. `-m`
supplies the reason; a missing destination does not append in these cases, even
with a reason. Include
retargeting a current packed branch and its associated HEAD-log effects. Writing
symbolic refs must not accidentally create commits, alter index/worktree state,
or evaluate identity when no log append needs it.

Document lock ownership, revalidation, publication, cleanup and visible failure
effects. Preserve existing locks. Retain current refusals for unsupported hooks,
filesystem indirections and reference backends. Do not introduce runtime child
processes or silently tighten Git's accepted chain behavior.

## Code ownership

- `git-cli` parses flags and presents exact stdout, stderr and status.
- A small planned `git-core/src/refs/symbolic.rs` owns symbolic reads/writes.
  Extract a bounded loose-ref decoder from existing resolution only if both
  callers need it; preserve existing object-ID update behavior.
- Reuse `git-platform::LockFile` and existing reflog framing where their semantics
  match. Add no generic backend framework or dependency for this package.
- Extend CLI regression tests and guarded conformance scenarios. Keep oracle
  fixture generation outside runtime crates.

## Acceptance matrix

Run independent reference/candidate fixtures with SHA-1 and SHA-256 on all four
native CI runners. Check exact bytes/status and relevant repository state.

| Area | Required cases |
| --- | --- |
| Reads | Attached/unborn HEAD; direct and packed direct refs; missing refs; quiet diagnostics; recursive and one-level chains |
| Bounds | Self/multiple-ref cycles, chain boundary, malformed/oversized data, invalid names |
| Writes | Create, retarget, unchanged target, direct-to-symbolic conversion, packed source, existing and missing destination |
| Logs | No reason, explicit/empty reason, logging disabled/always, existing logs, bare repositories, missing identity |
| Safety | Stale source lock, competing retarget/update writers, failed log/publication, owned cleanup, preserved target refs/objects |
| Platforms | Symlink/reparse-point refusal, applicable native path cases, executable process guard |

The current 170 guarded comparisons must continue passing. New cases must fail
against the current unimplemented command before the implementation is counted.
Require formatting, Clippy, workspace regressions, headers, architecture, MSRV,
credential scanning and inspected native CI results before publication is
described as qualified. This package does not close all of Phase 1.

## Following work

`--short` requires correct ambiguity handling through reference enumeration; do
not implement it by stripping `refs/heads/`. Symbolic-ref deletion must remove
the named pointer and preserve its target, unlike the dereferencing `update-ref
-d` operation. Schedule both explicitly after this bounded package. General
reference enumeration/listing, transactions, discovery/configuration, structured
object codecs/plumbing and failure-safety qualification remain in the Phase 1
checklist. Full worktree workflows, reftable and advanced maintenance remain in
their later phases; unsupported mandatory formats must still fail safely.

Reference behavior is pinned to
[Git 2.56 command implementation](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/symbolic-ref.c)
and its
[command specification](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-symbolic-ref.adoc).
