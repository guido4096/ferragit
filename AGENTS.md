<!-- SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be> -->
<!-- SPDX-License-Identifier: GPL-2.0-only -->

# Agent instructions for Ferragit

These instructions apply to this repository and its descendants. Follow explicit user instructions and any more specific, scoped `AGENTS.md` files. Keep this file practical; link to detailed specifications instead of duplicating them.

## Start here

- Read [README.md](README.md) for supported workflows and limitations, the **next work package** in [PLAN.md](PLAN.md) for implementation priorities, and [compat/status.json](compat/status.json) for current feature gaps.
- Consult [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/SECURITY.md](docs/SECURITY.md) before changing runtime boundaries, storage writes, or credentials. Read the relevant implementation and tests before editing.
- Inspect the working tree and preserve unrelated user changes. Keep each task focused. Continue authorized work without unnecessary confirmation; ask when a missing decision blocks correctness or an action would exceed the user's scope.
- Establish a bounded feature scope and observable acceptance criteria before implementation. Do not silently expand a foundation task into a new workflow or generic framework.

## Product and compatibility contract

- Ferragit is an independent Rust Git implementation and an experimental proof of concept. The eventual goal is full drop-in compatibility on Windows, macOS, and Linux; focused test success does not establish that goal.
- Use `ferragit` consistently for the executable, project identity, examples, fixtures, environment variables, and HTTP identifiers. Do not introduce legacy names or compatibility aliases without an explicit requirement.
- Use [compat/baselines.json](compat/baselines.json) for pinned reference profiles. An installed Git version is an oracle for that run, not automatic qualification against the pinned releases.
- Keep unsupported interfaces explicit. Reject unknown mandatory repository extensions before mutation; do not silently ignore features that affect repository interpretation or writes.
- Update the relevant compatibility inventory and documented limitations when behavior changes. Keep planned work distinct from implemented and verified behavior. Do not mark an entire phase complete because one slice passes.

## Code ownership and style

- `git-cli` owns argument parsing, presentation, diagnostics, and exit status. It composes `git-core` and platform services. `git-core` owns formats, algorithms, and repository operations; it depends on `git-platform` for OS primitives. No dependency points back toward the CLI.
- `git-conformance` owns oracle execution, fixture isolation, and comparison reports. Runtime code must not use C Git, libgit2, gitoxide/gix, credential helpers, or a subprocess fallback to implement Git behavior. Runtime child-process execution is currently prohibited by the architecture check.
- Prefer small modules, concrete structs/enums, explicit options, and ordinary functions. Add traits, crates, async code, parallelism, or dependencies only for a demonstrated need. Avoid unrelated refactors and broad dependency upgrades.
- Respect the workspace MSRV and locked dependencies in `Cargo.toml` and `Cargo.lock`. Use `cargo fmt` and resolve Clippy warnings; do not suppress a lint globally to hide a local problem.
- Keep codecs and algorithms separate from filesystem access, configuration discovery, and terminal output. Return structured results and errors from library code.
- Use `Result` for operational failures and invalid external input. Reserve panics and `unwrap`/`expect` in runtime code for established internal invariants; explain non-obvious invariants. Tests may assert fixture assumptions.
- Preserve repository path bytes. Convert explicitly at OS boundaries; do not use lossy filename conversion. Probe filesystem capabilities instead of assuming case sensitivity, symlinks, executable bits, or path syntax from the OS name alone.
- Carry the hash algorithm explicitly and cover both SHA-1 and SHA-256. Validate checksums, object identity, lengths, offsets, and integer arithmetic. Bound allocations, decompression, recursion, and aggregate work on untrusted input.
- Explain non-obvious compatibility rules with an invariant and a reference or reproducer. Comments should explain why a rule exists, not narrate obvious syntax. Unsafe code is currently forbidden by workspace lints; do not relax that boundary incidentally.

## Repository writes and failure safety

- Make lock ownership, expected old values, write order, publication, cleanup, and durability limits explicit. Never steal or remove another writer's lock.
- Preserve existing objects and metadata when an unsupported or malformed repository is encountered. Do not overwrite an existing object without verifying its identity and contents.
- Match Git's actual operation boundaries and failure effects. Do not claim atomicity across refs, reflogs, index, worktree, hooks, and remote effects without evidence.
- Test relevant failed writes and competing writers, including preservation of existing state and cleanup of locks owned by the candidate. A successful return must not hide a failed flush, publication, or remote ref update.
- Use disposable fixtures for corruption, repacking, crash, and interoperability tests. Never use this checkout, personal repositories, or the system Git installation as destructive test fixtures.

## Testing and verification

