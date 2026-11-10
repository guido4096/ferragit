<!-- SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be> -->
<!-- SPDX-License-Identifier: GPL-2.0-only -->

# ferragit — proof of concept

A **proof of concept** for an independent Rust Git implementation, exploring the full compatibility goal in [PLAN.md](PLAN.md). This is experimental software, not a production-ready Git replacement. The current milestone can stage this project, create Git-compatible trees and commits, and push a branch to GitHub using its own pack writer and smart-HTTP protocol implementation.

The repository-foundation slice also reads Git-generated packs and packed refs. It can continue committing and construct a push after the history has been repacked. See [packed-storage support and limits](docs/PACKED_STORAGE.md). Single-ref updates/deletions and commit reflogs use a shared files backend; see [reference support and failure boundaries](docs/REFERENCES.md).

This is an early development build. It implements small subsets of twelve commands and is **not a drop-in replacement for Git yet**. Use it on disposable repositories or this bootstrap project. [Compatibility status](compat/status.json) lists the gaps; [architecture](docs/ARCHITECTURE.md) explains the code.

The repository is public. See [credential handling and secret checks](docs/SECURITY.md) for the scope of the checks and how runtime authentication works. Contributor and agent instructions are in [AGENTS.md](AGENTS.md). The [Phase 1 checklist and prepared next package](docs/PHASE1.md) describe the remaining foundation work.

## Build and check

