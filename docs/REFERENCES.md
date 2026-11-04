<!-- SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be> -->
<!-- SPDX-License-Identifier: GPL-2.0-only -->

# Reference updates and reflogs

Ferragit's files backend reads loose and packed refs, follows bounded symbolic-ref chains, and creates or updates one terminal loose ref under lock. Commits and the supported `update-ref` subset use the same writer. This is a repository-foundation subset, not full reference-backend or Git command compatibility.

## Command surface

```sh
target/debug/ferragit update-ref [-m REASON] REF NEW [OLD]
```

`REF` is `HEAD` or a full `refs/...` name. Symbolic refs are dereferenced. `NEW` must name an existing, nonzero object; a terminal branch under `refs/heads/` must name a commit. Full object IDs, supported abbreviations, and the simple ref forms accepted by `rev-parse` are usable object arguments.

Omitting `OLD` permits replacement of the value observed under lock. A nonzero `OLD` requires that exact current ID. An empty string or a full-width zero `OLD` requires that the terminal ref does not exist. IDs and zero values use the repository's SHA-1 or SHA-256 width. A failed expected-value check changes neither refs nor logs.

`-m` supplies the reflog reason, and `--` can terminate option parsing before `REF`. Successful updates emit no output. Deletion, zero `NEW`, `-d`, `--stdin`, `--no-deref`, `--create-reflog`, multi-ref transactions, packed-ref rewriting, and other options are unsupported. Reflog files are written, but the `reflog` display/expiration command is not implemented.

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

## Failure boundaries

Ref publication and reflog appends are separate visible operations:

- For a symbolic HEAD update, its log can be appended before the terminal branch log or ref publication fails. Those earlier log entries remain visible.
- For a directly named branch, its log and ref can be updated before the associated HEAD log fails. The command reports failure with the branch already advanced.

Callers must inspect state before retrying a failed update. There is no claim of an atomic transaction across refs, reflogs, objects, index or worktree. Reflog appends and ref lock-file contents are synced, but directory crash durability, partial-append recovery and broad crash campaigns remain unqualified.

## Bounds and unsupported environments

Loose ref reads are bounded to 4 KiB, symbolic resolution to eight links, and reflog reasons to 64 MiB. Packed-ref limits are described in [PACKED_STORAGE.md](PACKED_STORAGE.md). Malformed IDs, cycles, invalid names, nonexistent new objects and non-commit branch targets fail explicitly.

Symlinks in ref or log write paths are refused. This check does not establish ownership safety, complete Windows reparse-point handling, or protection against hostile directory replacement races. Linked worktrees, alternate reference backends, non-UTF-8 ref names, additional pseudorefs and full diagnostics remain outside the qualified subset. A configured reference-transaction hook is refused; runtime code cannot invoke hooks.

## Verification

[reference_updates.rs](../crates/git-cli/tests/reference_updates.rs) compares independent C Git and Ferragit repositories for both hash algorithms, attached and detached ref updates, symbolic chains, packed-ref overrides, logging policies, unchanged refs, message bytes and expected-value diagnostics. It covers stale locks, competing writers, missing identity with logging disabled, relevant log failures, commit/index preservation, and publication failure cleanup. A Unix-specific case checks that a ref symlink cannot modify an outside file.

The conformance runner compares ref-update commands and stored HEAD/branch/topic logs alongside the existing object, index, commit and pack workflows. Object-validation diagnostics target Git 2.56; the available Git 2.53 oracle has a different outer error prefix. Tests assert each complete profile-specific message without normalizing error bytes. Reports record the actual oracle and platform; focused success does not qualify the full pinned-release or platform matrix.

Specifications: [git-update-ref](https://git-scm.com/docs/git-update-ref), [git-reflog](https://git-scm.com/docs/git-reflog).
