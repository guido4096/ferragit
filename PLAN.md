<!-- SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be> -->
<!-- SPDX-License-Identifier: GPL-2.0-only -->

# Rust Git replacement: implementation plan

Planning date: 2026-11-02. Status: the first implementation slice is underway; see README.md and compat/status.json for executed scope and remaining gaps. The full phase gates below are not yet satisfied.

## Objective

Build a Git implementation in Rust that can replace the reference Git installation on Windows, macOS, and Linux: every supplied command and option, scripting behavior, repository format, network protocol, and applicable platform behavior. Keep the code understandable through explicit dependencies, small modules, shared algorithms, and tests that explain compatibility requirements.

This is a multi-year compatibility program. Rust makes memory safety and code organization easier; it does not remove Git's behavioral complexity. Every milestone below is an intermediate result. None reduces the final scope to common commands.

The testable promise is compatibility with a named reference release and installation profile. Absolute equivalence with every past and future Git release, every third-party tool, and every possible environment is not a finite deliverable. Matching a named release still requires tracking newly discovered behavior after release.

## 1. Establish the compatibility contract first

### Reference implementations

Use these initial planning baselines:

| Platform | Reference |
| --- | --- |
| Linux | Upstream Git v2.56.0, with recorded build features and dependencies |
| macOS | Upstream Git v2.56.0 built for macOS, with recorded build features and dependencies |
| Windows | Git for Windows v2.56.0.windows.4 and its distribution defaults |

Git's download page currently identifies Git 2.56.0 and Git for Windows 2.56.0(4). Windows has additional patches and distribution components, so its reference is separate. [Release baseline](https://git-scm.com/install/windows), [Windows architecture](https://github.com/git-for-windows/git/blob/main/ARCHITECTURE.md).

During initial setup, resolve tags to full commit IDs, verify available release signatures, record source and binary hashes, and freeze build flags, dependency versions, configuration, and installer choices. These identifiers have not yet been resolved locally. Never use a mutable branch as a release oracle.

Apple's vendor-patched Git and individual Linux distribution patches are additional reference profiles if required; they are not silently treated as identical to upstream. Track baseline security updates immediately and evaluate new upstream releases continuously. Compatibility releases declare the precise baselines tested.

### What must match

| Surface | Required evidence |
| --- | --- |
| CLI parsing | Global flags, subcommands, hidden/deprecated options, abbreviations, negation, repeated flags, option ordering, `--`, aliases, unknown-option handling |
| Process behavior | Exit status, stdout/stderr separation, stdin handling, binary streams, signals, cancellation, broken pipes, TTY and non-TTY behavior |
| Presentation | Output bytes in controlled environments, quoting, ordering, colors, help, warnings, errors, prompts, pager/editor behavior, supported translations |
| Configuration | Files, scopes, includes, conditional includes, environment overrides, defaults, precedence, encodings, platform-specific paths |
| Repository state | Objects, refs, reflogs, index, worktree, operation state, permissions, lock files, failure effects, coexistence with C Git |
| Network | Client and server roles, capabilities, packet framing, negotiation, errors, HTTP/SSH/local transports, remote helper interfaces |
| Integrations | Hooks, credential helpers, signing tools, filters, mergetools, external commands, IDE and CI consumers |
| Installation | Executable names, dashed helpers, exec path, templates, documentation, completion, discoverability, runtime dependencies |

