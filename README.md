<!-- SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be> -->
<!-- SPDX-License-Identifier: GPL-2.0-only -->

# rugit — proof of concept

A **proof of concept** for an independent Rust Git implementation, exploring the full compatibility goal in [PLAN.md](PLAN.md). This is experimental software, not a production-ready Git replacement. The current milestone can stage this project, create Git-compatible trees and commits, and push a branch to GitHub using its own pack writer and smart-HTTP protocol implementation.

This is an early development build. It implements small subsets of ten commands and is **not a drop-in replacement for Git yet**. Use it on disposable repositories or this bootstrap project. [Compatibility status](compat/status.json) lists the gaps; [architecture](docs/ARCHITECTURE.md) explains the code.

The repository is public. See [credential handling and the publication audit](docs/SECURITY.md) for the scope of the secret checks and how runtime authentication works.

## Build and check

Rust 1.88 or newer is required by the locked dependencies. The initial local build uses Rust 1.98.1. The TLS dependency also needs the platform's C build tools. Python 3.11+ is used for repository tooling.

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check
python3 scripts/check_architecture.py
target/debug/git-conformance --candidate target/debug/rugit --report artifacts/conformance.json
```

On Windows, use `target/debug/rugit.exe` and `target/debug/git-conformance.exe`. CI defines native Linux, Windows, Intel macOS and ARM64 macOS jobs plus an MSRV check. Configuring those jobs does not establish that they have passed; inspect the workflow results. The first bootstrap run passed Linux, Windows and Rust 1.88; GitHub could not allocate the ARM64 macOS runner, so that job executed no checks. Local Intel macOS checks passed separately.

The executable comparison suite records the oracle's version and build options. Initial local results use the installed Git 2.53.0, not a claimed Git 2.56 qualification. Target source commits are pinned in [compat/baselines.json](compat/baselines.json). The [command inventory](compat/commands.json) is a starting list from upstream, not an exhaustive option audit.

## Try a local commit

Use a new directory. Initialization intentionally refuses to reinitialize existing repositories and does not install Git's sample hook templates.

```sh
target/debug/rugit init --template= -b main /tmp/rugit-demo
# Create files in /tmp/rugit-demo before staging.
target/debug/rugit -C /tmp/rugit-demo add -A
target/debug/rugit -C /tmp/rugit-demo -c user.name=Guido -c user.email=guido@ngjansen.be commit -m "First commit"
target/debug/rugit -C /tmp/rugit-demo rev-parse HEAD
```

`add -A` currently runs from the worktree root. It understands per-directory `.gitignore` rules and stages files, executable bits on Unix, and symbolic links. It refuses attributes, submodules, unsupported index flags/extensions and configured local conversion features. Global/system configuration and attributes are not implemented; this is another reason to use controlled repositories.

## Push without C Git

```sh
# On macOS, rugit reads GitHub's saved Internet Password directly from Keychain.
# Other platforms currently require RUGIT_TOKEN in the execution environment.
# Never put a token in a command argument or a remote URL.
target/debug/rugit push https://github.com/OWNER/REPO.git HEAD:refs/heads/main
```

The Rust executable reads the token directly from macOS Keychain through Security.framework, or uses an explicit `RUGIT_TOKEN` override. It discovers remote refs, checks the existing branch is an ancestor, constructs a complete non-delta pack, sends the ref update and pack over HTTPS, and checks the remote's unpack/ref status. Redirects and credentials in remote URLs are rejected. Push currently supports GitHub HTTPS branch destinations only; fetching, SSH transport, force pushes, named remotes and upstream tracking are pending. Use `RUGIT_CREDENTIAL_ACCOUNT` when the Keychain account differs from the URL owner.

`rugit repo create --private OWNER/NAME` creates an empty private GitHub repository using its API. This is a rugit extension: standard Git does not provision GitHub repositories. It verifies that the authenticated account matches the owner and refuses name collisions.

`scripts/publish_bootstrap.py` launches **rugit** for private repository creation, push and verification. It makes no C Git calls and never retrieves or forwards credentials. `rugit repo verify OWNER/NAME` checks GitHub's branch ref, tree, blob IDs and modes against the local commit/index, for public or private repositories. The wrapper writes a credential-free receipt under ignored `artifacts/` and records the newly created repository ID so an interrupted push can be retried against that exact repository. This repository was initially created privately and then made public by its owner.

## Current limits

- Loose objects only for reading; packed-object and packed-ref reading are pending. The outgoing pack writer is implemented.
- Objects are limited to 64 MiB; outgoing pack data to 128 MiB and 100,000 objects.
- `hash-object` supports raw stdin and file hashing with explicit `--no-filters`; non-blob writes require `--literally` until validation is implemented.
- `cat-file` supports type, size, existence, raw same-type data and non-tree pretty output, with full IDs, loose-object abbreviations and simple branch refs. Batch mode, revision expressions and tree pretty output are pending.
- Index writes support v2, normal stage-0 entries and safe TREE-cache removal. No sparse or conflict index support.
- Commits support a single parent and `-m`/`-F`, fixed raw dates or current UTC, explicit identities and quiet output. Signing, hooks, reflogs, normal message cleanup, amend, empty-commit rules and full output compatibility are pending. Hooks and signing configured in the local repository are refused.
- Configuration parsing is intentionally limited. Repository extensions not implemented here are refused before mutation. Discovery does not yet handle linked worktrees or ownership/cross-filesystem rules.
- File fsync and exclusive lock files are implemented. Directory crash durability, exhaustive fault injection and an OS-level subprocess monitor remain future work.
- Full upstream tests, all release architectures and minimum-OS/filesystem matrices have not been qualified.

## License and copyright

Original project code: **Copyright (C) 2026 Guido <guido@ngjansen.be>**.

Licensed under **GPL-2.0-only**; see [LICENSE](LICENSE). Source headers use SPDX identifiers. [REUSE.toml](REUSE.toml) covers generated data and the lockfile. Dependencies and any upstream material retain their own copyrights and license terms; [THIRD_PARTY.md](THIRD_PARTY.md) records provenance.
