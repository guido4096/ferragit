<!-- SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be> -->
<!-- SPDX-License-Identifier: GPL-2.0-only -->

# Baselines and measurement qualification

Phase 0 is **open**. Its measurement tooling is executable; a complete semantic
surface inventory and full distribution/build-profile audit remain required.
Passing the harness does not qualify Ferragit as a Git replacement.

## Native qualification

After `cargo build --workspace --locked`, run:

```sh
python3 -m unittest discover -s scripts/phase0 -p 'test_*.py'
python3 scripts/phase0/run.py --output artifacts/phase0
```

Use a new output directory for each run. Evidence is never silently overwritten.
Linux requires `strace`, a C toolchain, make, and curl/OpenSSL/zlib development
libraries. macOS requires Command Line Tools and permission to use `sandbox-exec`;
nested sandboxes may prevent its policy from loading. Windows qualification uses
the x86_64 PortableGit distribution. A failed provisioner retains `reference/build.log`.
Python 3.11+ is required. The upstream recipes also require tarfile's backported
`data` extraction filter (Python 3.11.8+, or Python 3.12+).

`provision.py` pins upstream Git 2.56.0's kernel.org archive by SHA-256. Linux and
macOS build an isolated core reference with gettext, Tcl/Tk, Perl and Python
disabled. These are **harness profiles**, not the full distribution profiles
required for a compatibility release. Windows extracts checksum-pinned
PortableGit 2.56.0.windows.4 without system installation or PATH changes. GitHub
downloads use Ferragit's native `repo download` extension. Downloads reject
unexpected storage hosts and never forward repository credentials to storage.

Each native run records the platform, architecture, reference executable hash,
`version --build-options`, build argv/tool versions or portable artifact identity,
and a manifest of installed file hashes and symlink targets. Archive checksums
and GitHub's signed-tag verification records are distinct from independent local
OpenPGP verification; the latter remains explicitly recorded in
[baselines.json](../compat/baselines.json).

CI executes the same entry point on Linux, Windows, Intel macOS and ARM64 macOS.
It uploads manifests, build logs, measurement fixtures, process-policy evidence,
and the candidate conformance report. Inspect results for the exact commit.
Minimum OS versions, Linux ARM64, Windows ARM64, musl and additional filesystem
profiles remain outside this hosted harness matrix.

## What the checks establish

`qualify.py` runs independent reference-versus-reference fixtures twice. It
compares binary stdout/stderr, exit status or POSIX signal, and snapshots of
files, empty directories, symlink targets and executable bits. Snapshots use
byte-preserving path encodings and content hashes. Timestamps and inode numbers
are intentionally excluded; commands whose output exposes those fields need
dedicated cases. Unsupported filesystem entry types fail snapshotting.

Deliberate probes exercise exit-status differences, stderr routing, CRLF versus
LF, state-only mismatches and binary input. An interrupted test writer leaves a
flushed partial lock file; the harness checks both its final state and its
termination status. This qualifies measurement of interruption, not Ferragit's
crash safety. The current focused Ferragit suite runs separately against the
pinned reference, with every candidate invocation using the process guard.

Every measurement fixture includes `before/`, `work/`, `stdin.bin`, raw output
files, `result.json` and `replay.json`. Re-run a retained case with:

```sh
python3 scripts/phase0/qualify.py --replay artifacts/phase0/measurement/mismatch-line-endings/right
```

Replay creates `replay-work/` and a controlled home directory beside the fixture;
it refuses to overwrite an existing replay. Keep the recorded executables
available. The qualification checks also copy and replay a fixture at a new
location. Reports retain only a controlled environment allowlist, never the
caller's credentials or full environment. Fixtures are disposable and ignored.

## Process boundary

The guard is test tooling, separate from the Rust runtime and the source/dependency
architecture check. Candidate PATH and Git exec-path point to an empty directory.

| Host | Guard | Evidence and limits |
| --- | --- | --- |
| Linux | `strace -f` process events | Detects extra execs, failed helper execs, fork/vfork and non-thread clones; rejects the run with status 125. Threads are allowed. Detection occurs after attempted execution; this is not a security sandbox. |
| macOS | Kernel `sandbox-exec` policy | Denies process creation and execution of programs other than the initial candidate. Records the policy and outcome; denied attempts are not a complete event trace. |
| Windows | One-process Job Object | Creates the candidate suspended, assigns it before resuming, forbids breakaway and kills job members on close. Records enforcement/outcome; denied attempts are not a complete event trace. |

Unprotected fallback controls must succeed; guarded fallback and missing-helper
probes must fail. POSIX direct-exec probes target the reference executable and
must fail. Windows has no equivalent process-replacing exec contract. Binary
I/O, stderr routing, exit codes and guarded interruption must survive the wrapper.
Missing guard prerequisites fail qualification; they do not produce a passing skip.

The Rust runner accepts `FERRAGIT_PROCESS_GUARD` as a JSON argv prefix whose first
element is an absolute executable path. The report records this prefix. Ordinary
manual conformance runs without this variable are unguarded and say so.

## Interface inventory gate

`compat/commands.json` tracks implementation status for the upstream command list.
`inventory.py` produces a separate, source-linked discovery catalog:

```sh
python3 scripts/phase0/inventory.py --source PATH_TO_PINNED_SOURCE \
  --profile upstream-v2.56.0 --output artifacts/inventory/upstream.json
```

Run it on pristine extracted source, before building. Run a separate catalog for
the pinned Git for Windows source. Its records contain declaration candidates,
source locations, file hashes and parser sources needing audit. Regular-expression
extraction includes false positives and misses computed interfaces; **it is not
an exhaustive semantic inventory**. It never promotes implementation statuses.

Closing this gate requires source/parser review of shared and handwritten option
parsers, dynamic configuration/environment families, platform conditionals,
installed helpers, optional tools, aliases and distribution components. Each
interface needs profile applicability, implementation/test ownership and an
explicit status. Build manifests supply an installed-file denominator; every
shipped helper must be classified. Unknowns remain open rather than being counted
as verified or platform-inapplicable. See [phase0.json](../compat/phase0.json).