Git's option semantics include command-specific behavior; a generic argument parser's defaults are not sufficient. [CLI conventions](https://git-scm.com/docs/gitcli).

For deterministic fixtures, compare bytes. For nondeterministic data, control time, identity, random seeds where available, locale, terminal, dependencies, and environment before comparing. Allow only field-specific, reviewed normalization for values such as temporary root paths and elapsed time. Never globally normalize whitespace, ordering, case, diagnostics, or line endings.

Do not require byte-identical compressed objects or pack layouts where the reference permits multiple valid encodings. Check interchangeability, visible results, and command-specific promises instead. Output that exposes ordering or compression choices still needs its own compatibility analysis.

Retain externally observable quirks required for scripts. Do not intentionally reproduce a vulnerability: track a secured-reference update and disclose any necessary compatibility change. Implementation-private C data structures and undefined behavior are not interface contracts.

### Command inventory and accounting

Create machine-readable manifests from the pinned source, documentation, command list, installed files, helper binaries, and actual option probes. Help output alone misses options. Include client commands, plumbing, server commands, email tools, migration tools, maintenance, interactive modes, and optional shipped programs.

Each entry records: reference profile; command/option/config/environment key; implementation module; dependencies; tests; platforms; status; differences; and the milestone responsible for completion. Use states `unimplemented`, `partial`, `verified`, and `platform-inapplicable`. An expected failure is never a pass.

Enumerate optional components explicitly: `git svn`, CVS tools, `send-email`, `imap-send`, `gitweb`, `instaweb`, `git gui`, `gitk`, shell completion, and Windows distribution helpers. Include commands present in the selected full reference profile even if rarely used. Missing platform prerequisites are recorded and provisioned in dedicated jobs, rather than counted as feature completion.

Third-party tools such as SSH, GPG, editors, Git LFS, and credential managers may remain external, with compatible invocation and protocols. This project implements Git-owned command logic in Rust and never delegates it to C Git in a qualifying runtime. Retaining upstream Git-owned shell/Perl implementations is an intermediate state, not completion of the Rust rewrite. Reimplementing the third-party tools themselves is outside Git's interface scope.

## 2. Platform support

Proposed first release matrix; these are project choices, not claims about upstream minimum requirements:

| Platform | Architectures | Initial operating environment |
| --- | --- | --- |
| Windows | x86_64, ARM64 | Windows 10 x64 and Windows 11; Windows 11 ARM64; native executable |
| macOS | x86_64, ARM64 | macOS 13 or newer |
| Linux | x86_64, ARM64 | Explicit glibc and musl build profiles; choose and record ABI floors during setup |

Use native execution on every advertised architecture before release. Cross-compilation alone does not establish support. Test the oldest advertised OS or ABI as well as a recent one. Containers supplement Linux testing but do not replace macOS or Windows runners. Windows Server versions need named jobs before being advertised; 32-bit targets and additional architectures are later support additions.

Mandatory filesystem coverage:

- Windows NTFS with long-path policy both enabled and disabled; drive-relative paths, UNC paths, junctions/reparse points, reserved names, sharing violations, symlink permissions, executable discovery, and deletion/rename of open files.
- macOS APFS in case-sensitive and case-insensitive configurations; filename normalization, executable modes, symlinks, and filesystem monitoring.
- Linux ext4 plus representative alternative local filesystems; byte-oriented filenames, permissions, symlinks, file descriptor limits, signals, and filesystem monitoring.
- Shared-filesystem jobs using SMB and NFS, including timestamp granularity, cache invalidation, locking, interruption, and atomic replacement assumptions. Add shared Windows/WSL repository scenarios; a Linux build in WSL alone does not validate the Windows binary.

Shell coverage: PowerShell, cmd.exe, and Git Bash on Windows; supported POSIX shells and zsh on macOS/Linux. Test child-process argument quoting and shell aliases independently. Use filesystem capability probes rather than assuming all volumes on an OS behave alike. Git already exposes configuration for several such differences. [Platform-related configuration](https://git-scm.com/docs/git-config).

## 3. Architecture

Start with a Cargo workspace containing four crates. Most concepts are modules, not independent crates. Split further only for a demonstrated build, dependency, ownership, or reuse benefit.

```text
crates/
  git-core/                 # Git models, formats, algorithms, repository operations
    src/
      object/              # Blob/tree/commit/tag codecs; hash-aware object IDs
      store/               # Loose objects, packs, indexes, alternates, caches
      refs/                # Files/packed refs, reftable, transactions, reflogs
      index/               # Index formats/extensions and staged entries
      config/              # Parsing, scopes, includes, precedence
      path/                # Repository paths, pathspecs, quoting
      worktree/            # Scanning, checkout, attributes, ignores, filters
      revision/            # Revision grammar, traversal, graph queries
      diff/                # Algorithms, rename detection, patch parsing/application
      merge/               # Blob/tree/commit merging and conflict representation
      sequence/            # Rebase/cherry-pick/revert/am state and recovery
      protocol/            # Packet lines, capabilities, negotiation, server/client
      transport/           # HTTP/local/SSH orchestration, remote helpers
      maintenance/         # Repack, GC, commit graphs, bitmaps, fsck
      repository/          # Discovery, contexts, shared operation entrypoints
  git-platform/            # Filesystem/process/terminal/credential OS primitives
    src/{unix,windows}/
  git-cli/                 # Git-compatible argument parsing, dispatch, rendering
    src/{args,commands,format,diagnostic}/
  git-conformance/         # Oracle runner, state comparator, reports, test tooling
compat/                    # Baseline manifests, feature inventory, difference ledger
tests/                     # Portable scenarios, regression fixtures, protocol peers
docs/                      # Contributor guide, formats, decisions, module map
```

Dependency direction: `git-cli -> git-core -> git-platform`; `git-conformance` may invoke both reference and candidate as separate processes. Low-level core modules do not depend on the CLI or higher-level operations. Object codecs do not discover repositories, read configuration, or spawn processes. The repository layer composes components; it is not a container holding all implementation logic.

### Rules that keep the implementation understandable

1. Prefer concrete structs, enums, and ordinary functions. Use traits for real alternatives: reference backends, transport peers, and fault-injected I/O. Avoid a universal backend/plugin framework.
2. Preserve repository path bytes independently of OS paths. Use explicit conversions at filesystem boundaries; never lossy-convert filenames in operational code.
3. Carry the hash algorithm in object ID and repository types. Cover both SHA-1 and SHA-256, including baseline-supported transition features; never encode a 20-byte assumption throughout the code.
4. Keep parsing and encoding mostly pure. Preserve data needed for round trips, including supported unknown fields. Reject unknown mandatory repository extensions before mutation. [Repository version rules](https://git-scm.com/docs/repository-version).
5. Separate algorithm results from terminal output. Library code returns structured results and errors; the CLI controls exact compatible text and exit status. Compatibility text belongs next to the command or shared formatter that owns it.
6. Parse arguments into typed command options, resolve configuration explicitly, and call shared operations. Avoid libraries whose parser defaults silently change Git behavior; implement Git's required semantics and test command-specific exceptions.
7. Make locking, compare-and-swap expectations, replacement, sync policy, and cleanup visible. Use scoped guards, but explicitly handle final commit/flush failures. A dropped guard cannot report a durability error.
8. Preserve Git's actual transaction boundaries. Do not invent a universal transaction promising atomicity across refs, index, worktree, hooks, and remote effects. Model partial progress and recovery for each operation.
9. Keep the core synchronous initially. Introduce bounded parallel workers for proven CPU/I/O bottlenecks. Use async only where measurements and transport needs justify its complexity.
10. Isolate and document unsafe code and FFI in narrow modules. OS calls may require them; memory safety does not make path handling or protocol logic automatically secure.
11. Each non-obvious compatibility rule needs a nearby explanation, a reference or reproducer, and a regression case. Document invariants and failure states, not a narration of obvious Rust syntax.
12. Keep runtime Git implementations independent. Use maintained compression, cryptographic, HTTP/TLS, Unicode, and OS libraries rather than recreating those domains. Evaluate Rust Git libraries as references or optional components; using one as the primary engine changes the from-scratch goal and is an explicit architecture decision.

## 4. Conformance infrastructure is the first implementation milestone

### Differential runner

For each scenario, create two independent but equivalent fixtures, run the pinned reference and the Rust candidate with controlled environments, and compare outcomes. Do not run both mutation paths in the same fixture.

Capture argv, working directory, relevant environment, stdin, stdout/stderr bytes, exit status or signal, TTY state, filesystem metadata, and final repository state. Environment capture must redact secrets and use test credentials. A failed comparison produces a minimal reproducer.

State comparison includes object identities/content, refs and reflogs, index entries and flags, worktree bytes/modes/symlinks, config changes, lock/temp cleanup, and in-progress operation files. Account for permitted storage/layout differences explicitly. Add round trips where Git creates a repository, Rust mutates it, and Git continues, and vice versa. Verify Git's interpretation, not only the candidate's own parser.

### Upstream tests without accidental fallback

Run the pinned upstream shell suite against the candidate using its installed-binary facilities. It still requires upstream test-support programs and build resources. Its documentation warns that an incomplete installation can silently use parts of the built Git. [Test harness documentation](https://raw.githubusercontent.com/git/git/v2.56.0/t/README).

Therefore use a hermetic exec path and manifest every permitted executable. Record child-process execution and distinguish test-only fixture generators from runtime command dispatch. Make any candidate invocation of reference Git, libgit2 wrappers, upstream Git command scripts, or missing helper fallbacks fail the qualifying run. A reference executable being available for the oracle must not make it available as a hidden candidate implementation. Validate the guard deliberately by removing a helper and proving the run fails.

Keep compatibility adaptations in an external runner or a small, reviewed patch set; do not rewrite upstream assertions to fit the candidate. Maintain a separate Windows patch/test set aligned with the Windows reference.

Tests that inspect private C implementation details or intentionally known failures need explicit classification and equivalent behavioral checks. A skip requires a recorded prerequisite or a demonstrated upstream platform limitation. Unimplemented candidate functionality cannot justify a platform skip. Publish pass/fail/skip/known-failure counts and the complete denominator by profile.

### Additional evidence

- Generated command sequences and property tests for object round trips, refs, revision traversal, pack decoding, and patch application.
- Fuzzing for all untrusted formats: objects, packs/deltas, index, reftable, config, revision/pathspec grammars, patches, URLs, and packet lines.
- Crash/fault injection at mutation boundaries: short writes, disk full, permission errors, cancellation, broken connections, failed sync/rename, stale locks, competing writers, and killed hooks.
- Real consumer workflows: build/package tools, CI checkout jobs, IDE process integrations, submodules, LFS filter integration, signing, and credential helpers. Do not substitute success in an IDE using its own Git library for testing the replacement executable.
- Interoperability matrix: Rust client/C Git server, C Git client/Rust server, Rust/Rust, and fixed older peer versions supported by the baseline.
- Dedicated PTY tests for interactive staging, rebase, credentials, pager/editor control, and translated diagnostics. CI's default non-TTY environment is insufficient.

## 5. Implementation phases and exit gates

Phases are dependency gates, not calendar promises. Continuous platform work starts in phase 0. Platform support is never postponed until the core is finished.

| Phase | Deliverables | Exit evidence |
| --- | --- | --- |
| 0 — Baselines and runner | Reference build manifests; complete surface inventory; platform CI; process guard; candidate launcher; first differential cases | Reference-vs-reference runs are reproducible on all three OS families; guard catches deliberate fallback; failures yield usable reproducers |
| 1 — Repository foundation | Discovery/config; object codecs; SHA-1/SHA-256; loose and packed object reading; refs; locking; low-level read/write plumbing | Bidirectional repository interchange; malformed-input coverage; no corruption in tested write failures; supported format inventory is explicit |
| 2 — Local workflow | Index versions/extensions; status; ignores/attributes/filters; add/rm/mv; commit; checkout/switch/restore/reset; branch/tag; log/show/diff | Controlled local command scenarios match outputs and state; checkout protections hold; interrupted operations have compatible recovery |
| 3 — Remote operation | Pack generation; clone/fetch/pull/push; protocol v0/v1/v2 as applicable; HTTP/SSH/local; upload-pack/receive-pack; remote helpers; credentials | Cross-implementation client/server matrix passes, including shallow/partial clone, refspecs, atomic push and hook rejection |
| 4 — History editing and interaction | Merge strategies; rename handling; rebase; sequencer; cherry-pick/revert; stash; bisect; blame; apply/am; format-patch/range-diff; signing | Conflict choices, markers, resulting trees, progress state, abort/continue/skip, hooks and PTY interactions match controlled fixtures |
| 5 — Advanced repositories and scale | Worktrees; submodules; sparse checkout/index; reftable; alternates/promisors; notes/replace; bundles/archives; fsck; GC/repack/prune; graphs/bitmaps/MIDX; maintenance/fsmonitor | Feature and combination suites pass; large-repository jobs meet budgets; concurrent access and maintenance preserve data |
| 6 — Remaining suite and distribution | Every outstanding command/option; email/migration tools; web/GUI entrypoints from reference profiles; translations; help/completion/templates; installers and dependency bundles | Inventory has no unimplemented or partial required entries; optional-feature jobs provision dependencies; installed layouts and helper interfaces pass |
| 7 — Qualification and release | Full baseline suites; generated-case corpus; consumer trials; security review; signed artifacts; reproducible builds where practical; upgrade/rollback docs | All release gates below pass and known differences are published; no unapproved feature exclusions are used to claim full compatibility |

Some foundations are designed before their full implementation: SHA-256 from phase 1; reference backend seams in phase 1; sparse-index preservation in phase 2; protocol feature seams in phase 3. Reftable and other advanced features can be completed later, but an earlier build must refuse unsupported mandatory formats safely and is never advertised as fully compatible.

Client v2 support does not imply that push has a corresponding v2 implementation. Match the reference's capability and transport behavior command by command. [Protocol v2 specification](https://git-scm.com/docs/gitprotocol-v2).

For merge/diff/revision behavior, implementing an algorithm with the same name is insufficient. Git-specific tie breaking, rename limits, path ordering, whitespace handling, strategy options, and conflict formatting require oracle cases.

## 6. Performance and integrity

Record C Git baselines on identical machines, inputs, dependencies, and cold/warm cache conditions. Cover startup in small repositories; status in large worktrees; log/blame/diff; clone/fetch; pack verification/generation; checkout; merge/rebase; and maintenance. Measure latency, peak resident memory, bytes read/written, process count, and output correctness.

Initial engineering targets: no unexplained regression beyond 25% median runtime or 50% peak memory on agreed representative workloads; no material degradation in tail latency or giant-repository viability. These are proposed budgets, not observed results. Refine them after phase 0 to account for measurement noise and workloads. Performance deviations do not get hidden in an overall compatibility percentage.

Use bounded memory and streaming for large objects and packs, checked sizes/offsets, bounded delta resolution, cancellation, and controlled worker counts. Cryptographic object integrity, safe checkout paths, ownership checks, protocol restrictions, quarantined incoming objects, and correct hook timing are required behaviors. Do not rely solely on Rust's memory safety.

No release containing an unresolved known data-loss bug qualifies. Preserve both successful-operation semantics and failure effects; silently restoring a different state can be incompatible too.

## 7. Release gates

The full-replacement claim requires all of the following for every advertised profile:

1. Every inventoried required command, option, helper, config key, and environment interface is verified; there are no partial or unimplemented entries.
2. All applicable baseline behavioral tests and added regressions pass. Every skip or implementation-specific test has an auditable classification and, where necessary, an equivalent check.
3. Differential, repository round-trip, and cross-peer suites pass with no unexplained differences and no hidden reference implementation calls.
4. Native OS/architecture and minimum-version jobs pass; filesystem, shell, TTY, and packaging jobs cover the declared support contract.
5. Crash/concurrency campaigns, format fuzzing, and security review have no unresolved release-blocking findings; duration and corpus coverage are published rather than treated as a proof of correctness.
6. Performance budgets are met or deviations are explicitly accepted and documented before release.
7. Installation, coexistence with reference Git, upgrade, and rollback preserve repository usability and support tooling discovery.
8. Signed package checksums, dependency/source notices, compatibility report, known differences, and contributor documentation ship with the release.

Passing a finite test suite establishes substantial evidence, not mathematical equivalence for every input. Unknown behavior reports remain release-maintenance work. A known semantic exception means the precise contract must say so; it cannot be concealed under a blanket full-compatibility claim.

During development use a distinct executable/install prefix. Install a `git`-named candidate only inside isolated test environments and explicit evaluation packages until qualification. Do not replace the developer's system Git or mutate existing personal repositories as a testing shortcut.

## 8. Maintainability and upstream tracking

Keep a short module guide describing ownership, dependency direction, invariants, error handling, and one complete command path. Use consistent formatting and linting; review complexity, dependency growth, panic paths, and unsafe boundaries. Avoid arbitrary line-count rules and generic frameworks that only serve one caller.

Changes to behavior include a minimal oracle fixture and identify the manifest entries they affect. Changes to writes explain locking, interruption, durability, and recovery. Routine refactors do not require a new architecture document; significant format, dependency, parser, or transaction decisions get short decision records.

Treat each new upstream release as an intake: enumerate changed commands/options/formats/defaults/tests, run the new suite against the candidate, classify differences, implement changes, and advance the declared baseline after qualification. Track Git for Windows separately. Maintain security-fix intake between feature releases.

The original project code uses GPL-2.0-only, with copyright attributed to Guido <guido@ngjansen.be>, following the user's request. Preserve upstream and dependency provenance and notices, and keep imported test assets distinguishable from independently written implementation. Git supplies its own licensing terms in [COPYING](https://github.com/git/git/blob/v2.56.0/COPYING).

Gitoxide remains useful prior art, but its goals explicitly do not require perfect command replication. Its presence does not close this project's compatibility work. [Gitoxide goals](https://github.com/GitoxideLabs/gitoxide#non-goals).

## 9. Staffing, uncertainty, and first executable work

A reasonable planning envelope is a dedicated team of roughly 6–10 experienced engineers over several years, with explicit ownership for Windows/platform behavior, formats/storage, history algorithms, protocols, CLI compatibility, and conformance/release work. A solo effort can produce valuable milestones but should expect a decade-scale or longer path to this complete scope. These are engineering judgments, not estimates derived from a completed inventory or measured implementation velocity.

Do not commit to a final ship date yet. Re-estimate after 6–8 weeks of phase 0 and a representative foundation slice. Report verified manifest entries, hard unresolved gaps, test results by profile, and measured defect/implementation rates. Avoid a percentage based only on command names; one partially working `rebase` is not equivalent to one completed simple plumbing command.

First work package:

1. Resolve and record upstream/Windows reference commits, artifacts, build profiles, dependencies, and minimum platform choices.
2. Generate the complete command/helper/option inventory and an initial difference ledger, marking unknown coverage honestly.
3. Create the four-crate workspace, candidate dispatch skeleton, and local contributor commands. Pick a supported Rust toolchain/MSRV and lock dependencies after checking the target matrix.
4. Implement the conformance runner's fixture isolation, byte capture, exit capture, state snapshots, and process fallback guard.
5. Validate the runner against two controlled reference builds and deliberate mismatches: wrong exit code, stderr routing, CRLF conversion, missing helper fallback, and interrupted writes.
6. Implement the first native vertical slice: version/help dispatch, repository discovery, `init`, `hash-object`, and `cat-file`, including invalid arguments and binary data. Allow only the version identity field to differ under an explicit identity rule; do not claim upstream provenance.
7. Run that slice on Windows, macOS, and Linux. Record unsupported interfaces as unimplemented instead of delegating them.
8. Publish the first compatibility report and use measured results to refine phase 1 estimates.

The first useful outcome is a small independently functioning implementation plus a trustworthy way to demonstrate exactly where it differs from Git. That measurement system stays central until the last compatibility gap closes.