For Rust behavior changes, run the relevant focused tests, then the workspace checks below. For documentation-only changes, run applicable header, link/path, and diff checks; do not invent unit tests for prose. Changes to repository tooling require its applicable checks. Do not repeat successful checks without a new change or unresolved concern.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
python3 scripts/check_headers.py
python3 scripts/check_architecture.py
target/debug/git-conformance --candidate target/debug/ferragit --report artifacts/conformance.json
```

On Windows, use `.exe` for the candidate and conformance executables. Python tooling requires Python 3.11+. A populated local Cargo cache can be selected through `CARGO_HOME`; use `--offline` only when the required dependencies are available. Do not hard-code a developer's absolute paths into maintained files.

- Unit tests live beside implementation under `#[cfg(test)]`. CLI integration tests are in `crates/git-cli/tests/`; oracle scenarios and the comparator are in `crates/git-conformance/src/lib.rs`.
- A behavior change needs an observable regression or oracle case where appropriate. Control identities, dates, locale, configuration, and environment. Compare stdout/stderr bytes, exit status, and relevant final repository state, including failure effects.
- Use independent equivalent fixtures for reference and candidate mutations. C Git may generate fixtures and serve as an oracle; it must not become a hidden candidate implementation. Keep the candidate's restricted PATH/exec-path checks intact.
- Do not normalize away line endings, whitespace, ordering, diagnostics, or other observable differences. Use only narrow, justified normalization for fields such as temporary roots. Do not weaken assertions or classify missing functionality as a passing skip.
- Include relevant malformed input, bounds, and concurrency cases. Storage changes cover both hash algorithms; platform-sensitive changes need native Windows, macOS, and Linux evidence. Cross-compilation alone is not execution evidence.
- Keep reports under ignored `artifacts/`. Record the actual oracle, platform, command, and result. Never fabricate counts, overwrite dated evidence to imply a new run, or claim CI passed before inspecting the checks for the published commit.
- Preserve formatting, Clippy, architecture, license, MSRV, and credential-scan CI gates. Missing pinned-release, minimum-OS, architecture, fuzzing, or crash evidence remains an explicit gap.

## Documentation: describe the current state

- Documentation must describe the current behavior, architecture, supported workflows, invariants, and limitations. It must never narrate the development process that produced them.
- Do not include earlier project names, migration stories, abandoned approaches, intermediate failures, implementation diaries, or statements such as "we used to", "then we changed", or "this was added". Rewrite affected sections into a coherent description of the current system instead of appending progress notes.
- Explain how the current code works and how to build, test, or use it; those operational instructions are useful. Explain the rationale for a current design without recounting its development timeline.
- Plans describe intended work and acceptance criteria, explicitly labeled as unimplemented. Current documentation must not present those plans as supported behavior.
- Keep factual provenance and dated validation data in their appropriate records. Preserve their accuracy; do not turn them into a development narrative or falsify past test evidence. Summarize current support in documentation and link to evidence.
- Update relevant existing docs and status records as part of a behavior change. Keep examples executable and links valid. Do not add release notes unless requested; any requested notes describe user-visible behavior, not the development process.

## Credentials, licensing, and dependencies

- Never print, persist, or commit credentials, authorization headers, credential-bearing URLs, or secret-containing reports. Use native macOS Keychain access or the explicit `FERRAGIT_TOKEN` environment override. `FERRAGIT_CREDENTIAL_ACCOUNT` selects a stored account; never put token values in command arguments.
- Keep `.cache/`, `target/`, `artifacts/`, local environment files, and Python caches out of commits. Inspect the actual staged paths and diff before publication; credential scans supplement that review.
- Preserve original-code copyright `2026 Guido <guido@ngjansen.be>` and `GPL-2.0-only` SPDX headers. Include new maintained files in `scripts/check_headers.py` or the existing REUSE coverage as appropriate. Preserve third-party notices and record imported material in `THIRD_PARTY.md`.
- Use maintained compression, cryptographic, TLS/HTTP, and OS libraries. Review new dependencies for purpose, portability, MSRV, licensing, and their effect on runtime independence. Pin downloaded CI tools/actions and verify artifact checksums as required by the workflow.

## Commit, publish, and hand off

- Use `target/debug/ferragit` for this project's staging, commits, pushes, and GitHub repository operations. C Git is permitted for the test oracle and read-only local inspection, not as a substitute for Ferragit's mutation or credential paths.
- When an authorized operation is unsupported, identify the precise limitation and implement a bounded native path if it belongs to the task. Do not silently substitute `git`, `gh`, a credential helper, or a token-handling script.
- The repository is `guido4096/ferragit`. Publish only when authorized by the current task or conversation. Do not create/rename repositories, force-push, rewrite history, or change visibility as an incidental publishing step.
- `python3 scripts/publish_bootstrap.py --owner guido4096 --name ferragit --publish` invokes Ferragit and verifies the recorded repository identity and published snapshot. It requires a built executable and configured native/environment credentials.
- Use `target/debug/ferragit repo verify guido4096/ferragit` to verify publication and `target/debug/ferragit repo checks guido4096/ferragit` to inspect CI for local `HEAD`. Respect its current command limitations; see README.md.
- Finish with the concrete result, checks actually run, and any remaining limitations or blockers. For incomplete work, leave a concise handoff with the next action; do not put a chat transcript or progress diary in maintained documentation.
