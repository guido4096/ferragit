<!-- SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be> -->
<!-- SPDX-License-Identifier: GPL-2.0-only -->

# Reference updates and reflogs

Ferragit's files backend reads loose and packed refs, follows bounded symbolic-ref chains, and creates, updates or deletes one terminal ref under lock. Commits and the supported `update-ref` subset use the same writer. This is a repository-foundation subset, not full reference-backend or Git command compatibility.

## Command surface

```sh
target/debug/ferragit update-ref [-m REASON] REF NEW [OLD]
target/debug/ferragit update-ref [-m REASON] -d REF [OLD]
```

`REF` is `HEAD` or a full `refs/...` name. Symbolic refs are dereferenced. `NEW` names an existing object or is a full-width zero ID for deletion; a terminal branch under `refs/heads/` must name a commit. Full object IDs, supported abbreviations, and the simple ref forms accepted by `rev-parse` are usable object arguments.

Omitting `OLD` permits replacement of the value observed under lock. A nonzero `OLD` requires that exact current ID. An empty string or a full-width zero `OLD` requires that the terminal ref does not exist. IDs and zero values use the repository's SHA-1 or SHA-256 width. A failed expected-value check changes neither refs nor logs.

`-m` supplies the reflog reason, and `--` can terminate option parsing before `REF`. Successful updates emit no output. `--stdin`, `--no-deref`, `--create-reflog`, multi-ref transactions, general `pack-refs` maintenance, and other options are unsupported. Reflog files are written, but the `reflog` display/expiration command is not implemented.

Deletion removes both the resolved loose ref and its packed entry. Symbolic refs remain symbolic while their terminal target is deleted; a detached HEAD can itself be deleted. `-d` treats an empty or zero `OLD` as unconditional, matching Git's compatibility rule. Zero `NEW` retains ordinary expected-old semantics, so zero `OLD` then requires a missing ref. Deleting a missing ref succeeds when its expected-value check permits it.

The deleted terminal ref's log is removed. Applicable symbolic/HEAD logs receive an old-to-zero record, including a zero-to-zero record when deleting an unborn current branch. No identity is evaluated when no log append is required. Empty nested ref/log directories are removed; namespace roots and nonempty directories are preserved.

An updated packed ref receives a loose override. Its packed record remains intact. The writer checks the referenced object through the normal loose/packed object store before publishing the new ID.

## Locking and commit behavior

The writer resolves the selected symbolic chain, acquires its locks in stable name order, and resolves it again while locked. A changed symbolic chain is rejected. Expected old values are checked after locks are held. Existing locks are never stolen; owned locks are removed on failure or drop.

Commits require an attached HEAD. They hold the selected ref-chain locks while reading the parent and hold the index lock while reading the staged snapshot and creating objects. The shared writer appends the applicable reflogs and publishes the terminal branch ref. Failed commits may leave complete unreachable objects, as allowed by Git storage. A failed log append does not replace the staged index.

The default commit reflog reason is `commit (initial): SUBJECT` for a root commit and `commit: SUBJECT` for a subsequent commit. The subject is the first message line. The existing commit message-cleanup, signing, hook, merge-parent and output limitations still apply.

## Reflog policy and records

The supported local `core.logAllRefUpdates` values are:

| Value | Creation behavior |
| --- | --- |
| Unset | Automatic creation for ordinary worktrees; disabled for bare repositories |
| True | Automatic logs for HEAD and `refs/heads/`, `refs/remotes/`, and `refs/notes/` |
| False | No automatic creation |
| `always` | Automatic creation for all supported refs |

An existing log is appended even when automatic creation is disabled. A committer identity is required only if an append is needed. Ferragit uses the supported committer environment/configuration values and raw timestamp format; it does not implement Git's full identity discovery or date syntax.

Records contain the full old/new IDs, committer identity and timestamp, an optional tab-separated reason, and a newline. A new ref uses a zero old ID. Reasons collapse ASCII whitespace and preserve other bytes; NUL-containing or excessive reasons are rejected. An unchanged terminal ref does not append its own log, although an associated or explicitly selected symbolic HEAD log can receive an entry.

