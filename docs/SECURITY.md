<!-- SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be> -->
<!-- SPDX-License-Identifier: GPL-2.0-only -->

# Proof-of-concept status and credential handling

This project is a **proof of concept**, not a production-ready Git replacement. Its focused interoperability tests do not establish full Git compatibility or constitute a security review of the implementation. Use controlled or disposable repositories while its documented gaps remain open.

## Credential checks

Credential checks cover committed history, the publishable source snapshot, and authentication paths. Keep generated files, build output, dependency caches and local receipts out of the published snapshot. Review staged paths and content for token literals, credential-bearing URLs, authorization debug output and accidental credential persistence.

Use Gitleaks to scan all available committed refs before publication:

```sh
gitleaks git . --log-opts=--all --redact=100 --exit-code=1 --ignore-gitleaks-allow --no-banner
```

Scan a reviewed export of publishable files separately when checking uncommitted source; a history scan does not examine pending working-tree changes. Keep scanner output redacted and any local reports under ignored `artifacts/`.

Passing scans are point-in-time evidence, not a guarantee that every possible secret can be detected. Record the actual scan scope and result in validation records. The contact email in copyright headers and commit authorship is intentionally public.

## Automated credential scan

Every push and pull request runs the separate **Credential scan** GitHub Actions job. It checks out full history and scans all available refs with Gitleaks 8.30.1. The scanner archive is pinned by version and SHA-256 checksum. Findings fail the job, output is redacted, and inline `gitleaks:allow` comments cannot suppress findings. Scanner reports containing source snippets are not uploaded as artifacts.

The scan runs after GitHub receives a push; it does not prevent the initial publication of a secret. A required status check can enforce a passing scan before merging a pull request.

## Runtime handling

On macOS, ferragit reads an existing GitHub Internet Password directly through Security.framework. It does not invoke C Git, a Git credential helper, or another executable for authentication. `FERRAGIT_CREDENTIAL_ACCOUNT` selects the Keychain account when its stored identifier differs from the GitHub login.

On other platforms, and as an explicit override on macOS, supply `FERRAGIT_TOKEN` through the execution environment. No token should be placed in source, command arguments, a remote URL, commit content or a report. GitHub API and push requests use Authorization headers. Push and repository-metadata requests reject redirects, and push URLs cannot contain credentials. Actions job-log retrieval follows one validated HTTPS storage destination without forwarding Authorization; signed download URLs are not printed or persisted by Ferragit. Job-log output is bounded to 16 MiB; keep any saved logs under ignored artifacts/.

The publication wrapper runs ferragit for repository creation, push and remote verification. It does not load credentials. Local receipts are metadata only and live under ignored `artifacts/`. `.gitignore` also excludes build caches, Python caches and local `.env` files.
