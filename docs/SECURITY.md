<!-- SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be> -->
<!-- SPDX-License-Identifier: GPL-2.0-only -->

# Proof-of-concept status and credential handling

This project is a **proof of concept**, not a production-ready Git replacement. Its focused interoperability tests do not establish full Git compatibility or constitute a security review of the implementation. Use controlled or disposable repositories while its documented gaps remain open.

## Credential audit

On 2026-11-03, the publication review covered the bootstrap commit `50a7c96b241d6efde242c44611f8434be8c2f176`, all local Git history, and the updated source files prepared for publication.

- Gitleaks 8.30.1 scanned the committed history with `--log-opts=--all` and reported no leaks.
- The saved GitHub credential and common encodings were compared against all 54 then-existing local loose Git objects, without displaying or persisting the credential. There were zero matches.
- The prepared source snapshot was scanned separately before committing; generated files, build output, dependency caches and local receipts were excluded from publication.
- Authentication paths were reviewed for token literals, credential-bearing URLs, debug output and credential persistence. The committed code contains runtime variable names and authentication logic, not stored credential values.

These are point-in-time checks, not a guarantee that a scanner can detect every possible secret. The contact email in copyright headers and commit authorship is intentionally public.

To repeat the history check with an installed scanner:

```sh
gitleaks git . --redact=100 --log-opts=--all --report-format=json --report-path /tmp/rugit-secrets.json
```

## Automated credential scan

Every push and pull request runs the separate **Credential scan** GitHub Actions job. It checks out full history and scans all available refs with Gitleaks 8.30.1. The scanner archive is pinned by version and SHA-256 checksum. Findings fail the job, output is redacted, and inline `gitleaks:allow` comments cannot suppress findings. Scanner reports containing source snippets are not uploaded as artifacts.

The scan runs after GitHub receives a push; it does not prevent the initial publication of a secret. A required status check can enforce a passing scan before merging a pull request.

## Runtime handling

On macOS, rugit reads an existing GitHub Internet Password directly through Security.framework. It does not invoke C Git, a Git credential helper, or another executable for authentication. `RUGIT_CREDENTIAL_ACCOUNT` selects the Keychain account when its stored identifier differs from the GitHub login.

On other platforms, and as an explicit override on macOS, supply `RUGIT_TOKEN` through the execution environment. No token should be placed in source, command arguments, a remote URL, commit content or a report. HTTPS requests use Authorization headers; redirects and credentials embedded in push URLs are rejected.

The publication wrapper runs rugit for repository creation, push and remote verification. It does not load credentials. Local receipts are metadata only and live under ignored `artifacts/`. `.gitignore` also excludes build caches, Python caches and local `.env` files.
