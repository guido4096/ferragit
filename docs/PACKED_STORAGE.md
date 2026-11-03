<!-- SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be> -->
<!-- SPDX-License-Identifier: GPL-2.0-only -->

# Repository-foundation slice: packed storage

This slice closes a practical proof-of-concept gap: Git-repacked history is now readable, and ferragit can create another commit and construct a push from it. It does not complete Phase 1 or establish full Git compatibility.

## Supported reads

- SHA-1 and SHA-256 object IDs, pack/index checksums and delta base IDs.
- PACK versions 2 and 3 with v1/v2 indexes, including escaped 64-bit index offsets.
- Plain objects, offset deltas and reference deltas; nested chains and reference bases found in the object store.
- Abbreviated IDs across loose objects and multiple packs, with duplicate IDs deduplicated and distinct matching IDs reported as ambiguous.
- `packed-refs` fallback for branches and full ref names, including annotated-tag peeled-record validation. A loose ref overrides its packed entry. Revision suffixes such as `^{}` remain unsupported.

Normal object writes remain loose. The outgoing sender writes complete, non-delta packs. This slice does not add `repack`, `gc`, `pack-refs`, alternates, linked worktrees or reftable commands/backends.

## Integrity and resource limits

The reader checks index checksums, fanout consistency, sorted unique IDs, offset tables and pack/index agreement. It checks pack checksums and counts, entry boundaries, v2-index CRCs, zlib lengths, delta copy/insert bounds and the reconstructed ID of each returned object/base. Cycles, excessive recursion and overflowing integers fail without returning object data. Validation of a requested chain is not a complete repository fsck.

| Resource | Current limit |
| --- | --- |
| Object or inflated delta | 64 MiB |
| Individual pack file | 128 MiB |
| Objects in one pack index | 100,000 |
| Individual index file | 16 MiB |
| Pack indexes discovered | 1,024 |
| Delta links | 64 |
| Cumulative pack/index input per lookup | 256 MiB |
| Cumulative inflated/reconstruction work per lookup | 256 MiB |
| Packed-ref file | 16 MiB / 100,000 refs |

Supporting a 64-bit offset table does not remove the 128 MiB pack limit. Limits fail explicitly rather than silently omitting history. Non-UTF-8 packed-ref files are currently unsupported.

Packs are verified during lookup and there is no persistent pack cache. Repeated reads of large packs can be expensive. Concurrent pack replacement/removal can cause a read error; retry/invalidation behavior during maintenance remains unqualified. The memory/work limits are implementation safeguards, not performance or process-RSS guarantees.

## Evidence

The executable tests create repositories with ferragit, have C Git generate packs and packed refs, remove the disposable fixtures' loose copies, then compare every object's raw bytes. Fixtures assert that both delta encodings actually occur. They cover both hash algorithms, index v1/v2, forced 64-bit offsets, v3 packs, tags, abbreviation ambiguity, duplicate loose/packed IDs, multiple packs and subsequent commits.

The Rust sender is also exercised against C Git receive-pack with packed source history, followed by strict Git fsck of the receiver. Corruption cases recompute outer checksums where needed so CRC, delta and object-ID validation are tested independently. Other cases cover cyclic/overdeep chains, malformed refs and lock cleanup after failed commits.

The conformance report now includes repacking, packed HEAD lookup, abbreviation lookup, continued commits and mixed-storage pushes for both hash algorithms. Native CI runs the tests on Linux, Windows and Intel/ARM64 macOS. Reference-version and platform records remain part of the reports; this is not a claim that the complete pinned-release suite has passed.

Format references: [pack/index format](https://git-scm.com/docs/gitformat-pack), [packed-ref behavior](https://git-scm.com/docs/git-pack-refs).