Rust 1.88 or newer is required by the locked dependencies. The TLS dependency also needs the platform's C build tools. Python 3.11+ is used for repository tooling.

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check
python3 scripts/check_architecture.py
target/debug/git-conformance --candidate target/debug/ferragit --report artifacts/conformance.json
```

On Windows, use `target/debug/ferragit.exe` and `target/debug/git-conformance.exe`. CI runs native Linux, Windows, Intel macOS and ARM64 macOS checks, a Rust 1.88 MSRV check, and credential scanning on pushes and pull requests. Inspect the workflow results for the commit being evaluated.

The executable comparison suite records the oracle's version and build options. Local macOS x86_64 validation includes the pinned Git 2.56.0 core profile: 226 guarded comparisons and measurement qualification. This does not establish the full release contract. Target source commits are pinned in [compat/baselines.json](compat/baselines.json). The [command inventory](compat/commands.json) is a starting list from upstream, not an exhaustive option audit. [Phase 0 tooling and remaining gates](docs/PHASE0.md) describe pinned provisioning, native process guards, state comparisons and replayable fixtures.

## Try a local commit

Use a new directory. Initialization intentionally refuses to reinitialize existing repositories and does not install Git's sample hook templates.

```sh
target/debug/ferragit init --template= -b main /tmp/ferragit-demo
# Create files in /tmp/ferragit-demo before staging.
target/debug/ferragit -C /tmp/ferragit-demo add -A
target/debug/ferragit -C /tmp/ferragit-demo -c user.name=Guido -c user.email=guido@ngjansen.be commit -m "First commit"
target/debug/ferragit -C /tmp/ferragit-demo rev-parse HEAD
```

`add -A` currently runs from the worktree root. It understands per-directory `.gitignore` rules and stages files, executable bits on Unix, and symbolic links. It refuses attributes, submodules, unsupported index flags/extensions and configured local conversion features. Global/system configuration and attributes are not implemented; this is another reason to use controlled repositories.

## Push without C Git

```sh
# On macOS, ferragit reads GitHub's saved Internet Password directly from Keychain.
# Other platforms currently require FERRAGIT_TOKEN in the execution environment.
# Never put a token in a command argument or a remote URL.
target/debug/ferragit push https://github.com/OWNER/REPO.git HEAD:refs/heads/main
```

The Rust executable reads the token directly from macOS Keychain through Security.framework, or uses an explicit `FERRAGIT_TOKEN` override. It discovers remote refs, checks the existing branch is an ancestor, constructs a complete non-delta pack, sends the ref update and pack over HTTPS, and checks the remote's unpack/ref status. Redirects and credentials in remote URLs are rejected. Push currently supports GitHub HTTPS branch destinations only; fetching, SSH transport, force pushes, named remotes and upstream tracking are pending. Use `FERRAGIT_CREDENTIAL_ACCOUNT` when the Keychain account differs from the URL owner.

`ferragit repo create --private OWNER/NAME` creates an empty private GitHub repository using its API. This is a ferragit extension: standard Git does not provision GitHub repositories. It verifies that the authenticated account matches the owner and refuses name collisions.

`ferragit repo rename OWNER/NAME NEW_NAME` updates a GitHub repository's name and verifies that its repository ID and visibility are preserved. The `repo` commands use the same native credential access as HTTPS push.

`ferragit repo checks OWNER/NAME` reads GitHub's CI checks for the local `HEAD` commit, including failed-check annotations and step metadata. `ferragit repo job-log OWNER/NAME JOB_ID` reads a job's text log with a 16 MiB limit. Its short-lived download URL is restricted to trusted HTTPS Actions storage hosts, and the GitHub credential is never forwarded to the download request.

`ferragit repo api [--anonymous] OWNER/NAME RESOURCE` reads bounded repository JSON. `ferragit repo download [--anonymous] OWNER/NAME RESOURCE DESTINATION` downloads an immutable source tarball (`tarball/COMMIT`) release asset (`releases/assets/ID`), or Actions artifact ZIP (`actions/artifacts/ID/zip`) with a SHA-256 receipt and a 512 MiB limit. Existing destinations are refused. The extensions support baseline evidence and do not implement standard Git commands.

`scripts/publish_bootstrap.py` launches **ferragit** for private repository creation, push and verification. It makes no C Git calls and never retrieves or forwards credentials. `ferragit repo verify OWNER/NAME` checks GitHub's branch ref, tree, blob IDs and modes against the local commit/index, for public or private repositories. The wrapper writes a credential-free receipt under ignored `artifacts/` and records the newly created repository ID so an interrupted push can be retried against that exact repository.

## Current limits

`symbolic-ref` reads HEAD/full refs with quiet and recursion controls. It creates or retargets the named pointer with `symbolic-ref [-m REASON] NAME TARGET`, including unborn destinations. Shortening and symbolic-pointer deletion remain pending. Named-ref and associated HEAD logs have separate publication boundaries; see [reference support](docs/REFERENCES.md). Reads follow the pinned five-lookup bound and never create logs or locks.

- Reads loose objects and PACK v2/v3 files through v1/v2 indexes, including OFS/REF deltas and escaped 64-bit offsets. Packed-ref lookup preserves loose-ref precedence. Pack maintenance commands and accelerated/cached lookups remain pending.
- Objects are limited to 64 MiB; incoming and outgoing packs to 128 MiB and 100,000 objects. Packed reads have additional explicit depth, index, ref and work limits described in [packed storage](docs/PACKED_STORAGE.md).
- `hash-object` supports raw stdin and file hashing with explicit `--no-filters`; non-blob writes require `--literally` until validation is implemented.
- `cat-file` supports type, size, existence, raw same-type data and non-tree pretty output, with full IDs, loose/packed-object abbreviations and simple branch/full ref names. Batch mode, revision expressions and tree pretty output are pending.
- `update-ref [-m REASON] REF NEW [OLD]` creates/updates one ref, dereferences symbolic refs and checks optional old IDs under lock. `-d` and full-width zero `NEW` delete the resolved ref, including a canonical packed entry. Multi-ref transactions, general `pack-refs`, additional pseudorefs and the `reflog` command remain unsupported. Packed rewrite and reflog-failure limits are documented in [reference support](docs/REFERENCES.md). Refs and logs have explicit partial-failure boundaries.
- Index writes support v2, normal stage-0 entries and safe TREE-cache removal. No sparse or conflict index support.
- Commits support a single parent and `-m`/`-F`, fixed raw dates or current UTC, explicit identities and quiet output. Commits write branch/HEAD reflogs through the shared reference writer. Signing, hooks, normal message cleanup, amend, empty-commit rules and full output compatibility are pending. Hooks and signing configured in the local repository are refused.
- Configuration parsing is intentionally limited. Repository extensions not implemented here are refused before mutation. Discovery does not yet handle linked worktrees or ownership/cross-filesystem rules.
- File fsync and exclusive lock files are implemented. Directory crash durability and exhaustive fault injection remain future work. Native test process guards are described in [Phase 0](docs/PHASE0.md).
- Full upstream tests, all release architectures and minimum-OS/filesystem matrices have not been qualified.

## License and copyright

Original project code: **Copyright (C) 2026 Guido <guido@ngjansen.be>**.

Licensed under **GPL-2.0-only**; see [LICENSE](LICENSE). Source headers use SPDX identifiers. [REUSE.toml](REUSE.toml) covers generated data and the lockfile. Dependencies and any upstream material retain their own copyrights and license terms; [THIRD_PARTY.md](THIRD_PARTY.md) records provenance.
