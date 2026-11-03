<!-- SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be> -->
<!-- SPDX-License-Identifier: GPL-2.0-only -->

# Architecture

The runtime dependency direction is `git-cli -> git-core -> git-platform`. The conformance crate is an independent test tool. There is no C Git, libgit2 or gitoxide implementation behind the runtime commands.

| Location | Responsibility |
| --- | --- |
| `git-core/object.rs` | Hash-aware IDs, pure Git object framing and checked decoding |
| `git-core/store.rs` | Loose/packed lookup, read budgets, hash verification and no-clobber loose writes |
| `git-core/store/pack.rs` | Pack checksums, CRCs, offset boundaries and bounded delta-chain traversal |
| `git-core/store/pack/index.rs` | v1/v2 index checksums, fanout tables, IDs and 32/64-bit offsets |
| `git-core/store/pack/delta.rs` | Bounded delta integer, copy and insert instructions |
| `git-core/config.rs` | Limited config syntax, local values and mandatory-extension checks |
| `git-core/repository.rs` | New repository initialization, discovery and simple refs |
| `git-core/repository/packed_refs.rs` | Validated packed refs and peeled-record syntax |
| `git-core/index.rs` | v2 index codec, checksum, staging and ignore rules |
| `git-core/commit.rs` | Git tree ordering, commit bytes and locked branch updates |
| `git-core/push.rs` | Reachability, pack encoding, packet lines, negotiation and HTTPS push |
| `git-core/github.rs` | GitHub repository provisioning, inspection, naming, publication verification and CI check reads |
| `git-platform` | Exclusive files, no-clobber publication, locks, native paths and direct macOS Keychain access |
| `git-cli` | Arguments, identities, command dispatch and presentation |
| `git-conformance` | Independent fixtures, controlled environments, byte/status comparisons and C Git receiving endpoints |

`add -A` acquires the index lock before reading existing entries, stores blobs, writes a complete checksummed index and publishes it by rename. Failed staging may leave unreachable blobs, which is valid Git storage, but must preserve the previous index. Existing locks are never stolen.

`commit` validates identities and supported operation state, locks the branch and index, reads the staged snapshot, creates subtrees in Git ordering, encodes the commit and then updates the branch. Readers must not observe a partially written object. There is no claim of an atomic transaction spanning objects, refs, reflogs and worktree; reflog updates remain unimplemented.

`push` reads locally available loose or packed objects. It traverses the commit/tree graph, checks fast-forward ancestry and emits complete, non-delta objects into a version-2 pack with a trailing hash. It requests report-status and requires both successful unpack and success for the exact destination branch. TLS/authentication use maintained libraries; packet and pack logic belongs to this implementation.

Packed reads keep format parsing separate from storage lookup. Index checksums and tables are checked before use; pack checksums and offsets are checked before decoding. Each requested entry and delta base is checked against its CRC where present and its reconstructed object ID. Per-read cycle sets and explicit input/output budgets bound recursive work. A reference delta first searches its current pack, then the object store. There is no persistent pack cache yet; invalidation during concurrent maintenance and efficient large-repository lookup need further work.

SHA-1 uses RustCrypto's collision-detecting implementation. SHA-256 repositories carry their algorithm through IDs, index checksums, tree entries and pack checksums. Object sizes and aggregate pack sizes have explicit limits.

Runtime source currently cannot launch child processes. `scripts/check_architecture.py` enforces that boundary against accidental changes, and candidate test invocations use empty PATH/exec paths. These checks are not an OS security sandbox; the full plan's process-monitoring gate is still open. Authentication orchestration and the test oracle run outside the runtime crates.

When adding a command, parse it into explicit options, reuse core operations, keep diagnostics in the CLI, add reference comparisons including failure effects, and update the compatibility inventory. Split modules or crates only when concrete ownership or dependency needs justify it.