An empty directory at the log filename can be replaced by a log file. A directory containing logs is a namespace conflict and is preserved.

## Packed deletion and publication

Deletion holds the ref-chain locks and then `packed-refs.lock`, rechecks resolution, and validates the complete packed file before changing logs or refs. A replacement is supported for the canonical `# pack-refs with: peeled fully-peeled sorted ` header emitted by Git. Unrelated records and their peeled IDs retain their exact bytes. Noncanonical rewrite headers, extra record comments, malformed/null IDs and unsupported names fail before mutation. Read-only packed lookup retains its documented wider header support.

`packed-refs.new.lock` is an exclusively created publication file. Its synced contents replace `packed-refs` while the separate `packed-refs.lock` remains held. Packed deletion precedes loose deletion, preventing a concurrent packer from restoring the old loose value. Existing lock/publication files are never stolen. Directory crash durability remains unqualified.

## Failure boundaries

Ref publication and reflog appends are separate visible operations:

- For a symbolic HEAD update, its log can be appended before the terminal branch log or ref publication fails. Those earlier log entries remain visible.
- For a directly named branch, its log and ref can be updated before the associated HEAD log fails. The command reports failure with the branch already advanced.

Deletion appends applicable symbolic/HEAD logs, removes the terminal log, publishes any packed replacement, and then unlinks the loose ref. A later failure leaves earlier effects visible. A directory at the terminal log path is refused before mutation; Git can instead warn and continue for failed log removal. Other failed terminal-log unlinks currently return failure, which remains a compatibility gap. General lock/I/O diagnostics also remain incomplete.

Callers must inspect state before retrying a failed update. There is no claim of an atomic transaction across refs, reflogs, objects, index or worktree. Reflog appends and ref lock-file contents are synced, but directory crash durability, partial-append recovery and broad crash campaigns remain unqualified.

## Bounds and unsupported environments

Loose ref reads are bounded to 4 KiB, symbolic resolution to eight links, and reflog reasons to 64 MiB. Packed-ref limits are described in [PACKED_STORAGE.md](PACKED_STORAGE.md). Malformed IDs, cycles, invalid names, nonexistent new objects and non-commit branch targets fail explicitly.

Symlinks in ref or log write paths are refused. This check does not establish ownership safety, complete Windows reparse-point handling, or protection against hostile directory replacement races. Linked worktrees, alternate reference backends, non-UTF-8 ref names, additional pseudorefs and full diagnostics remain outside the qualified subset. A configured reference-transaction hook is refused; runtime code cannot invoke hooks.

## Verification

[reference_updates.rs](../crates/git-cli/tests/reference_updates.rs) compares independent C Git and Ferragit repositories for both hash algorithms, attached and detached ref updates, symbolic chains, packed-ref overrides, logging policies, unchanged refs, message bytes and expected-value diagnostics. It covers stale locks, competing writers, missing identity with logging disabled, relevant log failures, commit/index preservation, and publication failure cleanup. Deletion comparisons cover zero-ID rules, missing and symbolic refs, detached HEAD, packed-only and loose-over-packed refs, retained peeled tags, nested directory cleanup, HEAD-log failures, stale publication locks, malformed files and competing delete/update writers. Bare and logging-disabled deletions verify lazy identity evaluation. A Unix-specific case checks that a ref symlink cannot modify an outside file.

The conformance runner compares ref-update commands and stored HEAD/branch/topic logs alongside the existing object, index, commit and pack workflows. Object-validation diagnostics target Git 2.56; the available Git 2.53 oracle has a different outer error prefix. Tests assert each complete profile-specific message without normalizing error bytes. Reports record the actual oracle and platform; focused success does not qualify the full pinned-release or platform matrix.

Specifications: [git-update-ref](https://git-scm.com/docs/git-update-ref), [git-reflog](https://git-scm.com/docs/git-reflog).
